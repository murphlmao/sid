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
    AnyElement, App, ClickEvent, Context, Div, Entity, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, SharedString, Window, div, prelude::*, px, rgb,
};
use sid_core::git::{Branch, GitError, GitStatus, RefKind, RepoSummary};
use sid_store::{DbConnection, Host, Scope, ViewFilters, WorkspaceId, WorkspaceMeta};

use crate::app::{AppState, Tab};
use crate::git_registry;
use crate::ui::session::ssh_runtime;
use sid_ui::component::{
    Column, ColumnSort, ContextMenuExt, PopupMenu, PopupMenuItem, TableDelegate, TableState,
};
use sid_ui::theme::{self, Theme};
use sid_ui::{
    Badge, BadgeTone, Button, ButtonSize, Card, CardGrid, ColumnWidth, Confirm, ConfirmArm,
    ConfirmButton, EmptyState, FillColumns, FillTable, FillTableDelegate, Icon, IconButton,
    InputState, List, Row, Segment, SegmentSelect, SegmentedControl, StyledExt as _, TextInput,
    Tipped as _, TypeRole, Typography as _, h_flex, scaled, sortable_th, v_flex,
};

// Workspaces v2 git UI (Changes / Stash / Branches / Log): pure helpers, the background
// operations, and the views. Kept beside this file so it stays reviewable.
#[path = "workspaces/model.rs"]
mod model;
#[path = "workspaces/ops.rs"]
mod ops;
#[path = "workspaces/views.rs"]
mod views;

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

/// The row's scope-items summary in words — replaces the `{n}h · {n}c` shorthand a
/// glance could mistake for a hash fragment. Pure (both counts are parameters), with
/// its own singular/plural agreement so "1 hosts" never ships.
fn scope_counts_label(hosts_n: usize, conns_n: usize) -> String {
    let host_word = if hosts_n == 1 { "host" } else { "hosts" };
    let conn_word = if conns_n == 1 {
        "connection"
    } else {
        "connections"
    };
    format!("{hosts_n} {host_word} · {conns_n} {conn_word}")
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
    Changes,
    Branches,
    Stash,
    Log,
}

impl DetailSubTab {
    const ALL: [DetailSubTab; 5] = [
        Self::Overview,
        Self::Changes,
        Self::Branches,
        Self::Stash,
        Self::Log,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Changes => "Changes",
            Self::Branches => "Branches",
            Self::Stash => "Stash",
            Self::Log => "Log",
        }
    }
}

