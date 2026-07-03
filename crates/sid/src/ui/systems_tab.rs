//! Systems tab v1 (Round D §C): a local system overview (CPU/memory/swap/load/uptime)
//! plus a processes table, sourced live from `sid_core::sys::SysProvider` — the same
//! trait seam `network_tab.rs`'s ports table already uses. Read-only except process
//! kill.
//!
//! [`SystemsTabState`] is deliberately **live/ephemeral**, same "no store, no scope, no
//! secrets" shape as [`super::network_tab::NetworkTabState`] — nothing here is ever
//! persisted; every render reflects the machine's current state and a refresh simply
//! re-probes it. `crates/sid` is the one crate allowed to name `sid-sysinfo`'s concrete
//! `SysinfoProvider::new()` constructor here — every call through it after
//! construction goes back out via `sid_core::sys::SysProvider`, matching
//! `network_tab.rs`'s seam for its own `SysinfoProvider`.
//!
//! ## Refresh
//!
//! Unlike the Network tab (manual `⟳` only), the Systems tab also self-refreshes every
//! 2 seconds *while it is the active primary tab* — a process/CPU monitor that goes
//! stale the moment you tab away and stays stale until you notice is a worse UX than
//! the extra background polling costs. The `AppState` impl block below spawns a
//! self-rescheduling task (`start_systems_refresh_loop`) that checks
//! `AppState::active_tab()` on every tick and stops — without rescheduling itself — the
//! instant the user switches to another primary tab; `systems_tab` (the render entry
//! point) restarts the loop on the next render if the tab becomes active again (see
//! `SystemsTabState::refresh_loop_running`'s doc comment for how that hand-off works).
//!
//! ## Kill
//!
//! Process kill reuses the exact `SysProvider::kill_process` call path the Network
//! tab's ports table uses — the pid-0 / i32-overflow guards live once, in
//! `sid-sysinfo`'s `kill` module, behind that one trait method (see `sid_sysinfo::
//! kill::kill_process`'s doc comment). [`ProcessesDelegate`] only adds the two-click
//! confirm UI state on top, mirroring `network_tab.rs`'s `PortsDelegate::kill`.

use std::cmp::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, IntoElement, SharedString, Subscription, Window,
    div, prelude::*, px, relative, rgb,
};
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableState};
use sid_core::sys::{Pid, ProcessInfo, Signal, SysProvider, SystemOverview};
use sid_sysinfo::SysinfoProvider;

use super::TextInput;
use crate::app::{AppState, Tab};
use crate::ui::session::ssh_runtime;

// Dark-theme palette, aligned with `app.rs`/`network_tab.rs`. Kept local so `ui` stays
// self-contained (same convention as those files).
const BORDER: u32 = 0x2c2c30;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_BG: u32 = 0x33343a;
const BRAND: u32 = 0x5a9ad0;
const DANGER: u32 = 0xd08a8a;
const WARN: u32 = 0xd0c88a;

/// Systems tab state. See the module doc comment for why this holds no store/scope.
pub struct SystemsTabState {
    /// The one seam this crate constructs concretely (`SysinfoProvider::new()`).
    /// Shared (via `Arc<Mutex<_>>`) between the refresh task and the processes table's
    /// own kill task, both of which run on `session::ssh_runtime()` — same shape as
    /// `NetworkTabState::provider`.
    provider: Arc<Mutex<SysinfoProvider>>,
    /// Set once the tab has triggered its first overview/processes refresh (on first
    /// paint) — guards against re-triggering it on every subsequent render.
    loaded: bool,
    /// True while an overview/processes refresh task is in flight — guards re-entrant
    /// ⟳ clicks and the periodic loop's own tick.
    refreshing: bool,
    /// True while the periodic 2s refresh loop (`AppState::start_systems_refresh_loop`)
    /// is alive. The loop clears this to `false` right before it stops itself (having
    /// noticed the tab is no longer active) rather than leaving it dangling `true` —
    /// `AppState::systems_tab` checks this on every render and restarts the loop
    /// whenever it finds it not running, which is exactly "the tab just (re)became
    /// active" since the loop only ever stops itself while inactive.
    refresh_loop_running: bool,
    overview: Option<SystemOverview>,
    error: Option<String>,
    /// The processes table. Lazily built by `ensure_systems_widgets` (needs `window`,
    /// which isn't available from `SystemsTabState::new`) — mirrors `NetworkTabState::
    /// table`.
    table: Option<Entity<TableState<ProcessesDelegate>>>,
    /// The filter input, shared by name/command/user/pid substring matching — same
    /// shared-filter-input pattern as `NetworkTabState::filter`.
    filter: Option<Entity<TextInput>>,
    /// Kept alive so the `cx.observe(&filter, ..)` subscription isn't dropped —
    /// mirrors `NetworkTabState::_filter_sub`.
    _filter_sub: Option<Subscription>,
}

