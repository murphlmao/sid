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
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*, px, rgb,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};
use sid_core::git::{
    Branch, CommitInfo, GitError, GitStatus, RepoSummary, StatusEntry, StatusKind,
};
use sid_store::{DbConnection, Host, Scope, ViewFilters, WorkspaceId, WorkspaceMeta};

use crate::app::{AppState, Tab};
use crate::git_registry;
use crate::ui::TextInput;
use crate::ui::session::ssh_runtime;
use sid_ui::theme;
use sid_ui::{
    Badge, BadgeTone, Button, ButtonSize, Card, ColumnWidth, Confirm, ConfirmArm, ConfirmButton,
    EmptyState, FillColumns, FillTable, FillTableDelegate, Icon, IconButton, List, Row, Segment,
    SegmentSelect, SegmentedControl, StyledExt as _, Toolbar, h_flex, sortable_th,
};

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

/// A workspace's identity, reduced to something [`ConfirmArm`] can hold.
///
/// `ConfirmArm<K>` needs `K: Copy` — it stores the armed key next to an `Instant` and
/// compares it on every press — and `WorkspaceId` is a `String` newtype. Hashing it
/// yields a `Copy` stand-in that is still derived from **identity** rather than from a
/// row's position, which is the property that matters: a list that re-sorts, re-filters
/// or reloads between the two clicks can never redirect an unregister onto a different
/// workspace. (A 64-bit collision would mis-target one destructive click that the user
/// still had to make deliberately on the colliding row; the store lookup that follows is
/// keyed on the real `WorkspaceId`, never on this.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct WsKey(u64);

