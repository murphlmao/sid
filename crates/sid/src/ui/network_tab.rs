//! Network tab v2: `[Ports] [Services] [Interfaces]` segmented sub-tabs under one
//! search box, sourced live from `sid_core::sys` (ports/interfaces) and the new
//! `sid_core::svc` (systemd services) adapter seams.
//!
//! [`NetworkTabState`] is deliberately **live/ephemeral** — CLAUDE.md's layered-scope
//! invariant (global store + per-workspace `.sid/config.toml`) does not apply here.
//! There is no store, no scope, no secrets, nothing committed; every render reflects
//! the machine's current state and a refresh simply re-probes it. `crates/sid` is the
//! one crate allowed to name `sid_sysinfo`'s and `sid_svcctl`'s concrete
//! `SysinfoProvider::new()` / `SvcctlProvider::new()` constructors — every call
//! through them after construction goes back out via the `sid_core::sys::SysProvider`
//! / `sid_core::svc::ServiceProvider` traits, matching `sid-db`'s `DbClient`/
//! `db_registry` seam.
//!
//! ## Sub-tabs
//!
//! - **Ports** (ported from inc-1): the listening-ports table + two-click kill.
//! - **Services** (new): systemd units via `sid_core::svc::ServiceProvider`, with a
//!   `system|user` scope toggle and the same two-click confirm pattern for
//!   restart/stop/kill.
//! - **Interfaces** (ported from inc-1, regrouped): primary interfaces are always
//!   visible; generic/virtual ones ([`is_hidden_interface`]) collapse under a
//!   `hidden (N) ▸` expandable row (Murphy: docker etc. "should literally be a
//!   dropdown to expand").
//!
//! ## Filtering
//!
//! One 🔍 [`TextInput`] in the top bar applies to whichever sub-tab is active.
//! `TextInput` has no change-callback of its own, so filtering is wired via
//! `cx.observe(&filter, ..)` (fired on every `cx.notify()` the input makes while
//! editing — i.e. every keystroke) rather than re-reading its content once per
//! render; `apply_network_filter` pushes the new query into whichever table
//! delegate(s) need it. The delegates cache both the full fetched row set and the
//! filtered rows, so `/`-style instant filtering never re-probes the OS or spawns
//! `systemctl` — it recomputes from the cache already sitting on the entity, exactly
//! the "render pure-from-cache" rule below.
//!
//! Ports/interfaces are rendered with `gpui-component`'s `Table`/`TableDelegate`
//! (cribbed from `db_tab.rs`'s `ResultDelegate`), reused on the shared
//! `session::ssh_runtime()` Tokio runtime for the same reason `db_tab.rs` does:
//! `sysinfo`/`netstat2`/`nix` calls are synchronous OS calls, not async-native, but
//! keeping them off gpui's own executor avoids blocking `render`. `sid_svcctl`'s
//! `systemctl` calls are genuinely async (`tokio::process`, see that crate's docs)
//! but run on the very same shared runtime for the same reason — never inline in
//! `render`. Unlike `ResultDelegate`, [`PortsDelegate`]/[`ServicesDelegate`] are
//! interactive (per-row kill/restart/stop) — `TableDelegate::render_td`'s
//! `cx: &mut Context<TableState<Self>>` is scoped to the table's own entity, so the
//! two-click confirm state lives on each delegate itself rather than routed back
//! through `AppState`.

use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, SharedString,
    Subscription, Window, div, prelude::*, px, rgb,
};
use gpui_component::table::{Column, Table, TableDelegate, TableState};
use sid_core::svc::{ServiceInfo, ServiceProvider, SvcAction, SvcActiveState, SvcScope};
use sid_core::sys::{ListeningPort, NetInterface, Pid, Protocol, Signal, SysProvider};
use sid_svcctl::SvcctlProvider;
use sid_sysinfo::SysinfoProvider;

use super::TextInput;
use crate::app::AppState;
use crate::ui::session::ssh_runtime;

// Dark-theme palette, aligned with `app.rs`/`db_tab.rs`. Kept local so `ui` stays
// self-contained (same convention as `db_tab.rs`).
const BORDER: u32 = 0x2c2c30;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_BG: u32 = 0x33343a;
const ACTIVE_FG: u32 = 0xffffff;
const BRAND: u32 = 0x5a9ad0;
const DANGER: u32 = 0xd08a8a;
const OK_GREEN: u32 = 0x8ad08a;

/// Which sub-view is active under the Network tab's segmented control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NetSubTab {
    Ports,
    Services,
    Interfaces,
}

impl NetSubTab {
    const ALL: [NetSubTab; 3] = [NetSubTab::Ports, NetSubTab::Services, NetSubTab::Interfaces];

    fn label(self) -> &'static str {
        match self {
            NetSubTab::Ports => "Ports",
            NetSubTab::Services => "Services",
            NetSubTab::Interfaces => "Interfaces",
        }
    }
}

