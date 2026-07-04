//! Network tab v3: `[Ports] [Services] [Interfaces] [Docker] [Kubernetes]` segmented
//! sub-tabs under one search box, sourced live from `sid_core::sys` (ports/interfaces),
//! `sid_core::svc` (systemd services), and `sid_core::containers` (Docker/Kubernetes)
//! adapter seams.
//!
//! [`NetworkTabState`] is deliberately **live/ephemeral** — CLAUDE.md's layered-scope
//! invariant (global store + per-workspace `.sid/config.toml`) does not apply here.
//! There is no store, no scope, no secrets, nothing committed; every render reflects
//! the machine's current state and a refresh simply re-probes it. `crates/sid` is the
//! one crate allowed to name `sid_sysinfo`'s, `sid_svcctl`'s, and `sid_containers`'s
//! concrete `SysinfoProvider::new()` / `SvcctlProvider::new()` / `DockerCliProvider::
//! new()` / `KubectlCliProvider::new()` constructors — every call through them after
//! construction goes back out via the `sid_core::sys::SysProvider` /
//! `sid_core::svc::ServiceProvider` / `sid_core::containers::{ContainerProvider,
//! KubeProvider}` traits, matching `sid-db`'s `DbClient`/`db_registry` seam.
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
//! - **Docker** (new, read-only): containers via `sid_core::containers::
//!   ContainerProvider` — name/image/state/status/ports. Management (start/stop/exec)
//!   is out of scope for this pass.
//! - **Kubernetes** (new, read-only): kubeconfig contexts + a pods table via
//!   `sid_core::containers::KubeProvider`. Both `docker`/`kubectl` are optional local
//!   tooling — a `NotInstalled` probe error degrades to a dim notice (reusing the same
//!   `sub`-line-plus-error-line two-tier pattern the Ports/Services error paths already
//!   use below) rather than a red error banner.
//!
//! ## Filtering
//!
//! One filter [`TextInput`] in the top bar applies to whichever sub-tab is active.
//! `TextInput` has no change-callback of its own, so filtering is wired via
//! `cx.observe(&filter, ..)` (fired on every `cx.notify()` the input makes while
//! editing — i.e. every keystroke) rather than re-reading its content once per
//! render; `apply_network_filter` pushes the new query into whichever table
//! delegate(s) need it. The delegates cache both the full fetched row set and the
//! filtered rows, so `/`-style instant filtering never re-probes the OS or spawns
//! `systemctl`/`docker`/`kubectl` — it recomputes from the cache already sitting on
//! the entity, exactly the "render pure-from-cache" rule below.
//!
//! Ports/interfaces are rendered with `gpui-component`'s `Table`/`TableDelegate`
//! (cribbed from `db_tab.rs`'s `ResultDelegate`), reused on the shared
//! `session::ssh_runtime()` Tokio runtime for the same reason `db_tab.rs` does:
//! `sysinfo`/`netstat2`/`nix` calls are synchronous OS calls, not async-native, but
//! keeping them off gpui's own executor avoids blocking `render`. `sid_svcctl`'s
//! `systemctl` and `sid_containers`'s `docker`/`kubectl` calls are genuinely async
//! (`tokio::process`, see those crates' docs) but run on the very same shared runtime
//! for the same reason — never inline in `render`. Unlike `ResultDelegate`,
//! [`PortsDelegate`]/[`ServicesDelegate`] are interactive (per-row kill/restart/stop) —
//! `TableDelegate::render_td`'s `cx: &mut Context<TableState<Self>>` is scoped to the
//! table's own entity, so the two-click confirm state lives on each delegate itself
//! rather than routed back through `AppState`. [`DockerDelegate`]/[`KubePodsDelegate`]
//! are plain read-only tables, closer in shape to `db_tab.rs`'s `ResultDelegate`.

use std::cmp::Ordering;
use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, SharedString,
    Subscription, Window, div, prelude::*, px, rgb,
};
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableState};
use sid_containers::{DockerCliProvider, KubectlCliProvider};
use sid_core::containers::{
    ContainerError, ContainerInfo, ContainerProvider, KubeContext, KubeError, KubePod, KubeProvider,
};
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
    Docker,
    Kubernetes,
}

impl NetSubTab {
    const ALL: [NetSubTab; 5] = [
        NetSubTab::Ports,
        NetSubTab::Services,
        NetSubTab::Interfaces,
        NetSubTab::Docker,
        NetSubTab::Kubernetes,
    ];

    fn label(self) -> &'static str {
        match self {
            NetSubTab::Ports => "Ports",
            NetSubTab::Services => "Services",
            NetSubTab::Interfaces => "Interfaces",
            NetSubTab::Docker => "Docker",
            NetSubTab::Kubernetes => "Kubernetes",
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
    /// the current filter query — populated by [`Self::recompute_interfaces`],
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
    /// The third seam this crate constructs concretely (`DockerCliProvider::new()`).
    /// No `Mutex` needed — stateless, same reasoning as `svc_provider`.
    docker_provider: Arc<dyn ContainerProvider>,
    /// The Docker table, lazily built alongside `table`/`services_table`.
    docker_table: Option<Entity<TableState<DockerDelegate>>>,
    /// Set once the Docker sub-tab has triggered its first load (lazy, same pattern as
    /// `svc_loaded`: `docker ps` is never called just because the Network tab is open).
    docker_loaded: bool,
    docker_refreshing: bool,
    /// `true` when the last probe returned `ContainerError::NotInstalled` — the Docker
    /// sub-tab renders a dim graceful-absence notice instead of a table when set.
    docker_not_installed: bool,
    /// A genuine (non-`NotInstalled`) probe failure, if any.
    docker_error: Option<String>,
    /// The fourth seam this crate constructs concretely (`KubectlCliProvider::new()`).
    /// No `Mutex` needed — stateless, same reasoning as `svc_provider`/`docker_provider`.
    kube_provider: Arc<dyn KubeProvider>,
    /// Configured kubeconfig contexts, refreshed alongside the pods table.
    kube_contexts: Vec<KubeContext>,
    /// Which context the pods table is scoped to. `None` means "use kubectl's own
    /// `current-context`" — distinct from "no context selected yet", which is instead
    /// represented by `kube_contexts` being empty before the first load.
    kube_selected_context: Option<String>,
    /// The Kubernetes pods table, lazily built alongside the other tables.
    kube_pods_table: Option<Entity<TableState<KubePodsDelegate>>>,
    /// Set once the Kubernetes sub-tab has triggered its first load (lazy, same pattern
    /// as `svc_loaded`/`docker_loaded`).
    kube_loaded: bool,
    kube_refreshing: bool,
    /// Bumped by every pods-fetch spawn (`refresh_network_kube`'s pods leg and
    /// `refresh_network_kube_pods`), and captured alongside the fetch. Round-D fix: a
    /// context switch used to call `refresh_network_kube_pods`, which early-returned
    /// (dropping the new fetch entirely) whenever `kube_refreshing` was already true —
    /// clicking a second context while the first fetch was still in flight silently
    /// stranded the table on the old context. The completion handler now applies its
    /// pods result only when `kube_fetch_generation` still matches what it captured at
    /// spawn time (see [`should_apply_pods`]), so a superseded fetch's late arrival is
    /// dropped instead of clobbering the newer selection.
    kube_fetch_generation: u64,
    /// `true` when the last probe returned `KubeError::NotInstalled` (covers both "no
    /// `kubectl` binary" and "no cluster reachable" — see `sid_core::containers::
    /// KubeError`'s doc comment) — the Kubernetes sub-tab renders the graceful
    /// "kubectl not installed — no cluster" notice instead of the context/pods UI.
    kube_not_installed: bool,
    /// A genuine (non-`NotInstalled`) probe failure, if any — from either the contexts
    /// or the pods fetch.
    kube_error: Option<String>,
    /// The one filter input shared by all five sub-tabs.
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
            docker_provider: Arc::new(DockerCliProvider::new()),
            docker_table: None,
            docker_loaded: false,
            docker_refreshing: false,
            docker_not_installed: false,
            docker_error: None,
            kube_provider: Arc::new(KubectlCliProvider::new()),
            kube_contexts: Vec::new(),
            kube_selected_context: None,
            kube_pods_table: None,
            kube_loaded: false,
            kube_refreshing: false,
            kube_fetch_generation: 0,
            kube_not_installed: false,
            kube_error: None,
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

    /// Move keyboard focus into the shared filter [`TextInput`] — `Action::FocusFilter`'s
    /// (`Ctrl+F`/`Ctrl+/`) target, dispatched from `app::dispatch_action`. A no-op before
    /// the Network tab has painted once (`ensure_network_widgets` hasn't built `filter`
    /// yet, e.g. `Ctrl+F` pressed while another primary tab is active — `dispatch_action`
    /// already gates the call on `active_tab == Tab::Network`, but the tab could in
    /// principle be active without ever having rendered).
    pub(crate) fn focus_filter(&self, window: &mut Window, cx: &App) {
        if let Some(filter) = &self.filter {
            filter.read(cx).focus(window);
        }
    }
}

/// Ascending/descending sort direction, derived from gpui-component's [`ColumnSort`] by
/// folding `ColumnSort::Default` into "no active sort" (see the `TableDelegate::
/// perform_sort` impls below — clicking a column cycles the library's own per-column
/// `ColumnSort` through `Default -> Descending -> Ascending -> Default`; `Default` clears
/// the delegate's `active_sort` rather than being tracked as a third direction here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    fn from_column_sort(sort: ColumnSort) -> Option<Self> {
        match sort {
            ColumnSort::Ascending => Some(SortDir::Asc),
            ColumnSort::Descending => Some(SortDir::Desc),
            ColumnSort::Default => None,
        }
    }

    /// Apply this direction to an `Ordering` computed by an ascending-order comparator.
    fn apply(self, order: Ordering) -> Ordering {
        match self {
            SortDir::Asc => order,
            SortDir::Desc => order.reverse(),
        }
    }
}

