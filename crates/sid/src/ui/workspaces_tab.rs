//! Workspaces tab (track U): register/rename/unregister workspaces, focus a workspace
//! as the active scope, and see its git state alongside what sid knows inside it.
//!
//! [`WorkspacesTabState`] is a sibling cache to `AppState`'s own fields — a second
//! `impl AppState` block lives here rather than in `app.rs`, same shape `ui::db_tab`'s
//! `DbTabState` established (see that module's doc comment): this file reaches back
//! into `AppState`'s `pub(crate)` fields (`store`, `scope`, `scopes`, `error`) and calls
//! `AppState::set_scope`/`switch_to_tab`/`reload_scopes_runtime` directly.
//!
//! A workspace row answers two questions at once: "what state is this repo in?" (git,
//! via `sid_core::git::GitProvider`) and "what does sid know inside it?" (its own
//! `.sid/config.toml` layer's hosts/connections). The detail pane's shape depends on
//! what the registered root actually IS — see [`workspace_shape`]:
//! - a git repo -> the single-repo sub-tabs (Overview/Branches/Status/Log);
//! - a plain directory containing >=1 sibling git repos one level deep (the BUILD
//!   ADDENDUM's "umbrella") -> a sortable fleet dashboard, one row per child repo;
//! - anything else -> a scope-only view with a muted "not a git repo" note.
//!
//! Every git fetch (`fetch_summary`/`ensure_branches_loaded`/`ensure_status_loaded`/
//! `ensure_log_loaded`/`checkout_branch`/the fleet's per-repo fetch) follows `db_tab`'s
//! `schema_generation` guard pattern exactly: bump a generation counter immediately
//! before `cx.spawn`, capture it, and apply the completed result only if it still
//! matches — see `WorkspacesTabState::list_generation`/`detail_generation`/
//! `fleet_generation`'s doc comments. sid-git's real implementation is landing on a
//! parallel branch; on this branch every `GitProvider` method returns
//! `GitError::Other("sid-git port in progress")` (via `crate::git_registry`), so every
//! git-backed panel here is built and verified against that honest loading/error state.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*, px, rgb,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableState};
use sid_core::git::{
    Branch, CommitInfo, GitError, GitStatus, RepoSummary, StatusEntry, StatusKind,
};
use sid_store::{DbConnection, Host, Scope, ViewFilters, WorkspaceId, WorkspaceMeta};

use crate::app::{AppState, Tab};
use crate::git_registry;
use crate::ui::TextInput;
use crate::ui::session::ssh_runtime;
use crate::ui::theme;

/// Monospace family for root/path subtitles; matches every other tab's `MONO`.
const MONO: &str = "DejaVu Sans Mono";

/// Recent-commits cap for the Log sub-tab, per the plan.
const LOG_LIMIT: usize = 50;

// ---- pure domain types ------------------------------------------------------------

/// One workspace root's shape, driving which detail sub-view renders — see the module
/// doc comment. Detection is filesystem-only (cheap `stat`s, no git `open`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceShape {
    Repo,
    /// A directory of sibling git repos, one level deep — the child repo paths.
    Umbrella(Vec<PathBuf>),
    Plain,
}

/// Classify `root` per [`WorkspaceShape`]'s doc comment: a repo itself -> `Repo`; else,
/// if any immediate child directory is itself a repo -> `Umbrella` (those children
/// only); else -> `Plain`. `is_repo`/`children` are injected so this is unit-tested
/// against a fake filesystem with no real `Path` ever touched — real call sites pass
/// [`fs_is_git_repo`]/[`fs_child_dirs`].
pub(crate) fn workspace_shape(
    root: &Path,
    is_repo: &dyn Fn(&Path) -> bool,
    children: &dyn Fn(&Path) -> Vec<PathBuf>,
) -> WorkspaceShape {
    if is_repo(root) {
        return WorkspaceShape::Repo;
    }
    let repos: Vec<PathBuf> = children(root).into_iter().filter(|c| is_repo(c)).collect();
    if repos.is_empty() {
        WorkspaceShape::Plain
    } else {
        WorkspaceShape::Umbrella(repos)
    }
}

/// Real filesystem probe for [`workspace_shape`]'s `is_repo` — a `.git` entry (dir, or
/// a file for a linked worktree's gitdir pointer) directly under `path`.
fn fs_is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Real filesystem probe for [`workspace_shape`]'s `children` — every immediate
/// subdirectory of `root`, one level deep. Empty (never an error) on a read failure —
/// callers only care whether any of them is a repo.
fn fs_child_dirs(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// The chip/panel-facing error shape — collapses `GitError` to what the UI actually
/// distinguishes: "not a git repo" (muted — expected for a `Plain` workspace someone
/// still asked for a summary of) vs. everything else (danger — a real failure).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitPanelError {
    NotARepo,
    Other(String),
}

impl std::fmt::Display for GitPanelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitPanelError::NotARepo => write!(f, "not a git repo"),
            GitPanelError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<GitError> for GitPanelError {
    fn from(e: GitError) -> Self {
        match e {
            GitError::NotARepo(_) => GitPanelError::NotARepo,
            other => GitPanelError::Other(other.to_string()),
        }
    }
}

/// One async git fetch's state. `None` (never fetched) is the caller's own
/// `Option<Fetch<T>>` field — folding that in here too would just be a third variant
/// indistinguishable from the field being absent.
#[derive(Clone)]
pub(crate) enum Fetch<T> {
    Loading,
    Done(Result<T, GitPanelError>),
}

impl<T> Fetch<T> {
    fn ok(&self) -> Option<&T> {
        match self {
            Fetch::Done(Ok(v)) => Some(v),
            _ => None,
        }
    }
}

/// Format a commit's age relative to `now_secs`, both in seconds since the Unix epoch.
/// Pure (both times are parameters) so it's unit-testable without touching the system
/// clock — real call sites pass [`now_secs`]. Buckets: "just now", then
/// minutes/hours/days/weeks/months/years "ago" (30-day months, 365-day years —
/// approximate, matching every other relative-time label's precision).
fn commit_age(now_secs: i64, then_secs: i64) -> String {
    let diff = (now_secs - then_secs).max(0);
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    if diff < MIN {
        "just now".to_string()
    } else if diff < HOUR {
        format!("{}m ago", diff / MIN)
    } else if diff < DAY {
        format!("{}h ago", diff / HOUR)
    } else if diff < WEEK {
        format!("{}d ago", diff / DAY)
    } else if diff < MONTH {
        format!("{}w ago", diff / WEEK)
    } else if diff < YEAR {
        format!("{}mo ago", diff / MONTH)
    } else {
        format!("{}y ago", diff / YEAR)
    }
}

/// Wall-clock "now" in Unix seconds, for [`commit_age`] call sites in `render`.
/// Reading the clock is not store/filesystem I/O — every other relative-time label in
/// the app (and everywhere else) does this in render.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

/// Tilde-expand a leading `~` component: `~/x` -> `{home}/x`, a bare `~` -> `home`. Any
/// other path is returned unchanged. Mirrors `systems_tab::expand_tilde` exactly (kept
/// as its own copy — neither module depends on the other, and it's three lines).
fn expand_tilde(path: &str, home: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if path == "~" {
        home.to_string()
    } else {
        path.to_string()
    }
}

/// The `+ add` input's validation: a non-empty path that is a directory. `is_dir` is
/// injected so this is unit-tested without touching the real filesystem.
fn validate_workspace_path(expanded: &str, is_dir: &dyn Fn(&str) -> bool) -> Result<(), String> {
    if expanded.is_empty() {
        return Err("enter a path".to_string());
    }
    if !is_dir(expanded) {
        return Err(format!("{expanded}: not a directory"));
    }
    Ok(())
}

/// Two-click unregister: `true` when `clicked` is the workspace already armed. Mirrors
/// `app::delete_click_executes`, keyed on `WorkspaceId` alone — a workspace
/// registration lives in exactly one place, unlike a host/connection's (alias, origin).
fn unregister_click_executes(armed: Option<&WorkspaceId>, clicked: &WorkspaceId) -> bool {
    armed == Some(clicked)
}