/// Network tab state. See the module doc comment for why this holds no store/scope.
pub struct NetworkTabState {
    /// The one seam this crate constructs concretely (`SysinfoProvider::new()`).
    /// Shared (via `Arc<Mutex<_>>`) between the refresh task and the ports table's own
    /// kill task, both of which run on `session::ssh_runtime()`.
    provider: Arc<Mutex<SysinfoProvider>>,
    /// The other seam this crate constructs concretely (`SvcctlProvider::new()`).
    /// No `Mutex` needed — unlike `SysinfoProvider`, it caches no probe handle, so
    /// concurrent calls need no serialization (see `sid_core::svc`'s module doc).
    svc_provider: Arc<dyn ServiceProvider>,
    /// Set once the tab has triggered its first ports/interfaces refresh (on first
    /// paint) — guards against re-triggering it on every subsequent render.
    loaded: bool,
    /// True while a ports/interfaces refresh task is in flight — guards re-entrant
    /// ⟳ clicks.
    refreshing: bool,
    /// Which of `[Ports] [Services] [Interfaces]` is active.
    sub_tab: NetSubTab,
    interfaces: Vec<NetInterface>,
    /// Cached partition of `interfaces` (per [`is_hidden_interface`]) after applying
    /// the current 🔍 filter query — populated by [`Self::recompute_interfaces`],
    /// mirroring how `PortsDelegate`/`ServicesDelegate` cache their filtered rows
    /// (perf audit finding #5). `interfaces_strip` just reads these instead of
    /// re-partitioning/re-filtering `interfaces` on every render.
    visible_interfaces: Vec<NetInterface>,
    /// See [`Self::visible_interfaces`].
    hidden_interfaces: Vec<NetInterface>,
    /// Name of the interface holding the default route, if any — sorted first and
    /// always visible regardless of [`is_hidden_interface`].
    default_route: Option<String>,
    /// Whether the Interfaces sub-tab's `hidden (N) ▸` group is expanded. Toggling
    /// this does NOT touch [`Self::visible_interfaces`]/[`Self::hidden_interfaces`] —
    /// it only changes whether the already-cached hidden group renders, not which
    /// interfaces belong in it.
    interfaces_expanded: bool,
    error: Option<String>,
    /// The ports table. Lazily built by `ensure_network_widgets` (needs `window`,
    /// which isn't available from `AppState::new`) — mirrors `DbTabState::results`.
    table: Option<Entity<TableState<PortsDelegate>>>,
    /// The services table, lazily built alongside `table`.
    services_table: Option<Entity<TableState<ServicesDelegate>>>,
    /// `system` or `user` — which systemd manager the Services sub-tab queries.
    svc_scope: SvcScope,
    /// Set once the Services sub-tab has triggered its first load (lazy: switching to
    /// systemctl calls the first time the tab is actually opened, not on every
    /// Network-tab paint). Reset to `false` when the scope toggle changes.
    svc_loaded: bool,
    svc_refreshing: bool,
    svc_error: Option<String>,
    /// The one 🔍 filter input shared by all three sub-tabs.
    filter: Option<Entity<TextInput>>,
    /// Kept alive so the `cx.observe(&filter, ..)` subscription (see module doc)
    /// isn't dropped — mirrors `AppState::_form_subscription`.
    _filter_sub: Option<Subscription>,
}

impl NetworkTabState {
    pub(crate) fn new() -> Self {
        Self {
            provider: Arc::new(Mutex::new(SysinfoProvider::new())),
            svc_provider: Arc::new(SvcctlProvider::new()),
            loaded: false,
            refreshing: false,
            sub_tab: NetSubTab::Ports,
            interfaces: Vec::new(),
            visible_interfaces: Vec::new(),
            hidden_interfaces: Vec::new(),
            default_route: None,
            interfaces_expanded: false,
            error: None,
            table: None,
            services_table: None,
            svc_scope: SvcScope::System,
            svc_loaded: false,
            svc_refreshing: false,
            svc_error: None,
            filter: None,
            _filter_sub: None,
        }
    }

    /// Recompute [`Self::visible_interfaces`]/[`Self::hidden_interfaces`] from
    /// `self.interfaces` + `self.default_route` and the given filter `query`
    /// (case-insensitive substring match against the interface name, same rule
    /// `interfaces_strip` used to apply inline) — perf audit finding #5. Called after
    /// every refresh (new `interfaces`) and every filter keystroke (new `query`); NOT
    /// called from the hidden-group expand/collapse toggle, which only flips a `bool`
    /// and touches neither input this depends on.
    fn recompute_interfaces(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        let (visible, hidden) =
            partition_interfaces(&self.interfaces, self.default_route.as_deref());
        let name_matches =
            |i: &&NetInterface| query.is_empty() || i.name.to_lowercase().contains(&query);
        self.visible_interfaces = visible.into_iter().filter(name_matches).cloned().collect();
        self.hidden_interfaces = hidden.into_iter().filter(name_matches).cloned().collect();
    }
}

/// Backs the ports [`Table`]. Constructed empty by `ensure_network_widgets`, then
/// mutated in place (`set_ports`) on every refresh — mirrors `db_tab.rs`'s
/// `ResultDelegate`. Unlike `ResultDelegate`, this delegate is interactive: it owns the
/// two-click kill-confirm state and spawns its own kill task, since `render_td`'s `cx`
/// is scoped to `TableState<Self>`, not the outer `AppState`.
struct PortsDelegate {
    provider: Arc<Mutex<SysinfoProvider>>,
    /// The full row set from the last refresh — never shown directly; `ports` (the
    /// filtered view) is what `TableDelegate` reads.
    all_ports: Vec<ListeningPort>,
    /// The currently displayed (filtered) rows.
    ports: Vec<ListeningPort>,
    /// The active filter query, cached so `set_ports` can re-apply it after a refresh.
    query: String,
    /// The pid whose kill button has been clicked once — the second click on the same
    /// pid sends the signal.
    armed_kill: Option<Pid>,
    /// Outcome of the last kill attempt, if it failed (e.g. `SysError::PermissionDenied`
    /// on a root-owned process). Cleared on the next refresh, arm, or successful kill.
    kill_error: Option<String>,
    columns: Vec<Column>,
}

impl PortsDelegate {
    fn new(provider: Arc<Mutex<SysinfoProvider>>) -> Self {
        Self {
            provider,
            all_ports: Vec::new(),
            ports: Vec::new(),
            query: String::new(),
            armed_kill: None,
            kill_error: None,
            columns: vec![
                Column::new("proto", "Proto").width(px(64.)),
                Column::new("port", "Port").width(px(72.)),
                Column::new("pid", "PID").width(px(80.)),
                Column::new("process", "Process").width(px(240.)),
                Column::new("kill", "").width(px(72.)),
            ],
        }
    }

    /// Replace the cached rows after a refresh, keeping the active filter applied.
    /// Disarms any pending kill confirmation — the row set just changed underneath it
    /// (mirrors `DbTabState::refresh` disarming a pending delete).
    fn set_ports(&mut self, ports: Vec<ListeningPort>) {
        self.all_ports = ports;
        self.armed_kill = None;
        self.recompute();
    }