/// The row currently mid-rename — meta-only (renames `WorkspaceMeta::name`, never
/// touches the filesystem), mirrors `db_tab::RenameState`'s shape.
struct RenameState {
    id: WorkspaceId,
    input: Entity<InputState>,
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

/// A line of body text in an ink the caller picks — a loading line, a git error, a
/// summary count. Deliberately **not** clamped: these run full-width in the detail pane,
/// and a `DirtyWorkingTree` refusal that wraps onto a second line is readable, where the
/// same refusal cut at `…` is a bug report nobody can file.
fn text_cell(theme: &Theme, ink: u32, s: impl Into<SharedString>) -> AnyElement {
    div()
        .text_body(theme)
        .text_color(rgb(ink))
        .child(s.into())
        .into_any_element()
}

/// One cell of the fleet table: `role` on the body rung, inked by the caller, one line,
/// cut with an ellipsis.
///
/// The columns differ by *role and ink*, never by size — `Mono` for the three columns
/// that are identifiers a machine chose (repo, branch, path), `Body` for the two that are
/// English (`3 days ago`, `no upstream`), all of them 14px so a row has one baseline. See
/// `sid_ui::typography`.
///
/// The clamp is not decoration: gpui reports a text element's min-content width as its
/// whole string, so without `min_w(0)` a `/home/murphy/vcs/…/some-long-repo` pushes its
/// column open instead of shrinking, and without `clamp_one_line` it is cut mid-glyph with
/// no `…` (never gpui's `truncate()` — see `StyledExt::clamp_one_line`).
///
/// `whitespace_normal()` is the third of those and the least obvious: the upstream table
/// wraps every cell in a `whitespace_nowrap` container (the library's private
/// `table::state::render_cell`), and `Nowrap` pins `TextElement`'s
/// `wrap_width` to `None` — the exact defect `clamp_one_line` documents in gpui's
/// `truncate()`, arriving by inheritance. It is why `reach_export_c` and
/// `feature/offline-fir` used to end flush against their column edge with no `…`.
fn fleet_cell(theme: &Theme, role: TypeRole, ink: u32, s: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .min_w(px(0.))
        .whitespace_normal()
        .clamp_one_line()
        .text_role(role, theme)
        .text_color(rgb(ink))
        .child(s.into())
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
fn ahead_behind_cell(theme: &Theme, ahead: Option<usize>, behind: Option<usize>) -> AnyElement {
    if ahead.is_none() && behind.is_none() {
        return text_cell(theme, theme.muted, "no upstream");
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
                    ColumnWidth::grow().min_width(160.).weight(1.),
                ),
                // Wide enough for a real `feature/…` branch name, not just `main`.
                (
                    Column::new("branch", "Branch").sortable(),
                    ColumnWidth::grow().min_width(180.).weight(1.2),
                ),
                (
                    Column::new("dirty", "Dirty").sortable(),
                    ColumnWidth::Fixed(100.),
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
                    ColumnWidth::grow().min_width(200.).weight(2.5),
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

        // Identifiers a machine chose are monospace; the two columns that read as English
        // stay proportional. Both are the same 14px rung — see `fleet_cell`.
        let ident =
            |ink: u32, s: SharedString| fleet_cell(t, TypeRole::Mono, ink, s).into_any_element();
        let words =
            |ink: u32, s: SharedString| fleet_cell(t, TypeRole::Cell, ink, s).into_any_element();

        let content: AnyElement = match self.rows.get(row_ix) {
            None => div().into_any_element(),
            Some(row) => match col_ix {
                0 => ident(fg, row.name.clone().into()),
                1 => match &row.fetch {
                    Fetch::Loading => words(muted, "…".into()),
                    Fetch::Done(Ok(s)) => ident(
                        fg,
                        s.branch
                            .clone()
                            .unwrap_or_else(|| "(detached)".into())
                            .into(),
                    ),
                    // Per-row errors render as muted text, never a panic.
                    Fetch::Done(Err(_)) => words(muted, "—".into()),
                },
                2 => match &row.fetch {
                    Fetch::Loading => words(muted, "…".into()),
                    // The chip a fleet of forty repos is scanned by: green means nothing
                    // to do, amber means work is pending. Never accent — accent is
                    // "engage", and a status chip is not an invitation to click.
                    // Content-sized (`h_flex` does not stretch), so the chip is a chip
                    // and not a bar the width of its column.
                    Fetch::Done(Ok(s)) => {
                        let n = s.staged + s.unstaged + s.untracked;
                        h_flex()
                            .child(if n == 0 {
                                dirty_badge(0)
                            } else {
                                Badge::new(format!("{n} dirty"))
                                    .tone(BadgeTone::Warning)
                                    .into_any_element()
                            })
                            .into_any_element()
                    }
                    Fetch::Done(Err(e)) => words(muted, e.to_string().into()),
                },
                3 => match &row.fetch {
                    Fetch::Loading => words(muted, "…".into()),
                    Fetch::Done(Ok(s)) => ahead_behind_cell(t, s.ahead, s.behind),
                    Fetch::Done(Err(_)) => words(muted, "—".into()),
                },
                4 => match &row.fetch {
                    Fetch::Loading => words(muted, "…".into()),
                    Fetch::Done(Ok(s)) => {
                        let label = s
                            .last_commit
                            .as_ref()
                            .map(|c| commit_age(now_secs(), c.timestamp_secs))
                            .unwrap_or_else(|| "—".into());
                        words(fg, label.into())
                    }
                    Fetch::Done(Err(_)) => words(muted, "—".into()),
                },
                5 => ident(muted, row.path.display().to_string().into()),
                _ => div().into_any_element(),
            },
        };

        // `min_w(0)` on the wrapper as well as on the cell: the clamp inside can only cut
        // to the width its parent concedes, and a flex child's automatic minimum is its
        // content — which, for text, gpui measures as the whole string.
        div()
            .id(cell_id)
            .px_2()
            .w_full()
            .min_w(px(0.))
            .child(content)
    }
}

/// How many Overview cards share a line at `viewport_w`: as many as fit at 340px, but
/// only a count that divides `cards` evenly, so the last line is as full as the first
/// (six cards are 3+3 or 2+2+2, never 4+2 with a hole). The pane is the window less the
/// workspace list (~320px) and the page gutters.
fn overview_columns(viewport_w: f32, cards: usize) -> usize {
    let pane = (viewport_w - 320. - 112.).max(0.);
    let fit = ((pane + 16.) / (340. + 16.)).floor().max(1.) as usize;
    (1..=fit.min(cards))
        .rev()
        .find(|n| cards.is_multiple_of(*n))
        .unwrap_or(1)
}

/// `cards` laid out `cols` to a line, every card on a line sharing it equally and the
/// lines stretching to the pane — `CardGrid`'s wrap-and-cap leaves a short last line
/// stranded at its cap, which is the hole this replaces.
fn even_card_rows(cards: Vec<AnyElement>, cols: usize) -> impl IntoElement {
    let cols = cols.max(1);
    let mut rows = v_flex().gap(sid_ui::SECTION_GAP.len());
    let mut it = cards.into_iter().peekable();
    while it.peek().is_some() {
        let line: Vec<AnyElement> = it.by_ref().take(cols).collect();
        rows = rows.child(
            div()
                .flex()
                .flex_row()
                .w_full()
                .gap(sid_ui::SECTION_GAP.len())
                .children(
                    line.into_iter()
                        .map(|c| div().flex().flex_1().min_w_0().child(c)),
                ),
        );
    }
    rows
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
    /// Workspaces v2 git UI state (diff, commit box, stash, branch edits, log pages).
    v2: ops::GitV2,
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
    /// The window's width at the last render — the Overview's card grid picks its column
    /// count from it (see `overview_columns`); a stale frame costs one reflow, nothing more.
    viewport_w: f32,

    // ---- `+ add` inline path input ---------------------------------------------------
    add_open: bool,
    add_input: Option<Entity<InputState>>,
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
    /// Detail sub-view names in segmented-control order, for the command palette.
    pub(crate) fn view_labels() -> Vec<&'static str> {
        DetailSubTab::ALL.iter().map(|t| t.label()).collect()
    }

    /// Switch the detail sub-view to the one at `ix` (palette navigation).
    pub(crate) fn show_view(&mut self, ix: usize) {
        if let Some(&tab) = DetailSubTab::ALL.get(ix) {
            self.sub_tab = tab;
        }
    }

    /// The add-path input is built eagerly here (needs `window`, like the fleet's
    /// `TableState`) — same as `ssh_home::HomeTabState::new`'s quick-connect box.
    pub(crate) fn new(window: &mut Window, cx: &mut Context<AppState>) -> Self {
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
            v2: ops::GitV2::new(),
            checkout_pending: None,
            checkout_error: None,
            detail_generation: 0,
            fleet: None,
            fleet_generation: 0,
            viewport_w: 1280.,
            add_open: false,
            add_input: Some(
                cx.new(|cx| InputState::new(window, cx).placeholder("~/path/to/workspace")),
            ),
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
        self.workspaces.viewport_w = f32::from(window.viewport_size().width);
        self.ensure_workspaces_widgets(window, cx);
        self.ensure_git_v2_widgets(window, cx);
        if !self.workspaces.loaded {
            self.workspaces.loaded = true;
            self.refresh_workspaces(cx);
        }
        if self.workspaces.selected.is_some()
            && matches!(self.workspaces.shape, Some(WorkspaceShape::Repo))
        {
            match self.workspaces.sub_tab {
                DetailSubTab::Overview => {
                    self.ensure_stash_loaded(cx);
                    self.ensure_log_loaded(cx);
                    self.ensure_remotes_loaded(cx);
                }
                DetailSubTab::Changes => self.ensure_status_loaded(cx),
                DetailSubTab::Branches => self.ensure_branches_loaded(cx),
                DetailSubTab::Stash => self.ensure_stash_loaded(cx),
                DetailSubTab::Log => self.ensure_log_loaded(cx),
            }
        }

        // Deliberately not `h_flex`: that centres its children on the cross axis, and
        // both panels here must *stretch* to the row's height. Same padding and gap
        // rhythm as the other two data tabs, so a panel edge lands in the same place
        // whichever tab is showing.
        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .gap(sid_ui::SECTION_GAP.len())
            .p(sid_ui::PAGE_GUTTER.len())
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
        self.workspaces.v2.reset_for_selection();
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
                        let _ = (id_for_apply, root);
                        this.after_mutation(cx);
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
            input.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    fn cancel_add_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspaces.add_open = false;
        self.workspaces.add_error = None;
        if let Some(input) = self.workspaces.add_input.clone() {
            input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        cx.notify();
    }

    /// Enter (or the "add" affordance): tilde-expand, validate it's a directory,
    /// register, then rebuild the scope switcher at RUNTIME — this closes the
    /// `reload_scopes` startup-only caveat the BUILD ADDENDUM calls out.
    fn submit_add_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self
            .workspaces
            .add_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
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
                    input.update(cx, |i, cx| i.set_value("", window, cx));
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
            let mut state = InputState::new(window, cx).placeholder("name");
            state.set_value(current_name, window, cx);
            state
        });
        // Select the whole name so typing replaces it instead of appending to it.
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.workspaces.renaming = Some(RenameState { id, input });
        cx.notify();
    }

    /// Meta-only rename: upserts the same id/root with a new `name` via
    /// `Store::register_workspace` (never touches the filesystem or `.sid/config.toml`).
    fn commit_workspace_rename(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.workspaces.renaming else {
            return;
        };
        let new_name = state.input.read(cx).value().trim().to_string();
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
        let count = self.workspaces.list.len();
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

        // The panel's header is the toolbar contract — `[WORKSPACES · n]` on the left,
        // then refresh and `+ add` on the right. `⟳` and the `+ add` pill were the last
        // two hand-styled `div`s on this screen; the hairline frame around them is the
        // same one every other region on every other tab now draws.
        Card::panel("workspaces")
            .count(count)
            .w(scaled(300.))
            .flex_none()
            .h_full()
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
                // scope's chip (when there is), never to a permanent header pill.
                Button::new("ws-add", "add")
                    .small()
                    .icon(Icon::Add)
                    .tooltip("register a repo, or a directory of repos")
                    .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                        this.open_add_workspace(window, cx);
                    })),
            )
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
                    .when(count > 0, |this| {
                        this.child(div().flex_1().min_h(scaled(24.)))
                    })
                    .context_menu(self.workspaces_context_menu(cx)),
            )
    }

    fn add_workspace_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (border, danger) = (t.border, t.danger);
        let input = self.workspaces.add_input.clone();
        let error = self.workspaces.add_error.clone();

        div()
            .flex()
            .flex_col()
            // The panel body hands its height to the scrolling list below this row, so
            // the row states that it is not the flexible one.
            .flex_none()
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
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "enter" => {
                                cx.stop_propagation();
                                this.submit_add_workspace(window, cx);
                            }
                            "escape" => {
                                cx.stop_propagation();
                                this.cancel_add_workspace(window, cx);
                            }
                            _ => {}
                        }
                    }))
                    .children(input.map(|i| div().flex_1().min_w(px(0.)).child(TextInput::new(&i))))
                    .child(
                        Button::new("ws-add-submit", "add")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                this.submit_add_workspace(window, cx);
                            })),
                    ),
            )
            .children(error.map(|e| div().text_meta(t).text_color(rgb(danger)).child(e)))
            .child(
                div()
                    .text_meta(t)
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
                // The `no git` chip already says this — the metadata line used to repeat
                // "not a git repo" underneath it, one fact rendered twice. Empty here,
                // not a second copy.
                Some(Fetch::Done(Err(GitPanelError::NotARepo))) => {
                    (Badge::new("no git").into_any_element(), "".into(), muted)
                }
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
                .child(TextInput::new(&input))
                .into_any_element()
        } else {
            let name_id = meta.id.clone();
            let current_name = meta.name.clone();
            // Body, not the MEDIUM this used to be: the scale spends its one emphasis
            // weight on headings, and every row in a list is not a heading. Selection is
            // already carried by the row's fill and by `fg_strong`.
            div()
                .id(("ws-name", ix))
                .flex_1()
                .min_w(px(0.))
                .clamp_one_line()
                .text_body(t)
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
        // label in a 300px sidebar; armed, the control says `confirm` in filled danger —
        // the click that does something is the one that gets the word. Arming is
        // `ConfirmArm`'s, keyed on the workspace, not on `ix`, and it all lives in
        // `unregister_workspace`: both spellings below call it on every press, so the
        // two-step behaviour is identical either way.
        //
        // Two spellings and not a `ConfirmButton` with an empty label, because that
        // renders a *labelled* button with nothing in the label slot — a 36px box beside
        // rename's 24px square, which is exactly the icon-only inconsistency this pass
        // exists to remove. Same element id across the transition, so gpui keeps the
        // control's identity when it arms.
        let unregister_btn: AnyElement = if armed {
            let id = meta.id.clone();
            ConfirmButton::new(("ws-row-unregister", ix), "unregister")
                .armed(true)
                .armed_label("confirm")
                .size(ButtonSize::Sm)
                .tooltip(
                    "click again to unregister — sid forgets this workspace, no files are touched",
                )
                .on_press(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                    this.unregister_workspace(id.clone(), cx);
                }))
                .into_any_element()
        } else {
            let id = meta.id.clone();
            IconButton::new(("ws-row-unregister", ix), Icon::Trash, "unregister")
                .small()
                .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                    this.unregister_workspace(id.clone(), cx);
                }))
                .into_any_element()
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
            // The row's tail: a dim path and a dim status line, both a rung below the
            // name — `MonoMeta` for the path because it is one, `Meta` for the rest.
            .child(
                div()
                    .min_w(px(0.))
                    .clamp_one_line()
                    .text_mono_meta(t)
                    .child(meta.root.display().to_string()),
            )
            // Left-aligned, not `justify_between`: that pushed the counts to the far
            // edge even when `git_label` is empty (a non-git row), because the empty
            // label still claimed `flex_1` and left the counts nowhere else to sit but
            // the trailing edge. The label div — and its grow — only renders when
            // there is a label to show; a bare row just carries the counts at the
            // start, under the name/path above it.
            .child(
                h_flex()
                    .gap_2()
                    .when(!git_label.is_empty(), |el| {
                        el.child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .clamp_one_line()
                                .text_meta(t)
                                .text_color(rgb(git_color))
                                .child(git_label),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_meta(t)
                            .child(scope_counts_label(hosts_n, conns_n)),
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
                "unregister — click again to confirm"
            } else {
                "unregister"
            };

            menu.item(
                PopupMenuItem::new("focus scope")
                    .icon(Icon::Folder.el())
                    .on_click({
                        let this = this.clone();
                        let id = id.clone();
                        move |_ev, _window, cx| {
                            let id = id.clone();
                            this.update(cx, |state, cx| state.focus_workspace_scope(id, cx));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("rename")
                    .icon(Icon::Rename.el())
                    .on_click({
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
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(unregister_label)
                    .icon(Icon::Trash.el())
                    .on_click({
                        let this = this.clone();
                        let id = id.clone();
                        move |_ev, _window, cx| {
                            let id = id.clone();
                            this.update(cx, |state, cx| state.unregister_workspace(id, cx));
                        }
                    }),
            )
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
            // Framed like Database's RESULTS empty state — a `Card::panel` around it,
            // not bare `div`s — so the right two-thirds reads as a panel with nothing
            // in it yet, not as the app failing to render one.
            return Card::panel("workspace")
                .flex_1()
                .min_h_0()
                .child(
                    div().flex_1().min_h(px(0.)).w_full().child(
                        EmptyState::new(if nothing_registered {
                            "no workspaces yet"
                        } else {
                            "select a workspace"
                        })
                        .guidance(if nothing_registered {
                            "Register one on the left to see its branches, status, log and \
                             scope items here"
                        } else {
                            "Pick one on the left to see its branches, status, log and \
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
                    ),
                )
                .into_any_element();
        };
        let Some(meta) = self.workspaces.list.iter().find(|m| m.id == id).cloned() else {
            return Card::panel("detail")
                .flex_1()
                .min_h_0()
                .child(
                    div().flex_1().min_h(px(0.)).w_full().child(
                        EmptyState::new("workspace no longer registered")
                            .guidance("it was unregistered while it was open")
                            .icon(Icon::Warning),
                    ),
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
        let fg = t.fg;
        let count = hosts.len() + connections.len();

        let host_rows: Vec<AnyElement> = hosts
            .iter()
            .enumerate()
            .map(|(ix, h)| {
                let id = id.clone();
                Row::new(("ws-scope-host", ix))
                    .child(
                        div()
                            .min_w(px(0.))
                            .clamp_one_line()
                            .text_body(t)
                            .text_color(rgb(fg))
                            .child(h.alias.clone()),
                    )
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
                    .child(
                        div()
                            .min_w(px(0.))
                            .clamp_one_line()
                            .text_body(t)
                            .text_color(rgb(fg))
                            .child(label),
                    )
                    .meta(Badge::new("Database"))
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                        this.jump_to_scope_tab(id.clone(), Tab::Database, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let body: AnyElement = if count == 0 {
            div()
                .text_meta(t)
                .child("This workspace has no hosts or connections of its own.")
                .into_any_element()
        } else {
            // Hosts and connections side by side at wide widths (a card each), one
            // column when narrow, instead of one row stretched across the window.
            CardGrid::new()
                .min_col(px(420.))
                .max_col(px(900.))
                .when(!host_rows.is_empty(), |g| {
                    g.child(
                        Card::new()
                            .w_full()
                            .title("Hosts")
                            .count(host_rows.len())
                            .child(List::stack().children(host_rows)),
                    )
                })
                .when(!conn_rows.is_empty(), |g| {
                    g.child(
                        Card::new()
                            .w_full()
                            .title("Connections")
                            .count(conn_rows.len())
                            .child(List::stack().children(conn_rows)),
                    )
                })
                .into_any_element()
        };
        Card::section("Scope items").count(count).child(body)
    }

    /// The detail pane's own top row: the workspace's name, and whatever switches or
    /// labels the shape carries. The same strip on all three shapes, so the panel below
    /// always starts at the same y.
    fn detail_header(
        &self,
        title: impl Into<SharedString>,
        trailing: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme::active(cx);
        h_flex()
            .flex_none()
            // One band height for all three shapes, and the same band the sibling
            // sidebar's panel header occupies — a heading whose height is set by whatever
            // its trailing control happens to be (a `Badge` here, a `SegmentedControl`
            // there) puts the detail pane's first panel at a different y per workspace.
            .h(scaled(40.))
            .px(sid_ui::CARD_PADDING.len())
            .justify_between()
            .gap_3()
            // A pane heading is the one place the scale goes above body text — 16px
            // Medium, where this used to be 14px BOLD (bold at 14 on a dark panel blooms
            // into a colour change rather than reading as a rank).
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .clamp_one_line()
                    .text_display(t)
                    .child(title.into()),
            )
            .child(div().flex_none().child(trailing))
    }

    /// The scrolling body of one detail panel — the wrapper between `Card::panel`'s
    /// unpadded body and content that is a stack of sections rather than a list.
    fn detail_body(id: &'static str, body: AnyElement) -> impl IntoElement + use<> {
        div()
            .id(id)
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .p_3()
            .child(body)
    }

    fn plain_detail(&mut self, meta: &WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let id = meta.id.clone();
        let hosts = self.workspaces.overview_hosts.clone();
        let connections = self.workspaces.overview_connections.clone();
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .min_w(px(0.))
                    .clamp_one_line()
                    .text_mono_meta(t)
                    .child(meta.root.display().to_string()),
            )
            .child(self.scope_items_section(&id, &hosts, &connections, cx))
            .into_any_element();

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .gap_2()
            .child(self.detail_header(meta.name.clone(), Badge::new("not a git repo"), cx))
            .child(
                Card::panel("overview")
                    .flex_1()
                    .min_h_0()
                    .child(Self::detail_body("ws-plain-body", body)),
            )
            .into_any_element()
    }

    fn repo_detail(&mut self, meta: &WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        let sub_tab = self.workspaces.sub_tab;
        let body = match sub_tab {
            DetailSubTab::Overview => self.repo_overview(meta, cx),
            DetailSubTab::Changes => self.repo_changes(cx),
            DetailSubTab::Branches => self.repo_branches(cx),
            DetailSubTab::Stash => self.repo_stash(cx),
            DetailSubTab::Log => self.repo_log(cx),
        };
        // The sub-view's own count, where it has one — the `· n` half of the panel
        // header, exactly as the Network tab's sub-views spell it.
        let count = self.detail_count(sub_tab);

        // Changes lays out its own two scrolling columns; the rest scroll as one body.
        let panel_body: AnyElement = if sub_tab == DetailSubTab::Changes {
            body
        } else {
            Self::detail_body("ws-repo-body", body).into_any_element()
        };
        let banner = self.v2_banner(cx);
        let trailing = h_flex()
            .gap_3()
            .child(self.remote_controls(cx))
            .child(self.repo_sub_tab_chips(cx));

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .gap_2()
            .child(self.detail_header(meta.name.clone(), trailing, cx))
            .children(banner)
            .child(
                Card::panel(sub_tab.label())
                    .when_some(count, Card::count)
                    .flex_1()
                    .min_h_0()
                    .child(panel_body),
            )
            .into_any_element()
    }

    /// How many rows the given sub-view is showing, when it is showing a countable list
    /// and has finished loading it. `None` — not `Some(0)` — while a fetch is in flight
    /// or has failed: `BRANCHES · 0` over a "loading branches…" line is a lie.
    fn detail_count(&self, sub_tab: DetailSubTab) -> Option<usize> {
        match sub_tab {
            DetailSubTab::Overview => None,
            DetailSubTab::Branches => match self.workspaces.branches.as_ref()? {
                Fetch::Done(Ok(branches)) => Some(branches.len()),
                _ => None,
            },
            // The Changes view carries its own per-group count pills; a panel count
            // on top of them read as the same number twice.
            DetailSubTab::Changes => None,
            DetailSubTab::Stash => match self.workspaces.v2.stash.as_ref()? {
                Fetch::Done(Ok(entries)) => Some(entries.len()),
                _ => None,
            },
            DetailSubTab::Log => match self.workspaces.v2.log.as_ref()? {
                Fetch::Done(Ok(log)) => Some(log.entries.len()),
                _ => None,
            },
        }
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
        let theme_owned = theme::active(cx).clone();
        let t = &theme_owned;
        let (muted, warning, danger) = (t.muted, t.warning, t.danger);

        let hosts = self.workspaces.overview_hosts.clone();
        let connections = self.workspaces.overview_connections.clone();
        let scope = self.scope_items_section(&meta.id, &hosts, &connections, cx);

        let s = match self.workspaces.summaries.get(&meta.id) {
            None | Some(Fetch::Loading) => {
                return div()
                    .flex()
                    .flex_col()
                    .gap(sid_ui::SECTION_GAP.len())
                    .child(text_cell(t, muted, "Loading git status…"))
                    .child(scope)
                    .into_any_element();
            }
            Some(Fetch::Done(Err(e))) => {
                return div()
                    .flex()
                    .flex_col()
                    .gap(sid_ui::SECTION_GAP.len())
                    .child(text_cell(t, danger, e.to_string()))
                    .child(scope)
                    .into_any_element();
            }
            Some(Fetch::Done(Ok(s))) => s.clone(),
        };

        let body_line = |ink: u32, text: String| {
            div()
                .min_w(px(0.))
                .clamp_one_line()
                .text_body(t)
                .text_color(rgb(ink))
                .child(text)
        };
        let meta_line = |text: String| {
            div()
                .min_w(px(0.))
                .clamp_one_line()
                .text_meta(t)
                .child(text)
        };

        // Branch + ahead/behind.
        let branch = s.branch.clone().unwrap_or_else(|| "(detached HEAD)".into());
        let tracking = match (s.ahead, s.behind) {
            (None, None) => "No upstream branch is set.".to_string(),
            (a, b) => format!(
                "{} ahead, {} behind its upstream.",
                a.unwrap_or(0),
                b.unwrap_or(0)
            ),
        };
        let branch_card = Card::new().title("Branch").child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(Badge::new(branch))
                        .when(s.ahead.is_some() || s.behind.is_some(), |h| {
                            h.child(ahead_behind_cell(t, s.ahead, s.behind))
                        }),
                )
                .child(meta_line(tracking)),
        );

        // Last commit.
        let now = now_secs();
        let commit_card = Card::new()
            .title("Last commit")
            .child(match &s.last_commit {
                Some(c) => v_flex()
                    .gap_2()
                    .child(body_line(t.fg, c.summary.clone()))
                    .child(meta_line(format!(
                        "{} · {}",
                        c.author_name,
                        commit_age(now, c.timestamp_secs)
                    )))
                    .child(
                        div()
                            .text_mono_meta(t)
                            .text_color(rgb(muted))
                            .child(c.oid.chars().take(8).collect::<String>()),
                    )
                    .into_any_element(),
                None => meta_line("No commits yet.".into()).into_any_element(),
            });

        // Working tree, with the shortcuts.
        let busy = self.workspaces.v2.busy.is_some();
        let tree_card = Card::new().title("Working tree").child(
            v_flex()
                .gap_2()
                .child(body_line(
                    if s.is_clean() { muted } else { warning },
                    if s.is_clean() {
                        "Clean — nothing to commit.".to_string()
                    } else {
                        format!(
                            "{} staged · {} unstaged · {} untracked",
                            s.staged, s.unstaged, s.untracked
                        )
                    },
                ))
                .when(!s.is_clean(), |d| {
                    d.child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new(
                                    "ws-ov-review",
                                    if s.staged > 0 {
                                        "Review and commit"
                                    } else {
                                        "Review changes"
                                    },
                                )
                                .small()
                                .primary()
                                .on_click(cx.listener(
                                    |this, _ev: &ClickEvent, _w, cx| {
                                        this.workspaces.sub_tab = DetailSubTab::Changes;
                                        cx.notify();
                                    },
                                )),
                            )
                            .when(s.unstaged + s.untracked > 0, |h| {
                                h.child(
                                    Button::new("ws-ov-stage", "Stage all")
                                        .small()
                                        .ghost()
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                            this.stage_all(cx)
                                        })),
                                )
                            }),
                    )
                }),
        );

        // Remotes.
        let remotes_card = Card::new()
            .title("Remotes")
            .child(match &self.workspaces.v2.remotes {
                Some(Fetch::Done(Ok(names))) if !names.is_empty() => v_flex()
                    .gap_1()
                    .children(names.iter().map(|n| {
                        div()
                            .min_w(px(0.))
                            .clamp_one_line()
                            .text_mono(t)
                            .text_color(rgb(t.fg))
                            .child(n.clone())
                    }))
                    .into_any_element(),
                Some(Fetch::Done(Ok(_))) => {
                    meta_line("No remotes configured.".into()).into_any_element()
                }
                Some(Fetch::Done(Err(e))) => meta_line(e.to_string()).into_any_element(),
                _ => meta_line("Loading…".into()).into_any_element(),
            });

        // Recent tags, from the first page of history.
        let tags_card = Card::new()
            .title("Recent tags")
            .child(match &self.workspaces.v2.log {
                Some(Fetch::Done(Ok(log))) => {
                    let mut tags: Vec<String> = Vec::new();
                    for e in &log.entries {
                        for r in &e.refs {
                            if r.kind == RefKind::Tag && !tags.contains(&r.name) {
                                tags.push(r.name.clone());
                            }
                        }
                    }
                    tags.drain(6.min(tags.len())..);
                    if tags.is_empty() {
                        meta_line("No tags in recent history.".into()).into_any_element()
                    } else {
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(tags.into_iter().enumerate().map(|(i, n)| {
                                // Content-sized up to a cap; past it the badge
                                // ellipsises and the tooltip carries the full name.
                                div()
                                    .id(("ws-ov-tag", i))
                                    .flex_none()
                                    .max_w(scaled(220.))
                                    .tip(n.clone())
                                    .child(Badge::new(n).tone(BadgeTone::Warning))
                            }))
                            .into_any_element()
                    }
                }
                _ => meta_line("Loading…".into()).into_any_element(),
            });

        // Stash.
        let stash_n = match &self.workspaces.v2.stash {
            Some(Fetch::Done(Ok(v))) => Some(v.len()),
            _ => None,
        };
        let stash_card = Card::new().title("Stash").child(
            v_flex()
                .gap_2()
                .child(meta_line(match stash_n {
                    Some(0) => "Nothing stashed.".to_string(),
                    Some(1) => "1 stashed change set.".to_string(),
                    Some(n) => format!("{n} stashed change sets."),
                    None => "Loading…".to_string(),
                }))
                .when(stash_n.is_some_and(|n| n > 0), |d| {
                    d.child(
                        h_flex().child(
                            Button::new("ws-ov-stash", "Open stash")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                    this.workspaces.sub_tab = DetailSubTab::Stash;
                                    cx.notify();
                                })),
                        ),
                    )
                }),
        );

        div()
            .flex()
            .flex_col()
            .gap(sid_ui::SECTION_GAP.len())
            .child(even_card_rows(
                vec![
                    branch_card.w_full().h_full().into_any_element(),
                    commit_card.w_full().h_full().into_any_element(),
                    tree_card.w_full().h_full().into_any_element(),
                    remotes_card.w_full().h_full().into_any_element(),
                    tags_card.w_full().h_full().into_any_element(),
                    stash_card.w_full().h_full().into_any_element(),
                ],
                overview_columns(self.workspaces.viewport_w, 6),
            ))
            .child(scope)
            .into_any_element()
    }

    fn umbrella_detail(&mut self, meta: &WorkspaceMeta, cx: &mut Context<Self>) -> AnyElement {
        // `FillTable`, not `Table`: the columns are resized to the width this pane
        // actually got, so `Path` stops truncating `/home/murphy/vcs/…` inside 280px
        // while the rest of a 2000px window sits empty. See `sid_ui::table`.
        let fleet = self.workspaces.fleet.clone();
        let count = fleet.as_ref().map(|t| t.read(cx).delegate().rows.len());
        let fleet_table = fleet.map(|table| {
            div()
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .child(FillTable::new(&table).stripe(true))
        });

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .gap_2()
            .child(self.detail_header(meta.name.clone(), Badge::new("fleet"), cx))
            .child(
                Card::panel("repos")
                    .when_some(count, Card::count)
                    .flex_1()
                    .min_h_0()
                    .children(fleet_table),
            )
            .into_any_element()
    }
}