/// The active detail sub-tab for a `Repo`-shaped workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetailSubTab {
    Overview,
    Branches,
    Status,
    Log,
}

impl DetailSubTab {
    const ALL: [DetailSubTab; 4] = [Self::Overview, Self::Branches, Self::Status, Self::Log];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Branches => "Branches",
            Self::Status => "Status",
            Self::Log => "Log",
        }
    }
}

/// The row currently mid-rename — meta-only (renames `WorkspaceMeta::name`, never
/// touches the filesystem), mirrors `db_tab::RenameState`'s shape.
struct RenameState {
    id: WorkspaceId,
    input: Entity<TextInput>,
}

// ---- Umbrella fleet table (gpui-component `TableDelegate`) -------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    fn from_column_sort(sort: ColumnSort) -> Option<Self> {
        match sort {
            ColumnSort::Ascending => Some(Self::Asc),
            ColumnSort::Descending => Some(Self::Desc),
            ColumnSort::Default => None,
        }
    }

    fn apply(self, order: Ordering) -> Ordering {
        match self {
            Self::Asc => order,
            Self::Desc => order.reverse(),
        }
    }
}

/// Keep a `Column`'s sort-chevron state in sync with `active_sort` — mirrors
/// `network_tab::mark_column_sort` exactly.
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

/// One fleet row: a child repo's name/path (known from the filesystem scan up front)
/// plus its `summary()` fetch, applied in place as it completes.
#[derive(Clone)]
struct FleetRow {
    name: String,
    path: PathBuf,
    fetch: Fetch<RepoSummary>,
}

fn fleet_branch(row: &FleetRow) -> Option<&str> {
    row.fetch.ok().and_then(|s| s.branch.as_deref())
}

fn fleet_dirty(row: &FleetRow) -> Option<usize> {
    row.fetch.ok().map(|s| s.staged + s.unstaged + s.untracked)
}

fn fleet_ahead(row: &FleetRow) -> Option<usize> {
    row.fetch.ok().and_then(|s| s.ahead)
}

fn fleet_behind(row: &FleetRow) -> Option<usize> {
    row.fetch.ok().and_then(|s| s.behind)
}

fn fleet_age(row: &FleetRow) -> Option<i64> {
    row.fetch
        .ok()
        .and_then(|s| s.last_commit.as_ref())
        .map(|c| c.timestamp_secs)
}