    /// Update the filter query and recompute the displayed rows from the cached full
    /// set — no re-probe, matches the "render pure-from-cache" rule.
    fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.recompute();
    }

    fn recompute(&mut self) {
        self.ports = filter_ports(&self.all_ports, &self.query)
            .into_iter()
            .cloned()
            .collect();
    }

    /// Second click on an armed row: send SIGTERM to `pid` on the shared runtime. On
    /// success the row is dropped from both the cached and displayed sets immediately
    /// (rather than waiting on the next refresh); on failure the error (esp.
    /// `SysError::PermissionDenied` for a root-owned process) is surfaced via
    /// `kill_error`.
    fn kill(&mut self, pid: Pid, cx: &mut Context<TableState<Self>>) {
        self.armed_kill = None;
        self.kill_error = None;
        let provider = self.provider.clone();
        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                provider
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .kill_process(pid, Signal::Term)
            });
            let outcome = handle.await;
            let _ = this.update(cx, |state, cx| {
                match outcome {
                    Ok(Ok(())) => {
                        let delegate = state.delegate_mut();
                        delegate.all_ports.retain(|p| p.pid != Some(pid));
                        delegate.ports.retain(|p| p.pid != Some(pid));
                    }
                    Ok(Err(e)) => state.delegate_mut().kill_error = Some(e.to_string()),
                    Err(join_err) => {
                        state.delegate_mut().kill_error =
                            Some(format!("kill task panicked: {join_err}"));
                    }
                }
                state.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

impl TableDelegate for PortsDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.ports.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Borrow, not clone (perf audit finding #6) — every field this fn reads is
        // either `Copy` (`protocol`, `pid`) or already individually `.clone()`d below
        // where a `String` needs to move into a label/closure, so cloning the whole
        // row up front was pure waste.
        let port = &self.ports[row_ix];
        // `ElementId` has no `From<(&str, usize, usize)>` impl — fold (row, col) into a
        // single index (8 columns, generous multiplier) instead of a 3-tuple.
        let cell_id = ("net-cell", row_ix * 8 + col_ix);
        match col_ix {
            0 => {
                let label = match port.protocol {
                    Protocol::Tcp => "tcp",
                    Protocol::Udp => "udp",
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(label)
            }
            1 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG))
                .child(port.port.to_string()),
            2 => {
                let label: SharedString = port
                    .pid
                    .map(|p| p.as_u32().to_string())
                    .unwrap_or_else(|| "—".to_string())
                    .into();
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(label)
            }
            3 => {
                let label: SharedString = if port.command.is_empty() {
                    "—".into()
                } else {
                    port.command.clone().into()
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(label)
            }
            _ => {
                let Some(pid) = port.pid else {
                    return div()
                        .id(cell_id)
                        .px_2()
                        .text_xs()
                        .text_color(rgb(FG_DIM))
                        .child("—");
                };
                let armed = kill_click_executes(self.armed_kill, pid);
                let (label, color) = if armed {
                    ("kill?", DANGER)
                } else {
                    ("kill", FG_DIM)
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(color))
                    .hover(|s| s.bg(rgb(ACTIVE_BG)))
                    .child(label)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        if kill_click_executes(this.delegate().armed_kill, pid) {
                            this.delegate_mut().kill(pid, cx);
                        } else {
                            this.delegate_mut().armed_kill = Some(pid);
                            cx.notify();
                        }
                    }))
            }
        }
    }
}

/// Backs the Services [`Table`]. Same shape as [`PortsDelegate`] (cache the full
/// fetched set + the filtered display set, two-click confirm state, its own task
/// spawned on the shared runtime) with one addition: `scope`, kept in sync with
/// [`NetworkTabState::svc_scope`] by `AppState::set_svc_scope` so `perform` always
/// controls the unit in the scope currently selected in the UI.
struct ServicesDelegate {
    svc_provider: Arc<dyn ServiceProvider>,
    scope: SvcScope,
    all_services: Vec<ServiceInfo>,
    services: Vec<ServiceInfo>,
    query: String,
    /// The (unit name, action) whose button has been clicked once — the second click
    /// on the same pair executes it.
    armed_action: Option<(String, SvcAction)>,
    /// Outcome of the last restart/stop/kill attempt, if it failed (esp.
    /// `SvcError::PermissionDenied` for a system-scope action without root).
    action_error: Option<String>,
    columns: Vec<Column>,
}

impl ServicesDelegate {
    fn new(svc_provider: Arc<dyn ServiceProvider>, scope: SvcScope) -> Self {
        Self {
            svc_provider,
            scope,
            all_services: Vec::new(),
            services: Vec::new(),
            query: String::new(),
            armed_action: None,
            action_error: None,
            columns: vec![
                Column::new("name", "Unit").width(px(240.)),
                Column::new("state", "State").width(px(90.)),
                Column::new("description", "Description").width(px(320.)),
                Column::new("actions", "").width(px(210.)),
            ],
        }
    }

    fn set_scope(&mut self, scope: SvcScope) {
        self.scope = scope;
    }

    fn set_services(&mut self, services: Vec<ServiceInfo>) {
        self.all_services = services;
        self.armed_action = None;
        self.recompute();
    }

    fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.recompute();
    }

    fn recompute(&mut self) {
        self.services = filter_services(&self.all_services, &self.query)
            .into_iter()
            .cloned()
            .collect();
    }

    /// Second click on an armed (unit, action) pair: run `control` on the shared
    /// runtime, then re-fetch the service list so the badge/sub-state reflect the new
    /// reality immediately rather than waiting on the next manual ⟳.
    fn perform(&mut self, name: String, action: SvcAction, cx: &mut Context<TableState<Self>>) {
        self.armed_action = None;
        self.action_error = None;
        let provider = self.svc_provider.clone();
        let scope = self.scope;
        let unit = name.clone();
        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                let ctrl = provider.control(&unit, action, scope).await;
                let refreshed = provider.list_services(scope).await;
                (ctrl, refreshed)
            });
            let outcome = handle.await;
            let _ = this.update(cx, |state, cx| {
                match outcome {
                    Ok((ctrl_res, refreshed_res)) => {
                        if let Err(e) = ctrl_res {
                            state.delegate_mut().action_error = Some(e.to_string());
                        }
                        if let Ok(services) = refreshed_res {
                            state.delegate_mut().set_services(services);
                        }
                    }
                    Err(join_err) => {
                        state.delegate_mut().action_error =
                            Some(format!("service control task panicked: {join_err}"));
                    }
                }
                state.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

impl TableDelegate for ServicesDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.services.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Borrow, not clone (perf audit finding #6) — `active` is `Copy`, and every
        // `String` field this fn needs already gets its own `.clone()` below.
        let svc = &self.services[row_ix];
        let cell_id = ("svc-cell", row_ix * 8 + col_ix);
        match col_ix {
            0 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG))
                .child(svc.name.clone()),
            1 => {
                let (label, color) = svc_state_badge(svc.active);
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(color))
                    .child(label)
            }
            2 => {
                let label: SharedString = if svc.description.is_empty() {
                    "—".into()
                } else {
                    svc.description.clone().into()
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(label)
            }
            _ => {
                let actions = [
                    (SvcAction::Restart, "restart"),
                    (SvcAction::Stop, "stop"),
                    (SvcAction::Kill, "kill"),
                ];
                div().id(cell_id).flex().flex_row().gap_2().px_2().children(
                    actions
                        .into_iter()
                        .enumerate()
                        .map(|(action_ix, (action, label))| {
                            let armed =
                                action_click_executes(&self.armed_action, &svc.name, action);
                            let (display, color): (SharedString, u32) = if armed {
                                (format!("{label}?").into(), DANGER)
                            } else {
                                (label.into(), FG_DIM)
                            };
                            let name = svc.name.clone();
                            div()
                                .id(("svc-action", row_ix * 8 + action_ix))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .text_color(rgb(color))
                                .hover(|s| s.bg(rgb(ACTIVE_BG)))
                                .child(display)
                                .on_click(cx.listener(
                                    move |this, _ev: &ClickEvent, _window, cx| {
                                        if action_click_executes(
                                            &this.delegate().armed_action,
                                            &name,
                                            action,
                                        ) {
                                            this.delegate_mut().perform(name.clone(), action, cx);
                                        } else {
                                            this.delegate_mut().armed_action =
                                                Some((name.clone(), action));
                                            cx.notify();
                                        }
                                    },
                                ))
                        }),
                )
            }
        }
    }
}

impl AppState {
    pub(crate) fn network_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_network_widgets(window, cx);
        if !self.network.loaded {
            self.network.loaded = true;
            self.refresh_network(cx);
        }
        if self.network.sub_tab == NetSubTab::Services && !self.network.svc_loaded {
            self.refresh_network_services(cx);
        }

        let filter = self.network.filter.clone();
        let refreshing = match self.network.sub_tab {
            NetSubTab::Services => self.network.svc_refreshing,
            NetSubTab::Ports | NetSubTab::Interfaces => self.network.refreshing,
        };
        let refresh_label = if refreshing { "…" } else { "⟳ refresh" };

        let port_count = self
            .network
            .table
            .as_ref()
            .map(|t| t.read(cx).delegate().ports.len())
            .unwrap_or(0);
        let svc_count = self
            .network
            .services_table
            .as_ref()
            .map(|t| t.read(cx).delegate().services.len())
            .unwrap_or(0);

        let sub: SharedString = match self.network.sub_tab {
            NetSubTab::Ports => match &self.network.error {
                Some(e) => format!("error: {e}").into(),
                None if self.network.refreshing => "refreshing…".into(),
                None => format!("{port_count} listening ports").into(),
            },
            NetSubTab::Services => match &self.network.svc_error {
                Some(e) => format!("error: {e}").into(),
                None if self.network.svc_refreshing => "refreshing…".into(),
                None => {
                    let scope_label = if self.network.svc_scope == SvcScope::User {
                        "user"
                    } else {
                        "system"
                    };
                    format!("{svc_count} {scope_label} services").into()
                }
            },
            NetSubTab::Interfaces => match &self.network.error {
                Some(e) => format!("error: {e}").into(),
                None => format!("{} interfaces", self.network.interfaces.len()).into(),
            },
        };

        let kill_error = self
            .network
            .table
            .as_ref()
            .and_then(|t| t.read(cx).delegate().kill_error.clone());
        let action_error = self
            .network
            .services_table
            .as_ref()
            .and_then(|t| t.read(cx).delegate().action_error.clone());