// ---- tests --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sid_core::git::CommitInfo;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn scope_counts_label_agrees_in_number() {
        assert_eq!(scope_counts_label(3, 0), "3 hosts · 0 connections");
        assert_eq!(scope_counts_label(1, 1), "1 host · 1 connection");
        assert_eq!(scope_counts_label(0, 2), "0 hosts · 2 connections");
    }

    /// Read back a cell's refined style — enough to assert what a helper decided, without
    /// a renderer. Same trick `sid_ui::styled`'s own tests use.
    fn style_of(mut d: gpui::Div) -> gpui::StyleRefinement {
        d.style().clone()
    }

    #[test]
    fn overview_columns_always_divide_the_cards_evenly() {
        // Six cards: 3+3 on a wide window, 2+2+2 on a mid one, a stack when narrow —
        // never 4+2, which leaves the last line short.
        assert_eq!(overview_columns(1920., 6), 3);
        assert_eq!(overview_columns(1280., 6), 2);
        assert_eq!(overview_columns(900., 6), 1);
        assert_eq!(overview_columns(0., 6), 1);
    }

    #[test]
    fn every_fleet_column_lands_on_the_same_rung() {
        // The rule the fleet table is built on: a column differs from its neighbour by
        // family and ink, never by size. `Repo`/`Branch`/`Path` are monospace because
        // they are identifiers a machine chose; `Last commit` is English. All 14px, so a
        // row has one baseline whichever columns have loaded.
        let t = theme::cosmos();
        let path = style_of(fleet_cell(
            &t,
            TypeRole::Mono,
            t.muted,
            "/home/murphy/vcs/sid",
        ))
        .text
        .clone();
        let age = style_of(fleet_cell(&t, TypeRole::Cell, t.fg, "3 days ago"))
            .text
            .clone();
        assert_eq!(path.font_size, age.font_size, "one rung");
        assert_eq!(path.font_size, Some(TypeRole::Mono.length().into()));
        assert_eq!(
            path.font_family.as_deref(),
            Some(sid_ui::UI_MONO),
            "a path is monospace"
        );
        assert_eq!(age.font_family, None, "an age is not");
        assert_eq!(path.color, Some(gpui::Hsla::from(rgb(t.muted))));
        assert_eq!(age.color, Some(gpui::Hsla::from(rgb(t.fg))));
    }

    #[test]
    fn a_fleet_cell_may_shrink_and_cuts_with_an_ellipsis() {
        // gpui reports a text element's min-content width as its whole string, so a repo
        // path without `min_w(0)` widens its column instead of clipping — and without the
        // clamp it is cut mid-glyph with no `…`. Both, or neither works.
        let t = theme::cosmos();
        let style = style_of(fleet_cell(
            &t,
            TypeRole::Mono,
            t.muted,
            "/home/murphy/vcs/some-repo-with-a-long-name",
        ));
        assert_eq!(style.min_size.width, Some(px(0.).into()), "may shrink");
        let text = style.text.clone();
        assert_eq!(text.line_clamp, Some(1));
        assert!(text.text_overflow.is_some(), "cut with a suffix");
        // Stated, not inherited — the upstream table's cell container sets
        // `whitespace_nowrap`, which is what kills the ellipsis pass.
        assert_eq!(
            text.white_space,
            Some(gpui::WhiteSpace::Normal),
            "the table's inherited nowrap has to be undone here or nothing truncates"
        );
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
        assert_eq!(
            arm.press(ws_key(&a), now + std::time::Duration::from_millis(400)),
            Confirm::Fire
        );
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
        // keep their declared widths at every viewport; the three text columns (repo,
        // branch, path) share the difference, `Path` taking the biggest share.
        let mut cols = FleetDelegate::empty().columns;
        for viewport in [1000., 1400., 1700., 2600.] {
            cols.sync(viewport, sid_ui::UiScale::DEFAULT);
            let widths = fleet_widths(&cols);
            assert_eq!(&widths[2..5], &[100., 150., 120.], "{viewport}px: numerics");
            assert!(widths[5] >= 200., "{viewport}px: path floor");
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
        cols.sync(600., sid_ui::UiScale::DEFAULT);
        assert_eq!(
            fleet_widths(&cols),
            vec![160., 180., 100., 150., 120., 200.]
        );
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