/// Set `columns[col_ix]`'s own [`ColumnSort`] marker to `sort` and reset every other
/// sortable column back to [`ColumnSort::Default`] — mirrors what `gpui_component::
/// table::TableState::perform_sort` does to its *internal* `col_groups` copy. Without
/// this, the delegate's own `columns` (the copy `TableDelegate::column` hands back on
/// every `TableState::refresh`, e.g. after a filter keystroke — see `recompute`'s
/// callers) would still report the construction-time `ColumnSort::Default` on every
/// sortable column, and the header's sort chevron would visually reset even though the
/// row order (driven by `active_sort`) stayed correctly sorted.
fn mark_column_sort(columns: &mut [Column], col_ix: usize, sort: ColumnSort) {
    for (ix, column) in columns.iter_mut().enumerate() {
        if column.sort.is_none() {
            continue;
        }
        column.sort = Some(if ix == col_ix {
            sort
        } else {
            ColumnSort::Default
        });
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
    /// The active sort column (index into `columns`) + direction, if any — `None` means
    /// unsorted (rows stay in probe order). Set by `TableDelegate::perform_sort`,
    /// applied by `recompute` after the filter.
    active_sort: Option<(usize, SortDir)>,
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
                Column::new("proto", "Proto").width(px(64.)).sortable(),
                Column::new("port", "Port").width(px(72.)).sortable(),
                Column::new("addr", "Addr").width(px(120.)).sortable(),
                Column::new("pid", "PID").width(px(80.)).sortable(),
                Column::new("process", "Process").width(px(240.)).sortable(),
                // Not sortable — an action column, not data.
                Column::new("kill", "").width(px(72.)),
            ],
            active_sort: None,
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

    /// Apply the filter, then the active sort (if any) — filtering never clears the
    /// sort, and sorting only ever reorders the already-filtered rows.
    fn recompute(&mut self) {
        let mut ports: Vec<ListeningPort> = filter_ports(&self.all_ports, &self.query)
            .into_iter()
            .cloned()
            .collect();
        if let Some((col_ix, dir)) = self.active_sort {
            sort_ports(&mut ports, col_ix, dir);
        }
        self.ports = ports;
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

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        mark_column_sort(&mut self.columns, col_ix, sort);
        self.active_sort = SortDir::from_column_sort(sort).map(|dir| (col_ix, dir));
        self.recompute();
        cx.notify();
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
                let label: SharedString = if port.local_addr.is_empty() {
                    "—".into()
                } else {
                    port.local_addr.clone().into()
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(label)
            }
            3 => {
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
            4 => {
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
    /// See [`PortsDelegate::active_sort`].
    active_sort: Option<(usize, SortDir)>,
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
                Column::new("name", "Unit").width(px(240.)).sortable(),
                Column::new("state", "State").width(px(90.)).sortable(),
                Column::new("sub_state", "Sub-state")
                    .width(px(110.))
                    .sortable(),
                Column::new("description", "Description")
                    .width(px(320.))
                    .sortable(),
                // Not sortable — an action column, not data.
                Column::new("actions", "").width(px(210.)),
            ],
            active_sort: None,
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

    /// Apply the filter, then the active sort — see [`PortsDelegate::recompute`].
    fn recompute(&mut self) {
        let mut services: Vec<ServiceInfo> = filter_services(&self.all_services, &self.query)
            .into_iter()
            .cloned()
            .collect();
        if let Some((col_ix, dir)) = self.active_sort {
            sort_services(&mut services, col_ix, dir);
        }
        self.services = services;
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

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        mark_column_sort(&mut self.columns, col_ix, sort);
        self.active_sort = SortDir::from_column_sort(sort).map(|dir| (col_ix, dir));
        self.recompute();
        cx.notify();
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
                let label: SharedString = if svc.sub_state.is_empty() {
                    "—".into()
                } else {
                    svc.sub_state.clone().into()
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(label)
            }
            3 => {
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

/// Backs the Docker [`Table`]. Read-only (no per-row actions, unlike
/// [`PortsDelegate`]/[`ServicesDelegate`]) — closer in shape to `db_tab.rs`'s
/// `ResultDelegate`: cache the full fetched set + the filtered display set, no
/// interactive state of its own.
struct DockerDelegate {
    all_containers: Vec<ContainerInfo>,
    containers: Vec<ContainerInfo>,
    query: String,
    columns: Vec<Column>,
    /// See [`PortsDelegate::active_sort`].
    active_sort: Option<(usize, SortDir)>,
}

impl DockerDelegate {
    fn new() -> Self {
        Self {
            all_containers: Vec::new(),
            containers: Vec::new(),
            query: String::new(),
            columns: vec![
                Column::new("name", "Name").width(px(200.)).sortable(),
                Column::new("image", "Image").width(px(220.)).sortable(),
                Column::new("state", "State").width(px(90.)).sortable(),
                Column::new("status", "Status").width(px(200.)).sortable(),
                Column::new("ports", "Ports").width(px(260.)).sortable(),
            ],
            active_sort: None,
        }
    }

    fn set_containers(&mut self, containers: Vec<ContainerInfo>) {
        self.all_containers = containers;
        self.recompute();
    }

    fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.recompute();
    }

    /// Apply the filter, then the active sort — see [`PortsDelegate::recompute`].
    fn recompute(&mut self) {
        let mut containers: Vec<ContainerInfo> =
            filter_containers(&self.all_containers, &self.query)
                .into_iter()
                .cloned()
                .collect();
        if let Some((col_ix, dir)) = self.active_sort {
            sort_containers(&mut containers, col_ix, dir);
        }
        self.containers = containers;
    }
}

impl TableDelegate for DockerDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.containers.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        mark_column_sort(&mut self.columns, col_ix, sort);
        self.active_sort = SortDir::from_column_sort(sort).map(|dir| (col_ix, dir));
        self.recompute();
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let container = &self.containers[row_ix];
        let cell_id = ("docker-cell", row_ix * 8 + col_ix);
        match col_ix {
            0 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG))
                .child(container.name.clone()),
            1 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(container.image.clone()),
            2 => {
                let (label, color) = docker_state_badge(&container.state);
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(color))
                    .child(label.to_string())
            }
            3 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(container.status.clone()),
            _ => {
                let label: SharedString = if container.ports.is_empty() {
                    "—".into()
                } else {
                    container.ports.join(", ").into()
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(label)
            }
        }
    }
}