        let body: AnyElement = match self.network.sub_tab {
            NetSubTab::Ports => self
                .network
                .table
                .clone()
                .map(|t| {
                    div()
                        .flex_1()
                        .w_full()
                        .child(Table::new(&t).stripe(true))
                        .into_any_element()
                })
                .unwrap_or_else(|| div().into_any_element()),
            NetSubTab::Services => {
                let scope_toggle = self.svc_scope_toggle(cx);
                let table = self.network.services_table.clone();
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(scope_toggle)
                    .children(
                        table.map(|t| div().flex_1().w_full().child(Table::new(&t).stripe(true))),
                    )
                    .into_any_element()
            }
            NetSubTab::Interfaces => self.interfaces_strip(cx).into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(self.network_sub_tab_strip(cx))
                    .children(filter.map(|f| div().flex_1().max_w(px(280.)).child(f)))
                    .child(
                        div()
                            .id("net-refresh")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .text_color(rgb(BRAND))
                            .hover(|s| s.bg(rgb(ACTIVE_BG)))
                            .child(refresh_label)
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                match this.network.sub_tab {
                                    NetSubTab::Services => this.refresh_network_services(cx),
                                    NetSubTab::Ports | NetSubTab::Interfaces => {
                                        this.refresh_network(cx)
                                    }
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(rgb(FG_DIM))
                    .child(sub),
            )
            .child(body)
            .children(kill_error.map(|e| {
                div()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(DANGER))
                    .child(format!("✗ {e}"))
            }))
            .children(action_error.map(|e| {
                div()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(DANGER))
                    .child(format!("✗ {e}"))
            }))
            .into_any_element()
    }

    /// The `[Ports] [Services] [Interfaces]` segmented control — mirrors `app.rs`'s
    /// main `tab_strip` / `host_form.rs`'s `auth_selector`.
    fn network_sub_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let active = self.network.sub_tab;
        div()
            .flex()
            .flex_row()
            .gap_1()
            .children(NetSubTab::ALL.iter().enumerate().map(|(ix, &tab)| {
                let is_active = tab == active;
                div()
                    .id(("net-subtab", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if is_active { ACTIVE_BG } else { BORDER }))
                    .text_color(rgb(if is_active { ACTIVE_FG } else { FG_DIM }))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.network.sub_tab = tab;
                        cx.notify();
                    }))
            }))
    }

    /// The Services sub-tab's `system|user` scope toggle. Switching scope forces a
    /// fresh `list_services` call (`svc_loaded` reset) — the two scopes are disjoint
    /// unit sets, not a filter over one cached list.
    fn svc_scope_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let scopes = [(SvcScope::System, "system"), (SvcScope::User, "user")];
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(div().text_xs().text_color(rgb(FG_DIM)).child("scope"))
            .children(scopes.iter().enumerate().map(|(ix, &(scope, label))| {
                let active = self.network.svc_scope == scope;
                div()
                    .id(("svc-scope", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if active { ACTIVE_BG } else { BORDER }))
                    .text_color(rgb(if active { ACTIVE_FG } else { FG_DIM }))
                    .child(label)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.set_svc_scope(scope, cx);
                    }))
            }))
    }

    /// Interfaces sub-tab: primary interfaces (default-route iface first, then
    /// alphabetical — `sort_interfaces_default_first`, applied on refresh) always
    /// shown; generic/virtual ones ([`is_hidden_interface`]) collapsed under a
    /// `hidden (N) ▸` row that expands in place. Reads the already-filtered/partitioned
    /// cache (`recompute_interfaces`, perf audit finding #5) instead of re-partitioning
    /// `self.network.interfaces` on every render — the cache is kept current on every
    /// refresh and every filter keystroke, so this never shows a stale count.
    fn interfaces_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let default_name = self.network.default_route.clone();
        let visible = &self.network.visible_interfaces;
        let hidden = &self.network.hidden_interfaces;
        let hidden_count = hidden.len();
        let expanded = self.network.interfaces_expanded;

        let default_for_rows = default_name.clone();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(
                visible
                    .iter()
                    .map(|iface| render_iface_row(iface, default_for_rows.as_deref())),
            )
            .when(hidden_count > 0, |el| {
                let toggle_label: SharedString = format!(
                    "{} hidden ({hidden_count})",
                    if expanded { "▾" } else { "▸" }
                )
                .into();
                let el = el.child(
                    div()
                        .id("net-hidden-toggle")
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_1()
                        .py_1()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(FG_DIM))
                        .hover(|s| s.text_color(rgb(FG)))
                        .child(toggle_label)
                        .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                            this.network.interfaces_expanded = !this.network.interfaces_expanded;
                            cx.notify();
                        })),
                );
                el.when(expanded, |el2| {
                    el2.children(
                        hidden
                            .iter()
                            .map(|iface| render_iface_row(iface, default_name.as_deref())),
                    )
                })
            })
    }

    /// Lazily build the ports table, the services table, and the shared filter input
    /// on first paint of the Network tab. Idempotent (checked every render) — mirrors
    /// `db_tab.rs`'s `ensure_query_widgets`.
    fn ensure_network_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.network.table.is_none() {
            let provider = self.network.provider.clone();
            let table = cx.new(|cx| TableState::new(PortsDelegate::new(provider), window, cx));
            self.network.table = Some(table);
        }
        if self.network.services_table.is_none() {
            let svc_provider = self.network.svc_provider.clone();
            let scope = self.network.svc_scope;
            let table = cx
                .new(|cx| TableState::new(ServicesDelegate::new(svc_provider, scope), window, cx));
            self.network.services_table = Some(table);
        }
        if self.network.filter.is_none() {
            let filter = cx.new(|cx| TextInput::new(cx, "🔍 filter"));
            // `TextInput` has no change-callback; `cx.observe` fires on every
            // `cx.notify()` it makes while editing, i.e. every keystroke — see the
            // module doc's "Filtering" section.
            let sub = cx.observe(&filter, |this: &mut Self, _filter, cx| {
                this.apply_network_filter(cx);
            });
            self.network.filter = Some(filter);
            self.network._filter_sub = Some(sub);
        }
    }

    /// Push the filter box's current text into whichever table delegate(s) it
    /// applies to, and into the Interfaces sub-tab's cache (`recompute_interfaces`,
    /// perf audit finding #5) — Interfaces has no `Table`/delegate of its own, but it
    /// mirrors the same "cache the filtered view" contract, so it's updated here too
    /// rather than at `interfaces_strip` render time.
    fn apply_network_filter(&mut self, cx: &mut Context<Self>) {
        let query = self
            .network
            .filter
            .as_ref()
            .map(|f| f.read(cx).content().to_string())
            .unwrap_or_default();
        if let Some(table) = self.network.table.clone() {
            table.update(cx, |state, cx| {
                state.delegate_mut().set_query(&query);
                state.refresh(cx);
            });
        }
        if let Some(table) = self.network.services_table.clone() {
            table.update(cx, |state, cx| {
                state.delegate_mut().set_query(&query);
                state.refresh(cx);
            });
        }
        self.network.recompute_interfaces(&query);
        cx.notify();
    }

    /// Switch the Services sub-tab's scope, forcing a fresh `list_services` call —
    /// system and user units are disjoint sets, so this isn't a filter over one
    /// cached list the way the 🔍 box is.
    fn set_svc_scope(&mut self, scope: SvcScope, cx: &mut Context<Self>) {
        if self.network.svc_scope == scope {
            return;
        }
        self.network.svc_scope = scope;
        self.network.svc_loaded = false;
        if let Some(table) = self.network.services_table.clone() {
            table.update(cx, |state, cx| {
                state.delegate_mut().set_scope(scope);
                state.refresh(cx);
            });
        }
        cx.notify();
    }

    /// ⟳ refresh (Ports/Interfaces): re-probe ports, interfaces, and the default
    /// route on the shared runtime, then apply the results. No blocking in `render` —
    /// this only ever runs from a click or the lazy first-paint trigger in
    /// `network_tab`.
    pub(crate) fn refresh_network(&mut self, cx: &mut Context<Self>) {
        if self.network.refreshing {
            return;
        }
        self.network.refreshing = true;
        self.network.error = None;
        cx.notify();

        let provider = self.network.provider.clone();
        let table = self.network.table.clone();

        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                let mut guard = provider.lock().unwrap_or_else(|e| e.into_inner());
                (
                    guard.list_listening_ports(),
                    guard.list_interfaces(),
                    guard.default_route_iface_name(),
                )
            });
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                this.network.refreshing = false;
                match outcome {
                    Ok((ports_res, ifaces_res, route_res)) => {
                        let mut err = None;
                        match ports_res {
                            Ok(ports) => {
                                if let Some(table) = &table {
                                    table.update(cx, |state, cx| {
                                        state.delegate_mut().set_ports(ports);
                                        state.refresh(cx);
                                    });
                                }
                            }
                            Err(e) => err = Some(e.to_string()),
                        }
                        match ifaces_res {
                            Ok(mut ifaces) => {
                                let default_name = route_res.unwrap_or(None);
                                sort_interfaces_default_first(&mut ifaces, default_name.as_deref());
                                this.network.default_route = default_name;
                                this.network.interfaces = ifaces;
                                // Perf audit finding #5: refresh the visible/hidden
                                // cache here (new `interfaces`) rather than leaving
                                // `interfaces_strip` to re-partition/re-filter on
                                // every render.
                                let query = this
                                    .network
                                    .filter
                                    .as_ref()
                                    .map(|f| f.read(cx).content().to_string())
                                    .unwrap_or_default();
                                this.network.recompute_interfaces(&query);
                            }
                            Err(e) => {
                                if err.is_none() {
                                    err = Some(e.to_string());
                                }
                            }
                        }
                        this.network.error = err;
                    }
                    Err(join_err) => {
                        this.network.error =
                            Some(format!("network probe task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ⟳ refresh (Services): re-fetch the unit list for the active scope on the
    /// shared runtime. Also the Services sub-tab's lazy first-load hook — `systemctl`
    /// is never called just because the Network tab is open, only once Services is
    /// actually selected.
    pub(crate) fn refresh_network_services(&mut self, cx: &mut Context<Self>) {
        if self.network.svc_refreshing {
            return;
        }
        self.network.svc_refreshing = true;
        self.network.svc_error = None;
        cx.notify();

        let provider = self.network.svc_provider.clone();
        let scope = self.network.svc_scope;
        let table = self.network.services_table.clone();

        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move { provider.list_services(scope).await });
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                this.network.svc_refreshing = false;
                this.network.svc_loaded = true;
                match outcome {
                    Ok(Ok(services)) => {
                        if let Some(table) = &table {
                            table.update(cx, |state, cx| {
                                state.delegate_mut().set_services(services);
                                state.refresh(cx);
                            });
                        }
                    }
                    Ok(Err(e)) => this.network.svc_error = Some(e.to_string()),
                    Err(join_err) => {
                        this.network.svc_error =
                            Some(format!("service probe task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

// ---- pure helpers (unit-tested) ---------------------------------------------------

/// Two-click kill confirm: `true` when `clicked` is the pid already armed. Mirrors
/// `app::delete_click_executes`, keyed on `Pid` since kill targets a process, not a row.
fn kill_click_executes(armed: Option<Pid>, clicked: Pid) -> bool {
    armed == Some(clicked)
}

/// Two-click confirm for a Services row action: `true` when `(name, action)` matches
/// the armed pair exactly (a different action on the same unit, or the same action on
/// a different unit, re-arms rather than executing).
fn action_click_executes(
    armed: &Option<(String, SvcAction)>,
    name: &str,
    action: SvcAction,
) -> bool {
    armed
        .as_ref()
        .is_some_and(|(armed_name, armed_action)| armed_name == name && *armed_action == action)
}

/// Case-insensitive filter over the ports table: port-number prefix match,
/// process/command substring, local-address substring, or an exact pid match. Empty
/// (or all-whitespace) query matches everything.
fn filter_ports<'a>(ports: &'a [ListeningPort], query: &str) -> Vec<&'a ListeningPort> {
    let query = query.trim();
    if query.is_empty() {
        return ports.iter().collect();
    }
    let lower = query.to_lowercase();
    let exact_pid: Option<u32> = query.parse().ok();
    ports
        .iter()
        .filter(|p| {
            p.port.to_string().starts_with(&lower)
                || p.command.to_lowercase().contains(&lower)
                || p.local_addr.to_lowercase().contains(&lower)
                || exact_pid.is_some_and(|pid| p.pid.map(Pid::as_u32) == Some(pid))
        })
        .collect()
}

/// Case-insensitive substring filter over a service's name/description. Empty (or
/// all-whitespace) query matches everything.
fn filter_services<'a>(services: &'a [ServiceInfo], query: &str) -> Vec<&'a ServiceInfo> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return services.iter().collect();
    }
    services
        .iter()
        .filter(|s| {
            s.name.to_lowercase().contains(&query) || s.description.to_lowercase().contains(&query)
        })
        .collect()
}

/// Badge label + color for a service's active state.
fn svc_state_badge(state: SvcActiveState) -> (&'static str, u32) {
    match state {
        SvcActiveState::Active => ("active", OK_GREEN),
        SvcActiveState::Failed => ("failed", DANGER),
        SvcActiveState::Inactive => ("inactive", FG_DIM),
        SvcActiveState::Other => ("other", FG_DIM),
    }
}

/// Whether interface `name` should default into the collapsed "hidden (N) ▸" group.
/// Never hidden if it holds the default route; otherwise visible only if it matches a
/// physical/VPN name prefix (`en*`, `eth*`, `wl*`, `wlan*`, `ww*`, `usb*`, `wg*`,
/// `tun*`, `tailscale*`) — anything else (loopback, container/VM virtual interfaces
/// such as `docker*`/`veth*`/`br-*`/`virbr*`/`vnet*`/`tap*`, or an unrecognized name)
/// is hidden by default.
fn is_hidden_interface(name: &str, is_default_route: bool) -> bool {
    if is_default_route {
        return false;
    }
    const VISIBLE_PREFIXES: &[&str] = &[
        "en",
        "eth",
        "wl",
        "wlan",
        "ww",
        "usb",
        "wg",
        "tun",
        "tailscale",
    ];
    !VISIBLE_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Split `interfaces` into `(visible, hidden)` per [`is_hidden_interface`], the
/// interface's default-route-ness supplied by comparing its name against
/// `default_name`.
fn partition_interfaces<'a>(
    interfaces: &'a [NetInterface],
    default_name: Option<&str>,
) -> (Vec<&'a NetInterface>, Vec<&'a NetInterface>) {
    interfaces
        .iter()
        .partition(|i| !is_hidden_interface(&i.name, default_name == Some(i.name.as_str())))
}

/// Render one interfaces-strip row: name (+ ★ if default route) · addrs · up/down ·
/// rx/tx (humanized). Free function (not `&self`) so it's shared between the visible
/// and expanded-hidden groups in `interfaces_strip`.
fn render_iface_row(iface: &NetInterface, default_name: Option<&str>) -> impl IntoElement + use<> {
    let is_default = default_name == Some(iface.name.as_str());
    let addrs: SharedString = if iface.addrs.is_empty() {
        "no addrs".into()
    } else {
        iface.addrs.join(", ").into()
    };
    let (status_label, status_color) = if iface.is_up {
        ("up", BRAND)
    } else {
        ("down", FG_DIM)
    };
    let throughput: SharedString = format!(
        "↓{} ↑{}",
        humanize_bytes(iface.rx_bytes),
        humanize_bytes(iface.tx_bytes)
    )
    .into();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(120.))
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(FG))
                .child(iface.name.clone()),
        )
        .when(is_default, |el| {
            el.child(div().text_xs().text_color(rgb(BRAND)).child("★"))
        })
        .child(
            div()
                .flex_1()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(addrs),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(status_color))
                .child(status_label),
        )
        .child(div().text_xs().text_color(rgb(FG_DIM)).child(throughput))
}

