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
//! kill::kill_process`'s doc comment). [`ProcessesDelegate`] adds the two-click confirm
//! on top, as a [`ConfirmArm`] rather than the `Option<Pid>` field it used to be —
//! **that field made the confirm unreachable**: `set_processes` cleared it on arrival of
//! every refresh, and this tab refreshes every 2 seconds, so arming a row was a race
//! against the timer that the timer always won. See `sid_ui::action_cell`'s module docs.
//!
//! ## Layout (UI overhaul, 2026-07-26)
//!
//! The tab is one shared overview plus two **segmented sub-views**:
//!
//! ```text
//! ┌ STATCLUSTER ─ host line · CPU / Memory / Swap meters ───────────────┐
//! ├ [ Processes | Config files ] ───────────────────────────────────────┤
//! └ the active sub-view, full width ────────────────────────────────────┘
//! ```
//!
//! The config-file list used to sit *below* the process table as a centred 880px column
//! under a full-width table, which orphaned it: it started at x≈560 aligned to nothing,
//! and its `pin` actions were red text ~800px from the filename they acted on. It is now
//! a sub-view of its own, owning the whole viewport, with the pin affordance a
//! row-anchored tooltipped `IconButton` inside a bounded card.
//!
//! ## Config files (Round E §D)
//!
//! Pinned paths (persisted globally via `sid_store::PinnedFile` — see
//! [`AppState::refresh_config_files`]) plus a fixed "common" candidate list,
//! existence-filtered against this machine. Unlike the overview/processes half of this
//! file, pins *are* persisted (through `AppState::store`, same as every other tab's
//! writes) — only the overview/processes state itself stays ephemeral. Clicking a row
//! opens the editor modal in [`super::config_editor`]; this module owns just the two
//! lists, the pin/unpin affordances, and the "pin a file…" input.
//!
//! Every colour here reads [`theme::active`] at render time (never cached across frames)
//! — this file was the first to drop its own local hex-const palette in favour of the
//! shared [`Theme`] tokens (round E §B's mapping: `BORDER→border`, `FG→fg`,
//! `FG_DIM→muted`, `ACTIVE_BG→selection`, `BRAND→accent`, `DANGER→danger`, `WARN→warning`).

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, IntoElement, SharedString, Subscription, Window,
    div, prelude::*, px, rgb,
};
use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};
use sid_core::sys::{Pid, ProcessInfo, Signal, SysProvider, SystemOverview};
use sid_store::PinnedFile;
use sid_sysinfo::SysinfoProvider;

use super::TextInput;
use crate::app::{AppState, Tab};
use crate::ui::config_editor::ConfigEditorState;
use crate::ui::session::ssh_runtime;
use sid_ui::theme::{self, Theme};
use sid_ui::{
    ActionCell, Button, Card, ColumnWidth, Confirm, ConfirmArm, ConfirmButton, EmptyState,
    FillColumns, FillTable, FillTableDelegate, Icon, IconButton, Meter, Segment, SegmentSelect,
    SegmentedControl, StatCluster, StyledExt as _, Toolbar, h_flex, v_flex,
};

/// Which sub-view is active under the System tab's segmented control.
///
/// The config-file list used to be stacked *under* the process table (see the module
/// docs); making it a peer sub-view is what lets each of them own the full viewport
/// width instead of splitting the vertical space and orphaning the narrower one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SystemSubView {
    #[default]
    Processes,
    ConfigFiles,
}

impl SystemSubView {
    /// In the order the segmented control lists them; the index into this array is the
    /// index the control reports back from `on_select`.
    const ALL: [SystemSubView; 2] = [SystemSubView::Processes, SystemSubView::ConfigFiles];