/// Backs the Kubernetes pods [`Table`]. Same read-only shape as [`DockerDelegate`].
struct KubePodsDelegate {
    all_pods: Vec<KubePod>,
    pods: Vec<KubePod>,
    query: String,
    columns: Vec<Column>,
    /// See [`PortsDelegate::active_sort`].
    active_sort: Option<(usize, SortDir)>,
}

impl KubePodsDelegate {
    fn new() -> Self {
        Self {
            all_pods: Vec::new(),
            pods: Vec::new(),
            query: String::new(),
            columns: vec![
                Column::new("namespace", "Namespace")
                    .width(px(140.))
                    .sortable(),
                Column::new("name", "Name").width(px(240.)).sortable(),
                Column::new("ready", "Ready").width(px(70.)).sortable(),
                Column::new("phase", "Phase").width(px(90.)).sortable(),
                Column::new("restarts", "Restarts")
                    .width(px(80.))
                    .sortable(),
                Column::new("node", "Node").width(px(140.)).sortable(),
            ],
            active_sort: None,
        }
    }

    fn set_pods(&mut self, pods: Vec<KubePod>) {
        self.all_pods = pods;
        self.recompute();
    }

    fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.recompute();
    }

    /// Apply the filter, then the active sort — see [`PortsDelegate::recompute`].
    fn recompute(&mut self) {
        let mut pods: Vec<KubePod> = filter_pods(&self.all_pods, &self.query)
            .into_iter()
            .cloned()
            .collect();
        if let Some((col_ix, dir)) = self.active_sort {
            sort_pods(&mut pods, col_ix, dir);
        }
        self.pods = pods;
    }
}