/// Human-readable byte count (binary units, one decimal place above `B`) — e.g. "340 B",
/// "1.2 MB". Pure so it's unit-tested without touching a real interface counter.
fn humanize_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit_ix = 0;
    while value >= 1024.0 && unit_ix < UNITS.len() - 1 {
        value /= 1024.0;
        unit_ix += 1;
    }
    if unit_ix == 0 {
        format!("{bytes} {}", UNITS[unit_ix])
    } else {
        format!("{value:.1} {}", UNITS[unit_ix])
    }
}

/// Put the default-route interface first (if present among `interfaces`), then sort the
/// rest by name. Pure so it's unit-tested without touching a real routing table.
fn sort_interfaces_default_first(interfaces: &mut [NetInterface], default_name: Option<&str>) {
    interfaces.sort_by(|a, b| {
        let a_is_default = default_name == Some(a.name.as_str());
        let b_is_default = default_name == Some(b.name.as_str());
        b_is_default
            .cmp(&a_is_default)
            .then_with(|| a.name.cmp(&b.name))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str) -> NetInterface {
        NetInterface {
            name: name.to_string(),
            addrs: Vec::new(),
            rx_bytes: 0,
            tx_bytes: 0,
            is_up: true,
        }
    }

    fn port(port: u16, pid: Option<u32>, command: &str, local_addr: &str) -> ListeningPort {
        ListeningPort {
            port,
            pid: pid.map(Pid::from_u32),
            command: command.to_string(),
            protocol: Protocol::Tcp,
            state: sid_core::sys::SocketState::Listen,
            local_addr: local_addr.to_string(),
        }
    }

    fn svc(name: &str, description: &str) -> ServiceInfo {
        ServiceInfo {
            name: name.to_string(),
            description: description.to_string(),
            active: SvcActiveState::Active,
            sub_state: "running".to_string(),
        }
    }

    #[test]
    fn kill_needs_two_clicks_on_the_same_pid() {
        let pid = Pid::from_u32(123);
        let other = Pid::from_u32(456);
        assert!(!kill_click_executes(None, pid));
        assert!(kill_click_executes(Some(pid), pid));
        assert!(!kill_click_executes(Some(pid), other));
    }

    #[test]
    fn action_click_needs_two_clicks_on_same_unit_and_action() {
        let armed = Some(("nginx.service".to_string(), SvcAction::Restart));
        assert!(action_click_executes(
            &armed,
            "nginx.service",
            SvcAction::Restart
        ));
        assert!(!action_click_executes(
            &armed,
            "nginx.service",
            SvcAction::Stop
        ));
        assert!(!action_click_executes(
            &armed,
            "sshd.service",
            SvcAction::Restart
        ));
        assert!(!action_click_executes(
            &None,
            "nginx.service",
            SvcAction::Restart
        ));
    }

    #[test]
    fn filter_ports_empty_query_matches_all() {
        let ports = vec![
            port(22, Some(1), "sshd", "0.0.0.0"),
            port(80, Some(2), "nginx", "127.0.0.1"),
        ];
        assert_eq!(filter_ports(&ports, "").len(), 2);
        assert_eq!(filter_ports(&ports, "   ").len(), 2);
    }

    #[test]
    fn filter_ports_matches_port_number_prefix() {
        let ports = vec![
            port(22, Some(1), "sshd", "0.0.0.0"),
            port(2222, Some(2), "sshd-alt", "0.0.0.0"),
            port(80, Some(3), "nginx", "0.0.0.0"),
        ];
        let got: Vec<u16> = filter_ports(&ports, "22").iter().map(|p| p.port).collect();
        assert_eq!(got, vec![22, 2222]);
    }

    #[test]
    fn filter_ports_matches_command_substring_case_insensitively() {
        let ports = vec![
            port(22, Some(1), "sshd", "0.0.0.0"),
            port(80, Some(2), "nginx", "0.0.0.0"),
        ];
        let got: Vec<u16> = filter_ports(&ports, "NGI").iter().map(|p| p.port).collect();
        assert_eq!(got, vec![80]);
    }

    #[test]
    fn filter_ports_matches_local_addr_substring() {
        let ports = vec![
            port(22, Some(1), "sshd", "127.0.0.1"),
            port(80, Some(2), "nginx", "0.0.0.0"),
        ];
        let got: Vec<u16> = filter_ports(&ports, "127.0")
            .iter()
            .map(|p| p.port)
            .collect();
        assert_eq!(got, vec![22]);
    }

    #[test]
    fn filter_ports_matches_exact_pid_only() {
        let ports = vec![
            port(22, Some(123), "sshd", "0.0.0.0"),
            port(80, Some(1230), "nginx", "0.0.0.0"),
        ];
        let got: Vec<u16> = filter_ports(&ports, "123").iter().map(|p| p.port).collect();
        assert_eq!(got, vec![22]);
    }

    #[test]
    fn filter_services_empty_query_matches_all() {
        let services = vec![
            svc("nginx.service", "web server"),
            svc("sshd.service", "secure shell"),
        ];
        assert_eq!(filter_services(&services, "").len(), 2);
    }

    #[test]
    fn filter_services_matches_name_or_description_case_insensitively() {
        let services = vec![
            svc("nginx.service", "web server"),
            svc("sshd.service", "secure shell"),
        ];
        let by_name: Vec<&str> = filter_services(&services, "NGINX")
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(by_name, vec!["nginx.service"]);
        let by_desc: Vec<&str> = filter_services(&services, "shell")
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(by_desc, vec!["sshd.service"]);
    }

    #[test]
    fn svc_state_badge_labels_cover_all_variants() {
        assert_eq!(svc_state_badge(SvcActiveState::Active).0, "active");
        assert_eq!(svc_state_badge(SvcActiveState::Failed).0, "failed");
        assert_eq!(svc_state_badge(SvcActiveState::Inactive).0, "inactive");
        assert_eq!(svc_state_badge(SvcActiveState::Other).0, "other");
    }

    #[test]
    fn physical_and_vpn_prefixes_are_visible() {
        for name in [
            "eth0",
            "en0",
            "enp3s0",
            "wlan0",
            "wl0",
            "wwan0",
            "usb0",
            "wg0",
            "tun0",
            "tailscale0",
        ] {
            assert!(
                !is_hidden_interface(name, false),
                "{name} should be visible"
            );
        }
    }

    #[test]
    fn virtual_and_unmatched_interfaces_are_hidden() {
        for name in [
            "lo",
            "docker0",
            "veth1234",
            "br-abcdef",
            "virbr0",
            "vnet0",
            "tap0",
            "randomiface0",
        ] {
            assert!(is_hidden_interface(name, false), "{name} should be hidden");
        }
    }

    #[test]
    fn default_route_iface_is_always_visible_even_if_it_would_otherwise_be_hidden() {
        assert!(!is_hidden_interface("docker0", true));
        assert!(!is_hidden_interface("lo", true));
    }

    #[test]
    fn partition_interfaces_splits_visible_and_hidden() {
        let ifaces = vec![iface("eth0"), iface("docker0"), iface("lo"), iface("wg0")];
        let (visible, hidden) = partition_interfaces(&ifaces, None);
        let visible_names: Vec<&str> = visible.iter().map(|i| i.name.as_str()).collect();
        let hidden_names: Vec<&str> = hidden.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(visible_names, vec!["eth0", "wg0"]);
        assert_eq!(hidden_names, vec!["docker0", "lo"]);
    }

    #[test]
    fn partition_interfaces_keeps_default_route_visible() {
        let ifaces = vec![iface("docker0"), iface("eth0")];
        let (visible, _hidden) = partition_interfaces(&ifaces, Some("docker0"));
        let visible_names: Vec<&str> = visible.iter().map(|i| i.name.as_str()).collect();
        assert!(visible_names.contains(&"docker0"));
    }

    #[test]
    fn humanize_bytes_picks_the_right_unit() {
        assert_eq!(humanize_bytes(0), "0 B");
        assert_eq!(humanize_bytes(340), "340 B");
        assert_eq!(humanize_bytes(1024), "1.0 KB");
        assert_eq!(humanize_bytes(1_536), "1.5 KB");
        assert_eq!(humanize_bytes(1_048_576), "1.0 MB");
    }

    #[test]
    fn default_route_iface_sorts_first_then_by_name() {
        let mut ifaces = vec![iface("wlan0"), iface("eth0"), iface("lo")];
        sort_interfaces_default_first(&mut ifaces, Some("wlan0"));
        let names: Vec<&str> = ifaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["wlan0", "eth0", "lo"]);
    }

    #[test]
    fn no_default_route_sorts_all_by_name() {
        let mut ifaces = vec![iface("wlan0"), iface("eth0")];
        sort_interfaces_default_first(&mut ifaces, None);
        let names: Vec<&str> = ifaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["eth0", "wlan0"]);
    }
}