impl SystemsTabState {
    pub(crate) fn new() -> Self {
        Self {
            provider: Arc::new(Mutex::new(SysinfoProvider::new())),
            loaded: false,
            refreshing: false,
            refresh_loop_running: false,
            overview: None,
            error: None,
            table: None,
            filter: None,
            _filter_sub: None,
        }
    }
}

/// Which column a process row is currently sorted by. `cpu` is the default (see
/// `ProcessesDelegate::new`'s `Column::new("cpu", ..).descending()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessSortKey {
    Cpu,
    Mem,
    Pid,
    Name,
    User,
}

/// Column index -> sort key, for the sortable columns only (the trailing "kill" column
/// has no `Column::sort` set, so `TableState::perform_sort` never calls into our
/// `perform_sort` for it — see that method's gate in gpui-component's `table/state.rs`).
const PROCESS_SORT_KEYS: [ProcessSortKey; 5] = [
    ProcessSortKey::Cpu,
    ProcessSortKey::Mem,
    ProcessSortKey::Pid,
    ProcessSortKey::Name,
    ProcessSortKey::User,
];

/// Backs the processes [`Table`]. Same shape as `network_tab.rs`'s `PortsDelegate`:
/// cache the full fetched set + the filtered/sorted display set, own the two-click
/// kill-confirm state, spawn its own kill task (`render_td`'s `cx` is scoped to
/// `TableState<Self>`, not the outer `AppState`). Adds sort state on top, which
/// `PortsDelegate` doesn't have yet (sortable network tables are a separate track).
struct ProcessesDelegate {
    provider: Arc<Mutex<SysinfoProvider>>,
    /// The full row set from the last refresh — never shown directly; `processes` (the
    /// filtered + sorted view) is what `TableDelegate` reads.
    all_processes: Vec<ProcessInfo>,
    /// The currently displayed (filtered + sorted) rows.
    processes: Vec<ProcessInfo>,
    /// The active filter query, cached so `set_processes` can re-apply it after a
    /// refresh.
    query: String,
    sort_key: ProcessSortKey,
    /// Only ever `Ascending` or `Descending` — `ColumnSort::Default` (gpui-component's
    /// third cycle state, meaning "no explicit sort") is deliberately never stored here;
    /// `perform_sort` below ignores that transition and keeps the previous direction, so
    /// this table is always sorted by something rather than occasionally falling back to
    /// insertion order (which would be a confusing, undocumented "current" order for a
    /// live process list).
    sort_dir: ColumnSort,
    /// The pid whose kill button has been clicked once — the second click on the same
    /// pid sends the signal. Mirrors `PortsDelegate::armed_kill`.
    armed_kill: Option<Pid>,
    /// Outcome of the last kill attempt, if it failed (e.g. `SysError::PermissionDenied`
    /// on a root-owned process). Cleared on the next refresh, arm, or successful kill.
    kill_error: Option<String>,
    columns: Vec<Column>,
}

impl ProcessesDelegate {
    fn new(provider: Arc<Mutex<SysinfoProvider>>) -> Self {
        Self {
            provider,
            all_processes: Vec::new(),
            processes: Vec::new(),
            query: String::new(),
            sort_key: ProcessSortKey::Cpu,
            sort_dir: ColumnSort::Descending,
            armed_kill: None,
            kill_error: None,
            columns: vec![
                Column::new("cpu", "CPU%").width(px(70.)).descending(),
                Column::new("mem", "Mem").width(px(90.)).sortable(),
                Column::new("pid", "PID").width(px(80.)).sortable(),
                Column::new("name", "Name").width(px(220.)).sortable(),
                Column::new("user", "User").width(px(120.)).sortable(),
                Column::new("kill", "").width(px(72.)),
            ],
        }
    }