fn ws_key(id: &WorkspaceId) -> WsKey {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    WsKey(hasher.finish())
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

/// The dirty-count chip — `clean`, or an amber count of what is outstanding.
///
/// Shared by the fleet table and the workspace list so one repo cannot read as clean in
/// one place and busy in the other. Tones only: `accent` means *engage*, and the state of
/// a working tree is something to read, not something to click.
fn dirty_badge(dirty: usize) -> AnyElement {
    if dirty == 0 {
        Badge::new("clean")
            .tone(BadgeTone::Success)
            .into_any_element()
    } else {
        Badge::count(dirty)
            .tone(BadgeTone::Warning)
            .into_any_element()
    }
}

/// Ahead/behind as two chips, amber on whichever side is non-zero — so a scan down the
/// fleet's column picks out the repos that owe a push or a pull without reading a number.
/// No upstream at all is metadata rather than a state: muted text, and now that the
/// column is sized to its header rather than to 80px, it can say so in words.
fn ahead_behind_cell(ahead: Option<usize>, behind: Option<usize>, muted: u32) -> AnyElement {
    if ahead.is_none() && behind.is_none() {
        return text_cell(muted, "no upstream");
    }
    let (a, b) = (ahead.unwrap_or(0), behind.unwrap_or(0));
    let chip = |label: String, n: usize| {
        Badge::new(label).tone(if n > 0 {
            BadgeTone::Warning
        } else {
            BadgeTone::Neutral
        })
    };
    h_flex()
        .gap_1()
        .child(chip(format!("↑{a}"), a))
        .child(chip(format!("↓{b}"), b))
        .into_any_element()
}

/// Read-only fleet delegate — no armed/interactive state, per `network_tab::
/// DockerDelegate`'s template for a read-only table.
struct FleetDelegate {
    rows: Vec<FleetRow>,
    /// The columns and the width each one declared, resized to the live viewport by
    /// [`FillTable`] — see `sid_ui::table`'s module docs.
    columns: FillColumns,
    active_sort: Option<(usize, SortDir)>,
}

impl FleetDelegate {
    fn empty() -> Self {
        Self {
            rows: Vec::new(),
            // Widths are declared as intent, not pixels. The three numeric columns have
            // a known upper bound and stay exactly as wide as their header; `Repo` and
            // `Branch` hold a floor so a short name cannot collapse them; `Path` — the
            // one column whose content is genuinely unbounded, and the one that was
            // truncating `/home/murphy/vcs/…` inside 280px while the rest of a 2000px
            // window sat empty — absorbs everything left over.
            columns: FillColumns::new([
                (
                    Column::new("repo", "Repo").sortable(),
                    ColumnWidth::Min(150.),
                ),
                // Wide enough for a real `feature/…` branch name, not just `main`.
                (
                    Column::new("branch", "Branch").sortable(),
                    ColumnWidth::Min(190.),
                ),
                (
                    Column::new("dirty", "Dirty").sortable(),
                    ColumnWidth::Fixed(90.),
                ),
                // Fixed widths are the header's width, not the cell's: upstream lays the
                // label and the sort chevron in one `justify_between` row, so a column
                // sized to its content clips its own heading.
                (
                    Column::new("ahead_behind", "Ahead / behind").sortable(),
                    ColumnWidth::Fixed(150.),
                ),
                (
                    Column::new("age", "Last commit").sortable(),
                    ColumnWidth::Fixed(120.),
                ),
                (
                    Column::new("path", "Path").sortable(),
                    ColumnWidth::grow().min_width(240.),
                ),
            ]),
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

impl FillTableDelegate for FleetDelegate {
    fn fill_columns(&mut self) -> &mut FillColumns {
        &mut self.columns
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
        self.columns.column(col_ix)
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        // Mirror the sort onto our own columns so the header indicator survives the next
        // `TableState::refresh` — which a viewport change now triggers on every resize.
        // See `FillColumns::apply_sort`.
        self.columns.apply_sort(col_ix, sort);
        self.active_sort = SortDir::from_column_sort(sort).map(|dir| (col_ix, dir));
        self.recompute();
        cx.notify();
    }

    /// Sort on a click anywhere in the header cell, not only on the chevron — upstream's
    /// default `render_th` hands the label to the column-selection handler and leaves a
    /// ~6x8px sort target. See `sid_ui::table::sortable_th`.
    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        sortable_th(col_ix, self.columns.column(col_ix), cx)
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
                    // The chip a fleet of forty repos is scanned by: green means nothing
                    // to do, amber means work is pending. Never accent — accent is
                    // "engage", and a status chip is not an invitation to click.
                    Fetch::Done(Ok(s)) => dirty_badge(s.staged + s.unstaged + s.untracked),
                    Fetch::Done(Err(e)) => text_cell(muted, e.to_string()),
                },
                3 => match &row.fetch {
                    Fetch::Loading => text_cell(muted, "…"),
                    Fetch::Done(Ok(s)) => ahead_behind_cell(s.ahead, s.behind, muted),
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
    /// The two-step unregister, keyed by workspace identity ([`WsKey`]) rather than by
    /// row position, and *surviving* a list refresh rather than being cleared by one —
    /// see `sid_ui::action_cell`, whose module docs record the confirm that never fired
    /// because every refresh disarmed it.
    unregister_arm: ConfirmArm<WsKey>,
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
            unregister_arm: ConfirmArm::new(),
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

        // A pending unregister confirm survives this reload unless its workspace is gone.
        // Clearing it unconditionally is exactly the bug `ConfirmArm` was built against:
        // `+ add`, rename and unregister all land here, so an arm that a refresh could
        // wipe would be a race the user loses. See `ConfirmArm::retain`.
        let live: HashSet<WsKey> = self.workspaces.list.iter().map(|m| ws_key(&m.id)).collect();
        self.workspaces
            .unregister_arm
            .retain(|key| live.contains(&key));

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
        self.workspaces.unregister_arm.disarm();
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
        self.workspaces.unregister_arm.disarm();
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

    /// Armed two-click unregister, from either the row's own button or the context
    /// menu's item — the first press arms this workspace (the button turns into a
    /// `confirm`, the menu item's label switches phrasing), the second unregisters.
    /// Never touches `.sid/config.toml` (`Store::unregister_workspace`'s own contract) —
    /// only forgets sid's pointer, then rebuilds the scope switcher (falling back to
    /// Global if this was the focused scope) and this list.
    ///
    /// The arm lives in [`ConfirmArm`], so it is bound to the workspace's identity and
    /// expires on its own; the store call below is still keyed on the real `WorkspaceId`.
    fn unregister_workspace(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        if self
            .workspaces
            .unregister_arm
            .press(ws_key(&id), Instant::now())
            == Confirm::Armed
        {
            cx.notify();
            return;
        }
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
        let t = theme::active(cx).clone();
        let border = t.border;
        let count = self.workspaces.list.len();

        // The Toolbar's wide left slot carries the panel's own label; the count and the
        // two controls take the right edge. `⟳` and the `+ add` pill were the last two
        // hand-styled `div`s on this screen.
        let header = Toolbar::new()
            .filter(div().section_label(&t).child("WORKSPACES"))
            .count(count, "workspace")
            .action(
                IconButton::new("ws-refresh", Icon::Refresh, "refresh")
                    .small()
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                        this.refresh_workspaces(cx);
                    })),
            )
            .action(
                // Secondary, not primary: the screen's one accent belongs to the empty
                // state's `add workspace` (when there is nothing) or to the focused
                // scope's chip (when there is), never to a permanent toolbar pill.
                Button::new("ws-add", "add")
                    .small()
                    .icon(Icon::Add)
                    .tooltip("register a repo, or a directory of repos")
                    .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                        this.open_add_workspace(window, cx);
                    })),
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

        // Nothing registered: a headline, one line of what-to-do, and the control that
        // does it — rather than a muted sentence with a `+ add` buried in its prose.
        let empty = (self.workspaces.list.is_empty() && !self.workspaces.add_open).then(|| {
            EmptyState::new("no workspaces yet")
                .guidance("register a git repo — or a directory of repos — to see its branches, status and scope items here")
                .icon(Icon::Folder)
                .action(
                    Button::new("ws-empty-add", "add workspace")
                        .primary()
                        .icon(Icon::Add)
                        .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                            this.open_add_workspace(window, cx);
                        })),
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
                List::scrolling("ws-list")
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
                    // The empty state owns the pane's height; the tail spacer is only
                    // there to give a short list somewhere to right-click.
                    .when_some(empty, |this, empty| this.child(empty))
                    .when(count > 0, |this| this.child(div().flex_1().min_h(px(24.))))
                    .context_menu(self.workspaces_context_menu(cx)),
            )
    }

    fn add_workspace_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (border, danger, muted) = (t.border, t.danger, t.muted);
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
                    .children(input.map(|i| div().flex_1().min_w(px(0.)).child(i)))
                    .child(
                        Button::new("ws-add-submit", "add")
                            .primary()
                            .small()
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
        let (muted, fg, fg_strong, warning, danger) =
            (t.muted, t.fg, t.fg_strong, t.warning, t.danger);

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

        // The leading chip is a **bounded status word** — `clean`, a dirty count,
        // `no git` — so a long branch name can never push the row's actions off the edge
        // of a 300px sidebar; the branch itself, and any real error message, go on the
        // row's metadata line where they have the whole row to truncate into.
        let (chip, git_label, git_color): (AnyElement, SharedString, u32) =
            match self.workspaces.summaries.get(&meta.id) {
                None | Some(Fetch::Loading) => (
                    Badge::new("…").into_any_element(),
                    "loading git status…".into(),
                    muted,
                ),
                Some(Fetch::Done(Err(GitPanelError::NotARepo))) => (
                    Badge::new("no git").into_any_element(),
                    "not a git repo".into(),
                    muted,
                ),
                Some(Fetch::Done(Err(GitPanelError::Other(e)))) => (
                    Badge::new("error")
                        .tone(BadgeTone::Danger)
                        .into_any_element(),
                    e.clone().into(),
                    danger,
                ),
                Some(Fetch::Done(Ok(s))) => {
                    let branch = s.branch.clone().unwrap_or_else(|| "(detached)".into());
                    (
                        dirty_badge(s.staged + s.unstaged + s.untracked),
                        branch.into(),
                        if s.is_clean() { muted } else { warning },
                    )
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
                .min_w(px(0.))
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .clamp_one_line()
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

        let armed = self
            .workspaces
            .unregister_arm
            .is_armed(ws_key(&meta.id), Instant::now());
        let rename_btn = {
            let id = meta.id.clone();
            let name = meta.name.clone();
            IconButton::new(("ws-row-rename", ix), Icon::Rename, "rename")
                .small()
                .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                    this.start_workspace_rename(id.clone(), name.clone(), window, cx);
                }))
        };
        // Icon at rest, word when armed. The trash glyph plus its tooltip is the whole
        // label in a 300px sidebar; armed, `ConfirmButton` drops the icon and says
        // `confirm` in filled danger — the click that does something is the one that gets
        // the word. Arming is `ConfirmArm`'s, keyed on the workspace, not on `ix`.
        let unregister_btn = {
            let id = meta.id.clone();
            ConfirmButton::new(("ws-row-unregister", ix), "")
                .icon(Icon::Trash)
                .armed(armed)
                .armed_label("confirm")
                .size(ButtonSize::Sm)
                .tooltip(if armed {
                    "click again to unregister — sid forgets this workspace, no files are touched"
                } else {
                    "unregister"
                })
                .on_press(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                    this.unregister_workspace(id.clone(), cx);
                }))
        };

        let row_id = meta.id.clone();
        let row_id_for_menu = meta.id.clone();

        Row::new(("ws-row", ix))
            .selected(is_selected)
            .leading(chip)
            .child(
                h_flex()
                    .gap_2()
                    .child(name_area)
                    // The focused scope was an unlabelled 6px accent dot. It is the one
                    // genuinely "engage"-coloured fact on this screen, and now it says so.
                    .when(is_focused_scope, |el| {
                        el.child(Badge::new("focused").tone(BadgeTone::Accent))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .font_family(MONO)
                    .clamp_one_line()
                    .child(meta.root.display().to_string()),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_xs()
                            .clamp_one_line()
                            .text_color(rgb(git_color))
                            .child(git_label),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(rgb(muted))
                            .child(format!("{hosts_n}h · {conns_n}c")),
                    ),
            )
            .action(rename_btn)
            .action(unregister_btn)
            .on_secondary_mouse_down(cx.listener(move |this, _ev: &MouseDownEvent, _window, cx| {
                this.workspaces.right_click_target = Some(row_id_for_menu.clone());
                cx.notify();
            }))
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
            let armed = this
                .read(cx)
                .workspaces
                .unregister_arm
                .is_armed(ws_key(&id), Instant::now());
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
        let Some(id) = self.workspaces.selected.clone() else {
            // 1700px of black with the words `select a workspace` in the middle of it was
            // the single worst thing about this tab at a wide window.
            //
            // With nothing registered at all, the list panel is already showing its own
            // empty state and its own `add workspace` button; repeating the button here
            // would put two accent fills on one screen for the same verb, so this pane
            // states the situation and points at the one control that exists.
            let nothing_registered = self.workspaces.list.is_empty();
            return div()
                .flex_1()
                .child(
                    EmptyState::new(if nothing_registered {
                        "no workspaces yet"
                    } else {
                        "select a workspace"
                    })
                    .guidance(if nothing_registered {
                        "register one on the left to see its branches, status, log and \
                         scope items here"
                    } else {
                        "pick one on the left to see its branches, status, log and \
                         scope items — or register another"
                    })
                    .icon(Icon::Folder)
                    .when(!nothing_registered, |empty| {
                        empty.action(
                            Button::new("ws-detail-add", "add workspace")
                                .primary()
                                .icon(Icon::Add)
                                .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                    this.open_add_workspace(window, cx);
                                })),
                        )
                    }),
                )
                .into_any_element();
        };
        let Some(meta) = self.workspaces.list.iter().find(|m| m.id == id).cloned() else {
            return div()
                .flex_1()
                .child(
                    EmptyState::new("workspace no longer registered")
                        .guidance("it was unregistered while it was open")
                        .icon(Icon::Warning),
                )
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
        let (muted, fg) = (t.muted, t.fg);
        let count = hosts.len() + connections.len();

        let host_rows: Vec<AnyElement> = hosts
            .iter()
            .enumerate()
            .map(|(ix, h)| {
                let id = id.clone();
                Row::new(("ws-scope-host", ix))
                    .child(div().text_sm().text_color(rgb(fg)).child(h.alias.clone()))
                    // Where the row goes, as orientation rather than as accent-coloured
                    // prose (`→ SSH`) pretending to be a link.
                    .meta(Badge::new("SSH"))
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
                Row::new(("ws-scope-conn", ix))
                    .child(div().text_sm().text_color(rgb(fg)).child(label))
                    .meta(Badge::new("Database"))
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                        this.jump_to_scope_tab(id.clone(), Tab::Database, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let empty = (count == 0).then(|| {
            div()
                .text_xs()
                .text_color(rgb(muted))
                .child("no hosts or connections in this workspace's own layer")
        });

        Card::section("Scope items").count(count).child(
            List::stack()
                .children(host_rows)
                .children(conn_rows)
                .children(empty),
        )
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
            .child(h_flex().child(Badge::new("not a git repo")))
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

    /// The Overview/Branches/Status/Log switch.
    ///
    /// Four hand-rolled chips whose *unselected* state was filled with `border` and whose
    /// selected state was filled with `selection` — on cosmos the inactive chips were the
    /// brighter of the two, so the strip read as three raised chips and one dent. The
    /// shared control fixes that structurally (a recessed track, one raised chip).
    fn repo_sub_tab_chips(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let active = self.workspaces.sub_tab;
        SegmentedControl::new("ws-subtab")
            .segments(DetailSubTab::ALL.map(|tab| Segment::new(tab.label())))
            .selected(
                DetailSubTab::ALL
                    .iter()
                    .position(|tab| *tab == active)
                    .unwrap_or(0),
            )
            .on_select(cx.listener(|this, ev: &SegmentSelect, _window, cx| {
                if let Some(&tab) = DetailSubTab::ALL.get(ev.index) {
                    this.workspaces.sub_tab = tab;
                    cx.notify();
                }
            }))
    }

    fn repo_overview(&mut self, meta: &WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let (muted, warning, danger) = (t.muted, t.warning, t.danger);

        let summary_view: AnyElement = match self.workspaces.summaries.get(&meta.id) {
            None | Some(Fetch::Loading) => text_cell(muted, "loading git status…"),
            Some(Fetch::Done(Err(e))) => text_cell(danger, e.to_string()),
            Some(Fetch::Done(Ok(s))) => {
                let branch = s.branch.clone().unwrap_or_else(|| "(detached HEAD)".into());
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
                    .gap_2()
                    // The same chips the fleet table and the list row use, so one repo
                    // reads identically wherever it is shown.
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(Badge::new(branch))
                            .child(dirty_badge(s.staged + s.unstaged + s.untracked))
                            .child(ahead_behind_cell(s.ahead, s.behind, muted)),
                    )
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
        let (fg, muted, danger, fg_strong) = (t.fg, t.muted, t.danger, t.fg_strong);

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
                        // Click-to-checkout is the row itself, exactly as before — and so
                        // is the `DirtyWorkingTree` refusal it surfaces above. What
                        // changes is that the row now *looks* clickable (hover fill,
                        // pointer) and the current branch is a labelled chip instead of a
                        // `●` prefix nudging the name two spaces right.
                        Row::new(("ws-branch", ix))
                            .selected(is_current)
                            .child(text_cell(
                                if is_current { fg_strong } else { fg },
                                name.clone(),
                            ))
                            .when(is_current, |row| row.meta(Badge::new("current").solid()))
                            .when_some(b.upstream.clone(), |row, u| {
                                row.meta(div().text_xs().text_color(rgb(muted)).child(u))
                            })
                            .when(is_pending, |row| {
                                row.meta(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(muted))
                                        .child("checking out…"),
                                )
                            })
                            .when(!is_current, |row| {
                                row.on_click(cx.listener(
                                    move |this, _ev: &ClickEvent, _window, cx| {
                                        this.checkout_branch(name.clone(), cx);
                                    },
                                ))
                            })
                            .into_any_element()
                    })
                    .collect();
                List::stack().children(rows).into_any_element()
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
                        // Inert rows: `Row` paints no hover on something nothing can be
                        // done to, which is the whole difference between a log and a list
                        // of links.
                        Row::new(("ws-log", ix))
                            .child(
                                div()
                                    .text_sm()
                                    .clamp_one_line()
                                    .text_color(rgb(fg))
                                    .child(c.summary.clone()),
                            )
                            .meta(
                                div()
                                    .text_xs()
                                    .text_color(rgb(muted))
                                    .child(c.author_name.clone()),
                            )
                            .meta(
                                div()
                                    .text_xs()
                                    .text_color(rgb(muted))
                                    .child(commit_age(now, c.timestamp_secs)),
                            )
                            .into_any_element()
                    })
                    .collect();
                List::stack().children(rows).into_any_element()
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

        // `FillTable`, not `Table`: the columns are resized to the width this pane
        // actually got, so `Path` stops truncating `/home/murphy/vcs/…` inside 280px
        // while the rest of a 2000px window sits empty. See `sid_ui::table`.
        let fleet_table = self.workspaces.fleet.clone().map(|table| {
            div()
                .flex_1()
                .w_full()
                .child(FillTable::new(&table).stripe(true))
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

    // ---- the unregister confirm, keyed by workspace identity --------------------------

    #[test]
    fn ws_key_is_a_function_of_identity_alone() {
        // Two `WorkspaceId`s that are equal must key the same arm, and two that are not
        // must not — that is the entire contract the `Copy` stand-in has to keep.
        let a = WorkspaceId("/w/a".into());
        let b = WorkspaceId("/w/b".into());
        assert_eq!(ws_key(&a), ws_key(&WorkspaceId("/w/a".into())));
        assert_ne!(ws_key(&a), ws_key(&b));
    }

    #[test]
    fn the_unregister_confirm_needs_two_presses_on_the_same_workspace() {
        let a = WorkspaceId("/w/a".into());
        let now = Instant::now();
        let mut arm: ConfirmArm<WsKey> = ConfirmArm::new();
        assert_eq!(arm.press(ws_key(&a), now), Confirm::Armed);
        assert_eq!(arm.press(ws_key(&a), now), Confirm::Fire);
    }

    #[test]
    fn arming_one_workspace_never_fires_on_another() {
        // The property a positional key cannot give: the list reorders (a rename, a
        // refresh, a new registration) between the two clicks and the confirm still can
        // only ever land on the workspace it was armed with.
        let a = WorkspaceId("/w/a".into());
        let b = WorkspaceId("/w/b".into());
        let now = Instant::now();
        let mut arm: ConfirmArm<WsKey> = ConfirmArm::new();
        assert_eq!(arm.press(ws_key(&a), now), Confirm::Armed);
        assert_eq!(
            arm.press(ws_key(&b), now),
            Confirm::Armed,
            "a press on a different row moves the arm, it does not fire"
        );
        assert!(!arm.is_armed(ws_key(&a), now));
    }

    #[test]
    fn an_armed_unregister_survives_a_list_reload_that_still_has_the_row() {
        // `refresh_workspaces` runs on add/rename/unregister, so an arm it cleared
        // unconditionally would be a race the user loses — the bug `ConfirmArm` exists
        // for. Only the row disappearing may drop it.
        let a = WorkspaceId("/w/a".into());
        let b = WorkspaceId("/w/b".into());
        let now = Instant::now();
        let mut arm: ConfirmArm<WsKey> = ConfirmArm::new();
        arm.press(ws_key(&a), now);

        let still_there: HashSet<WsKey> = [ws_key(&a), ws_key(&b)].into_iter().collect();
        arm.retain(|key| still_there.contains(&key));
        assert!(arm.is_armed(ws_key(&a), now));

        let gone: HashSet<WsKey> = [ws_key(&b)].into_iter().collect();
        arm.retain(|key| gone.contains(&key));
        assert!(!arm.is_armed(ws_key(&a), now));
    }

    // ---- the fleet table's column plan ------------------------------------------------

    fn fleet_widths(cols: &FillColumns) -> Vec<f32> {
        (0..cols.len())
            .map(|ix| f32::from(cols.column(ix).width))
            .collect()
    }

    #[test]
    fn the_fleet_gives_every_spare_pixel_to_the_path_column() {
        // The whole point of the migration: `Path` was 280 fixed pixels truncating
        // `/home/murphy/vcs/…` while ~1350px sat unused to its right. The numeric columns
        // keep their declared widths at every viewport; `Path` absorbs the difference.
        let mut cols = FleetDelegate::empty().columns;
        for viewport in [1000., 1400., 1700., 2600.] {
            cols.sync(viewport);
            let widths = fleet_widths(&cols);
            assert_eq!(&widths[2..5], &[90., 150., 120.], "{viewport}px: numerics");
            assert!(widths[5] >= 240., "{viewport}px: path floor");
            let total: f32 = widths.iter().sum();
            assert!(
                (total - (viewport - sid_ui::table::TABLE_CHROME)).abs() < 0.01,
                "{viewport}px: columns total {total} — dead space or an overflow"
            );
        }
    }

    #[test]
    fn the_fleet_columns_hold_their_floors_in_a_narrow_window() {
        // Degenerate but real: too narrow to honour the declaration, so every column
        // stays legible and the table scrolls, rather than squeezing `Path` to nothing.
        let mut cols = FleetDelegate::empty().columns;
        cols.sync(600.);
        assert_eq!(fleet_widths(&cols), vec![150., 190., 90., 150., 120., 240.]);
    }

    #[test]
    fn a_fleet_sort_is_mirrored_onto_the_delegates_own_columns() {
        // Without this the chevron resets to the declared state on the next
        // `TableState::refresh` — which the fill-width model now triggers on every
        // viewport change — while the rows stay sorted the way the user asked.
        let mut delegate = FleetDelegate::empty();
        delegate.columns.apply_sort(2, ColumnSort::Descending);
        assert_eq!(
            delegate.columns.column(2).sort,
            Some(ColumnSort::Descending)
        );
        for ix in [0, 1, 3, 4, 5] {
            assert_eq!(
                delegate.columns.column(ix).sort,
                Some(ColumnSort::Default),
                "column {ix} should have surrendered the chevron"
            );
        }
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