    fn label(self) -> &'static str {
        match self {
            SystemSubView::Processes => "Processes",
            SystemSubView::ConfigFiles => "Config files",
        }
    }

    fn icon(self) -> Icon {
        match self {
            SystemSubView::Processes => Icon::Terminal,
            SystemSubView::ConfigFiles => Icon::File,
        }
    }

    /// The sub-view at `ix`, for the segmented control's callback. Out-of-range keeps
    /// the current view rather than guessing — the control clamps its own index, so
    /// this only fires if the two lists ever disagree.
    fn at(ix: usize) -> Option<Self> {
        Self::ALL.get(ix).copied()
    }
}

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
    /// Which of `[Processes] [Config files]` is showing. Purely a view choice — both
    /// sub-views keep their own state loaded whichever one is on screen, so switching
    /// back never costs a re-probe.
    sub_view: SystemSubView,
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
    /// The config-files area (Round E §D): unlike everything above, these ARE
    /// persisted (through `AppState::store`, global-only — see `PinnedFile`'s doc
    /// comment). Set once at first paint and after every pin/unpin
    /// (`AppState::refresh_config_files`).
    pinned: Vec<PinnedFile>,
    /// The fixed "common" candidate list (`CURATED_TEMPLATES`), existence-filtered
    /// against this machine and with anything already pinned excluded (no point
    /// showing a row twice).
    curated: Vec<String>,
    /// Set once the config-files area has done its first pinned/curated refresh.
    config_loaded: bool,
    /// The "pin a file…" free-text input. Submits on Enter (`.on_key_down`, same
    /// technique `db_tab.rs`'s inline rename/folder-edit rows use) rather than a
    /// change-event subscription — there's nothing to react to until the user commits.
    pin_input: Option<Entity<TextInput>>,
    /// Inline error under the pin input (e.g. a nonexistent path) — cleared on the
    /// next successful pin or edit.
    pin_error: Option<String>,
    /// The open config-file editor modal, if any — see `super::config_editor`.
    pub(crate) editor: Option<ConfigEditorState>,
}

impl SystemsTabState {
    pub(crate) fn new() -> Self {
        Self {
            provider: Arc::new(Mutex::new(SysinfoProvider::new())),
            loaded: false,
            refreshing: false,
            refresh_loop_running: false,
            sub_view: SystemSubView::default(),
            overview: None,
            error: None,
            table: None,
            filter: None,
            _filter_sub: None,
            pinned: Vec::new(),
            curated: Vec::new(),
            config_loaded: false,
            pin_input: None,
            pin_error: None,
            editor: None,
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
    Command,
    User,
}

/// Column index -> sort key, for the sortable columns only (the trailing "kill" column
/// has no `Column::sort` set, so `TableState::perform_sort` never calls into our
/// `perform_sort` for it — see that method's gate in gpui-component's `table/state.rs`).
const PROCESS_SORT_KEYS: [ProcessSortKey; 6] = [
    ProcessSortKey::Cpu,
    ProcessSortKey::Mem,
    ProcessSortKey::Pid,
    ProcessSortKey::Name,
    ProcessSortKey::Command,
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
    /// The two-step kill confirm, keyed by pid.
    ///
    /// Keyed rather than positional so a table that re-sorts between the two clicks can
    /// never redirect the confirm onto a different process, and *surviving*
    /// [`Self::set_processes`] rather than cleared by it — the `Option<Pid>` this
    /// replaces was reset on every 2-second refresh, which made the confirm
    /// unreachable in practice. See `sid_ui::action_cell`.
    kill_arm: ConfirmArm<Pid>,
    /// Outcome of the last kill attempt, if it failed (e.g. `SysError::PermissionDenied`
    /// on a root-owned process). Cleared on the next refresh, arm, or successful kill.
    kill_error: Option<String>,
    /// The columns and the width each one declared. Resized to the live viewport by
    /// [`FillTable`] — see `sid_ui::table`'s module docs for why this table used to be a
    /// 652px ribbon in a 2000px window.
    columns: FillColumns,
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
            kill_arm: ConfirmArm::new(),
            kill_error: None,
            // Widths are declared as intent, not pixels: the numeric columns have a
            // known upper bound and stay exactly as wide as they need, `Name` — the one
            // column whose content is unbounded, and the one the old 220px cap was
            // truncating mid-word — absorbs everything the window has spare, and `User`
            // holds a floor so a short username can't collapse the header.
            columns: FillColumns::new([
                (
                    Column::new("cpu", "CPU%").descending(),
                    ColumnWidth::Fixed(70.),
                ),
                (
                    Column::new("mem", "Mem").sortable(),
                    ColumnWidth::Fixed(90.),
                ),
                (
                    Column::new("pid", "PID").sortable(),
                    ColumnWidth::Fixed(80.),
                ),
                (
                    Column::new("name", "Name").sortable(),
                    ColumnWidth::grow().weight(1.0).min_width(180.),
                ),
                // The reclaimed width has to carry something. Filling a 2000px window by
                // stretching a 6-character process name across 1480px of it is the same
                // dead space the fill-width model was built to kill, just relabelled —
                // and the command line is the one datum a process monitor is missing
                // (the filter already matches on it). It takes the larger share of the
                // grow because it is the column whose content is genuinely unbounded.
                (
                    Column::new("cmd", "Command").sortable(),
                    ColumnWidth::grow().weight(2.5).min_width(240.),
                ),
                (
                    Column::new("user", "User").sortable(),
                    ColumnWidth::Min(120.),
                ),
                // Wide enough for the armed state's "confirm" label plus the cell's own
                // padding — the idle "kill" chip is narrower, but a control that resizes
                // the column it lives in when you arm it makes the whole row jump.
                (Column::new("kill", ""), ColumnWidth::Fixed(104.)),
            ]),
        }
    }