/// `None` always sorts after `Some` (an unknown/error/no-upstream value, not a "low"
/// one) — mirrors `network_tab::cmp_port_pid`'s idiom exactly.
fn cmp_opt_usize(a: Option<usize>, b: Option<usize>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn cmp_opt_i64(a: Option<i64>, b: Option<i64>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn cmp_opt_str(a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn cmp_fleet_repo(a: &FleetRow, b: &FleetRow) -> Ordering {
    a.name.to_lowercase().cmp(&b.name.to_lowercase())
}

fn cmp_fleet_branch(a: &FleetRow, b: &FleetRow) -> Ordering {
    cmp_opt_str(fleet_branch(a), fleet_branch(b))
}

/// Numeric — never lexicographic (`9 < 10`, not the string order).
fn cmp_fleet_dirty(a: &FleetRow, b: &FleetRow) -> Ordering {
    cmp_opt_usize(fleet_dirty(a), fleet_dirty(b))
}

fn cmp_fleet_ahead_behind(a: &FleetRow, b: &FleetRow) -> Ordering {
    cmp_opt_usize(fleet_ahead(a), fleet_ahead(b))
        .then_with(|| cmp_opt_usize(fleet_behind(a), fleet_behind(b)))
}

fn cmp_fleet_age(a: &FleetRow, b: &FleetRow) -> Ordering {
    cmp_opt_i64(fleet_age(a), fleet_age(b))
}

fn cmp_fleet_path(a: &FleetRow, b: &FleetRow) -> Ordering {
    a.path.cmp(&b.path)
}

fn sort_fleet_rows(rows: &mut [FleetRow], col_ix: usize, dir: SortDir) {
    let cmp: fn(&FleetRow, &FleetRow) -> Ordering = match col_ix {
        0 => cmp_fleet_repo,
        1 => cmp_fleet_branch,
        2 => cmp_fleet_dirty,
        3 => cmp_fleet_ahead_behind,
        4 => cmp_fleet_age,
        5 => cmp_fleet_path,
        _ => return,
    };
    rows.sort_by(|a, b| dir.apply(cmp(a, b)));
}

fn text_cell(color: u32, s: impl Into<SharedString>) -> AnyElement {
    div()
        .text_sm()
        .text_color(rgb(color))
        .child(s.into())
        .into_any_element()
}

/// Read-only fleet delegate — no armed/interactive state, per `network_tab::
/// DockerDelegate`'s template for a read-only table.
struct FleetDelegate {
    rows: Vec<FleetRow>,
    columns: Vec<Column>,
    active_sort: Option<(usize, SortDir)>,
}

impl FleetDelegate {
    fn empty() -> Self {
        Self {
            rows: Vec::new(),
            columns: vec![
                Column::new("repo", "Repo").width(px(160.)).sortable(),
                Column::new("branch", "Branch").width(px(120.)).sortable(),
                Column::new("dirty", "Dirty").width(px(70.)).sortable(),
                Column::new("ahead_behind", "↑ / ↓")
                    .width(px(80.))
                    .sortable(),
                Column::new("age", "Last commit").width(px(110.)).sortable(),
                Column::new("path", "Path").width(px(280.)).sortable(),
            ],
            active_sort: None,
        }
    }

    fn set_rows(&mut self, rows: Vec<FleetRow>) {
        self.rows = rows;
        self.recompute();
    }

    /// Apply completed per-repo fetches in place, keyed by path — a slow repo elsewhere
    /// in the batch never blocks an already-arrived row from updating.
    fn apply_results(&mut self, results: Vec<(PathBuf, Result<RepoSummary, GitPanelError>)>) {
        for (path, outcome) in results {
            if let Some(row) = self.rows.iter_mut().find(|r| r.path == path) {
                row.fetch = Fetch::Done(outcome);
            }
        }
        self.recompute();
    }

    fn recompute(&mut self) {
        if let Some((col_ix, dir)) = self.active_sort {
            sort_fleet_rows(&mut self.rows, col_ix, dir);
        }
    }
}

impl TableDelegate for FleetDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.rows.len()
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
        let t = theme::active(cx);
        let (fg, muted, warning) = (t.fg, t.muted, t.warning);
        // `ElementId` has no `From<(&str, usize, usize)>` impl — fold (row, col) into a
        // single index, same trick `network_tab` uses.
        let cell_id = ("ws-fleet-cell", row_ix * 8 + col_ix);

        let content: AnyElement = match self.rows.get(row_ix) {
            None => div().into_any_element(),
            Some(row) => match col_ix {
                0 => text_cell(fg, row.name.clone()),
                1 => match &row.fetch {
                    Fetch::Loading => text_cell(muted, "…"),
                    Fetch::Done(Ok(s)) => {
                        text_cell(fg, s.branch.clone().unwrap_or_else(|| "(detached)".into()))
                    }
                    // Per-row errors render as muted text, never a panic.
                    Fetch::Done(Err(_)) => text_cell(muted, "—"),
                },
                2 => match &row.fetch {
                    Fetch::Loading => text_cell(muted, "…"),
                    Fetch::Done(Ok(s)) => {
                        let dirty = s.staged + s.unstaged + s.untracked;
                        let color = if dirty > 0 { warning } else { muted };
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(div().size(px(6.)).rounded_full().bg(rgb(color)))
                            .child(text_cell(fg, dirty.to_string()))
                            .into_any_element()
                    }
                    Fetch::Done(Err(e)) => text_cell(muted, e.to_string()),
                },
                3 => match &row.fetch {
                    Fetch::Loading => text_cell(muted, "…"),
                    Fetch::Done(Ok(s)) => {
                        let label = match (s.ahead, s.behind) {
                            (None, None) => "—".to_string(),
                            (a, b) => format!("↑{} ↓{}", a.unwrap_or(0), b.unwrap_or(0)),
                        };
                        text_cell(fg, label)
                    }
                    Fetch::Done(Err(_)) => text_cell(muted, "—"),
                },
                4 => match &row.fetch {
                    Fetch::Loading => text_cell(muted, "…"),
                    Fetch::Done(Ok(s)) => {
                        let label = s
                            .last_commit
                            .as_ref()
                            .map(|c| commit_age(now_secs(), c.timestamp_secs))
                            .unwrap_or_else(|| "—".into());
                        text_cell(fg, label)
                    }
                    Fetch::Done(Err(_)) => text_cell(muted, "—"),
                },
                5 => div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .font_family(MONO)
                    .child(row.path.display().to_string())
                    .into_any_element(),
                _ => div().into_any_element(),
            },
        };

        div().id(cell_id).px_2().child(content)
    }
}

// ---- state --------------------------------------------------------------------------

/// Workspaces tab state: the registered-workspace list, per-workspace git fetches, and
/// the row-level interaction state (rename/unregister/add/right-click).
pub struct WorkspacesTabState {
    /// `Store::list_workspaces()`, refreshed on tab activation and after every
    /// register/rename/unregister.
    list: Vec<WorkspaceMeta>,
    /// Whether the list has loaded at least once this session — the `systems_tab`
    /// convention (load on first activation, not at `AppState::new`).
    loaded: bool,
    /// The selected row, if any — drives the detail pane.
    selected: Option<WorkspaceId>,
    /// The selected workspace's shape, computed once at selection time (cheap
    /// filesystem `stat`s — never in `render`; see `workspace_shape`).
    shape: Option<WorkspaceShape>,
    /// One `summary()` result per workspace — shared by the list row's git chip AND
    /// (for a `Repo`-shaped selection) the Overview sub-tab, since `RepoSummary`
    /// already carries everything both need.
    summaries: HashMap<WorkspaceId, Fetch<RepoSummary>>,
    /// (hosts, connections) counts in each workspace's OWN layer (no composition with
    /// global) — refreshed alongside `list`.
    scope_counts: HashMap<WorkspaceId, (usize, usize)>,
    /// Bumped on every list reload (`refresh_workspaces`); a summary-fetch completion
    /// applies only if it still matches — see the module doc's guard pattern.
    list_generation: u64,

    // ---- Repo detail: Overview / Branches / Status / Log ---------------------------
    sub_tab: DetailSubTab,
    /// The selected workspace's own hosts/connections (Overview's scope-items list).
    overview_hosts: Vec<Host>,
    overview_connections: Vec<DbConnection>,
    branches: Option<Fetch<Vec<Branch>>>,
    status: Option<Fetch<GitStatus>>,
    log: Option<Fetch<Vec<CommitInfo>>>,
    /// The branch name a checkout is currently running against, if any.
    checkout_pending: Option<String>,
    checkout_error: Option<String>,
    /// Bumped on selection change and manual refresh; guards branches/status/log/
    /// checkout completions against a stale selection — see the module doc.
    detail_generation: u64,

    // ---- Umbrella detail: the fleet table -------------------------------------------
    /// Lazily built (`TableState::new` needs `window`) — see `ensure_workspaces_widgets`.
    fleet: Option<Entity<TableState<FleetDelegate>>>,
    /// Bumped on every umbrella (re)selection/refresh; guards the concurrent per-repo
    /// fetch — see the module doc.
    fleet_generation: u64,

    // ---- `+ add` inline path input ---------------------------------------------------
    add_open: bool,
    add_input: Option<Entity<TextInput>>,
    add_error: Option<String>,

    // ---- row-level interaction state ------------------------------------------------
    renaming: Option<RenameState>,
    armed_unregister: Option<WorkspaceId>,
    /// The list's single right-click target — mirrors `ssh_home::HomeTabState::
    /// right_click_target`'s doc comment on why one indirection replaces a
    /// `.context_menu()` attached per row (every row's wrapper would collide on the
    /// same `GlobalElementId`).
    right_click_target: Option<WorkspaceId>,
}

impl WorkspacesTabState {
    /// `TextInput::new` needs no `window` (unlike the fleet's `TableState`), so the
    /// add-path input is built eagerly here — same as `ssh_home::HomeTabState::new`'s
    /// quick-connect box.
    pub(crate) fn new(cx: &mut Context<AppState>) -> Self {
        Self {
            list: Vec::new(),
            loaded: false,
            selected: None,
            shape: None,
            summaries: HashMap::new(),
            scope_counts: HashMap::new(),
            list_generation: 0,
            sub_tab: DetailSubTab::Overview,
            overview_hosts: Vec::new(),
            overview_connections: Vec::new(),
            branches: None,
            status: None,
            log: None,
            checkout_pending: None,
            checkout_error: None,
            detail_generation: 0,
            fleet: None,
            fleet_generation: 0,
            add_open: false,
            add_input: Some(cx.new(|cx| TextInput::new(cx, "~/path/to/workspace"))),
            add_error: None,
            renaming: None,
            armed_unregister: None,
            right_click_target: None,
        }
    }
}

// ---- AppState: render + mutation -----------------------------------------------------

impl AppState {
    /// Lazily build widgets that need `window` (the fleet's `TableState`) — called
    /// unconditionally at the top of `workspaces_tab`, idempotent after the first call.
    fn ensure_workspaces_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.fleet.is_none() {
            self.workspaces.fleet =
                Some(cx.new(|cx| TableState::new(FleetDelegate::empty(), window, cx)));
        }
    }

    pub(crate) fn workspaces_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_workspaces_widgets(window, cx);
        if !self.workspaces.loaded {
            self.workspaces.loaded = true;
            self.refresh_workspaces(cx);
        }
        if self.workspaces.selected.is_some()
            && matches!(self.workspaces.shape, Some(WorkspaceShape::Repo))
        {
            match self.workspaces.sub_tab {
                DetailSubTab::Overview => {}
                DetailSubTab::Branches => self.ensure_branches_loaded(cx),
                DetailSubTab::Status => self.ensure_status_loaded(cx),
                DetailSubTab::Log => self.ensure_log_loaded(cx),
            }
        }

        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .child(self.workspaces_list_panel(cx))
            .child(self.workspaces_detail_panel(cx))
            .into_any_element()
    }

    /// Re-list registered workspaces + their per-workspace scope-item counts, and
    /// (re)kick a `summary()` fetch for each. The `+ add`, rename, and unregister flows
    /// all call this (after `reload_scopes_runtime`, so the scope chips and this list
    /// never disagree about what's registered) — also reachable via the header's `⟳`.
    pub(crate) fn refresh_workspaces(&mut self, cx: &mut Context<Self>) {
        self.workspaces.armed_unregister = None;
        self.workspaces.right_click_target = None;
        self.workspaces.list_generation += 1;

        let filters = ViewFilters {
            collapse_duplicates: false,
            hide_global: true,
        };
        match self.store.list_workspaces() {
            Ok(list) => {
                self.workspaces.scope_counts = list
                    .iter()
                    .map(|m| {
                        let hosts = self
                            .store
                            .read_hosts(&Scope::Workspace(m.id.clone()), filters)
                            .map(|v| v.len())
                            .unwrap_or(0);
                        let conns = self
                            .store
                            .read_connections(&Scope::Workspace(m.id.clone()), filters)
                            .map(|v| v.len())
                            .unwrap_or(0);
                        (m.id.clone(), (hosts, conns))
                    })
                    .collect();
                self.workspaces.list = list;
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.workspaces.list = Vec::new();
                self.workspaces.scope_counts = HashMap::new();
            }
        }

        if let Some(id) = &self.workspaces.selected
            && !self.workspaces.list.iter().any(|m| &m.id == id)
        {
            self.workspaces.selected = None;
            self.workspaces.shape = None;
        }

        for meta in self.workspaces.list.clone() {
            self.fetch_summary(meta.id, meta.root, cx);
        }
        cx.notify();
    }

    /// Kick off (or re-kick) a `summary()` fetch for `id` at `root`. Shared by the list
    /// refresh (every row) and a `Repo`-shaped selection (Overview) — see
    /// `WorkspacesTabState::summaries`'s doc comment.
    fn fetch_summary(&mut self, id: WorkspaceId, root: PathBuf, cx: &mut Context<Self>) {
        self.workspaces.summaries.insert(id.clone(), Fetch::Loading);
        let generation = self.workspaces.list_generation;
        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                git_registry::factory()
                    .open(&root)
                    .and_then(|repo| repo.summary())
            });
            let outcome = match handle.await {
                Ok(r) => r.map_err(GitPanelError::from),
                Err(join_err) => Err(GitPanelError::Other(format!(
                    "git task panicked: {join_err}"
                ))),
            };
            let _ = this.update(cx, |this, cx| {
                if this.workspaces.list_generation != generation {
                    // Stale: the workspace list was reloaded since this fetch started.
                    return;
                }
                this.workspaces.summaries.insert(id, Fetch::Done(outcome));
                cx.notify();
            });
        })
        .detach();
    }

    /// (id, root) for the currently selected workspace, if it's still registered.
    fn selected_workspace_root(&self) -> Option<(WorkspaceId, PathBuf)> {
        let id = self.workspaces.selected.clone()?;
        let meta = self.workspaces.list.iter().find(|m| m.id == id)?;
        Some((id, meta.root.clone()))
    }

    /// Row click: select `id`, compute its shape, load its scope items, and kick the
    /// shape-appropriate git fetch (a `Repo`'s summary, or an `Umbrella`'s fleet).
    fn select_workspace(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        self.workspaces.renaming = None;
        self.workspaces.armed_unregister = None;
        self.workspaces.selected = Some(id.clone());
        self.workspaces.sub_tab = DetailSubTab::Overview;
        self.workspaces.branches = None;
        self.workspaces.status = None;
        self.workspaces.log = None;
        self.workspaces.checkout_error = None;
        self.workspaces.checkout_pending = None;
        self.workspaces.detail_generation += 1;

        let Some(meta) = self.workspaces.list.iter().find(|m| m.id == id).cloned() else {
            self.workspaces.shape = None;
            cx.notify();
            return;
        };

        let shape = workspace_shape(&meta.root, &fs_is_git_repo, &fs_child_dirs);
        self.workspaces.shape = Some(shape.clone());

        let filters = ViewFilters {
            collapse_duplicates: false,
            hide_global: true,
        };
        self.workspaces.overview_hosts = self
            .store
            .read_hosts(&Scope::Workspace(id.clone()), filters)
            .map(|v| v.into_iter().map(|a| a.item).collect())
            .unwrap_or_default();
        self.workspaces.overview_connections = self
            .store
            .read_connections(&Scope::Workspace(id.clone()), filters)
            .map(|v| v.into_iter().map(|a| a.item).collect())
            .unwrap_or_default();

        match shape {
            WorkspaceShape::Repo => self.fetch_summary(id, meta.root, cx),
            WorkspaceShape::Umbrella(children) => self.fetch_fleet(children, cx),
            WorkspaceShape::Plain => {}
        }
        cx.notify();
    }

    /// Build the fleet's row set from `children` and fetch one `summary()` per repo,
    /// concurrently, on the shared runtime — the Umbrella dashboard's data source.
    fn fetch_fleet(&mut self, children: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.workspaces.fleet_generation += 1;
        let generation = self.workspaces.fleet_generation;

        let rows: Vec<FleetRow> = children
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                FleetRow {
                    name,
                    path,
                    fetch: Fetch::Loading,
                }
            })
            .collect();
        let paths: Vec<PathBuf> = rows.iter().map(|r| r.path.clone()).collect();

        if let Some(table) = self.workspaces.fleet.clone() {
            table.update(cx, |state, cx| {
                state.delegate_mut().set_rows(rows);
                cx.notify();
            });
        }

        let table_for_apply = self.workspaces.fleet.clone();
        cx.spawn(async move |this, cx| {
            // Spawn every repo's summary as its own task first (so they all run
            // concurrently on the shared runtime), then collect in order.
            let handles: Vec<(PathBuf, _)> = paths
                .into_iter()
                .map(|path| {
                    let p = path.clone();
                    let h = ssh_runtime().spawn(async move {
                        git_registry::factory()
                            .open(&p)
                            .and_then(|repo| repo.summary())
                    });
                    (path, h)
                })
                .collect();
            let mut results = Vec::with_capacity(handles.len());
            for (path, h) in handles {
                let outcome = match h.await {
                    Ok(r) => r.map_err(GitPanelError::from),
                    Err(join_err) => Err(GitPanelError::Other(format!(
                        "git task panicked: {join_err}"
                    ))),
                };
                results.push((path, outcome));
            }
            let _ = this.update(cx, |this, cx| {
                if this.workspaces.fleet_generation != generation {
                    // Stale: a different (or re-)selection superseded this scan.
                    return;
                }
                if let Some(table) = &table_for_apply {
                    table.update(cx, |state, cx| {
                        state.delegate_mut().apply_results(results);
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }

    fn ensure_branches_loaded(&mut self, cx: &mut Context<Self>) {
        if self.workspaces.branches.is_some() {
            return;
        }
        let Some((id, root)) = self.selected_workspace_root() else {
            return;
        };
        self.workspaces.branches = Some(Fetch::Loading);
        let generation = self.workspaces.detail_generation;
        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                git_registry::factory()
                    .open(&root)
                    .and_then(|repo| repo.list_branches())
            });
            let outcome = match handle.await {
                Ok(r) => r.map_err(GitPanelError::from),
                Err(join_err) => Err(GitPanelError::Other(format!(
                    "git task panicked: {join_err}"
                ))),
            };
            let _ = this.update(cx, |this, cx| {
                if this.workspaces.detail_generation != generation
                    || this.workspaces.selected.as_ref() != Some(&id)
                {
                    return;
                }
                this.workspaces.branches = Some(Fetch::Done(outcome));
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_status_loaded(&mut self, cx: &mut Context<Self>) {
        if self.workspaces.status.is_some() {
            return;
        }
        let Some((id, root)) = self.selected_workspace_root() else {
            return;
        };
        self.workspaces.status = Some(Fetch::Loading);
        let generation = self.workspaces.detail_generation;
        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                git_registry::factory()
                    .open(&root)
                    .and_then(|repo| repo.status())
            });
            let outcome = match handle.await {
                Ok(r) => r.map_err(GitPanelError::from),
                Err(join_err) => Err(GitPanelError::Other(format!(
                    "git task panicked: {join_err}"
                ))),
            };
            let _ = this.update(cx, |this, cx| {
                if this.workspaces.detail_generation != generation
                    || this.workspaces.selected.as_ref() != Some(&id)
                {
                    return;
                }
                this.workspaces.status = Some(Fetch::Done(outcome));
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_log_loaded(&mut self, cx: &mut Context<Self>) {
        if self.workspaces.log.is_some() {
            return;
        }
        let Some((id, root)) = self.selected_workspace_root() else {
            return;
        };
        self.workspaces.log = Some(Fetch::Loading);
        let generation = self.workspaces.detail_generation;
        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                git_registry::factory()
                    .open(&root)
                    .and_then(|repo| repo.commit_log(LOG_LIMIT))
            });
            let outcome = match handle.await {
                Ok(r) => r.map_err(GitPanelError::from),
                Err(join_err) => Err(GitPanelError::Other(format!(
                    "git task panicked: {join_err}"
                ))),
            };
            let _ = this.update(cx, |this, cx| {
                if this.workspaces.detail_generation != generation
                    || this.workspaces.selected.as_ref() != Some(&id)
                {
                    return;
                }
                this.workspaces.log = Some(Fetch::Done(outcome));
                cx.notify();
            });
        })
        .detach();
    }

    /// Branches row click (non-current branch only): checkout on the shared runtime.
    /// `GitError::DirtyWorkingTree` (sid never destroys uncommitted work) surfaces
    /// inline in danger text; success refreshes branches + the summary.
    fn checkout_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let Some((id, root)) = self.selected_workspace_root() else {
            return;
        };
        self.workspaces.checkout_error = None;
        self.workspaces.checkout_pending = Some(name.clone());
        let generation = self.workspaces.detail_generation;
        cx.notify();
        let root_for_task = root.clone();
        let id_for_apply = id.clone();
        cx.spawn(async move |this, cx| {
            let name_for_task = name.clone();
            let handle = ssh_runtime().spawn(async move {
                git_registry::factory()
                    .open(&root_for_task)
                    .and_then(|mut repo| repo.checkout_branch(&name_for_task))
            });
            let outcome = match handle.await {
                Ok(r) => r.map_err(GitPanelError::from),
                Err(join_err) => Err(GitPanelError::Other(format!(
                    "git task panicked: {join_err}"
                ))),
            };
            let _ = this.update(cx, |this, cx| {
                this.workspaces.checkout_pending = None;
                if this.workspaces.detail_generation != generation
                    || this.workspaces.selected.as_ref() != Some(&id_for_apply)
                {
                    return;
                }
                match outcome {
                    Ok(()) => {
                        this.workspaces.branches = None;
                        this.fetch_summary(id_for_apply, root, cx);
                        this.ensure_branches_loaded(cx);
                    }
                    Err(e) => this.workspaces.checkout_error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- add / rename / unregister / focus-scope -------------------------------------

    fn open_add_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspaces.add_open = true;
        self.workspaces.add_error = None;
        if let Some(input) = self.workspaces.add_input.clone() {
            input.read(cx).focus(window);
        }
        cx.notify();
    }

    fn cancel_add_workspace(&mut self, cx: &mut Context<Self>) {
        self.workspaces.add_open = false;
        self.workspaces.add_error = None;
        if let Some(input) = self.workspaces.add_input.clone() {
            input.update(cx, |i, cx| i.reset(cx));
        }
        cx.notify();
    }

    /// Enter (or the "add" affordance): tilde-expand, validate it's a directory,
    /// register, then rebuild the scope switcher at RUNTIME — this closes the
    /// `reload_scopes` startup-only caveat the BUILD ADDENDUM calls out.
    fn submit_add_workspace(&mut self, cx: &mut Context<Self>) {
        let raw = self
            .workspaces
            .add_input
            .as_ref()
            .map(|i| i.read(cx).content().to_string())
            .unwrap_or_default();
        let expanded = expand_tilde(raw.trim(), &home_dir());
        if let Err(e) = validate_workspace_path(&expanded, &|p| Path::new(p).is_dir()) {
            self.workspaces.add_error = Some(e);
            cx.notify();
            return;
        }
        match self.store.register_workspace_at(Path::new(&expanded)) {
            Ok(_meta) => {
                self.workspaces.add_open = false;
                self.workspaces.add_error = None;
                if let Some(input) = self.workspaces.add_input.clone() {
                    input.update(cx, |i, cx| i.reset(cx));
                }
                self.reload_scopes_runtime(cx);
                self.refresh_workspaces(cx);
            }
            Err(e) => self.workspaces.add_error = Some(e.to_string()),
        }
        cx.notify();
    }

    fn start_workspace_rename(
        &mut self,
        id: WorkspaceId,
        current_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspaces.armed_unregister = None;
        let input = cx.new(|cx| {
            let mut t = TextInput::new(cx, "name");
            t.set_content(current_name, cx);
            t
        });
        input.read(cx).focus(window);
        self.workspaces.renaming = Some(RenameState { id, input });
        cx.notify();
    }

    /// Meta-only rename: upserts the same id/root with a new `name` via
    /// `Store::register_workspace` (never touches the filesystem or `.sid/config.toml`).
    fn commit_workspace_rename(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.workspaces.renaming else {
            return;
        };
        let new_name = state.input.read(cx).content().trim().to_string();
        if new_name.is_empty() {
            self.error = Some("workspace name must not be empty".to_string());
            cx.notify();
            return;
        }
        let RenameState { id, .. } = self.workspaces.renaming.take().expect("checked above");
        let Some(meta) = self.workspaces.list.iter().find(|m| m.id == id).cloned() else {
            cx.notify();
            return;
        };
        let updated = WorkspaceMeta {
            id: meta.id,
            root: meta.root,
            name: new_name,
        };
        match self.store.register_workspace(&updated) {
            Ok(()) => {
                // The scope chip's label is also the workspace's name — keep it in sync.
                self.reload_scopes_runtime(cx);
                self.refresh_workspaces(cx);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    fn cancel_workspace_rename(&mut self, cx: &mut Context<Self>) {
        self.workspaces.renaming = None;
        cx.notify();
    }

    /// Context menu's "Unregister": armed two-click, like every other delete in the
    /// app — the first click arms it (the menu's label switches to a confirm phrasing
    /// on the next right-click), the second actually unregisters. Never touches
    /// `.sid/config.toml` (`Store::unregister_workspace`'s own contract) — only forgets
    /// sid's pointer, then rebuilds the scope switcher (falling back to Global if this
    /// was the focused scope) and this list.
    fn unregister_workspace(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        if unregister_click_executes(self.workspaces.armed_unregister.as_ref(), &id) {
            self.workspaces.armed_unregister = None;
            match self.store.unregister_workspace(&id) {
                Ok(_removed) => {
                    if self.workspaces.selected.as_ref() == Some(&id) {
                        self.workspaces.selected = None;
                        self.workspaces.shape = None;
                    }
                    self.reload_scopes_runtime(cx);
                    self.refresh_workspaces(cx);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        } else {
            self.workspaces.armed_unregister = Some(id);
        }
        cx.notify();
    }

    fn focus_workspace_scope(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        self.set_scope(Scope::Workspace(id));
        cx.notify();
    }

    /// Overview's scope-items jump affordance: focus the item's workspace as the active
    /// scope, then switch to the tab that shows it.
    fn jump_to_scope_tab(
        &mut self,
        id: WorkspaceId,
        tab: Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_scope(Scope::Workspace(id));
        self.switch_to_tab(tab, window, cx);
    }

    // ---- rendering: list panel --------------------------------------------------------

    fn workspaces_list_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (border, muted, accent, selection, fg_strong) =
            (t.border, t.muted, t.accent, t.selection, t.fg_strong);
        let count = self.workspaces.list.len();

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(border))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .child(format!("WORKSPACES · {count}")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("ws-refresh")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(accent))
                            .hover(|s| s.bg(rgb(selection)))
                            .child("⟳")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.refresh_workspaces(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("ws-add")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .bg(rgb(selection))
                            .text_color(rgb(fg_strong))
                            .child("+ add")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                this.open_add_workspace(window, cx);
                            })),
                    ),
            );

        let add_row = self.workspaces.add_open.then(|| self.add_workspace_row(cx));

        let rows: Vec<AnyElement> = self
            .workspaces
            .list
            .iter()
            .cloned()
            .enumerate()
            .map(|(ix, meta)| self.workspace_row(ix, meta, cx))
            .collect();

        let empty_hint =
            (self.workspaces.list.is_empty() && !self.workspaces.add_open).then(|| {
                div().p_3().text_xs().text_color(rgb(muted)).child(
                "no workspaces registered — + add a repo (or a directory of repos) to get started",
            )
            });

        div()
            .w(px(300.))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(border))
            .child(header)
            .children(add_row)
            .child(
                div()
                    .id("ws-list")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .py_1()
                    // Right-click anywhere in the list defaults to "no row" — see
                    // `ssh_home`'s identical `capture_any_mouse_down` for why the
                    // CAPTURE-phase reset must run before any row's own bubble-phase
                    // `on_mouse_down` sets a specific target.
                    .capture_any_mouse_down(cx.listener(
                        |this, ev: &MouseDownEvent, _window, cx| {
                            if ev.button == MouseButton::Right {
                                this.workspaces.right_click_target = None;
                                cx.notify();
                            }
                        },
                    ))
                    .children(rows)
                    .children(empty_hint)
                    .child(div().flex_1().min_h(px(24.)))
                    .context_menu(self.workspaces_context_menu(cx)),
            )
    }

    fn add_workspace_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (border, danger, muted, accent, selection) =
            (t.border, t.danger, t.muted, t.accent, t.selection);
        let input = self.workspaces.add_input.clone();
        let error = self.workspaces.add_error.clone();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(border))
            .child(
                div()
                    .id("ws-add-input-wrap")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                        match ev.keystroke.key.as_str() {
                            "enter" => {
                                cx.stop_propagation();
                                this.submit_add_workspace(cx);
                            }
                            "escape" => {
                                cx.stop_propagation();
                                this.cancel_add_workspace(cx);
                            }
                            _ => {}
                        }
                    }))
                    .children(input.map(|i| div().flex_1().child(i)))
                    .child(
                        div()
                            .id("ws-add-submit")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .text_color(rgb(accent))
                            .hover(|s| s.bg(rgb(selection)))
                            .child("add")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.submit_add_workspace(cx);
                            })),
                    ),
            )
            .children(error.map(|e| div().text_xs().text_color(rgb(danger)).child(e)))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .child("~/ ok · a directory of repos becomes a fleet"),
            )
    }

    fn workspace_row(&self, ix: usize, meta: WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (border, muted, fg, fg_strong, accent, selection, warning, danger, bg) = (
            t.border,
            t.muted,
            t.fg,
            t.fg_strong,
            t.accent,
            t.selection,
            t.warning,
            t.danger,
            t.bg,
        );

        let is_focused_scope = matches!(&self.scope, Scope::Workspace(id) if id == &meta.id);
        let is_selected = self.workspaces.selected.as_ref() == Some(&meta.id);
        let is_renaming = self
            .workspaces
            .renaming
            .as_ref()
            .is_some_and(|r| r.id == meta.id);
        let (hosts_n, conns_n) = self
            .workspaces
            .scope_counts
            .get(&meta.id)
            .copied()
            .unwrap_or((0, 0));

        let (chip_label, chip_color): (SharedString, u32) =
            match self.workspaces.summaries.get(&meta.id) {
                None | Some(Fetch::Loading) => ("…".into(), muted),
                Some(Fetch::Done(Err(GitPanelError::NotARepo))) => ("not a git repo".into(), muted),
                Some(Fetch::Done(Err(GitPanelError::Other(e)))) => (e.clone().into(), danger),
                Some(Fetch::Done(Ok(s))) => {
                    let branch = s.branch.clone().unwrap_or_else(|| "(detached)".into());
                    (branch.into(), if s.is_clean() { muted } else { warning })
                }
            };

        let name_area: AnyElement = if is_renaming {
            let input = self.workspaces.renaming.as_ref().unwrap().input.clone();
            div()
                .id(("ws-rename", ix))
                .flex_1()
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                    match ev.keystroke.key.as_str() {
                        "enter" => {
                            cx.stop_propagation();
                            this.commit_workspace_rename(cx);
                        }
                        "escape" => {
                            cx.stop_propagation();
                            this.cancel_workspace_rename(cx);
                        }
                        _ => {}
                    }
                }))
                .child(input)
                .into_any_element()
        } else {
            let name_id = meta.id.clone();
            let current_name = meta.name.clone();
            div()
                .id(("ws-name", ix))
                .flex_1()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(if is_selected { fg_strong } else { fg }))
                .child(meta.name.clone())
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.start_workspace_rename(
                            name_id.clone(),
                            current_name.clone(),
                            window,
                            cx,
                        );
                    }
                }))
                .into_any_element()
        };

        let row_id = meta.id.clone();
        let row_id_for_menu = meta.id.clone();

        div()
            .id(("ws-row", ix))
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(if is_selected { selection } else { bg }))
            .hover(|s| s.bg(rgb(selection)))
            .border_b_1()
            .border_color(rgb(border))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _ev: &MouseDownEvent, _window, cx| {
                    this.workspaces.right_click_target = Some(row_id_for_menu.clone());
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .when(is_focused_scope, |el| {
                        el.child(div().size(px(6.)).rounded_full().bg(rgb(accent)))
                    })
                    .child(name_area),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .font_family(MONO)
                    .truncate()
                    .child(meta.root.display().to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(muted))
                            .child(format!("{hosts_n} hosts · {conns_n} connections")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(chip_color))
                            .child(chip_label),
                    ),
            )
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                this.select_workspace(row_id.clone(), cx);
            }))
            .into_any_element()
    }

    /// The list's single context menu — see `right_click_target`'s doc comment.
    fn workspaces_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + use<> {
        let this = cx.entity();
        move |menu, _window, cx| {
            let target = this.read(cx).workspaces.right_click_target.clone();
            let Some(id) = target else { return menu };
            let armed = this.read(cx).workspaces.armed_unregister.as_ref() == Some(&id);
            let unregister_label = if armed {
                "Unregister — click again to confirm"
            } else {
                "Unregister"
            };

            menu.item(PopupMenuItem::new("Focus scope").on_click({
                let this = this.clone();
                let id = id.clone();
                move |_ev, _window, cx| {
                    let id = id.clone();
                    this.update(cx, |state, cx| state.focus_workspace_scope(id, cx));
                }
            }))
            .item(PopupMenuItem::new("Rename").on_click({
                let this = this.clone();
                let id = id.clone();
                move |_ev, window, cx| {
                    let id = id.clone();
                    this.update(cx, |state, cx| {
                        if let Some(meta) =
                            state.workspaces.list.iter().find(|m| m.id == id).cloned()
                        {
                            state.start_workspace_rename(id, meta.name, window, cx);
                        }
                    });
                }
            }))
            .separator()
            .item(PopupMenuItem::new(unregister_label).on_click({
                let this = this.clone();
                let id = id.clone();
                move |_ev, _window, cx| {
                    let id = id.clone();
                    this.update(cx, |state, cx| state.unregister_workspace(id, cx));
                }
            }))
        }
    }

    // ---- rendering: detail panel -------------------------------------------------------

    fn workspaces_detail_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let muted = t.muted;
        let Some(id) = self.workspaces.selected.clone() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(muted))
                .child("select a workspace")
                .into_any_element();
        };
        let Some(meta) = self.workspaces.list.iter().find(|m| m.id == id).cloned() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(muted))
                .child("workspace no longer registered")
                .into_any_element();
        };
        match self.workspaces.shape.clone() {
            Some(WorkspaceShape::Repo) => self.repo_detail(&meta, cx),
            Some(WorkspaceShape::Umbrella(_)) => self.umbrella_detail(&meta, cx),
            Some(WorkspaceShape::Plain) | None => self.plain_detail(&meta, cx),
        }
    }

    fn scope_items_section(
        &self,
        id: &WorkspaceId,
        hosts: &[Host],
        connections: &[DbConnection],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (muted, fg, accent, selection) = (t.muted, t.fg, t.accent, t.selection);

        let host_rows: Vec<AnyElement> = hosts
            .iter()
            .enumerate()
            .map(|(ix, h)| {
                let id = id.clone();
                div()
                    .id(("ws-scope-host", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(selection)))
                    .child(div().text_sm().text_color(rgb(fg)).child(h.alias.clone()))
                    .child(div().text_xs().text_color(rgb(accent)).child("→ SSH"))
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                        this.jump_to_scope_tab(id.clone(), Tab::Ssh, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let conn_rows: Vec<AnyElement> = connections
            .iter()
            .enumerate()
            .map(|(ix, c)| {
                let id = id.clone();
                let label = if c.name.is_empty() {
                    c.id.clone()
                } else {
                    c.name.clone()
                };
                div()
                    .id(("ws-scope-conn", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(selection)))
                    .child(div().text_sm().text_color(rgb(fg)).child(label))
                    .child(div().text_xs().text_color(rgb(accent)).child("→ Database"))
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                        this.jump_to_scope_tab(id.clone(), Tab::Database, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let empty = (host_rows.is_empty() && conn_rows.is_empty()).then(|| {
            div()
                .text_xs()
                .text_color(rgb(muted))
                .child("no hosts or connections in this workspace's own layer")
        });

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(rgb(muted)).child("SCOPE ITEMS"))
            .children(host_rows)
            .children(conn_rows)
            .children(empty)
    }

    fn plain_detail(&mut self, meta: &WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (muted, fg_strong) = (t.muted, t.fg_strong);
        let id = meta.id.clone();
        let hosts = self.workspaces.overview_hosts.clone();
        let connections = self.workspaces.overview_connections.clone();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(fg_strong))
                    .child(meta.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .font_family(MONO)
                    .child(meta.root.display().to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .child("not a git repo"),
            )
            .child(self.scope_items_section(&id, &hosts, &connections, cx))
            .into_any_element()
    }

    fn repo_detail(&mut self, meta: &WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (border, fg_strong) = (t.border, t.fg_strong);
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(border))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(fg_strong))
                    .child(meta.name.clone()),
            )
            .child(self.repo_sub_tab_chips(cx));

        let body = match self.workspaces.sub_tab {
            DetailSubTab::Overview => self.repo_overview(meta, cx),
            DetailSubTab::Branches => self.repo_branches(cx),
            DetailSubTab::Status => self.repo_status(cx),
            DetailSubTab::Log => self.repo_log(cx),
        };

        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .id("ws-repo-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p_4()
                    .child(body),
            )
            .into_any_element()
    }

    fn repo_sub_tab_chips(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (selection, border, fg_strong, muted) = (t.selection, t.border, t.fg_strong, t.muted);
        let active = self.workspaces.sub_tab;
        div()
            .flex()
            .flex_row()
            .gap_1()
            .children(DetailSubTab::ALL.iter().enumerate().map(|(ix, &tab)| {
                let is_active = tab == active;
                div()
                    .id(("ws-subtab", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if is_active { selection } else { border }))
                    .text_color(rgb(if is_active { fg_strong } else { muted }))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.workspaces.sub_tab = tab;
                        cx.notify();
                    }))
            }))
    }

    fn repo_overview(&mut self, meta: &WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (fg, muted, warning, danger) = (t.fg, t.muted, t.warning, t.danger);

        let summary_view: AnyElement = match self.workspaces.summaries.get(&meta.id) {
            None | Some(Fetch::Loading) => text_cell(muted, "loading git status…"),
            Some(Fetch::Done(Err(e))) => text_cell(danger, e.to_string()),
            Some(Fetch::Done(Ok(s))) => {
                let branch = s.branch.clone().unwrap_or_else(|| "(detached HEAD)".into());
                let ahead_behind = match (s.ahead, s.behind) {
                    (None, None) => "no upstream".to_string(),
                    (a, b) => format!("↑{} ↓{}", a.unwrap_or(0), b.unwrap_or(0)),
                };
                let last_commit = s
                    .last_commit
                    .as_ref()
                    .map(|c| {
                        format!(
                            "{} · {}",
                            c.summary,
                            commit_age(now_secs(), c.timestamp_secs)
                        )
                    })
                    .unwrap_or_else(|| "no commits yet".to_string());
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(text_cell(fg, format!("{branch} · {ahead_behind}")))
                    .child(text_cell(
                        if s.is_clean() { muted } else { warning },
                        format!(
                            "{} staged · {} unstaged · {} untracked",
                            s.staged, s.unstaged, s.untracked
                        ),
                    ))
                    .child(text_cell(muted, last_commit))
                    .into_any_element()
            }
        };

        let hosts = self.workspaces.overview_hosts.clone();
        let connections = self.workspaces.overview_connections.clone();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(summary_view)
            .child(self.scope_items_section(&meta.id, &hosts, &connections, cx))
            .into_any_element()
    }

    fn repo_branches(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (fg, muted, danger, accent, selection) =
            (t.fg, t.muted, t.danger, t.accent, t.selection);

        let error_line = self
            .workspaces
            .checkout_error
            .clone()
            .map(|e| text_cell(danger, format!("checkout failed: {e}")));

        let body: AnyElement = match self.workspaces.branches.clone() {
            None | Some(Fetch::Loading) => text_cell(muted, "loading branches…"),
            Some(Fetch::Done(Err(e))) => text_cell(danger, e.to_string()),
            Some(Fetch::Done(Ok(branches))) => {
                let pending = self.workspaces.checkout_pending.clone();
                let rows: Vec<AnyElement> = branches
                    .into_iter()
                    .enumerate()
                    .map(|(ix, b)| {
                        let name = b.name.clone();
                        let is_current = b.is_current;
                        let is_pending = pending.as_deref() == Some(name.as_str());
                        let label = if is_current {
                            format!("● {name}")
                        } else {
                            format!("  {name}")
                        };
                        div()
                            .id(("ws-branch", ix))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .when(!is_current, |el| {
                                el.cursor_pointer().hover(|s| s.bg(rgb(selection)))
                            })
                            .child(text_cell(if is_current { accent } else { fg }, label))
                            .children(b.upstream.clone().map(|u| text_cell(muted, u)))
                            .children(is_pending.then(|| text_cell(muted, "checking out…")))
                            .when(!is_current, |el| {
                                el.on_click(cx.listener(
                                    move |this, _ev: &ClickEvent, _window, cx| {
                                        this.checkout_branch(name.clone(), cx);
                                    },
                                ))
                            })
                            .into_any_element()
                    })
                    .collect();
                div().flex().flex_col().children(rows).into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(error_line)
            .child(body)
            .into_any_element()
    }

    fn repo_status(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (fg, muted, danger, success, warning) = (t.fg, t.muted, t.danger, t.success, t.warning);

        match self.workspaces.status.clone() {
            None | Some(Fetch::Loading) => text_cell(muted, "loading status…"),
            Some(Fetch::Done(Err(e))) => text_cell(danger, e.to_string()),
            Some(Fetch::Done(Ok(status))) => {
                if status.is_clean {
                    return text_cell(success, "clean — no changes");
                }
                let staged: Vec<&StatusEntry> =
                    status.entries.iter().filter(|e| e.staged).collect();
                let untracked: Vec<&StatusEntry> = status
                    .entries
                    .iter()
                    .filter(|e| !e.staged && e.kind == StatusKind::Untracked)
                    .collect();
                let unstaged: Vec<&StatusEntry> = status
                    .entries
                    .iter()
                    .filter(|e| !e.staged && e.kind != StatusKind::Untracked)
                    .collect();

                let group = |label: &'static str, color: u32, entries: Vec<&StatusEntry>| {
                    let rows: Vec<AnyElement> = entries
                        .iter()
                        .enumerate()
                        .map(|(ix, e)| {
                            div()
                                .id((label, ix))
                                .text_xs()
                                .font_family(MONO)
                                .text_color(rgb(fg))
                                .child(e.path.clone())
                                .into_any_element()
                        })
                        .collect();
                    (!rows.is_empty()).then(|| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(text_cell(color, format!("{} · {}", label, rows.len())))
                            .children(rows)
                    })
                };

                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(group("STAGED", success, staged))
                    .children(group("UNSTAGED", warning, unstaged))
                    .children(group("UNTRACKED", muted, untracked))
                    .into_any_element()
            }
        }
    }

    fn repo_log(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (fg, muted, danger) = (t.fg, t.muted, t.danger);

        match self.workspaces.log.clone() {
            None | Some(Fetch::Loading) => text_cell(muted, "loading log…"),
            Some(Fetch::Done(Err(e))) => text_cell(danger, e.to_string()),
            Some(Fetch::Done(Ok(commits))) => {
                if commits.is_empty() {
                    return text_cell(muted, "no commits yet");
                }
                let now = now_secs();
                let rows: Vec<AnyElement> = commits
                    .iter()
                    .enumerate()
                    .map(|(ix, c)| {
                        div()
                            .id(("ws-log", ix))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .py_1()
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(rgb(fg))
                                    .child(c.summary.clone()),
                            )
                            .child(text_cell(muted, c.author_name.clone()))
                            .child(text_cell(muted, commit_age(now, c.timestamp_secs)))
                            .into_any_element()
                    })
                    .collect();
                div().flex().flex_col().children(rows).into_any_element()
            }
        }
    }

    fn umbrella_detail(&mut self, meta: &WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (border, fg_strong, muted) = (t.border, t.fg_strong, t.muted);
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(border))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(fg_strong))
                    .child(format!("{} — fleet", meta.name)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .child("sorted, live git status per repo"),
            );

        let fleet_table = self.workspaces.fleet.clone().map(|table| {
            div()
                .flex_1()
                .w_full()
                .child(Table::new(&table).stripe(true))
        });

        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .p_2()
                    .flex()
                    .children(fleet_table),
            )
            .into_any_element()
    }
}