impl TableDelegate for KubePodsDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.pods.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        mark_column_sort(&mut self.columns, col_ix, sort);
        self.active_sort = SortDir::from_column_sort(sort).map(|dir| (col_ix, dir));
        self.recompute();
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let pod = &self.pods[row_ix];
        let cell_id = ("kube-cell", row_ix * 8 + col_ix);
        match col_ix {
            0 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(pod.namespace.clone()),
            1 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG))
                .child(pod.name.clone()),
            2 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(pod.ready.clone()),
            3 => {
                let (label, color) = kube_phase_badge(&pod.phase);
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(color))
                    .child(label.to_string())
            }
            4 => {
                let color = if pod.restarts > 0 { DANGER } else { FG_DIM };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(color))
                    .child(pod.restarts.to_string())
            }
            _ => {
                let label: SharedString = if pod.node.is_empty() {
                    "—".into()
                } else {
                    pod.node.clone().into()
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(label)
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
        if self.network.sub_tab == NetSubTab::Docker && !self.network.docker_loaded {
            self.refresh_network_docker(cx);
        }
        if self.network.sub_tab == NetSubTab::Kubernetes && !self.network.kube_loaded {
            self.refresh_network_kube(cx);
        }

        let filter = self.network.filter.clone();
        let refreshing = match self.network.sub_tab {
            NetSubTab::Services => self.network.svc_refreshing,
            NetSubTab::Ports | NetSubTab::Interfaces => self.network.refreshing,
            NetSubTab::Docker => self.network.docker_refreshing,
            NetSubTab::Kubernetes => self.network.kube_refreshing,
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
        let container_count = self
            .network
            .docker_table
            .as_ref()
            .map(|t| t.read(cx).delegate().containers.len())
            .unwrap_or(0);
        let pod_count = self
            .network
            .kube_pods_table
            .as_ref()
            .map(|t| t.read(cx).delegate().pods.len())
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
            NetSubTab::Docker => {
                if self.network.docker_not_installed {
                    "docker not installed — no daemon reachable".into()
                } else {
                    match &self.network.docker_error {
                        Some(e) => format!("error: {e}").into(),
                        None if self.network.docker_refreshing => "refreshing…".into(),
                        None => format!("{container_count} containers").into(),
                    }
                }
            }
            NetSubTab::Kubernetes => {
                if self.network.kube_not_installed {
                    "kubectl not installed — no cluster".into()
                } else {
                    match &self.network.kube_error {
                        Some(e) => format!("error: {e}").into(),
                        None if self.network.kube_refreshing => "refreshing…".into(),
                        None => format!(
                            "{} contexts · {pod_count} pods",
                            self.network.kube_contexts.len()
                        )
                        .into(),
                    }
                }
            }
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
            NetSubTab::Docker => {
                if self.network.docker_not_installed {
                    graceful_absence_notice(
                        "docker not installed — no daemon reachable",
                        "install Docker and start the daemon to see containers here",
                    )
                    .into_any_element()
                } else {
                    self.network
                        .docker_table
                        .clone()
                        .map(|t| {
                            div()
                                .flex_1()
                                .w_full()
                                .child(Table::new(&t).stripe(true))
                                .into_any_element()
                        })
                        .unwrap_or_else(|| div().into_any_element())
                }
            }
            NetSubTab::Kubernetes => {
                if self.network.kube_not_installed {
                    graceful_absence_notice(
                        "kubectl not installed — no cluster",
                        "install kubectl and configure a context to see pods here",
                    )
                    .into_any_element()
                } else {
                    let context_strip = self.kube_context_strip(cx);
                    let table = self.network.kube_pods_table.clone();
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .child(context_strip)
                        .children(
                            table.map(|t| {
                                div().flex_1().w_full().child(Table::new(&t).stripe(true))
                            }),
                        )
                        .into_any_element()
                }
            }
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
                                    NetSubTab::Docker => this.refresh_network_docker(cx),
                                    NetSubTab::Kubernetes => this.refresh_network_kube(cx),
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

    /// The Kubernetes sub-tab's context strip: one chip per configured context
    /// (★ marks kubeconfig's own `current-context`), clicking a chip re-scopes the
    /// pods table to that context (`select_kube_context`). Mirrors
    /// [`Self::svc_scope_toggle`]'s shape; empty when no contexts are configured
    /// (kubectl installed but nothing set up yet — a valid non-`NotInstalled` state).
    fn kube_context_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let contexts = self.network.kube_contexts.clone();
        let selected = self.network.kube_selected_context.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(div().text_xs().text_color(rgb(FG_DIM)).child("context"))
            .when(contexts.is_empty(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(rgb(FG_DIM))
                        .child("no kube contexts configured"),
                )
            })
            .children(contexts.iter().enumerate().map(|(ix, ctx)| {
                // A context is "active" either because the user explicitly selected it,
                // or (nothing explicitly selected yet) because it's kubeconfig's own
                // `current-context` — matches which context `list_pods(None)` actually
                // queried on the initial load.
                let active = match &selected {
                    Some(name) => name == &ctx.name,
                    None => ctx.current,
                };
                let label: SharedString = if ctx.current {
                    format!("★ {}", ctx.name).into()
                } else {
                    ctx.name.clone().into()
                };
                let name = ctx.name.clone();
                div()
                    .id(("kube-context", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if active { ACTIVE_BG } else { BORDER }))
                    .text_color(rgb(if active { ACTIVE_FG } else { FG_DIM }))
                    .child(label)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.select_kube_context(Some(name.clone()), cx);
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

    /// Lazily build the ports table, the services table, the Docker/Kubernetes tables,
    /// and the shared filter input on first paint of the Network tab. Idempotent
    /// (checked every render) — mirrors `db_tab.rs`'s `ensure_query_widgets`.
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
        if self.network.docker_table.is_none() {
            let table = cx.new(|cx| TableState::new(DockerDelegate::new(), window, cx));
            self.network.docker_table = Some(table);
        }
        if self.network.kube_pods_table.is_none() {
            let table = cx.new(|cx| TableState::new(KubePodsDelegate::new(), window, cx));
            self.network.kube_pods_table = Some(table);
        }
        if self.network.filter.is_none() {
            let filter = cx.new(|cx| TextInput::new(cx, "filter"));
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
        if let Some(table) = self.network.docker_table.clone() {
            table.update(cx, |state, cx| {
                state.delegate_mut().set_query(&query);
                state.refresh(cx);
            });
        }
        if let Some(table) = self.network.kube_pods_table.clone() {
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
    /// cached list the way the filter box is.
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

    /// ⟳ refresh (Docker): re-fetch `docker ps -a` on the shared runtime. Also the
    /// Docker sub-tab's lazy first-load hook — `docker` is never called just because
    /// the Network tab is open, only once Docker is actually selected.
    pub(crate) fn refresh_network_docker(&mut self, cx: &mut Context<Self>) {
        if self.network.docker_refreshing {
            return;
        }
        self.network.docker_refreshing = true;
        self.network.docker_error = None;
        self.network.docker_not_installed = false;
        cx.notify();

        let provider = self.network.docker_provider.clone();
        let table = self.network.docker_table.clone();

        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move { provider.list_containers().await });
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                this.network.docker_refreshing = false;
                this.network.docker_loaded = true;
                match outcome {
                    Ok(Ok(containers)) => {
                        if let Some(table) = &table {
                            table.update(cx, |state, cx| {
                                state.delegate_mut().set_containers(containers);
                                state.refresh(cx);
                            });
                        }
                    }
                    // `NotInstalled` degrades to the dim graceful-absence notice
                    // (`docker_not_installed`), never `docker_error`'s red-ish "error:"
                    // line — see `NetworkTabState::docker_not_installed`'s doc comment.
                    Ok(Err(ContainerError::NotInstalled)) => {
                        this.network.docker_not_installed = true;
                        if let Some(table) = &table {
                            table.update(cx, |state, cx| {
                                state.delegate_mut().set_containers(Vec::new());
                                state.refresh(cx);
                            });
                        }
                    }
                    Ok(Err(e)) => this.network.docker_error = Some(e.to_string()),
                    Err(join_err) => {
                        this.network.docker_error =
                            Some(format!("docker probe task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Apply a Kubernetes contexts-fetch outcome: populate `kube_contexts`, folding a
    /// `KubeError::NotInstalled` into `kube_not_installed` rather than `kube_error` —
    /// see `NetworkTabState::kube_not_installed`'s doc comment.
    fn apply_kube_contexts(&mut self, contexts_res: Result<Vec<KubeContext>, KubeError>) {
        match contexts_res {
            Ok(contexts) => self.network.kube_contexts = contexts,
            Err(KubeError::NotInstalled) => {
                self.network.kube_not_installed = true;
                self.network.kube_contexts.clear();
            }
            Err(e) => self.network.kube_error = Some(e.to_string()),
        }
    }

    /// Apply a Kubernetes pods-fetch outcome into `table`, folding
    /// `KubeError::NotInstalled` into `kube_not_installed` the same way as
    /// [`Self::apply_kube_contexts`]. Shared by [`Self::refresh_network_kube`] (full
    /// reload) and [`Self::refresh_network_kube_pods`] (context-switch reload).
    fn apply_kube_pods(
        &mut self,
        pods_res: Result<Vec<KubePod>, KubeError>,
        table: Option<&Entity<TableState<KubePodsDelegate>>>,
        cx: &mut Context<Self>,
    ) {
        match pods_res {
            Ok(pods) => {
                if let Some(table) = table {
                    table.update(cx, |state, cx| {
                        state.delegate_mut().set_pods(pods);
                        state.refresh(cx);
                    });
                }
            }
            Err(KubeError::NotInstalled) => {
                self.network.kube_not_installed = true;
                if let Some(table) = table {
                    table.update(cx, |state, cx| {
                        state.delegate_mut().set_pods(Vec::new());
                        state.refresh(cx);
                    });
                }
            }
            Err(e) => {
                // Don't clobber a contexts-fetch error already recorded by
                // `apply_kube_contexts` in the same refresh pass.
                if self.network.kube_error.is_none() {
                    self.network.kube_error = Some(e.to_string());
                }
            }
        }
    }

    /// ⟳ refresh (Kubernetes): re-fetch the context list, then the pods table scoped to
    /// whichever context is selected (`None` = kubectl's own `current-context`), on the
    /// shared runtime. Also the Kubernetes sub-tab's lazy first-load hook.
    pub(crate) fn refresh_network_kube(&mut self, cx: &mut Context<Self>) {
        if self.network.kube_refreshing {
            return;
        }
        self.network.kube_refreshing = true;
        self.network.kube_error = None;
        self.network.kube_not_installed = false;
        self.network.kube_fetch_generation += 1;
        let generation = self.network.kube_fetch_generation;
        cx.notify();

        let provider = self.network.kube_provider.clone();
        let selected = self.network.kube_selected_context.clone();
        let table = self.network.kube_pods_table.clone();

        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                let contexts = provider.list_contexts().await;
                let pods = provider.list_pods(selected.as_deref()).await;
                (contexts, pods)
            });
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                this.network.kube_loaded = true;
                // Round-D generation guard: a context switch (`select_kube_context`)
                // may have started AFTER this fetch and already be the current
                // generation by the time this completes. Drop this stale result
                // instead of clobbering whatever the newer fetch already applied —
                // `kube_refreshing` is left for the current generation's own
                // completion to clear.
                if !should_apply_pods(this.network.kube_fetch_generation, generation) {
                    return;
                }
                this.network.kube_refreshing = false;
                match outcome {
                    Ok((contexts_res, pods_res)) => {
                        this.apply_kube_contexts(contexts_res);
                        this.apply_kube_pods(pods_res, table.as_ref(), cx);
                    }
                    Err(join_err) => {
                        this.network.kube_error =
                            Some(format!("kube probe task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-fetch only the pods table for the currently selected context — used when the
    /// user clicks a different context chip. Doesn't re-fetch `kube_contexts` (the
    /// context list itself didn't change, just which context the pods table is scoped
    /// to).
    ///
    /// Deliberately does NOT early-return on `kube_refreshing` (round-D fix): its only
    /// caller, `select_kube_context`, must always spawn a fresh fetch for the newly
    /// selected context even if a previous fetch is still in flight — otherwise a
    /// second context click during that window used to be silently dropped, stranding
    /// the pods table on the old context. Correctness against a stale in-flight fetch
    /// is via the generation guard (`kube_fetch_generation`/[`should_apply_pods`]), not
    /// reentrancy prevention.
    pub(crate) fn refresh_network_kube_pods(&mut self, cx: &mut Context<Self>) {
        self.network.kube_refreshing = true;
        self.network.kube_error = None;
        self.network.kube_fetch_generation += 1;
        let generation = self.network.kube_fetch_generation;
        cx.notify();

        let provider = self.network.kube_provider.clone();
        let selected = self.network.kube_selected_context.clone();
        let table = self.network.kube_pods_table.clone();

        cx.spawn(async move |this, cx| {
            let handle =
                ssh_runtime().spawn(async move { provider.list_pods(selected.as_deref()).await });
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                if !should_apply_pods(this.network.kube_fetch_generation, generation) {
                    return;
                }
                this.network.kube_refreshing = false;
                match outcome {
                    Ok(pods_res) => this.apply_kube_pods(pods_res, table.as_ref(), cx),
                    Err(join_err) => {
                        this.network.kube_error =
                            Some(format!("kube probe task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Switch the Kubernetes sub-tab's selected context and re-fetch just the pods
    /// table for it — mirrors `set_svc_scope`'s "switching context forces a fresh
    /// fetch, it isn't a filter over one cached list" reasoning.
    fn select_kube_context(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.network.kube_selected_context == name {
            return;
        }
        self.network.kube_selected_context = name;
        self.refresh_network_kube_pods(cx);
    }
}

/// Whether a completed Kubernetes pods fetch should still be applied to the table: only
/// when its captured generation still matches the current one. `select_kube_context`
/// (via `refresh_network_kube_pods`) and `refresh_network_kube` each bump
/// `kube_fetch_generation` before spawning their own fetch, so a fetch that finishes
/// AFTER being superseded by a newer one reports a stale `fetched_gen` here and its
/// result is dropped rather than clobbering the newer selection's pods with stale data.
fn should_apply_pods(current_gen: u64, fetched_gen: u64) -> bool {
    current_gen == fetched_gen
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

/// Column-index dispatch for the Ports table's sort — `col_ix` matches
/// [`PortsDelegate::columns`]' order (`proto, port, addr, pid, process`; `kill` at
/// index 5 carries no comparator and is unreachable since it's never marked
/// `.sortable()`, so `TableState::perform_sort` never calls back with that index).
fn sort_ports(rows: &mut [ListeningPort], col_ix: usize, dir: SortDir) {
    let cmp: fn(&ListeningPort, &ListeningPort) -> Ordering = match col_ix {
        0 => cmp_port_proto,
        1 => cmp_port_number,
        2 => cmp_port_addr,
        3 => cmp_port_pid,
        4 => cmp_port_process,
        _ => return,
    };
    rows.sort_by(|a, b| dir.apply(cmp(a, b)));
}

/// Numeric port compare — never lexicographic (`9 < 10 < 443 < 8080`, not the string
/// order `10 < 443 < 8080 < 9`).
fn cmp_port_number(a: &ListeningPort, b: &ListeningPort) -> Ordering {
    a.port.cmp(&b.port)
}

fn cmp_port_proto(a: &ListeningPort, b: &ListeningPort) -> Ordering {
    protocol_label(a.protocol).cmp(protocol_label(b.protocol))
}

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
    }
}

fn cmp_port_addr(a: &ListeningPort, b: &ListeningPort) -> Ordering {
    a.local_addr.cmp(&b.local_addr)
}

/// Case-insensitive process/command compare.
fn cmp_port_process(a: &ListeningPort, b: &ListeningPort) -> Ordering {
    a.command.to_lowercase().cmp(&b.command.to_lowercase())
}

/// Numeric PID compare (never lexicographic). A row with no attributable PID (`None`)
/// sorts after every row that has one when ascending; `SortDir::apply`'s blanket
/// `Ordering::reverse` for descending flips that leg too, so `None` rows sort first
/// when descending — the same thing that happens to every other value under reversal,
/// not a separate "blanks always last" rule.
fn cmp_port_pid(a: &ListeningPort, b: &ListeningPort) -> Ordering {
    match (a.pid, b.pid) {
        (Some(x), Some(y)) => x.as_u32().cmp(&y.as_u32()),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
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

/// Column-index dispatch for the Services table's sort — `col_ix` matches
/// [`ServicesDelegate::columns`]' order (`name, state, sub_state, description`;
/// `actions` at index 4 carries no comparator, same reasoning as [`sort_ports`]'s
/// `kill` column).
fn sort_services(rows: &mut [ServiceInfo], col_ix: usize, dir: SortDir) {
    let cmp: fn(&ServiceInfo, &ServiceInfo) -> Ordering = match col_ix {
        0 => cmp_svc_name,
        1 => cmp_svc_state,
        2 => cmp_svc_sub_state,
        3 => cmp_svc_description,
        _ => return,
    };
    rows.sort_by(|a, b| dir.apply(cmp(a, b)));
}

/// Case-insensitive unit-name compare.
fn cmp_svc_name(a: &ServiceInfo, b: &ServiceInfo) -> Ordering {
    a.name.to_lowercase().cmp(&b.name.to_lowercase())
}

/// Active-first rank used by [`cmp_svc_state`]: `active < activating < reloading <
/// (any other/unrecognized "other" sub-state) < inactive < failed`.
/// `SvcActiveState::Other` folds systemd's `activating`/`deactivating`/`reloading`
/// together (see the enum's own doc comment) — `sub_state`'s text is what actually
/// distinguishes them for ranking purposes.
fn svc_state_rank(svc: &ServiceInfo) -> u8 {
    match svc.active {
        SvcActiveState::Active => 0,
        SvcActiveState::Other if svc.sub_state.eq_ignore_ascii_case("activating") => 1,
        SvcActiveState::Other if svc.sub_state.eq_ignore_ascii_case("reloading") => 2,
        SvcActiveState::Other => 3,
        SvcActiveState::Inactive => 4,
        SvcActiveState::Failed => 5,
    }
}

/// State rank first, then alphabetical by name for units sharing a rank.
fn cmp_svc_state(a: &ServiceInfo, b: &ServiceInfo) -> Ordering {
    svc_state_rank(a)
        .cmp(&svc_state_rank(b))
        .then_with(|| cmp_svc_name(a, b))
}

fn cmp_svc_sub_state(a: &ServiceInfo, b: &ServiceInfo) -> Ordering {
    a.sub_state.cmp(&b.sub_state)
}

fn cmp_svc_description(a: &ServiceInfo, b: &ServiceInfo) -> Ordering {
    a.description.cmp(&b.description)
}

/// Case-insensitive filter over the Docker containers table: name, image, state, or
/// status substring. Empty (or all-whitespace) query matches everything.
fn filter_containers<'a>(containers: &'a [ContainerInfo], query: &str) -> Vec<&'a ContainerInfo> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return containers.iter().collect();
    }
    containers
        .iter()
        .filter(|c| {
            c.name.to_lowercase().contains(&query)
                || c.image.to_lowercase().contains(&query)
                || c.state.to_lowercase().contains(&query)
                || c.status.to_lowercase().contains(&query)
        })
        .collect()
}

/// Case-insensitive filter over the Kubernetes pods table: namespace or name
/// substring. Empty (or all-whitespace) query matches everything.
fn filter_pods<'a>(pods: &'a [KubePod], query: &str) -> Vec<&'a KubePod> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return pods.iter().collect();
    }
    pods.iter()
        .filter(|p| {
            p.namespace.to_lowercase().contains(&query) || p.name.to_lowercase().contains(&query)
        })
        .collect()
}

/// Badge label + color for a Docker container's coarse lifecycle state
/// (`docker ps`'s `.State`: `"running"`, `"exited"`, `"paused"`, `"restarting"`, ...).
/// Unrecognized values pass through verbatim with the dim "other" color rather than
/// erroring — same "still render the row" rule as `svc_state_badge`.
fn docker_state_badge(state: &str) -> (&str, u32) {
    match state {
        "running" => ("running", OK_GREEN),
        "exited" | "dead" => ("exited", FG_DIM),
        "paused" => ("paused", FG_DIM),
        "restarting" => ("restarting", DANGER),
        other => (other, FG_DIM),
    }
}

/// Column-index dispatch for the Docker table's sort — `col_ix` matches
/// [`DockerDelegate::columns`]' order (`name, image, state, status, ports`, all five
/// sortable).
fn sort_containers(rows: &mut [ContainerInfo], col_ix: usize, dir: SortDir) {
    let cmp: fn(&ContainerInfo, &ContainerInfo) -> Ordering = match col_ix {
        0 => cmp_container_name,
        1 => cmp_container_image,
        2 => cmp_container_state,
        3 => cmp_container_status,
        4 => cmp_container_ports,
        _ => return,
    };
    rows.sort_by(|a, b| dir.apply(cmp(a, b)));
}

fn cmp_container_name(a: &ContainerInfo, b: &ContainerInfo) -> Ordering {
    a.name.cmp(&b.name)
}

fn cmp_container_image(a: &ContainerInfo, b: &ContainerInfo) -> Ordering {
    a.image.cmp(&b.image)
}

/// Lifecycle rank used by [`cmp_container_state`]: `running < paused < restarting <
/// (exited/dead) < anything unrecognized` — matches [`docker_state_badge`]'s known
/// values, with the transient `restarting` state (already flagged `DANGER` there)
/// placed ahead of the terminal `exited`/`dead` states.
fn docker_state_rank(state: &str) -> u8 {
    match state {
        "running" => 0,
        "paused" => 1,
        "restarting" => 2,
        "exited" | "dead" => 3,
        _ => 4,
    }
}

/// State rank first, then name for containers sharing a rank.
fn cmp_container_state(a: &ContainerInfo, b: &ContainerInfo) -> Ordering {
    docker_state_rank(&a.state)
        .cmp(&docker_state_rank(&b.state))
        .then_with(|| cmp_container_name(a, b))
}

fn cmp_container_status(a: &ContainerInfo, b: &ContainerInfo) -> Ordering {
    a.status.cmp(&b.status)
}

/// Compares the same joined label `render_td` displays (`"host:5432->5432/tcp, ..."`).
fn cmp_container_ports(a: &ContainerInfo, b: &ContainerInfo) -> Ordering {
    a.ports.join(", ").cmp(&b.ports.join(", "))
}

/// Badge label + color for a Kubernetes pod's phase (`status.phase`).
fn kube_phase_badge(phase: &str) -> (&str, u32) {
    match phase {
        "Running" => ("Running", OK_GREEN),
        "Succeeded" => ("Succeeded", OK_GREEN),
        "Pending" => ("Pending", FG_DIM),
        "Failed" => ("Failed", DANGER),
        "Unknown" => ("Unknown", DANGER),
        other => (other, FG_DIM),
    }
}

/// Column-index dispatch for the Kubernetes pods table's sort — `col_ix` matches
/// [`KubePodsDelegate::columns`]' order (`namespace, name, ready, phase, restarts,
/// node`, all six sortable).
fn sort_pods(rows: &mut [KubePod], col_ix: usize, dir: SortDir) {
    let cmp: fn(&KubePod, &KubePod) -> Ordering = match col_ix {
        0 => cmp_pod_namespace,
        1 => cmp_pod_name,
        2 => cmp_pod_ready,
        3 => cmp_pod_phase,
        4 => cmp_pod_restarts,
        5 => cmp_pod_node,
        _ => return,
    };
    rows.sort_by(|a, b| dir.apply(cmp(a, b)));
}

fn cmp_pod_namespace(a: &KubePod, b: &KubePod) -> Ordering {
    a.namespace.cmp(&b.namespace)
}

fn cmp_pod_name(a: &KubePod, b: &KubePod) -> Ordering {
    a.name.cmp(&b.name)
}

fn cmp_pod_ready(a: &KubePod, b: &KubePod) -> Ordering {
    a.ready.cmp(&b.ready)
}

/// Running-first rank used by [`cmp_pod_phase`], mirroring [`kube_phase_badge`]'s known
/// values: `Running < Succeeded < Pending < Failed < Unknown < anything unrecognized`.
fn kube_phase_rank(phase: &str) -> u8 {
    match phase {
        "Running" => 0,
        "Succeeded" => 1,
        "Pending" => 2,
        "Failed" => 3,
        "Unknown" => 4,
        _ => 5,
    }
}

/// Phase rank first, then name for pods sharing a rank.
fn cmp_pod_phase(a: &KubePod, b: &KubePod) -> Ordering {
    kube_phase_rank(&a.phase)
        .cmp(&kube_phase_rank(&b.phase))
        .then_with(|| cmp_pod_name(a, b))
}

/// Numeric restart-count compare — never lexicographic.
fn cmp_pod_restarts(a: &KubePod, b: &KubePod) -> Ordering {
    a.restarts.cmp(&b.restarts)
}

fn cmp_pod_node(a: &KubePod, b: &KubePod) -> Ordering {
    a.node.cmp(&b.node)
}

/// Two-tier graceful-absence notice, reusing the same "dim status line + secondary
/// detail line" shape already used for the Ports/Services error paths (`sub` line +
/// `kill_error`/`action_error` line in `network_tab`) — here both tiers are
/// intentionally dim (not `DANGER`-colored), since "docker/kubectl isn't set up" is an
/// expected, common local-machine state, not a failure.
fn graceful_absence_notice(headline: &str, detail: &str) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_4()
        .py_6()
        .items_center()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(FG))
                .child(headline.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(detail.to_string()),
        )
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

    fn container(name: &str, image: &str, state: &str, status: &str) -> ContainerInfo {
        ContainerInfo {
            id: "abc123".to_string(),
            name: name.to_string(),
            image: image.to_string(),
            state: state.to_string(),
            status: status.to_string(),
            ports: Vec::new(),
        }
    }

    fn pod(namespace: &str, name: &str) -> KubePod {
        KubePod {
            namespace: namespace.to_string(),
            name: name.to_string(),
            ready: "1/1".to_string(),
            phase: "Running".to_string(),
            restarts: 0,
            node: "node-1".to_string(),
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
    fn should_apply_pods_matches_only_the_current_generation() {
        // The fetch that's still current when it completes wins.
        assert!(should_apply_pods(1, 1));
        // Round-D bug: a context switch bumps the generation before its own fetch
        // completes -- an OLDER fetch (still carrying the stale generation it
        // captured at spawn time) arriving late must be dropped, not applied.
        assert!(!should_apply_pods(2, 1));
        // A fetch somehow reporting a generation ahead of the current one (should
        // never happen since the counter only increases) is also not "current".
        assert!(!should_apply_pods(1, 2));
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
    fn filter_containers_empty_query_matches_all() {
        let containers = vec![
            container("web-1", "nginx:latest", "running", "Up 1 hour"),
            container("db-1", "postgres:16", "exited", "Exited (0) 2 days ago"),
        ];
        assert_eq!(filter_containers(&containers, "").len(), 2);
        assert_eq!(filter_containers(&containers, "   ").len(), 2);
    }

    #[test]
    fn filter_containers_matches_name_case_insensitively() {
        let containers = vec![
            container("web-1", "nginx:latest", "running", "Up"),
            container("db-1", "postgres:16", "running", "Up"),
        ];
        let got: Vec<&str> = filter_containers(&containers, "WEB")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(got, vec!["web-1"]);
    }

    #[test]
    fn filter_containers_matches_image_state_or_status() {
        let containers = vec![
            container("a", "postgres:16", "running", "Up 3 hours"),
            container("b", "redis:7", "exited", "Exited (0) 2 days ago"),
        ];
        assert_eq!(filter_containers(&containers, "postgres").len(), 1);
        assert_eq!(filter_containers(&containers, "exited").len(), 1);
        assert_eq!(filter_containers(&containers, "2 days").len(), 1);
        assert_eq!(filter_containers(&containers, "nonexistent").len(), 0);
    }

    #[test]
    fn docker_state_badge_covers_known_and_unknown_states() {
        assert_eq!(docker_state_badge("running").0, "running");
        assert_eq!(docker_state_badge("exited").0, "exited");
        assert_eq!(docker_state_badge("dead").0, "exited");
        assert_eq!(docker_state_badge("paused").0, "paused");
        assert_eq!(docker_state_badge("restarting").0, "restarting");
        // Unrecognized values pass through verbatim rather than erroring.
        assert_eq!(
            docker_state_badge("weird-future-state").0,
            "weird-future-state"
        );
    }

    #[test]
    fn filter_pods_empty_query_matches_all() {
        let pods = vec![pod("default", "web-1"), pod("kube-system", "coredns-1")];
        assert_eq!(filter_pods(&pods, "").len(), 2);
    }

    #[test]
    fn filter_pods_matches_namespace_or_name_case_insensitively() {
        let pods = vec![pod("default", "web-1"), pod("kube-system", "coredns-1")];
        let by_ns: Vec<&str> = filter_pods(&pods, "KUBE-SYS")
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(by_ns, vec!["coredns-1"]);
        let by_name: Vec<&str> = filter_pods(&pods, "WEB")
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(by_name, vec!["web-1"]);
    }

    #[test]
    fn kube_phase_badge_covers_known_and_unknown_phases() {
        assert_eq!(kube_phase_badge("Running").0, "Running");
        assert_eq!(kube_phase_badge("Succeeded").0, "Succeeded");
        assert_eq!(kube_phase_badge("Pending").0, "Pending");
        assert_eq!(kube_phase_badge("Failed").0, "Failed");
        assert_eq!(kube_phase_badge("Unknown").0, "Unknown");
        assert_eq!(kube_phase_badge("SomeFuturePhase").0, "SomeFuturePhase");
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

    // ---- SortDir / mark_column_sort ---------------------------------------------

    #[test]
    fn sort_dir_from_column_sort_maps_ascending_and_descending_only() {
        assert_eq!(
            SortDir::from_column_sort(ColumnSort::Ascending),
            Some(SortDir::Asc)
        );
        assert_eq!(
            SortDir::from_column_sort(ColumnSort::Descending),
            Some(SortDir::Desc)
        );
        assert_eq!(SortDir::from_column_sort(ColumnSort::Default), None);
    }

    #[test]
    fn sort_dir_apply_reverses_only_for_descending() {
        assert_eq!(SortDir::Asc.apply(Ordering::Less), Ordering::Less);
        assert_eq!(SortDir::Desc.apply(Ordering::Less), Ordering::Greater);
        assert_eq!(SortDir::Asc.apply(Ordering::Equal), Ordering::Equal);
        assert_eq!(SortDir::Desc.apply(Ordering::Equal), Ordering::Equal);
    }

    #[test]
    fn mark_column_sort_sets_target_and_clears_other_sortable_columns() {
        let mut columns = vec![
            Column::new("a", "A").sortable(),
            Column::new("b", "B").sortable(),
            // Not sortable to begin with — must stay `None`, not get stamped `Default`.
            Column::new("c", "C"),
        ];
        // Simulate column "a" having been sorted descending on a previous click.
        columns[0].sort = Some(ColumnSort::Descending);

        mark_column_sort(&mut columns, 1, ColumnSort::Ascending);

        assert_eq!(columns[0].sort, Some(ColumnSort::Default));
        assert_eq!(columns[1].sort, Some(ColumnSort::Ascending));
        assert_eq!(columns[2].sort, None);
    }

    // ---- Ports: sort comparators --------------------------------------------------

    #[test]
    fn cmp_port_number_orders_numerically_not_lexicographically() {
        let mut ports = vec![
            port(8080, None, "", ""),
            port(9, None, "", ""),
            port(443, None, "", ""),
            port(10, None, "", ""),
        ];
        sort_ports(&mut ports, 1, SortDir::Asc);
        let nums: Vec<u16> = ports.iter().map(|p| p.port).collect();
        // Lexicographic order would give 10, 443, 8080, 9 — numeric order must not.
        assert_eq!(nums, vec![9, 10, 443, 8080]);
    }

    #[test]
    fn cmp_port_number_descending_reverses_numeric_order() {
        let mut ports = vec![port(9, None, "", ""), port(443, None, "", "")];
        sort_ports(&mut ports, 1, SortDir::Desc);
        let nums: Vec<u16> = ports.iter().map(|p| p.port).collect();
        assert_eq!(nums, vec![443, 9]);
    }

    #[test]
    fn cmp_port_proto_orders_tcp_before_udp() {
        let mut udp = port(53, None, "", "");
        udp.protocol = Protocol::Udp;
        let tcp = port(80, None, "", "");
        let mut ports = vec![udp, tcp.clone()];
        sort_ports(&mut ports, 0, SortDir::Asc);
        assert_eq!(ports[0].port, tcp.port);
    }

    #[test]
    fn cmp_port_addr_orders_lexicographically() {
        let mut ports = vec![port(1, None, "", "127.0.0.1"), port(2, None, "", "0.0.0.0")];
        sort_ports(&mut ports, 2, SortDir::Asc);
        let addrs: Vec<&str> = ports.iter().map(|p| p.local_addr.as_str()).collect();
        assert_eq!(addrs, vec!["0.0.0.0", "127.0.0.1"]);
    }

    #[test]
    fn cmp_port_process_is_case_insensitive() {
        let mut ports = vec![
            port(1, None, "Zsh", ""),
            port(2, None, "bash", ""),
            port(3, None, "Ash", ""),
        ];
        sort_ports(&mut ports, 4, SortDir::Asc);
        let commands: Vec<&str> = ports.iter().map(|p| p.command.as_str()).collect();
        assert_eq!(commands, vec!["Ash", "bash", "Zsh"]);
    }

    #[test]
    fn cmp_port_pid_none_sorts_after_some_ascending_and_before_descending() {
        let mut ports = vec![
            port(1, None, "", ""),
            port(2, Some(456), "", ""),
            port(3, Some(123), "", ""),
        ];
        sort_ports(&mut ports, 3, SortDir::Asc);
        let pids: Vec<Option<u32>> = ports.iter().map(|p| p.pid.map(Pid::as_u32)).collect();
        assert_eq!(pids, vec![Some(123), Some(456), None]);

        sort_ports(&mut ports, 3, SortDir::Desc);
        let pids: Vec<Option<u32>> = ports.iter().map(|p| p.pid.map(Pid::as_u32)).collect();
        assert_eq!(pids, vec![None, Some(456), Some(123)]);
    }

    #[test]
    fn sort_ports_unsortable_column_index_is_a_no_op() {
        let mut ports = vec![port(80, None, "b", ""), port(22, None, "a", "")];
        let before: Vec<u16> = ports.iter().map(|p| p.port).collect();
        // Index 5 is the `kill` action column — never `.sortable()`, so `sort_ports`
        // must leave row order untouched rather than panic on an out-of-range match.
        sort_ports(&mut ports, 5, SortDir::Asc);
        let after: Vec<u16> = ports.iter().map(|p| p.port).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn filter_then_sort_composes_for_ports() {
        // Mirrors `PortsDelegate::recompute`'s pipeline: filter first, then sort —
        // filtering must never lose or reorder-around the active sort.
        let ports = vec![
            port(8080, Some(3), "app", "0.0.0.0"),
            port(9, Some(1), "small", "127.0.0.1"), // filtered out below
            port(443, Some(2), "https", "0.0.0.0"),
            port(80, Some(4), "http", "0.0.0.0"),
        ];
        let mut filtered: Vec<ListeningPort> = filter_ports(&ports, "0.0.0.0")
            .into_iter()
            .cloned()
            .collect();
        sort_ports(&mut filtered, 1, SortDir::Asc);
        let nums: Vec<u16> = filtered.iter().map(|p| p.port).collect();
        assert_eq!(nums, vec![80, 443, 8080]);
    }

    // ---- Services: sort comparators ------------------------------------------------

    fn svc_with_state(name: &str, active: SvcActiveState, sub_state: &str) -> ServiceInfo {
        ServiceInfo {
            name: name.to_string(),
            description: String::new(),
            active,
            sub_state: sub_state.to_string(),
        }
    }

    #[test]
    fn svc_state_rank_orders_active_first() {
        let active = svc_with_state("a", SvcActiveState::Active, "running");
        let activating = svc_with_state("b", SvcActiveState::Other, "activating");
        let reloading = svc_with_state("c", SvcActiveState::Other, "reloading");
        let inactive = svc_with_state("d", SvcActiveState::Inactive, "dead");
        let failed = svc_with_state("e", SvcActiveState::Failed, "failed");

        assert!(svc_state_rank(&active) < svc_state_rank(&activating));
        assert!(svc_state_rank(&activating) < svc_state_rank(&reloading));
        assert!(svc_state_rank(&reloading) < svc_state_rank(&inactive));
        assert!(svc_state_rank(&inactive) < svc_state_rank(&failed));
    }

    #[test]
    fn cmp_svc_state_sorts_by_rank_then_name() {
        let mut services = vec![
            svc_with_state("z-active", SvcActiveState::Active, "running"),
            svc_with_state("nginx.service", SvcActiveState::Failed, "failed"),
            svc_with_state("a-active", SvcActiveState::Active, "running"),
        ];
        sort_services(&mut services, 1, SortDir::Asc);
        let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["a-active", "z-active", "nginx.service"]);
    }

    #[test]
    fn cmp_svc_name_is_case_insensitive() {
        let mut services = vec![svc("Zsh.service", ""), svc("bash.service", "")];
        sort_services(&mut services, 0, SortDir::Asc);
        let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["bash.service", "Zsh.service"]);
    }

    #[test]
    fn cmp_svc_sub_state_orders_lexicographically() {
        let mut services = vec![
            svc_with_state("a", SvcActiveState::Active, "running"),
            svc_with_state("b", SvcActiveState::Active, "dead"),
        ];
        sort_services(&mut services, 2, SortDir::Asc);
        let sub_states: Vec<&str> = services.iter().map(|s| s.sub_state.as_str()).collect();
        assert_eq!(sub_states, vec!["dead", "running"]);
    }

    #[test]
    fn cmp_svc_description_orders_lexicographically() {
        let mut services = vec![svc("a", "web server"), svc("b", "database")];
        sort_services(&mut services, 3, SortDir::Asc);
        let descriptions: Vec<&str> = services.iter().map(|s| s.description.as_str()).collect();
        assert_eq!(descriptions, vec!["database", "web server"]);
    }

    // ---- Docker: sort comparators ---------------------------------------------------

    #[test]
    fn docker_state_rank_orders_running_first() {
        assert!(docker_state_rank("running") < docker_state_rank("paused"));
        assert!(docker_state_rank("paused") < docker_state_rank("exited"));
        assert!(docker_state_rank("running") < docker_state_rank("exited"));
    }

    #[test]
    fn cmp_container_state_sorts_running_before_exited() {
        let mut containers = vec![
            container("b", "img", "exited", ""),
            container("a", "img", "running", ""),
            container("c", "img", "paused", ""),
        ];
        sort_containers(&mut containers, 2, SortDir::Asc);
        let names: Vec<&str> = containers.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "c", "b"]);
    }

    #[test]
    fn cmp_container_name_orders_lexicographically() {
        let mut containers = vec![
            container("web-2", "img", "running", ""),
            container("web-1", "img", "running", ""),
        ];
        sort_containers(&mut containers, 0, SortDir::Asc);
        let names: Vec<&str> = containers.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["web-1", "web-2"]);
    }

    #[test]
    fn cmp_container_image_orders_lexicographically() {
        let mut containers = vec![
            container("a", "redis:7", "running", ""),
            container("b", "nginx:latest", "running", ""),
        ];
        sort_containers(&mut containers, 1, SortDir::Asc);
        let images: Vec<&str> = containers.iter().map(|c| c.image.as_str()).collect();
        assert_eq!(images, vec!["nginx:latest", "redis:7"]);
    }

    #[test]
    fn cmp_container_status_orders_lexicographically() {
        let mut containers = vec![
            container("a", "img", "running", "Up 3 hours"),
            container("b", "img", "exited", "Exited (0) 2 days ago"),
        ];
        sort_containers(&mut containers, 3, SortDir::Asc);
        let statuses: Vec<&str> = containers.iter().map(|c| c.status.as_str()).collect();
        assert_eq!(statuses, vec!["Exited (0) 2 days ago", "Up 3 hours"]);
    }

    #[test]
    fn cmp_container_ports_orders_by_joined_label() {
        let mut a = container("a", "img", "running", "");
        a.ports = vec!["0.0.0.0:8080->80/tcp".to_string()];
        let mut b = container("b", "img", "running", "");
        b.ports = vec!["0.0.0.0:5432->5432/tcp".to_string()];
        let mut containers = vec![a.clone(), b.clone()];
        sort_containers(&mut containers, 4, SortDir::Asc);
        assert_eq!(containers[0].name, b.name);
    }

    // ---- Kubernetes: sort comparators ------------------------------------------------

    #[test]
    fn kube_phase_rank_orders_running_first() {
        assert!(kube_phase_rank("Running") < kube_phase_rank("Succeeded"));
        assert!(kube_phase_rank("Running") < kube_phase_rank("Pending"));
        assert!(kube_phase_rank("Running") < kube_phase_rank("Failed"));
        assert!(kube_phase_rank("Running") < kube_phase_rank("Unknown"));
    }

    #[test]
    fn cmp_pod_phase_sorts_running_before_others() {
        let mut a = pod("default", "a");
        a.phase = "Failed".to_string();
        let mut b = pod("default", "b");
        b.phase = "Running".to_string();
        let mut pods = vec![a, b];
        sort_pods(&mut pods, 3, SortDir::Asc);
        assert_eq!(pods[0].name, "b");
    }

    #[test]
    fn cmp_pod_restarts_orders_numerically_not_lexicographically() {
        let mut p9 = pod("default", "nine");
        p9.restarts = 9;
        let mut p10 = pod("default", "ten");
        p10.restarts = 10;
        let mut p2 = pod("default", "two");
        p2.restarts = 2;
        let mut pods = vec![p9, p10, p2];
        sort_pods(&mut pods, 4, SortDir::Asc);
        let restarts: Vec<u32> = pods.iter().map(|p| p.restarts).collect();
        assert_eq!(restarts, vec![2, 9, 10]);
    }

    #[test]
    fn cmp_pod_namespace_orders_lexicographically() {
        let mut pods = vec![pod("kube-system", "a"), pod("default", "b")];
        sort_pods(&mut pods, 0, SortDir::Asc);
        let namespaces: Vec<&str> = pods.iter().map(|p| p.namespace.as_str()).collect();
        assert_eq!(namespaces, vec!["default", "kube-system"]);
    }

    #[test]
    fn cmp_pod_name_orders_lexicographically() {
        let mut pods = vec![pod("default", "web-2"), pod("default", "web-1")];
        sort_pods(&mut pods, 1, SortDir::Asc);
        let names: Vec<&str> = pods.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["web-1", "web-2"]);
    }

    #[test]
    fn cmp_pod_ready_and_node_order_lexicographically() {
        let mut a = pod("default", "a");
        a.ready = "0/1".to_string();
        a.node = "node-2".to_string();
        let mut b = pod("default", "b");
        b.ready = "1/1".to_string();
        b.node = "node-1".to_string();
        let mut pods = vec![a, b];

        sort_pods(&mut pods, 2, SortDir::Asc);
        assert_eq!(pods[0].ready, "0/1");

        sort_pods(&mut pods, 5, SortDir::Asc);
        assert_eq!(pods[0].node, "node-1");
    }
}