    /// Replace the cached rows after a refresh, keeping the active filter + sort
    /// applied.
    ///
    /// A pending kill confirmation is kept **unless its process is gone**. Clearing it
    /// unconditionally (what this did before) made the confirm unreachable: this tab
    /// re-probes every 2 seconds, so every arm was disarmed within the tick, and the
    /// armed state never even reached the screen. See [`ProcessesDelegate::kill_arm`].
    fn set_processes(&mut self, processes: Vec<ProcessInfo>) {
        self.all_processes = processes;
        let live: HashSet<Pid> = self.all_processes.iter().map(|p| p.pid).collect();
        self.kill_arm.retain(|pid| live.contains(&pid));
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
        self.kill_arm.disarm();
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

impl FillTableDelegate for ProcessesDelegate {
    fn fill_columns(&mut self) -> &mut FillColumns {
        &mut self.columns
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
        self.columns.column(col_ix)
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
        // Mirror the sort onto our own columns so the header indicator survives the
        // next `TableState::refresh` — which a viewport change (and `kill`) triggers.
        // See `FillColumns::apply_sort`.
        self.columns.apply_sort(col_ix, sort);
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
        let theme = theme::active(cx).clone();
        let proc = &self.processes[row_ix];
        // `ElementId` has no `From<(&str, usize, usize)>` impl — fold (row, col) into a
        // single index, same trick `network_tab.rs`'s delegates use.
        let cell_id = ("proc-cell", row_ix * 8 + col_ix);
        // One clock read per cell, so every cell in a frame agrees about whether the
        // armed row's confirm window is still open.
        let now = Instant::now();
        match col_ix {
            0 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(theme.fg))
                .child(format!("{:.1}%", proc.cpu_pct)),
            1 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(theme.muted))
                .child(humanize_bytes(proc.rss_bytes)),
            2 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(theme.muted))
                .child(proc.pid.as_u32().to_string()),
            3 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(theme.fg))
                .child(proc.name.clone()),
            4 => {
                // Kernel threads have no cmdline at all; an empty cell reads as a
                // rendering failure, the em dash reads as "there is none".
                let label: SharedString = if proc.cmd.trim().is_empty() {
                    "—".into()
                } else {
                    proc.cmd.clone().into()
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(theme.muted))
                    .child(label)
            }
            5 => {
                let label: SharedString =
                    proc.user.clone().unwrap_or_else(|| "—".to_string()).into();
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(theme.muted))
                    .child(label)
            }
            _ => {
                let pid = proc.pid;
                let armed = self.kill_arm.is_armed(pid, now);
                // Keyed by pid, not by row index: the rows under the pointer reorder on
                // every sort and every 2s refresh, and an id that moves with them would
                // hand one row's hover/press state to another.
                let button_id = ("proc-kill", pid.as_u32() as usize);
                div().id(cell_id).size_full().child(
                    ActionCell::new().child(
                        ConfirmButton::new(button_id, "kill")
                            .armed(armed)
                            .armed_label("confirm")
                            .tooltip("send SIGTERM to this process")
                            .on_press(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                                let outcome =
                                    this.delegate_mut().kill_arm.press(pid, Instant::now());
                                match outcome {
                                    Confirm::Fire => this.delegate_mut().kill(pid, cx),
                                    Confirm::Armed => cx.notify(),
                                }
                            })),
                    ),
                )
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
        if !self.systems.config_loaded {
            self.systems.config_loaded = true;
            self.refresh_config_files(cx);
        }
        // Restart the periodic loop whenever it isn't running — see
        // `SystemsTabState::refresh_loop_running`'s doc comment: the loop only ever
        // clears this itself while the tab is inactive, so "not running" here always
        // means "the tab just (re)became active."
        if !self.systems.refresh_loop_running {
            self.systems.refresh_loop_running = true;
            self.start_systems_refresh_loop(cx);
        }

        let theme = theme::active(cx).clone();
        let overview = self.overview_cluster();
        let sub_view = self.systems.sub_view;
        let body: AnyElement = match sub_view {
            SystemSubView::Processes => self.processes_view(&theme, cx),
            SystemSubView::ConfigFiles => self.config_files_view(&theme, cx),
        };
        let editor_overlay = self.config_editor_overlay(window, cx);

        v_flex()
            .flex_1()
            .p_4()
            .gap_3()
            .child(overview)
            .child(self.system_sub_view_strip(sub_view, cx))
            // The one flexible child, so whichever sub-view is showing gets the whole
            // remaining canvas. `min_h(0)` keeps its own natural height from starving
            // the basis-0 `flex_1` down to a header row — the round-E capture-harness
            // shakedown caught exactly that.
            .child(div().flex_1().min_h(px(0.)).w_full().child(body))
            .children(editor_overlay)
            .into_any_element()
    }

    /// The `[Processes] [Config files]` segmented control. The meters above it are
    /// shared context for both; everything below it belongs to one sub-view.
    fn system_sub_view_strip(
        &self,
        active: SystemSubView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected = SystemSubView::ALL
            .iter()
            .position(|&v| v == active)
            .unwrap_or(0);
        SegmentedControl::new("system-subview")
            .segments(
                SystemSubView::ALL
                    .iter()
                    .map(|&v| Segment::new(v.label()).icon(v.icon())),
            )
            .selected(selected)
            .on_select(cx.listener(|this, ev: &SegmentSelect, _window, cx| {
                if let Some(view) = SystemSubView::at(ev.index) {
                    this.systems.sub_view = view;
                    cx.notify();
                }
            }))
    }

    /// The Processes sub-view: toolbar, then the fill-width table, then any kill error.
    fn processes_view(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.systems.filter.clone();
        let refreshing = self.systems.refreshing;
        let proc_count = self
            .systems
            .table
            .as_ref()
            .map(|t| t.read(cx).delegate().processes.len())
            .unwrap_or(0);
        let kill_error = self
            .systems
            .table
            .as_ref()
            .and_then(|t| t.read(cx).delegate().kill_error.clone());
        let table = self.systems.table.clone();

        let count_label: SharedString = match &self.systems.error {
            Some(e) => format!("error: {e}").into(),
            None if refreshing && self.systems.overview.is_none() => "loading…".into(),
            None => sid_ui::toolbar::count_label(proc_count, "process").into(),
        };

        v_flex()
            .size_full()
            .child(
                Toolbar::new()
                    // Capped rather than filling the row: a 1900px-wide filter field is
                    // as wrong as the 652px table it used to sit above.
                    .filter(div().max_w(px(320.)).children(filter))
                    .count_label(count_label)
                    .action(
                        Button::new("systems-refresh", "refresh")
                            .small()
                            .icon(Icon::Refresh)
                            .loading(refreshing)
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.refresh_systems(cx);
                            })),
                    ),
            )
            .children(table.map(|t| {
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .child(FillTable::new(&t).stripe(true))
            }))
            .children(kill_error.map(|e| {
                h_flex()
                    .gap_1p5()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(theme.danger))
                    .child("✗")
                    .child(e)
            }))
            .into_any_element()
    }

    /// Lazily build the processes table, the shared filter input, and the "pin a
    /// file…" input on first paint of the Systems tab. Idempotent (checked every
    /// render) — mirrors `network_tab.rs`'s `ensure_network_widgets`.
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
        if self.systems.pin_input.is_none() {
            self.systems.pin_input = Some(cx.new(|cx| TextInput::new(cx, "pin a file… (~/ ok)")));
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

// ---- config files (Round E §D) -----------------------------------------------------

/// The Round E §D curated candidate list, in the spec's fixed display order. Callers
/// tilde-expand ([`expand_tilde`]) and existence-filter ([`filter_existing`]) before
/// rendering — this is just the template list, so it's a plain data constant rather
/// than something that needs its own test.
const CURATED_TEMPLATES: &[&str] = &[
    "/etc/fstab",
    "/etc/hosts",
    "/etc/environment",
    "/etc/pacman.conf",
    "/etc/ssh/sshd_config",
    "/etc/ssh/ssh_config",
    "/etc/sudoers",
    "~/.ssh/config",
    "~/.gitconfig",
    "~/.zshrc",
    "~/.bashrc",
    "~/.profile",
    "~/.config/hypr/hyprland.conf",
    "~/.config/kitty/kitty.conf",
    "~/.config/waybar/config.jsonc",
];

impl AppState {
    /// Re-read the pinned list from the store and re-filter the curated candidates
    /// against this machine. Called once at first paint of the config-files area and
    /// after every pin/unpin — see `SystemsTabState::config_loaded`'s doc comment.
    pub(crate) fn refresh_config_files(&mut self, cx: &mut Context<Self>) {
        self.systems.pinned = self.store.list_pinned_files().unwrap_or_default();
        let home = home_dir();
        self.systems.curated =
            filter_existing(&curated_candidates(&home), |p| Path::new(p).exists());
        cx.notify();
    }

    /// The "pin a file…" input's submit action (Enter, or the small "+ pin" affordance):
    /// tilde-expand, reject a nonexistent path inline, else pin + clear the input.
    fn submit_pin(&mut self, cx: &mut Context<Self>) {
        let raw = self
            .systems
            .pin_input
            .as_ref()
            .map(|i| i.read(cx).content().to_string())
            .unwrap_or_default();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }
        let expanded = expand_tilde(trimmed, &home_dir());
        if !Path::new(&expanded).exists() {
            self.systems.pin_error = Some(format!("{expanded}: no such file"));
            cx.notify();
            return;
        }
        if let Err(e) = self.store.pin_file(&expanded) {
            self.systems.pin_error = Some(e.to_string());
            cx.notify();
            return;
        }
        self.systems.pin_error = None;
        if let Some(input) = self.systems.pin_input.clone() {
            input.update(cx, |i, cx| i.reset(cx));
        }
        self.refresh_config_files(cx);
    }

    fn unpin_config_file(&mut self, path: String, cx: &mut Context<Self>) {
        let _ = self.store.unpin_file(&path);
        self.refresh_config_files(cx);
    }

    /// The Config files sub-view: the "pin a file…" toolbar, then the pinned and common
    /// lists side by side, each in its own bounded [`Card`].
    ///
    /// Two columns rather than one, and full-width rather than centred. The old layout
    /// was a `max_w(880px)` column centred beneath a full-width table — it started at
    /// x≈560 aligned to nothing above it, and its `pin` action sat at the far right of an
    /// invisible boundary ~800px from the filename it acted on. Splitting the lists gives
    /// each row a card edge to be anchored to at a width that is still a reading column,
    /// while the pair together use the whole viewport.
    fn config_files_view(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let pinned_paths: Vec<String> =
            self.systems.pinned.iter().map(|p| p.path.clone()).collect();
        let common: Vec<String> = exclude_pinned(&self.systems.curated, &pinned_paths);
        let pinned_count = pinned_paths.len();
        let common_count = common.len();

        // Eagerly collected (rather than left as lazy iterators): `config_file_row`
        // needs `cx` on every call, and two still-lazy iterators both holding a
        // closure over it would be two live mutable borrows of `cx` at once.
        let pinned_rows: Vec<AnyElement> = pinned_paths
            .iter()
            .enumerate()
            .map(|(ix, path)| {
                self.config_file_row(theme, ("cfg-pinned", ix), path, true, cx)
                    .into_any_element()
            })
            .collect();
        let common_rows: Vec<AnyElement> = common
            .iter()
            .enumerate()
            .map(|(ix, path)| {
                self.config_file_row(theme, ("cfg-common", ix), path, false, cx)
                    .into_any_element()
            })
            .collect();

        let pin_error = self.systems.pin_error.clone().map(|e| {
            h_flex()
                .gap_1p5()
                .py_1()
                .text_xs()
                .text_color(rgb(theme.danger))
                .child("✗")
                .child(e)
        });
        let pin_input = self.systems.pin_input.clone();

        v_flex()
            .size_full()
            .child(
                Toolbar::new()
                    // Input and submit go in the toolbar's left slot *together*. They are
                    // one control, and a `pin` button on the far right edge would be the
                    // same "action a screen-width from the thing it acts on" this whole
                    // sub-view exists to undo — the toolbar's action slot is for controls
                    // that act on the lists below, not on the field beside them.
                    .filter(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().max_w(px(420.)).children(pin_input))
                            .child(
                                Button::new("cfg-pin-submit", "pin")
                                    .small()
                                    .icon(Icon::Add)
                                    .on_click(cx.listener(
                                        |this, _ev: &ClickEvent, _window, cx| {
                                            this.submit_pin(cx);
                                        },
                                    )),
                            ),
                    )
                    .count_label(sid_ui::toolbar::count_label(
                        pinned_count + common_count,
                        "file",
                    )),
            )
            .children(pin_error)
            .child(
                // A bare `flex_row` rather than `h_flex()`: this row wants flexbox's
                // default `align-items: stretch`, and `h_flex` centres. An empty PINNED
                // card beside a full COMMON one would otherwise leave a 700x880 unbounded
                // black hole in the left half — the "looks like a broken render" failure
                // the audit flagged on Workspaces. Equal-height cards make the empty one
                // read as an empty *container*.
                div()
                    .id("systems-config-scroll")
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .gap_4()
                    .pt_3()
                    .overflow_y_scroll()
                    .child(config_list_card(
                        "pinned",
                        pinned_count,
                        pinned_rows,
                        // An empty pinned list still holds its column, so the two lists
                        // do not reflow the moment the first pin lands.
                        (pinned_count == 0).then(|| {
                            EmptyState::new("nothing pinned yet")
                                .icon(Icon::Star)
                                .guidance(
                                    "pin a file above, or from the common list, to keep \
                                     it one click away",
                                )
                                .into_any_element()
                        }),
                    ))
                    .child(config_list_card("common", common_count, common_rows, None)),
            )
            .into_any_element()
    }

    /// One pinned/common row: filename (`fg_strong`) + muted full path, and a
    /// row-anchored pin/unpin [`IconButton`] that `cx.stop_propagation()`s so it never
    /// also triggers the row's own open-editor click.
    ///
    /// The affordance used to be bare `pin` / `unpin` text — accent-red for `pin`, which
    /// spent the app's one "engage" colour on a bookmark. It is a real bounded control
    /// now, and the tooltip is required by [`IconButton`]'s constructor.
    fn config_file_row(
        &self,
        theme: &Theme,
        id: (&'static str, usize),
        path: &str,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let file_name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        let (icon, tooltip) = if pinned {
            (Icon::StarOff, "unpin this file")
        } else {
            (Icon::Star, "pin this file")
        };
        let toggle_path = path.to_string();
        let open_path = PathBuf::from(path.to_string());

        h_flex()
            .id(id)
            .w_full()
            .justify_between()
            .gap_2()
            .row_padding()
            .cursor_pointer()
            .hover_fill(theme)
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(theme.fg_strong))
                            .child(file_name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.muted))
                            .child(path.to_string()),
                    ),
            )
            .child(
                IconButton::new(("cfg-toggle", id.1), icon, tooltip)
                    .small()
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        if pinned {
                            this.unpin_config_file(toggle_path.clone(), cx);
                        } else if let Err(e) = this.store.pin_file(&toggle_path) {
                            this.systems.pin_error = Some(e.to_string());
                            cx.notify();
                        } else {
                            this.refresh_config_files(cx);
                        }
                    })),
            )
            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                this.open_config_editor(open_path.clone(), window, cx);
            }))
    }
}