// ---- tests --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn workspace_shape_repo_when_root_itself_is_a_git_repo() {
        let root = p("/w/repo");
        let is_repo = |path: &Path| path == Path::new("/w/repo");
        let children = |_: &Path| vec![];
        assert_eq!(
            workspace_shape(&root, &is_repo, &children),
            WorkspaceShape::Repo
        );
    }

    #[test]
    fn workspace_shape_umbrella_when_one_level_deep_children_are_repos() {
        let root = p("/w/vcs");
        let a = p("/w/vcs/a");
        let b = p("/w/vcs/b");
        let a2 = a.clone();
        let b2 = b.clone();
        let is_repo = move |path: &Path| path == a2 || path == b2;
        let a3 = a.clone();
        let b3 = b.clone();
        let children = move |_: &Path| vec![a3.clone(), b3.clone()];
        assert_eq!(
            workspace_shape(&root, &is_repo, &children),
            WorkspaceShape::Umbrella(vec![a, b])
        );
    }

    #[test]
    fn workspace_shape_plain_when_no_repo_anywhere() {
        let root = p("/w/notes");
        let is_repo = |_: &Path| false;
        let children = |_: &Path| vec![p("/w/notes/sub")];
        assert_eq!(
            workspace_shape(&root, &is_repo, &children),
            WorkspaceShape::Plain
        );
    }

    #[test]
    fn workspace_shape_umbrella_filters_out_non_repo_children() {
        let root = p("/w/vcs");
        let repo = p("/w/vcs/repo");
        let not_repo = p("/w/vcs/notes");
        let repo2 = repo.clone();
        let is_repo = move |path: &Path| path == repo2;
        let repo3 = repo.clone();
        let not_repo3 = not_repo.clone();
        let children = move |_: &Path| vec![repo3.clone(), not_repo3.clone()];
        assert_eq!(
            workspace_shape(&root, &is_repo, &children),
            WorkspaceShape::Umbrella(vec![repo])
        );
    }

    #[test]
    fn commit_age_buckets() {
        assert_eq!(commit_age(1000, 1000), "just now");
        assert_eq!(commit_age(1000, 950), "just now");
        assert_eq!(commit_age(1000, 940), "1m ago");
        assert_eq!(commit_age(4600, 1000), "1h ago");
        assert_eq!(commit_age(1000 + 90_000, 1000), "1d ago");
        assert_eq!(commit_age(1000 + 700_000, 1000), "1w ago");
        assert_eq!(commit_age(1000 + 40 * 86_400, 1000), "1mo ago");
        assert_eq!(commit_age(1000 + 400 * 86_400, 1000), "1y ago");
    }

    #[test]
    fn commit_age_never_negative_on_clock_skew() {
        assert_eq!(commit_age(1000, 2000), "just now");
    }

    fn summary(branch: &str, dirty: usize, ahead: Option<usize>, age: i64) -> RepoSummary {
        RepoSummary {
            branch: Some(branch.to_string()),
            detached: false,
            staged: 0,
            unstaged: dirty,
            untracked: 0,
            ahead,
            behind: Some(0),
            last_commit: Some(CommitInfo {
                oid: "a".repeat(40),
                summary: "x".into(),
                author_name: "m".into(),
                author_email: "m@x".into(),
                timestamp_secs: age,
            }),
        }
    }

    fn row(name: &str, s: Option<RepoSummary>) -> FleetRow {
        FleetRow {
            name: name.into(),
            path: PathBuf::from(format!("/vcs/{name}")),
            fetch: match s {
                Some(s) => Fetch::Done(Ok(s)),
                None => Fetch::Done(Err(GitPanelError::Other("boom".into()))),
            },
        }
    }

    #[test]
    fn cmp_fleet_repo_is_case_insensitive_alphabetical() {
        let a = row("Alpha", None);
        let b = row("beta", None);
        assert_eq!(cmp_fleet_repo(&a, &b), Ordering::Less);
    }

    #[test]
    fn cmp_fleet_dirty_sorts_numerically_and_errors_last() {
        let clean = row("a", Some(summary("main", 0, None, 0)));
        let dirty = row("b", Some(summary("main", 5, None, 0)));
        let errored = row("c", None);
        assert_eq!(cmp_fleet_dirty(&clean, &dirty), Ordering::Less);
        assert_eq!(cmp_fleet_dirty(&dirty, &errored), Ordering::Less);
    }

    #[test]
    fn cmp_fleet_age_sorts_oldest_first_ascending() {
        let older = row("a", Some(summary("main", 0, None, 100)));
        let newer = row("b", Some(summary("main", 0, None, 200)));
        assert_eq!(cmp_fleet_age(&older, &newer), Ordering::Less);
    }

    #[test]
    fn cmp_fleet_ahead_behind_sorts_by_ahead_then_behind() {
        let a = row("a", Some(summary("main", 0, Some(1), 0)));
        let b = row("b", Some(summary("main", 0, Some(2), 0)));
        assert_eq!(cmp_fleet_ahead_behind(&a, &b), Ordering::Less);
    }

    #[test]
    fn sort_dir_reverses_ascending_comparator() {
        let a = row("a", Some(summary("main", 0, None, 0)));
        let b = row("b", Some(summary("main", 5, None, 0)));
        assert_eq!(
            SortDir::Desc.apply(cmp_fleet_dirty(&a, &b)),
            Ordering::Greater
        );
    }

    #[test]
    fn expand_tilde_home_relative() {
        assert_eq!(expand_tilde("~/vcs", "/home/m"), "/home/m/vcs");
    }

    #[test]
    fn expand_tilde_bare_tilde_is_home() {
        assert_eq!(expand_tilde("~", "/home/m"), "/home/m");
    }

    #[test]
    fn expand_tilde_leaves_absolute_paths_alone() {
        assert_eq!(expand_tilde("/etc/x", "/home/m"), "/etc/x");
    }

    #[test]
    fn validate_workspace_path_rejects_empty() {
        assert!(validate_workspace_path("", &|_| true).is_err());
    }

    #[test]
    fn validate_workspace_path_rejects_non_directory() {
        assert!(validate_workspace_path("/nope", &|_| false).is_err());
    }

    #[test]
    fn validate_workspace_path_accepts_a_real_directory() {
        assert!(validate_workspace_path("/vcs/sid", &|_| true).is_ok());
    }

    #[test]
    fn unregister_click_executes_only_on_the_same_armed_id() {
        let a = WorkspaceId("a".into());
        let b = WorkspaceId("b".into());
        assert!(!unregister_click_executes(None, &a));
        assert!(unregister_click_executes(Some(&a), &a));
        assert!(!unregister_click_executes(Some(&a), &b));
    }

    #[test]
    fn git_panel_error_from_not_a_repo_is_the_muted_variant() {
        let e: GitPanelError = GitError::NotARepo("/x".into()).into();
        assert_eq!(e, GitPanelError::NotARepo);
    }

    #[test]
    fn git_panel_error_from_other_carries_the_message() {
        let e: GitPanelError = GitError::Other("boom".into()).into();
        assert_eq!(
            e,
            GitPanelError::Other("git operation failed: boom".to_string())
        );
    }
}
