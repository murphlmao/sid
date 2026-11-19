//! Services: systemd units through `sid_core::svc::ServiceProvider`, a sub-view of the
//! System tab (`[Processes] [Services] [Config files]`).
//!
//! Moved out of the Network tab on 2026-11-19 (Murphy: "services does not belong in
//! network, it belongs in system"). Like the rest of System it is live and ephemeral:
//! no store, no scope, nothing committed. `crates/sid` is the one crate allowed to name
//! `sid_svcctl`'s concrete `SvcctlProvider::new()`; every call after construction goes
//! back out through the `sid_core::svc::ServiceProvider` trait.
//!
//! ## Refresh
//!
//! Lazy and manual, exactly as it was under Network: the first `list_services` runs the
//! first time the sub-view renders, then only on the refresh button or a scope switch.
//! There is no timer, so a hidden System tab or an unfocused window costs nothing
//! (CLAUDE.md rule 4); the System tab's own 2-second loop drives the overview and the
//! processes table only.
//!
//! ## Rows
//!
//! [`ServicesDelegate`] owns the two-click confirm for restart/stop/kill and spawns its
//! own control task, since `render_td`'s `cx` is scoped to the table entity, not
//! `AppState`. System-scope actions need root: a non-root sid shows `needs root` in the
//! action cell instead of buttons.

use std::cmp::Ordering;
use std::sync::Arc;

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, Entity, IntoElement, SharedString, Subscription,
    Window, div, prelude::*, px, rgb,
};
use sid_core::svc::{ServiceInfo, ServiceProvider, SvcAction, SvcActiveState, SvcScope};
use sid_svcctl::SvcctlProvider;

use crate::app::AppState;
use crate::ui::network_tab::{
    SelectableRows, SortDir, Tone, badge_tone, data_cell, empty_guidance, notice_row,
    reveal_on_row, row_group, table_pane, track_selection,
};
use crate::ui::session::ssh_runtime;
use sid_ui::component::{Column, ColumnSort, TableDelegate, TableState};
use sid_ui::theme;
use sid_ui::{
    ActionCell, Badge, Card, ColumnWidth, ConfirmButton, EmptyState, FillColumns,
    FillTableDelegate, Icon, IconButton, InputState, PANEL_FILTER_FLOOR, Segment, SegmentSelect,
    SegmentedControl, StyledExt as _, TextInput, Tipped as _, Typography as _, h_flex, scaled,
    sortable_th,
};

/// Services sub-view state. See the module docs for why it holds no store/scope.
pub struct ServicesState {
    /// The seam this crate constructs concretely (`SvcctlProvider::new()`). No `Mutex`:
    /// it caches no probe handle, so concurrent calls need no serialization.
    provider: Arc<dyn ServiceProvider>,
    /// The table. Lazily built on first paint (`TableState::new` needs `window`).
    table: Option<Entity<TableState<ServicesDelegate>>>,
    /// `system` or `user` — which systemd manager is listed.
    scope: SvcScope,
    /// Set once the first load has completed; reset by a scope switch so the other
    /// manager's units load on the next render.
    loaded: bool,
    /// True while a `list_services` task is in flight — guards re-entrant refreshes.
    refreshing: bool,
    error: Option<String>,
    /// The sub-view's own filter (unit name / description substring).
    filter: Option<Entity<InputState>>,
    /// Keeps the `cx.observe(&filter, ..)` subscription alive.
    _filter_sub: Option<Subscription>,
}

impl ServicesState {
    pub(crate) fn new() -> Self {
        Self {
            provider: Arc::new(SvcctlProvider::new()),
            table: None,
            scope: SvcScope::System,
            loaded: false,
            refreshing: false,
            error: None,
            filter: None,
            _filter_sub: None,
        }
    }

    /// `Ctrl+F`'s target while the sub-view is showing; a no-op before its first paint.
    pub(crate) fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        if let Some(filter) = &self.filter {
            filter.update(cx, |state, cx| state.focus(window, cx));
        }
    }
}

/// Whether this process is root (uid 0). Only meaningful on unix; elsewhere there is no
/// uid concept to gate on, so it reports true (nothing is hidden).
fn running_as_root() -> bool {
    static ROOT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ROOT.get_or_init(|| {
        if !cfg!(unix) {
            return true;
        }
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .is_some_and(|s| s.trim() == "0")
    })
}