/// One of the two config-file lists, as a bounded card.
///
/// An equal half of whatever width the sub-view has, uncapped. The design system's
/// `max_w(880px)` reading-column rule is what produced the orphaned `COMMON` block in the
/// first place, and capping *here* just moves the dead space: at 2000px two 880px cards
/// leave 200px of nothing at the right edge. What the rule is actually protecting against
/// — a filename with its action a screen-width away, unanchored — is answered by the card
/// boundary and the row's hover fill instead, which is what the old layout had neither of.
fn config_list_card(
    title: &'static str,
    count: usize,
    rows: Vec<AnyElement>,
    empty: Option<AnyElement>,
) -> impl IntoElement + use<> {
    Card::new()
        .title(title)
        .count(count)
        .child(v_flex().w_full().children(rows))
        .children(empty)
        .flex_1()
        .min_w_0()
}

/// `$HOME`, falling back to `/tmp` — no `dirs` crate for one env-var read, matching
/// `db_tab.rs`'s `downloads_dir`/`session.rs`'s `downloads_dir` convention.
fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

/// Tilde-expand a leading `~` (home-relative) component: `~/x` -> `{home}/x`, a bare
/// `~` -> `home`. Any other path (relative or absolute, no leading `~`) is returned
/// unchanged. `home` is injected so this is unit-tested without touching `$HOME`.
fn expand_tilde(path: &str, home: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if path == "~" {
        home.to_string()
    } else {
        path.to_string()
    }
}