    /// Replace the cached rows after a refresh, keeping the active filter + sort
    /// applied. Disarms any pending kill confirmation — the row set just changed
    /// underneath it (mirrors `PortsDelegate::set_ports`).
    fn set_processes(&mut self, processes: Vec<ProcessInfo>) {
        self.all_processes = processes;
        self.armed_kill = None;
        self.recompute();
    }

    /// Update the filter query and recompute the displayed rows from the cached full
    /// set — no re-probe, matches the "render pure-from-cache" rule `network_tab.rs`
    /// documents.
    fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.recompute();
    }

    fn recompute(&mut self) {
        let mut filtered: Vec<ProcessInfo> = filter_processes(&self.all_processes, &self.query)
            .into_iter()
            .cloned()
            .collect();
        sort_processes(&mut filtered, self.sort_key, self.sort_dir);
        self.processes = filtered;
    }

    /// Second click on an armed row: send SIGTERM to `pid` on the shared runtime,
    /// through the exact same `SysProvider::kill_process` call `PortsDelegate::kill`
    /// makes — see the module doc's "Kill" section. On success the row is dropped from
    /// both the cached and displayed sets immediately (rather than waiting on the next
    /// 2s refresh tick); on failure the error is surfaced via `kill_error`.
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
                        delegate.all_processes.retain(|p| p.pid != pid);
                        delegate.processes.retain(|p| p.pid != pid);
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