/// Backs the Services table. Same shape as `network_tab`'s `PortsDelegate` (cache the
/// full fetched set + the filtered display set, two-click confirm state, its own task
/// spawned on the shared runtime) with one addition: `scope`, kept in sync with
/// [`ServicesState::scope`] by `AppState::set_svc_scope` so `perform` always controls
/// the unit in the scope currently selected in the UI.
struct ServicesDelegate {
    /// The row the table has selected (its actions stay visible; others reveal on hover).
    selected_row: Option<usize>,
    svc_provider: Arc<dyn ServiceProvider>,
    scope: SvcScope,
    all_services: Vec<ServiceInfo>,
    services: Vec<ServiceInfo>,
    query: String,
    /// The (unit name, action) whose button has been clicked once — the second click
    /// on the same pair executes it.
    ///
    /// Deliberately **not** a `sid_ui::ConfirmArm`, which is what the Ports table's pid-keyed
    /// kill uses: `ConfirmArm<K>` needs `K: Copy`, and this arm's key is a unit *name*
    /// plus an action. It gets `ConfirmArm`'s load-bearing property anyway —
    /// [`Self::retain_armed_action`] survives a refresh, dropping the arm only when the
    /// unit itself is gone — but not its self-expiry.
    armed_action: Option<(String, SvcAction)>,
    /// Outcome of the last restart/stop/kill attempt, if it failed (esp.
    /// `SvcError::PermissionDenied` for a system-scope action without root).
    action_error: Option<String>,
    /// Declared column intent, resized to the viewport by `FillTable`.
    columns: FillColumns,
    /// The user's column sort, if any; `None` leaves the default (active-first) order.
    active_sort: Option<(usize, SortDir)>,
}

impl ServicesDelegate {
    fn new(svc_provider: Arc<dyn ServiceProvider>, scope: SvcScope) -> Self {
        Self {
            selected_row: None,
            svc_provider,
            scope,
            all_services: Vec::new(),
            services: Vec::new(),
            query: String::new(),
            armed_action: None,
            action_error: None,
            // `Unit` and `Description` are the growers: a unit name and its description
            // are the two unbounded strings here, and the description is the one that
            // actually reads as prose, so it takes the larger share.
            columns: FillColumns::new([
                (
                    Column::new("name", "Unit").sortable(),
                    ColumnWidth::grow().weight(1.0).min_width(190.),
                ),
                (
                    Column::new("state", "State").sortable(),
                    ColumnWidth::Fixed(100.),
                ),
                (
                    Column::new("sub_state", "Sub-state").sortable(),
                    ColumnWidth::Fixed(110.),
                ),
                (
                    Column::new("description", "Description").sortable(),
                    ColumnWidth::grow().weight(1.8).min_width(220.),
                ),
                // Not sortable — an action column, not data. Fits `restart` `stop` `kill`
                // side by side with the widest of them swapped for `confirm`.
                (Column::new("actions", ""), ColumnWidth::Fixed(220.)),
            ]),
            active_sort: None,
        }
    }

    fn set_scope(&mut self, scope: SvcScope) {
        self.scope = scope;
    }

    fn set_services(&mut self, services: Vec<ServiceInfo>) {
        self.all_services = services;
        self.retain_armed_action();
        self.recompute();
    }

    /// Keep a pending confirm only while the unit it is armed on is still in the list.
    ///
    /// The same rule `ConfirmArm::retain` enforces for the Ports table, hand-rolled
    /// here because this arm's key is not `Copy` (see [`Self::armed_action`]). Clearing
    /// the arm on every refresh — what this did before — is what makes a two-step confirm
    /// unreachable on a table that reloads itself after every action.
    fn retain_armed_action(&mut self) {
        let unit_still_listed = self
            .armed_action
            .as_ref()
            .is_some_and(|(name, _)| self.all_services.iter().any(|s| &s.name == name));
        if !unit_still_listed {
            self.armed_action = None;
        }
    }

    fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.recompute();
    }

    /// Apply the filter, the default order (active first, by name), then the active
    /// sort — stable, so the default order breaks the column sort's ties.
    fn recompute(&mut self) {
        let mut services: Vec<ServiceInfo> = filter_services(&self.all_services, &self.query)
            .into_iter()
            .cloned()
            .collect();
        services.sort_by(cmp_svc_state);
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

impl FillTableDelegate for ServicesDelegate {
    fn fill_columns(&mut self) -> &mut FillColumns {
        &mut self.columns
    }
}

impl SelectableRows for ServicesDelegate {
    fn set_selected_row(&mut self, row: Option<usize>) {
        self.selected_row = row;
    }
}

impl TableDelegate for ServicesDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.services.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> Column {
        self.columns.column(col_ix).clone()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.columns.apply_sort(col_ix, sort);
        self.active_sort = SortDir::from_column_sort(sort).map(|dir| (col_ix, dir));
        self.recompute();
        cx.notify();
    }

    /// Click-anywhere sortable header (`sid_ui::sortable_th`).
    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        sortable_th(col_ix, self.columns.column(col_ix), cx)
    }

    /// Names the row so its action cell can reveal on hover (see `reveal_on_row`).
    fn render_tr(
        &mut self,
        row_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> gpui::Stateful<Div> {
        div().id(("row", row_ix)).group(row_group("svc", row_ix))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let t = theme::active(cx);
        let (fg, muted) = (t.fg, t.muted);
        // Borrow, not clone (perf audit finding #6) — `active` is `Copy`, and every
        // `String` field this fn needs already gets its own `.clone()` below.
        let svc = &self.services[row_ix];
        // Deliberately **no `.id()` on these cells** — see `sid_ui::table`'s "Why a cell
        // must not carry an `ElementId`" section.
        match col_ix {
            0 => data_cell(t, fg, svc.name.clone()),
            1 => {
                let (label, tone) = svc_state_badge(svc.active);
                h_flex()
                    .size_full()
                    .px_2()
                    .child(Badge::new(label).tone(badge_tone(tone)))
            }
            2 => {
                let label: SharedString = if svc.sub_state.is_empty() {
                    "—".into()
                } else {
                    svc.sub_state.clone().into()
                };
                data_cell(t, muted, label)
            }
            3 => {
                let label: SharedString = if svc.description.is_empty() {
                    "—".into()
                } else {
                    svc.description.clone().into()
                };
                // The one column on this tab that is prose rather than data — a unit
                // description is a sentence — so it takes the proportional twin of the
                // same rung: `Body`, dimmed. Setting it in mono would make the widest
                // column on the screen the hardest one to read a sentence in.
                div()
                    .px_2()
                    .w_full()
                    .min_w(px(0.))
                    .clamp_one_line()
                    .text_body(t)
                    .text_color(rgb(muted))
                    .child(label)
            }
            _ => {
                let actions = [
                    (SvcAction::Restart, "restart"),
                    (SvcAction::Stop, "stop"),
                    (SvcAction::Kill, "kill"),
                ];
                // System units need root to control; a user session can only read them.
                // The provider has no capability probe, so gate on the process uid for the
                // system scope (user-scope units are always the user's own).
                if self.scope == SvcScope::System && !running_as_root() {
                    return reveal_on_row(
                        row_group("svc", row_ix),
                        false,
                        ActionCell::new().child(
                            div()
                                .id(("svc-needs-root", row_ix))
                                .text_meta(t)
                                .child("needs root")
                                .tip(NEEDS_ROOT_TIP),
                        ),
                    );
                }
                reveal_on_row(
                    row_group("svc", row_ix),
                    self.armed_action
                        .as_ref()
                        .is_some_and(|(n, _)| n == &svc.name)
                        || self.selected_row == Some(row_ix),
                    ActionCell::new().children(actions.into_iter().enumerate().map(
                        |(action_ix, (action, label))| {
                            let armed =
                                action_click_executes(&self.armed_action, &svc.name, action);
                            let name = svc.name.clone();
                            ConfirmButton::new(("svc-action", row_ix * 8 + action_ix), label)
                                .armed(armed)
                                .armed_label("confirm")
                                .tooltip(svc_action_tooltip(action))
                                .on_press(cx.listener(
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
                        },
                    )),
                )
            }
        }
    }
}

/// Why a system-scope unit shows no actions for a non-root user.
const NEEDS_ROOT_TIP: &str = "Needs root — run sid with sudo or use systemctl";

impl AppState {
    /// The Services sub-view, mounted by `systems_tab`: the panel header (filter, the
    /// `system|user` scope control, refresh), the table, then any list or action error.
    /// Also the lazy first-load hook: `systemctl` runs only once this has been opened.
    pub(crate) fn services_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_services_widgets(window, cx);
        if !self.services.loaded {
            self.refresh_services(cx);
        }
        let refreshing = self.services.refreshing;
        let scope = self.services.scope;
        let table = self.services.table.clone();
        let count = table
            .as_ref()
            .map(|t| t.read(cx).delegate().services.len())
            .unwrap_or(0);
        let action_error = table
            .as_ref()
            .and_then(|t| t.read(cx).delegate().action_error.clone());
        let unsupported = self.services.error.as_deref().is_some_and(svc_unsupported);
        let empty = unsupported.then(|| {
            EmptyState::new("Services aren't available here")
                .icon(Icon::Settings)
                .guidance(
                    "sid manages services through systemd, which this system doesn't use. \
                     Processes and Config files work as usual.",
                )
        });
        let query = self
            .services
            .filter
            .as_ref()
            .map(|f| f.read(cx).value().to_string())
            .unwrap_or_default();
        let empty = empty.or_else(|| {
            (count == 0 && !refreshing && self.services.error.is_none()).then(|| {
                EmptyState::new("No service units")
                    .icon(Icon::Settings)
                    .guidance(empty_guidance(
                        &query,
                        "This systemd manager reports no units. Try the other scope.",
                    ))
            })
        });

        // The two scopes are disjoint unit sets, so `SYSTEM SERVICES · 12` and `USER
        // SERVICES · 3` are counts of different things and the header says which.
        Card::panel(format!("{} services", svc_scope_label(scope)))
            .count(count)
            // `size_full`, not `flex_1`: `systems_tab` mounts every sub-view in a plain
            // `div().flex_1()` wrapper, not a flex container (see `processes_view`).
            .size_full()
            .action(
                div()
                    .flex()
                    .min_w_0()
                    .children(self.services.filter.clone().map(|f| {
                        TextInput::new(&f)
                            .small()
                            .fixed(px(240.))
                            .flex_shrink_1()
                            .min_w(scaled(f32::from(PANEL_FILTER_FLOOR)))
                    })),
            )
            // Secondary action between the filter and refresh: it narrows the same list.
            .action(self.svc_scope_control(scope, cx))
            .action(
                IconButton::new("svc-refresh", Icon::Refresh, "Refresh (F5)")
                    .loading(refreshing)
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                        this.refresh_services(cx);
                    })),
            )
            .children(notice_row(
                "Couldn't read services",
                self.services.error.clone().filter(|e| !svc_unsupported(e)),
            ))
            .child(table_pane(table.as_ref(), empty))
            .children(notice_row("That service action failed", action_error))
            .into_any_element()
    }

    /// Build the table and the filter on first paint. Idempotent.
    fn ensure_services_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.services.table.is_none() {
            let provider = self.services.provider.clone();
            let scope = self.services.scope;
            let table =
                cx.new(|cx| TableState::new(ServicesDelegate::new(provider, scope), window, cx));
            track_selection(&table, cx);
            self.services.table = Some(table);
        }
        if self.services.filter.is_none() {
            let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
            // `TextInput` has no change-callback; `cx.observe` fires on every keystroke
            // (see `network_tab.rs`'s "Filtering" docs). Filtering never re-probes.
            let sub = cx.observe(&filter, |this: &mut Self, filter, cx| {
                let query = filter.read(cx).value().to_string();
                if let Some(table) = this.services.table.clone() {
                    table.update(cx, |state, cx| {
                        state.delegate_mut().set_query(&query);
                        state.refresh(cx);
                    });
                }
                cx.notify();
            });
            self.services.filter = Some(filter);
            self.services._filter_sub = Some(sub);
        }
    }

    /// The `system|user` scope control. Switching scope forces a fresh `list_services`
    /// call (`loaded` reset) — the two scopes are disjoint unit sets, not a filter over
    /// one cached list.
    fn svc_scope_control(
        &self,
        active: SvcScope,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected = SVC_SCOPES
            .iter()
            .position(|&(scope, _)| scope == active)
            .unwrap_or(0);
        SegmentedControl::new("svc-scope")
            .segments(SVC_SCOPES.iter().map(|&(_, label)| Segment::new(label)))
            .selected(selected)
            .on_select(cx.listener(|this, ev: &SegmentSelect, _window, cx| {
                if let Some(&(scope, _)) = SVC_SCOPES.get(ev.index) {
                    this.set_svc_scope(scope, cx);
                }
            }))
    }

    /// Switch scope; the next render loads the other manager's units.
    fn set_svc_scope(&mut self, scope: SvcScope, cx: &mut Context<Self>) {
        if self.services.scope == scope {
            return;
        }
        self.services.scope = scope;
        self.services.loaded = false;
        if let Some(table) = self.services.table.clone() {
            table.update(cx, |state, cx| {
                state.delegate_mut().set_scope(scope);
                state.refresh(cx);
            });
        }
        cx.notify();
    }

    /// Re-fetch the unit list for the active scope on the shared runtime. Runs from the
    /// refresh button and the sub-view's lazy first load, never from a timer.
    fn refresh_services(&mut self, cx: &mut Context<Self>) {
        if self.services.refreshing {
            return;
        }
        self.services.refreshing = true;
        self.services.error = None;
        cx.notify();

        let provider = self.services.provider.clone();
        let scope = self.services.scope;
        let table = self.services.table.clone();

        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move { provider.list_services(scope).await });
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                this.services.refreshing = false;
                this.services.loaded = true;
                match outcome {
                    Ok(Ok(services)) => {
                        if let Some(table) = &table {
                            table.update(cx, |state, cx| {
                                state.delegate_mut().set_services(services);
                                state.refresh(cx);
                            });
                        }
                    }
                    Ok(Err(e)) => this.services.error = Some(e.to_string()),
                    Err(join_err) => {
                        this.services.error =
                            Some(format!("service probe task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The Services scope control's choices, in display order. The index into this array is
/// the index `SegmentedControl` reports back from `on_select`.
const SVC_SCOPES: [(SvcScope, &str); 2] = [(SvcScope::System, "system"), (SvcScope::User, "user")];

/// The word for a systemd manager scope, for the count line ("12 system services").
fn svc_scope_label(scope: SvcScope) -> &'static str {
    match scope {
        SvcScope::User => "user",
        SvcScope::System => "system",
    }
}

/// A tooltip naming what a Services row action actually does. Required by
/// [`ConfirmButton`]'s tooltip slot for the same reason `IconButton` demands one: `kill`
/// on a unit row is not self-describing, and it is the destructive one.
fn svc_action_tooltip(action: SvcAction) -> &'static str {
    match action {
        SvcAction::Start => "systemctl start this unit",
        SvcAction::Restart => "systemctl restart this unit",
        SvcAction::Stop => "systemctl stop this unit",
        SvcAction::Kill => "systemctl kill this unit",
    }
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

/// Badge label + tone for a service's active state.
fn svc_state_badge(state: SvcActiveState) -> (&'static str, Tone) {
    match state {
        SvcActiveState::Active => ("active", Tone::Ok),
        SvcActiveState::Failed => ("failed", Tone::Danger),
        SvcActiveState::Inactive => ("inactive", Tone::Muted),
        SvcActiveState::Other => ("other", Tone::Muted),
    }
}

/// Column-index dispatch for the Services table's sort — `col_ix` matches
/// [`ServicesDelegate::columns`]' order (`name, state, sub_state, description`;
/// `actions` at index 4 carries no comparator, same reasoning as `network_tab::sort_ports`'s
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

/// Active-first rank used by [`cmp_svc_state`] and the default order: `active <
/// activating < reloading < failed < (any other "other" sub-state) < inactive`. Failed
/// sits right after the live units because it is the one a reader needs to notice; the
/// long tail of inactive units goes last. `SvcActiveState::Other` folds systemd's
/// `activating`/`deactivating`/`reloading` together (see the enum's own doc comment) —
/// `sub_state`'s text is what actually distinguishes them for ranking purposes.
fn svc_state_rank(svc: &ServiceInfo) -> u8 {
    match svc.active {
        SvcActiveState::Active => 0,
        SvcActiveState::Other if svc.sub_state.eq_ignore_ascii_case("activating") => 1,
        SvcActiveState::Other if svc.sub_state.eq_ignore_ascii_case("reloading") => 2,
        SvcActiveState::Failed => 3,
        SvcActiveState::Other => 4,
        SvcActiveState::Inactive => 5,
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

/// True when the service provider reported "not supported on <os> (systemd only)" — an
/// expected state on macOS/Windows, shown as a friendly empty state, not an error toast.
fn svc_unsupported(err: &str) -> bool {
    err.contains("not supported on") && err.contains("systemd only")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resolved pixel width of every column at `viewport`.
    fn resolved(columns: &mut FillColumns, viewport: f32) -> Vec<f32> {
        columns.sync(viewport, sid_ui::UiScale::DEFAULT);
        (0..columns.len())
            .map(|ix| f32::from(columns.column(ix).width))
            .collect()
    }

    /// The column-plan pins that used to live in `network_tab.rs`'s sweep: fill the
    /// width at every window size, never overflow a laptop, action column unsortable,
    /// and the arity `sort_services`'s positional dispatch assumes.
    #[test]
    fn the_column_plan_fills_fits_and_matches_its_comparators() {
        let mut d = services_delegate();
        for viewport in [900., 1440., 2000., 3440.] {
            let total: f32 = resolved(&mut d.columns, viewport).iter().sum();
            let want = viewport - sid_ui::table::TABLE_CHROME;
            assert!(
                (total - want).abs() < 0.01,
                "{viewport}px: {total} vs {want}"
            );
        }
        let floors: f32 = resolved(&mut d.columns, 900.).iter().sum();
        assert!(floors <= 900., "declares {floors}px of floors");
        let last = d.columns.len() - 1;
        assert_eq!(last, 4);
        assert_eq!(d.columns.column(last).sort, None);
        for ix in 0..last {
            assert!(d.columns.column(ix).sort.is_some(), "column {ix}");
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

    fn services_delegate() -> ServicesDelegate {
        ServicesDelegate::new(Arc::new(SvcctlProvider::new()), SvcScope::System)
    }

    #[test]
    fn svc_unsupported_recognises_the_provider_message_only() {
        assert!(svc_unsupported(
            "service management is not supported on macos (systemd only)"
        ));
        assert!(!svc_unsupported("systemctl failed: access denied"));
    }

    /// The Services arm is hand-rolled (its key is not `Copy`, so `ConfirmArm` cannot
    /// hold it — see `ServicesDelegate::armed_action`), so it needs the same pin.
    #[test]
    fn a_refresh_that_still_lists_the_unit_keeps_the_action_armed() {
        let mut delegate = services_delegate();
        delegate.armed_action = Some(("nginx.service".to_string(), SvcAction::Restart));

        delegate.set_services(vec![svc("nginx.service", "web server")]);
        assert!(action_click_executes(
            &delegate.armed_action,
            "nginx.service",
            SvcAction::Restart
        ));

        delegate.set_services(vec![svc("sshd.service", "openssh")]);
        assert_eq!(delegate.armed_action, None);
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

    /// The scope control reports back an index into `SVC_SCOPES`, same contract.
    #[test]
    fn the_scope_control_agrees_with_the_scope_label() {
        for &(scope, label) in &SVC_SCOPES {
            assert_eq!(svc_scope_label(scope), label);
        }
        assert_eq!(SVC_SCOPES[0].0, SvcScope::System);
    }

    #[test]
    fn every_service_action_names_its_consequence() {
        let actions = [
            SvcAction::Start,
            SvcAction::Restart,
            SvcAction::Stop,
            SvcAction::Kill,
        ];
        let tips: Vec<&str> = actions.iter().map(|&a| svc_action_tooltip(a)).collect();
        assert!(tips.iter().all(|t| t.starts_with("systemctl ")));
        let mut unique = tips.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), tips.len(), "{tips:?}");
    }

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
        assert!(svc_state_rank(&reloading) < svc_state_rank(&failed));
        assert!(svc_state_rank(&failed) < svc_state_rank(&inactive));
    }

    #[test]
    fn services_list_active_first_then_failed_then_inactive_by_default() {
        let mut d = services_delegate();
        d.set_services(vec![
            svc_with_state("b-idle.service", SvcActiveState::Inactive, "dead"),
            svc_with_state("Zed.service", SvcActiveState::Active, "running"),
            svc_with_state("broken.service", SvcActiveState::Failed, "failed"),
            svc_with_state("a-idle.service", SvcActiveState::Inactive, "dead"),
            svc_with_state("audio.service", SvcActiveState::Active, "running"),
        ]);
        let names: Vec<&str> = d.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "audio.service",
                "Zed.service",
                "broken.service",
                "a-idle.service",
                "b-idle.service",
            ]
        );
        // A column sort is stable over the default: equal descriptions keep it.
        d.active_sort = Some((3, SortDir::Asc));
        d.recompute();
        assert_eq!(d.services[0].name, "audio.service");
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
}