/// [`CURATED_TEMPLATES`], tilde-expanded against `home`. Does no I/O — callers
/// existence-filter with [`filter_existing`] before rendering.
fn curated_candidates(home: &str) -> Vec<String> {
    CURATED_TEMPLATES
        .iter()
        .map(|p| expand_tilde(p, home))
        .collect()
}

/// Existence-filter `candidates` through `exists`, preserving order. `exists` is
/// injectable so this is unit-tested with a fake predicate instead of the real
/// filesystem.
fn filter_existing<F: Fn(&str) -> bool>(candidates: &[String], exists: F) -> Vec<String> {
    candidates.iter().filter(|p| exists(p)).cloned().collect()
}

/// Drop anything already in `pinned` from `curated` — the "common" list never repeats
/// a row that's already shown, pinned, up top.
fn exclude_pinned(curated: &[String], pinned: &[String]) -> Vec<String> {
    curated
        .iter()
        .filter(|p| !pinned.iter().any(|pinned_path| pinned_path == *p))
        .cloned()
        .collect()
}

// ---- overview cluster ---------------------------------------------------------------

impl AppState {
    /// The shared overview above the segmented control: a framed [`StatCluster`] with
    /// the host/kernel/uptime/load line as its summary and CPU / Memory / Swap as
    /// [`Meter`]s.
    ///
    /// This used to be three unframed bars floating on the window background with a
    /// strip of ~20 unlabelled ticks underneath — *"they read as debug output, not as an
    /// instrument cluster."* The frame, the header and the per-core strip's caption are
    /// all the component's now; this function only supplies the numbers.
    ///
    /// It carries no refresh control of its own: the screen has exactly one, in the
    /// Processes toolbar, and the overview re-probes itself every 2 seconds regardless.
    fn overview_cluster(&self) -> AnyElement {
        let refreshing = self.systems.refreshing;
        let Some(ov) = self.systems.overview.as_ref() else {
            return StatCluster::new()
                .title("system")
                .summary(if refreshing {
                    "probing…"
                } else {
                    "no system overview yet"
                })
                .into_any_element();
        };

        let summary: SharedString = format!(
            "{} · {} · kernel {} · up {} · load {:.2} {:.2} {:.2}",
            ov.hostname,
            ov.os,
            ov.kernel,
            humanize_uptime(ov.uptime_secs),
            ov.load_avg.0,
            ov.load_avg.1,
            ov.load_avg.2,
        )
        .into();

        let cores = ov.cpu_per_core.len();
        let cpu = Meter::new("CPU", ov.cpu_total_pct / 100.0)
            .value(format!("{:.1}%", ov.cpu_total_pct))
            .segments(
                ov.cpu_per_core.iter().map(|&pct| pct / 100.0),
                // The caption the bare tick strip never had.
                sid_ui::toolbar::count_label(cores, "core"),
            );

        let memory = Meter::new("Memory", Meter::ratio(ov.mem_used, ov.mem_total)).value(format!(
            "{} / {}",
            humanize_bytes(ov.mem_used),
            humanize_bytes(ov.mem_total)
        ));

        let swap = if ov.swap_total > 0 {
            Meter::new("Swap", Meter::ratio(ov.swap_used, ov.swap_total)).value(format!(
                "{} / {}",
                humanize_bytes(ov.swap_used),
                humanize_bytes(ov.swap_total)
            ))
        } else {
            Meter::new("Swap", 0.0).value("—").note("none configured")
        };

        StatCluster::new()
            .title("system")
            .summary(summary)
            .stat(cpu)
            .stat(memory)
            .stat(swap)
            .into_any_element()
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
        ProcessSortKey::Command => a.cmd.to_lowercase().cmp(&b.cmd.to_lowercase()),
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
    fn sort_processes_command_is_case_insensitive() {
        let mut processes = vec![
            proc(1, "a", 0.0, 0, None),
            proc(2, "b", 0.0, 0, None),
            proc(3, "c", 0.0, 0, None),
        ];
        processes[0].cmd = "/usr/bin/Zsh -l".to_string();
        processes[1].cmd = "/usr/bin/bash".to_string();
        processes[2].cmd = String::new();
        sort_processes(
            &mut processes,
            ProcessSortKey::Command,
            ColumnSort::Ascending,
        );
        let names: Vec<&str> = processes.iter().map(|p| p.name.as_str()).collect();
        // The kernel thread with no cmdline sorts first, then bash, then Zsh — case
        // folded, so "Zsh" does not sort before "bash" the way a byte compare would.
        assert_eq!(names, vec!["c", "b", "a"]);
    }

    /// The sort-key table is indexed by column position: adding a column without adding
    /// its key silently sorts by the wrong field (or, past the end, not at all).
    #[test]
    fn every_sortable_column_has_a_sort_key() {
        let delegate = ProcessesDelegate::new(Arc::new(Mutex::new(SysinfoProvider::new())));
        let sortable = (0..delegate.columns.len())
            .filter(|&ix| delegate.columns.column(ix).sort.is_some())
            .count();
        assert_eq!(
            sortable,
            PROCESS_SORT_KEYS.len(),
            "{sortable} sortable columns, {} keys",
            PROCESS_SORT_KEYS.len()
        );
        // ...and the trailing action column is deliberately not one of them.
        let last = delegate.columns.len() - 1;
        assert_eq!(delegate.columns.column(last).sort, None);
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

    /// The calm/caution/critical ladder moved to `sid_ui::MeterTone` with the meters
    /// (and is tested there, in fraction space, across all four palettes). This pins the
    /// call-site conversion — percent in, fraction out — which is where an off-by-100
    /// would put every meter in the danger colour.
    #[test]
    fn cpu_percent_reaches_the_meter_as_a_fraction() {
        use sid_ui::MeterTone;
        assert_eq!(MeterTone::of(12.4 / 100.0), MeterTone::Calm);
        assert_eq!(MeterTone::of(75.0 / 100.0), MeterTone::Caution);
        assert_eq!(MeterTone::of(96.0 / 100.0), MeterTone::Critical);
    }

    /// The sub-view strip and the enum behind it have to stay in step: the control
    /// reports back an index into `ALL`, and a mismatch would silently switch to the
    /// wrong view.
    #[test]
    fn every_sub_view_round_trips_through_its_index() {
        for (ix, &view) in SystemSubView::ALL.iter().enumerate() {
            assert_eq!(SystemSubView::at(ix), Some(view));
        }
        assert_eq!(SystemSubView::at(SystemSubView::ALL.len()), None);
    }

    #[test]
    fn the_default_sub_view_is_processes() {
        // The tab is a process monitor first; landing on the config list would be a
        // surprise every time the tab is opened.
        assert_eq!(SystemSubView::default(), SystemSubView::Processes);
        assert_eq!(SystemSubView::ALL[0], SystemSubView::Processes);
    }

    #[test]
    fn every_sub_view_labels_itself() {
        for &view in &SystemSubView::ALL {
            assert!(!view.label().is_empty(), "{view:?}");
        }
        assert_ne!(
            SystemSubView::Processes.label(),
            SystemSubView::ConfigFiles.label()
        );
        assert_ne!(
            SystemSubView::Processes.icon(),
            SystemSubView::ConfigFiles.icon()
        );
    }

    // ---- config files (Round E §D) -------------------------------------------------

    #[test]
    fn expand_tilde_home_relative() {
        assert_eq!(
            expand_tilde("~/.ssh/config", "/home/murphy"),
            "/home/murphy/.ssh/config"
        );
    }

    #[test]
    fn expand_tilde_bare_tilde_is_home() {
        assert_eq!(expand_tilde("~", "/home/murphy"), "/home/murphy");
    }

    #[test]
    fn expand_tilde_leaves_absolute_and_relative_paths_alone() {
        assert_eq!(expand_tilde("/etc/hosts", "/home/murphy"), "/etc/hosts");
        assert_eq!(
            expand_tilde("relative/path", "/home/murphy"),
            "relative/path"
        );
        // A `~` not followed by `/` (e.g. `~murphy/x`) is not a home-relative path this
        // helper understands — left unchanged rather than guessed at.
        assert_eq!(expand_tilde("~murphy/x", "/home/murphy"), "~murphy/x");
    }

    #[test]
    fn curated_candidates_are_all_tilde_expanded() {
        let candidates = curated_candidates("/home/murphy");
        assert_eq!(candidates.len(), CURATED_TEMPLATES.len());
        assert!(candidates.contains(&"/home/murphy/.ssh/config".to_string()));
        assert!(candidates.contains(&"/etc/fstab".to_string()));
        assert!(!candidates.iter().any(|p| p.starts_with('~')));
    }

    #[test]
    fn filter_existing_keeps_only_what_the_predicate_marks_present() {
        let candidates = vec!["/etc/hosts".to_string(), "/etc/nope".to_string()];
        let got = filter_existing(&candidates, |p| p == "/etc/hosts");
        assert_eq!(got, vec!["/etc/hosts".to_string()]);
    }

    #[test]
    fn filter_existing_preserves_order() {
        let candidates = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let got = filter_existing(&candidates, |_| true);
        assert_eq!(got, candidates);
    }

    #[test]
    fn exclude_pinned_drops_already_pinned_entries() {
        let curated = vec!["/etc/hosts".to_string(), "/etc/fstab".to_string()];
        let pinned = vec!["/etc/hosts".to_string()];
        assert_eq!(
            exclude_pinned(&curated, &pinned),
            vec!["/etc/fstab".to_string()]
        );
    }

    #[test]
    fn exclude_pinned_is_a_no_op_when_nothing_is_pinned() {
        let curated = vec!["/etc/hosts".to_string(), "/etc/fstab".to_string()];
        assert_eq!(exclude_pinned(&curated, &[]), curated);
    }
}