impl TableDelegate for ProcessesDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.processes.len()
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
        let Some(&key) = PROCESS_SORT_KEYS.get(col_ix) else {
            return;
        };
        self.sort_key = key;
        // See `sort_dir`'s doc comment: the transient `Default` cycle state keeps
        // whatever direction was already active rather than falling back to it.
        if matches!(sort, ColumnSort::Ascending | ColumnSort::Descending) {
            self.sort_dir = sort;
        }
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
        let proc = &self.processes[row_ix];
        // `ElementId` has no `From<(&str, usize, usize)>` impl — fold (row, col) into a
        // single index, same trick `network_tab.rs`'s delegates use.
        let cell_id = ("proc-cell", row_ix * 8 + col_ix);
        match col_ix {
            0 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG))
                .child(format!("{:.1}", proc.cpu_pct)),
            1 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(humanize_bytes(proc.rss_bytes)),
            2 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(proc.pid.as_u32().to_string()),
            3 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG))
                .child(proc.name.clone()),
            4 => {
                let label: SharedString =
                    proc.user.clone().unwrap_or_else(|| "—".to_string()).into();
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(label)
            }
            _ => {
                let pid = proc.pid;
                let armed = self.armed_kill == Some(pid);
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
                        if this.delegate().armed_kill == Some(pid) {
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

impl AppState {
    pub(crate) fn systems_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_systems_widgets(window, cx);
        if !self.systems.loaded {
            self.systems.loaded = true;
            self.refresh_systems(cx);
        }
        // Restart the periodic loop whenever it isn't running — see
        // `SystemsTabState::refresh_loop_running`'s doc comment: the loop only ever
        // clears this itself while the tab is inactive, so "not running" here always
        // means "the tab just (re)became active."
        if !self.systems.refresh_loop_running {
            self.systems.refresh_loop_running = true;
            self.start_systems_refresh_loop(cx);
        }

        let filter = self.systems.filter.clone();
        let refresh_label = if self.systems.refreshing {
            "…"
        } else {
            "⟳ refresh"
        };
        let proc_count = self
            .systems
            .table
            .as_ref()
            .map(|t| t.read(cx).delegate().processes.len())
            .unwrap_or(0);

        let sub: SharedString = match &self.systems.error {
            Some(e) => format!("error: {e}").into(),
            None if self.systems.refreshing && self.systems.overview.is_none() => "loading…".into(),
            None => format!("{proc_count} processes").into(),
        };

        let overview = overview_section(self.systems.overview.as_ref());

        let kill_error = self
            .systems
            .table
            .as_ref()
            .and_then(|t| t.read(cx).delegate().kill_error.clone());

        let table = self.systems.table.clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .child(overview)
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
                    .children(filter.map(|f| div().flex_1().max_w(px(280.)).child(f)))
                    .child(
                        div()
                            .id("systems-refresh")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .text_color(rgb(BRAND))
                            .hover(|s| s.bg(rgb(ACTIVE_BG)))
                            .child(refresh_label)
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.refresh_systems(cx);
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
            .children(table.map(|t| div().flex_1().w_full().child(Table::new(&t).stripe(true))))
            .children(kill_error.map(|e| {
                div()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(DANGER))
                    .child(format!("✗ {e}"))
            }))
            .into_any_element()
    }

    /// Lazily build the processes table and the shared filter input on first paint of
    /// the Systems tab. Idempotent (checked every render) — mirrors `network_tab.rs`'s
    /// `ensure_network_widgets`.
    fn ensure_systems_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.systems.table.is_none() {
            let provider = self.systems.provider.clone();
            let table = cx.new(|cx| TableState::new(ProcessesDelegate::new(provider), window, cx));
            self.systems.table = Some(table);
        }
        if self.systems.filter.is_none() {
            let filter = cx.new(|cx| TextInput::new(cx, "filter"));
            // `TextInput` has no change-callback; `cx.observe` fires on every
            // `cx.notify()` it makes while editing — see `network_tab.rs`'s "Filtering"
            // doc section for why this is the wiring pattern rather than a callback.
            let sub = cx.observe(&filter, |this: &mut Self, _filter, cx| {
                this.apply_systems_filter(cx);
            });
            self.systems.filter = Some(filter);
            self.systems._filter_sub = Some(sub);
        }
    }

    /// Push the filter box's current text into the processes table delegate — no
    /// re-probe, matches `network_tab.rs`'s `apply_network_filter`.
    fn apply_systems_filter(&mut self, cx: &mut Context<Self>) {
        let query = self
            .systems
            .filter
            .as_ref()
            .map(|f| f.read(cx).content().to_string())
            .unwrap_or_default();
        if let Some(table) = self.systems.table.clone() {
            table.update(cx, |state, cx| {
                state.delegate_mut().set_query(&query);
                state.refresh(cx);
            });
        }
        cx.notify();
    }

    /// ⟳ refresh: re-probe the overview + processes on the shared runtime, then apply
    /// the results. No blocking in `render` — this only ever runs from a click, the
    /// lazy first-paint trigger in `systems_tab`, or the periodic loop's tick. Mirrors
    /// `network_tab.rs`'s `refresh_network` (overview + processes share the one
    /// `Mutex<SysinfoProvider>` lock for the same reason ports + interfaces do there:
    /// serialized `&mut` access to the cached `sysinfo::System`).
    pub(crate) fn refresh_systems(&mut self, cx: &mut Context<Self>) {
        if self.systems.refreshing {
            return;
        }
        self.systems.refreshing = true;
        self.systems.error = None;
        cx.notify();

        let provider = self.systems.provider.clone();
        let table = self.systems.table.clone();

        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                let mut guard = provider.lock().unwrap_or_else(|e| e.into_inner());
                (guard.overview(), guard.list_processes())
            });
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                this.systems.refreshing = false;
                match outcome {
                    Ok((overview_res, procs_res)) => {
                        let mut err = None;
                        match overview_res {
                            Ok(overview) => this.systems.overview = Some(overview),
                            Err(e) => err = Some(e.to_string()),
                        }
                        match procs_res {
                            Ok(procs) => {
                                if let Some(table) = &table {
                                    table.update(cx, |state, cx| {
                                        state.delegate_mut().set_processes(procs);
                                        state.refresh(cx);
                                    });
                                }
                            }
                            Err(e) => {
                                if err.is_none() {
                                    err = Some(e.to_string());
                                }
                            }
                        }
                        this.systems.error = err;
                    }
                    Err(join_err) => {
                        this.systems.error =
                            Some(format!("system probe task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Spawn a self-rescheduling task that re-probes the system every 2 seconds for as
    /// long as the Systems tab stays the active primary tab — see the module doc's
    /// "Refresh" section. Each tick checks `active_tab()` *before* refreshing; the loop
    /// stops (without rescheduling itself) the instant that check fails, rather than
    /// refreshing one more time off-tab.
    fn start_systems_refresh_loop(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let keep_going = this.update(cx, |state, cx| {
                    if state.active_tab() != Tab::System {
                        state.systems.refresh_loop_running = false;
                        return false;
                    }
                    state.refresh_systems(cx);
                    true
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}

// ---- overview card rendering ------------------------------------------------------

/// The top overview strip: one host/kernel/uptime/load line, then CPU total + per-core
/// bars and memory/swap bars side by side. Renders a dim "loading…" line instead while
/// the first probe is still in flight (`overview` is `None`).
fn overview_section(overview: Option<&SystemOverview>) -> AnyElement {
    let Some(ov) = overview else {
        return div()
            .px_4()
            .py_3()
            .text_sm()
            .text_color(rgb(FG_DIM))
            .child("loading system overview…")
            .into_any_element();
    };

    let host_line: SharedString = format!(
        "{} · {} · up {} · load {:.2} {:.2} {:.2}",
        ov.hostname,
        ov.kernel,
        humanize_uptime(ov.uptime_secs),
        ov.load_avg.0,
        ov.load_avg.1,
        ov.load_avg.2,
    )
    .into();

    let swap_card: AnyElement = if ov.swap_total > 0 {
        mem_card("Swap", ov.swap_used, ov.swap_total).into_any_element()
    } else {
        div()
            .flex_1()
            .text_xs()
            .text_color(rgb(FG_DIM))
            .child("Swap — none configured")
            .into_any_element()
    };

    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(div().text_sm().text_color(rgb(FG)).child(host_line))
        .child(
            div()
                .flex()
                .flex_row()
                .gap_6()
                .child(cpu_card(ov))
                .child(mem_card("Memory", ov.mem_used, ov.mem_total))
                .child(swap_card),
        )
        .into_any_element()
}

/// CPU card: aggregate percent + a thin horizontal bar, then one thin vertical bar per
/// logical core underneath (visual density over per-core numeric labels — this is an
/// overview card, not the processes table).
fn cpu_card(ov: &SystemOverview) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(div().text_xs().text_color(rgb(FG_DIM)).child("CPU"))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(FG))
                        .child(format!("{:.1}%", ov.cpu_total_pct)),
                ),
        )
        .child(horizontal_bar(
            ov.cpu_total_pct / 100.0,
            bar_color(ov.cpu_total_pct),
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .mt_1()
                .children(ov.cpu_per_core.iter().map(|&pct| vertical_core_bar(pct))),
        )
}

/// Memory/swap card: `label` · `used / total` (humanized bytes) + a thin horizontal
/// bar. Shared by both the Memory and (when swap is configured) Swap cards.
fn mem_card(label: &'static str, used: u64, total: u64) -> impl IntoElement {
    let pct = if total == 0 {
        0.0
    } else {
        (used as f32 / total as f32) * 100.0
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(div().text_xs().text_color(rgb(FG_DIM)).child(label))
                .child(div().text_xs().text_color(rgb(FG)).child(format!(
                    "{} / {}",
                    humanize_bytes(used),
                    humanize_bytes(total)
                ))),
        )
        .child(horizontal_bar(pct / 100.0, bar_color(pct)))
}

/// A thin horizontal filled bar: a dim track the full width of its container, with a
/// colored fill proportional to `fraction` (clamped to `0.0..=1.0`).
fn horizontal_bar(fraction: f32, color: u32) -> impl IntoElement {
    let frac = fraction.clamp(0.0, 1.0);
    div()
        .w_full()
        .h(px(6.))
        .rounded_sm()
        .bg(rgb(BORDER))
        .child(div().h_full().rounded_sm().bg(rgb(color)).w(relative(frac)))
}

/// A thin vertical filled bar (one per CPU core): a dim track of fixed height, with a
/// colored fill anchored to the bottom, proportional to `pct` (0..=100, clamped).
fn vertical_core_bar(pct: f32) -> impl IntoElement {
    let frac = (pct / 100.0).clamp(0.0, 1.0);
    div()
        .w(px(5.))
        .h(px(20.))
        .flex()
        .flex_col()
        .justify_end()
        .rounded_sm()
        .bg(rgb(BORDER))
        .child(
            div()
                .w_full()
                .rounded_sm()
                .bg(rgb(bar_color(pct)))
                .h(relative(frac)),
        )
}

/// Bar fill color by load: calm (`BRAND`) under 70%, `WARN` 70..90%, `DANGER` at/above
/// 90% — same three-tier convention as the rest of the app's status colors.
fn bar_color(pct: f32) -> u32 {
    if pct >= 90.0 {
        DANGER
    } else if pct >= 70.0 {
        WARN
    } else {
        BRAND
    }
}

// ---- pure helpers (unit-tested) ---------------------------------------------------

/// Case-insensitive filter over the processes table: name/command/user substring, or
/// an exact pid match. Empty (or all-whitespace) query matches everything. Mirrors
/// `network_tab.rs`'s `filter_ports`.
fn filter_processes<'a>(processes: &'a [ProcessInfo], query: &str) -> Vec<&'a ProcessInfo> {
    let query = query.trim();
    if query.is_empty() {
        return processes.iter().collect();
    }
    let lower = query.to_lowercase();
    let exact_pid: Option<u32> = query.parse().ok();
    processes
        .iter()
        .filter(|p| {
            p.name.to_lowercase().contains(&lower)
                || p.cmd.to_lowercase().contains(&lower)
                || p.user
                    .as_deref()
                    .is_some_and(|u| u.to_lowercase().contains(&lower))
                || exact_pid.is_some_and(|pid| p.pid.as_u32() == pid)
        })
        .collect()
}

/// Typed comparator for one [`ProcessSortKey`] — never lexicographic on numeric
/// columns (`cpu_pct`/`rss_bytes`/`pid`), case-insensitive on text columns
/// (`name`/`user`). A missing `user` sorts as `""` (first, ascending).
fn process_cmp(a: &ProcessInfo, b: &ProcessInfo, key: ProcessSortKey) -> Ordering {
    match key {
        ProcessSortKey::Cpu => a.cpu_pct.partial_cmp(&b.cpu_pct).unwrap_or(Ordering::Equal),
        ProcessSortKey::Mem => a.rss_bytes.cmp(&b.rss_bytes),
        ProcessSortKey::Pid => a.pid.as_u32().cmp(&b.pid.as_u32()),
        ProcessSortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        ProcessSortKey::User => {
            let a_user = a.user.as_deref().unwrap_or("").to_lowercase();
            let b_user = b.user.as_deref().unwrap_or("").to_lowercase();
            a_user.cmp(&b_user)
        }
    }
}

/// Sort `processes` in place by `key`/`dir`. `dir` is expected to be `Ascending` or
/// `Descending` — see [`ProcessesDelegate::sort_dir`]'s doc comment for why the
/// `Default` cycle state is never passed here.
fn sort_processes(processes: &mut [ProcessInfo], key: ProcessSortKey, dir: ColumnSort) {
    let ascending = matches!(dir, ColumnSort::Ascending);
    processes.sort_by(|a, b| {
        let ord = process_cmp(a, b, key);
        if ascending { ord } else { ord.reverse() }
    });
}

/// Human-readable byte count (binary units, one decimal place above `B`) — e.g.
/// "340 B", "1.2 MB". Pure so it's unit-tested without touching real memory counters.
/// Identical to `network_tab.rs`'s `humanize_bytes` — kept local per this codebase's
/// "self-contained `ui` module" convention (see that file's palette-const doc comment).
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

/// Human-readable uptime — e.g. "42s", "5m 3s", "3h 12m", "2d 4h 1m". Pure so it's
/// unit-tested without touching the real system clock. Only the two coarsest non-zero
/// units are shown (dropping seconds once hours are in play, etc.) — a Systems tab
/// overview line has no use for second-level precision on a multi-day uptime.
fn humanize_uptime(total_secs: u64) -> String {
    let days = total_secs / 86_400;
    let hours = (total_secs % 86_400) / 3_600;
    let mins = (total_secs % 3_600) / 60;
    let secs = total_secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, cpu: f32, mem: u64, user: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid: Pid::from_u32(pid),
            name: name.to_string(),
            cmd: name.to_string(),
            cpu_pct: cpu,
            rss_bytes: mem,
            started_unix_secs: 0,
            parent: None,
            user: user.map(|s| s.to_string()),
        }
    }

    #[test]
    fn humanize_bytes_scales_units() {
        assert_eq!(humanize_bytes(0), "0 B");
        assert_eq!(humanize_bytes(340), "340 B");
        assert_eq!(humanize_bytes(1024), "1.0 KB");
        assert_eq!(humanize_bytes(1_258_291), "1.2 MB");
    }

    #[test]
    fn humanize_uptime_scales_units() {
        assert_eq!(humanize_uptime(5), "5s");
        assert_eq!(humanize_uptime(65), "1m 5s");
        assert_eq!(humanize_uptime(3_665), "1h 1m");
        assert_eq!(humanize_uptime(90_061), "1d 1h 1m");
    }

    #[test]
    fn humanize_uptime_zero_is_zero_seconds() {
        assert_eq!(humanize_uptime(0), "0s");
    }

    #[test]
    fn filter_processes_matches_name_cmd_user_or_exact_pid() {
        let processes = vec![
            proc(1, "init", 0.0, 0, Some("root")),
            proc(200, "nginx", 1.0, 0, Some("www-data")),
        ];
        assert_eq!(filter_processes(&processes, "nginx").len(), 1);
        assert_eq!(filter_processes(&processes, "www-data").len(), 1);
        assert_eq!(filter_processes(&processes, "200").len(), 1);
        assert_eq!(filter_processes(&processes, "").len(), 2);
        assert_eq!(filter_processes(&processes, "   ").len(), 2);
        assert!(filter_processes(&processes, "nope").is_empty());
    }

    #[test]
    fn sort_processes_cpu_descending_puts_hottest_first() {
        let mut processes = vec![
            proc(1, "a", 5.0, 0, None),
            proc(2, "b", 90.0, 0, None),
            proc(3, "c", 12.0, 0, None),
        ];
        sort_processes(&mut processes, ProcessSortKey::Cpu, ColumnSort::Descending);
        let names: Vec<&str> = processes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn sort_processes_mem_ascending() {
        let mut processes = vec![
            proc(1, "a", 0.0, 300, None),
            proc(2, "b", 0.0, 100, None),
            proc(3, "c", 0.0, 200, None),
        ];
        sort_processes(&mut processes, ProcessSortKey::Mem, ColumnSort::Ascending);
        let names: Vec<&str> = processes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    /// Load-bearing: pid must sort numerically (9 < 80 < 700), never lexicographically
    /// (which would put "700" before "80").
    #[test]
    fn sort_processes_pid_is_numeric_not_lexicographic() {
        let mut processes = vec![
            proc(700, "c", 0.0, 0, None),
            proc(9, "a", 0.0, 0, None),
            proc(80, "b", 0.0, 0, None),
        ];
        sort_processes(&mut processes, ProcessSortKey::Pid, ColumnSort::Ascending);
        let pids: Vec<u32> = processes.iter().map(|p| p.pid.as_u32()).collect();
        assert_eq!(pids, vec![9, 80, 700]);
    }

    #[test]
    fn sort_processes_name_is_case_insensitive() {
        let mut processes = vec![proc(1, "Zsh", 0.0, 0, None), proc(2, "bash", 0.0, 0, None)];
        sort_processes(&mut processes, ProcessSortKey::Name, ColumnSort::Ascending);
        let names: Vec<&str> = processes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["bash", "Zsh"]);
    }

    #[test]
    fn sort_processes_user_missing_sorts_as_empty_string() {
        let mut processes = vec![
            proc(1, "a", 0.0, 0, Some("zed")),
            proc(2, "b", 0.0, 0, None),
        ];
        sort_processes(&mut processes, ProcessSortKey::User, ColumnSort::Ascending);
        assert_eq!(processes[0].name, "b");
        assert_eq!(processes[1].name, "a");
    }

    /// `ColumnSort::Default` (gpui-component's third, "no explicit sort" cycle state)
    /// must not silently fall back to ascending or reset direction — see
    /// `sort_processes`'s doc comment. `sort_processes` itself only ever receives
    /// `Ascending`/`Descending` from `ProcessesDelegate::perform_sort`, so this test
    /// pins that a stray `Default` (if ever passed) is treated the same as
    /// `Descending` rather than panicking or silently reordering ascending.
    #[test]
    fn sort_processes_default_direction_behaves_like_descending() {
        let mut processes = vec![proc(1, "a", 5.0, 0, None), proc(2, "b", 90.0, 0, None)];
        sort_processes(&mut processes, ProcessSortKey::Cpu, ColumnSort::Default);
        let names: Vec<&str> = processes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b", "a"]);
    }

    #[test]
    fn bar_color_thresholds() {
        assert_eq!(bar_color(0.0), BRAND);
        assert_eq!(bar_color(69.9), BRAND);
        assert_eq!(bar_color(70.0), WARN);
        assert_eq!(bar_color(89.9), WARN);
        assert_eq!(bar_color(90.0), DANGER);
        assert_eq!(bar_color(100.0), DANGER);
    }
}
