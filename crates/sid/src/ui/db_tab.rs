//! Database tab: connection picker (W3), add/edit form (W4), SQL editor + results (W5).
//!
//! [`DbTabState`] is a sibling cache to [`AppState`]'s host list — a composed
//! [`DbConnection`] list for the active scope, refreshed on the same events (scope
//! switch, form submit). The render/mutation methods live in a *second* `impl AppState`
//! block here rather than in `app.rs`, so the SSH track (editing `app.rs`/`session.rs`
//! concurrently, per Plan 3C) only ever sees a one-field, one-match-arm diff there; this
//! module reaches back into `AppState`'s `pub(crate)` fields (`store`, `secrets`, `scope`,
//! `filters`, `scopes`, `error`) to do it. See `app.rs`'s module doc comment for the
//! host-tab equivalent this mirrors.
//!
//! W5 (SQL editor + results) reuses `session::ssh_runtime()` — the process-lifetime
//! Tokio runtime the SSH track already built. It isn't SSH-specific in mechanism (just
//! named for its original purpose): `tokio-postgres`/`rusqlite` both need an ambient
//! Tokio context the same way `russh` does, and standing up a second runtime just for
//! this tab would be pure duplication. `session::ssh_runtime` is `pub(crate)`, so no
//! visibility change to `session.rs` (off-limits this slice) was needed.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, Corner, Entity, FocusHandle,
    FontWeight, IntoElement, KeyDownEvent, SharedString, Subscription, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowOptions, anchored, deferred, div, point, prelude::*, px, rgb, rgba,
    size,
};
use gpui_component::Root;
use gpui_component::input::{Input, InputEvent, InputState, Position};
use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};
use sid_core::db::{
    Column as DbColumn, ColumnType, DbClient, DbError, DbKind, OpenParams, PageCursor, QueryPage,
    Row, SchemaGraph, SchemaInfo, TableInfo,
};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{Attributed, DbConnection, Scope, Store, ViewFilters};

use crate::app::{AppState, can_demote, can_promote, delete_click_executes};
use crate::db_registry::DbRegistry;
use crate::ui::TextInput;
use crate::ui::db_conn_form::{
    DbConnForm, DbConnFormEvent, Submission, add_guard, plan_secret, stage_secret,
};
use crate::ui::db_diagram::DiagramView;
use crate::ui::session::ssh_runtime;
use sid_ui::{
    Badge, Button, ButtonSize, ColumnWidth, ConfirmButton, ConnectionState, Elevation, EmptyState,
    FillColumns, FillTable, FillTableDelegate, Icon, IconButton, List, Row as UiRow, ScopeChip,
    StatusDot, StyledExt as _, Theme, Toolbar, Typography as _, h_flex, sortable_th, theme, v_flex,
};

/// Monospace family for the DSN subtitle; matches `app.rs`'s host rows.
const MONO: &str = "DejaVu Sans Mono";

/// Seeded into the SQL editor on first paint — works unmodified against every engine
/// (SQLite, Postgres, and the redb browse engine all accept a bare `select 1;`), so it
/// isn't tied to the demo SQLite connection's schema.
const DEMO_SQL: &str = "select 1;";

/// Rows per `query_paged` call. Small enough to make the "⭳ next page" control
/// exercisable by hand against the demo seed without a huge fixture table.
const PAGE_SIZE: u32 = 100;

/// The one size every control in the query toolbar's action cluster is built at.
///
/// The bug this const exists to make unrepeatable: `Run` was written as
/// `Button::new(..).primary()` with no size, so it took [`ButtonSize`]'s default `Md`
/// — a 32px box with a body-rung label — while `Export` and `next page`, sitting in
/// the same [`Toolbar`] action slot two lines below, were written `.small()` and came
/// out at 24px with a meta-rung label. Three buttons in one cluster, two heights,
/// because the size was retyped per call site instead of declared once.
///
/// `Sm` rather than `Md`, on the house rule the other toolbars already follow
/// (`systems_tab`'s refresh, `network_tab`'s refresh): **a toolbar action is sized by
/// its container, and emphasised by its variant.** Run stays the loudest control on
/// the tab — it is the only `Primary` (accent fill) — without also being the tallest.
const QUERY_ACTION_SIZE: ButtonSize = ButtonSize::Sm;

// ---- increment 2: schema tree / cell copy-view / CSV export / history --------------------

/// A result cell longer than this (in `char`s) gets a `view` affordance opening the
/// read-only popover, rather than relying on the grid's truncated inline text (D2).
const CELL_VIEW_THRESHOLD: usize = 48;

/// D4's in-memory query-history ring cap. No persistence (ponytail) — cleared on restart.
const HISTORY_CAP: usize = 50;

/// Database tab state: the composed connection list for the active scope, the row
/// currently selected as "active", and (once armed) a pending two-click delete.
pub struct DbTabState {
    /// The client/descriptor factory, shared with every [`DbConnForm`] this tab opens
    /// (W4) and the query session it will hold (W5).
    registry: Rc<DbRegistry>,
    connections: Vec<Attributed<DbConnection>>,
    /// The connection id last clicked — "selecting a connection sets the active
    /// connection" (W3). W5 runs queries against whichever connection this names.
    active_id: Option<String>,
    armed_delete: Option<(String, Scope)>,
    /// The open connection add/edit modal (W4), if any. `pub(crate)` — `app.rs`'s
    /// `Render for AppState` reads it directly to paint the overlay (the exact mirror
    /// of `AppState.form`/`HostForm`).
    pub(crate) form: Option<Entity<DbConnForm>>,
    /// Keeps the form's event subscription alive exactly as long as the form is open.
    _form_subscription: Option<Subscription>,

    // ---- W5: SQL editor + results ------------------------------------------------
    /// The SQL editor. Lazily built by `ensure_query_widgets` (needs `window`, which
    /// `DbTabState::new` doesn't have) the first time the Database tab paints.
    sql: Option<Entity<InputState>>,
    /// Keeps the SQL editor's `PressEnter{secondary: true}` (Ctrl/Cmd-Enter) subscription
    /// alive for as long as the editor exists — i.e. for the tab's whole lifetime.
    _sql_subscription: Option<Subscription>,
    /// Results table. Built alongside `sql`, once. Its delegate is mutated *in place* on
    /// every query completion/page — never rebuilt (`TableState::new` needs `window`,
    /// unavailable from an async completion callback).
    results: Option<Entity<TableState<ResultDelegate>>>,
    /// The open client for `client_for`, reused across repeat queries against the same
    /// connection so Run doesn't reconnect every time.
    client: Option<Arc<dyn DbClient>>,
    /// Which connection id `client` is open against. Compared to `active_id` on Run to
    /// decide whether the cached client is still usable.
    client_for: Option<String>,
    /// True while a connect-or-query task is in flight — guards re-entrant Run/next-page
    /// clicks.
    running: bool,
    status: QueryStatus,
    /// The exact SQL text of the last run query, so "next page" repeats it without
    /// depending on the editor's current (possibly since-edited) contents.
    last_sql: Option<String>,
    /// The cursor `query_paged` returned for the next page, if any.
    next_cursor: Option<PageCursor>,
    /// The results grid's filter box (inc-3). Lazily built beside `sql` by
    /// `ensure_query_widgets`; its text is pushed into [`ResultDelegate::set_query`]
    /// by `apply_result_filter`.
    result_filter: Option<Entity<TextInput>>,
    /// Keeps the filter box's `cx.observe` alive. [`TextInput`] has no change
    /// callback, so an observation of its `cx.notify()` is the wiring — the same
    /// pattern `systems_tab`/`network_tab` use for theirs.
    _result_filter_sub: Option<Subscription>,
    /// The plan the user last asked for, while it is showing (inc-3). `Some` swaps the
    /// results grid for the plan pane; the grid's own state is untouched underneath, so
    /// closing the plan restores it with its sort and filter intact.
    plan: Option<PlanView>,
    /// The most recently completed [`QueryPage`] — the source [`export_csv`] writes
    /// from. Kept as the raw domain type (not derived back out of `results`'s
    /// `gpui-component` delegate) so CSV export stays a pure function over data sid
    /// already owns, independent of the table widget's internal representation.
    last_page: Option<QueryPage>,

    // ---- D1: schema tree -----------------------------------------------------------
    /// Cached schema for whichever connection `client_for` names. `None` before the
    /// first successful fetch (or after switching to a connection with none yet).
    schema: Option<SchemaInfo>,
    /// Relationship metadata (FK edges + primary keys) for the same connection as
    /// `schema` — fetched alongside it in [`fetch_schema`] and cleared on the same
    /// triggers (connection switch, re-fetch). Feeds the "diagram" pop-out window
    /// (`db_diagram::DiagramView`); `None` before the first successful fetch, same as
    /// `schema`.
    schema_graph: Option<SchemaGraph>,
    /// True while a `schema_introspect` task is in flight — guards re-entrant
    /// selection/⟳ clicks the same way `running` guards Run.
    schema_loading: bool,
    /// Monotonic staleness guard for schema fetches: bumped at every fetch spawn and on
    /// every connection switch; a completion applies only if its captured value still
    /// matches, so an in-flight fetch for a previously-selected connection can never
    /// overwrite the newer selection's schema/graph (bug-hunt round D, HIGH).
    schema_generation: u64,
    /// The query-path mirror of `schema_generation`, guarding `run_query`/`next_page`
    /// completions (and, through `last_page`, what CSV export sees) against landing
    /// under a different connection than the one they were started for.
    query_generation: u64,
    schema_error: Option<String>,
    /// Which tables are expanded (columns visible), keyed by [`table_display_name`].
    /// Cleared whenever the active connection changes or the schema is re-fetched.
    schema_expanded: HashSet<String>,
    /// The relationships pop-out, while one is open — see
    /// [`AppState::push_schema_to_diagram`] for why the main window keeps a handle to a
    /// view living in another window. Weak on purpose: the strong reference belongs to
    /// that window's `Root`, so closing the window is all it takes to drop the view.
    diagram: Option<WeakEntity<DiagramView>>,

    // ---- D2: cell copy / view -------------------------------------------------------
    /// The `view` popover's contents, if open.
    cell_view: Option<CellView>,
    /// Transient one-line feedback for cell-copy and CSV-export actions (D2/D3) —
    /// shown under the query status line. Not cleared automatically; the next action
    /// (or query run) overwrites or clears it.
    notice: Option<String>,
    /// Whether the "⭳ Export ▾" menu is open — toggled by its own click, closed by
    /// picking a format (see [`AppState::export`]).
    export_menu_open: bool,

    // ---- D4: query history -----------------------------------------------------------
    /// Most-recent-first ring of run queries, capped at [`HISTORY_CAP`] with
    /// consecutive-duplicate suppression — see [`push_history`].
    history: Vec<String>,

    // ---- connections panel (top of the unified left column): folders / inline rename --
    /// Which folders are collapsed in the connections panel, keyed by folder name.
    /// Presence encodes "collapsed" (mirrors `schema_expanded`'s presence-encodes-state
    /// convention, inverted: there, presence means expanded — here it means collapsed,
    /// since a freshly-created folder should default to expanded/visible).
    collapsed_folders: HashSet<String>,
    /// The connections panel's own focus handle — lazily created by `ensure_query_widgets`
    /// (needs `Context`, unlike `DbTabState::new`). Focused on every row click so a
    /// subsequent F2 (with no text field focused) reaches [`AppState::begin_rename_active`]
    /// via the panel's `on_key_down`.
    conn_focus: Option<FocusHandle>,
    /// VS Code-style inline rename in progress on one connection row, if any — see
    /// [`AppState::begin_rename`]/[`AppState::commit_rename`]/[`AppState::cancel_rename`].
    renaming: Option<RenameState>,
    /// Inline folder-assignment edit in progress on one connection row, if any (the
    /// minimal "row hover-menu → small input" affordance for Task 2's grouping) — see
    /// [`AppState::begin_folder_edit`]/[`AppState::commit_folder_edit`]/
    /// [`AppState::cancel_folder_edit`].
    folder_editing: Option<FolderEditState>,
}

/// An in-progress inline rename (F2 / double-click the name) — the row's identity/origin
/// (needed for [`Store::rename_connection`]'s scope-qualified write) plus the live-edit
/// [`TextInput`]. Only one row can be mid-rename at a time; starting a new one replaces
/// this outright (see [`AppState::begin_rename`]).
struct RenameState {
    id: String,
    origin: Scope,
    input: Entity<TextInput>,
}

/// An in-progress inline folder edit — same shape/lifecycle as [`RenameState`], committed
/// via [`Store::set_connection_folder`] instead. An empty/blank commit clears the
/// connection's folder (moves it back to the ungrouped top level).
struct FolderEditState {
    id: String,
    origin: Scope,
    input: Entity<TextInput>,
}

/// The `view` popover's contents (D2) — the column a long cell came from, and its
/// full (untruncated) text. Read-only; mirrors `session.rs`'s `Preview`/`PreviewContent`
/// shape but simpler (no oversize/binary cases — grid cells are always the display
/// strings `DbClient` already rendered to text).
#[derive(Clone, Debug, PartialEq, Eq)]
struct CellView {
    column: String,
    text: String,
}

/// Outcome of the last query run, driving the query pane's status line.
///
/// `Ok` deliberately carries **no row count**. It used to, and the toolbar read it —
/// but the grid is now a filtered view over the page ([`visible_rows`]), so "how many
/// rows" has two answers (fetched, and shown) that only [`ResultDelegate::counts`]
/// knows. Keeping a third copy here would be a number that goes stale the moment the
/// user types in the filter box.
enum QueryStatus {
    Idle,
    Err(String),
    Ok { duration_ms: u64, has_more: bool },
}

/// The query pane's "⭳ Export ▾" control (Murphy: "we should also make it a generic
/// export option so we can add more export types in the future") — the whole seam for a
/// new format is one variant here + one arm in [`AppState::export`]; the menu itself
/// renders from [`Self::ALL`] and needs no other change. Exactly one format today (CSV),
/// carried over unchanged from the old standalone `⭳ CSV` button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportFormat {
    Csv,
}

impl ExportFormat {
    /// Every format, in menu order. The one place a new variant needs to be listed.
    const ALL: &'static [ExportFormat] = &[ExportFormat::Csv];

    /// The menu item's label.
    fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV (current page)",
        }
    }
}

// ---- results grid: the column plan (pure `result-set shape -> ColumnWidth`) -------------

/// How wide one character of the results grid's `text_xs` cells is, in logical pixels.
///
/// Every other table in sid declares its columns by hand, once, because its schema is
/// known at compile time. This one's is not: the shape arrives with the data, so the
/// widths have to be *derived*, and deriving them needs a character advance. gpui only
/// measures text inside a `Window`, and a plan that needed a window could not be a pure
/// function — so this is a deliberately slightly-generous average for the bundled
/// proportional face at 12px. Over-estimating costs a few pixels of slack inside a
/// column; under-estimating truncates, which is the bug being fixed.
const CELL_CHAR_PX: f32 = 6.8;

/// Chrome inside one cell that is not available to its text: `render_td`'s `px_2` on
/// both sides, plus room in the header for the sort chevron `render_th` leaves space for.
const CELL_CHROME_PX: f32 = 28.0;

/// The narrowest a bounded column (integer, float, bool, null) may render. Wide enough
/// for a 7-digit value or a short header; narrow enough that a table of counters does
/// not push its text columns off the screen.
const NUMERIC_MIN_PX: f32 = 88.0;

/// The widest a bounded column is *floored* at. A 40-digit bignum is real, but it must
/// not be allowed to claim a third of the grid before the text columns have had any.
const NUMERIC_MAX_PX: f32 = 200.0;

/// The narrowest an unbounded column (text, bytes, driver-specific) may render — the old
/// hard-coded width every column used to get, kept as the floor so no result set is
/// worse off than before.
const TEXT_MIN_PX: f32 = 140.0;

/// The widest an unbounded column is *floored* at. Past this the column is a `Grow`
/// anyway, so a wide window still gives it more; the cap only stops one JSON blob column
/// from pushing every sibling into horizontal scroll on a narrow one.
const TEXT_MAX_PX: f32 = 320.0;

/// Whether a column's values have a known upper bound on their rendered width.
///
/// The split that the plan turns on: bounded columns stay compact (`Min`), unbounded ones
/// absorb the leftover (`Grow`). `Other` — the driver-specific escape hatch, which is
/// where `uuid`, `json`, `timestamptz` and friends land — counts as unbounded: guessing
/// "narrow" for a type sid does not recognise is how a uuid column ends up truncated.
fn is_bounded(ty: &ColumnType) -> bool {
    matches!(
        ty,
        ColumnType::Integer | ColumnType::Float | ColumnType::Bool | ColumnType::Null
    )
}

/// Plan one result page's column widths from its **shape** — the column names and types,
/// plus the widest value each column actually holds on this page.
///
/// This is the one decision the results grid makes that is worth testing, so it is a pure
/// function of the page: no `Window`, no delegate, no theme. `set_page` is the only
/// caller.
///
/// The rules, in order:
///
/// 1. A column's *ideal* width is its widest content — header name or cell value,
///    whichever is longer — at [`CELL_CHAR_PX`] plus [`CELL_CHROME_PX`]. Measuring the
///    header too is what stops `avg_order_value_usd` from being clipped above a column of
///    three-digit numbers.
/// 2. A **bounded** column ([`is_bounded`]) becomes `Min`, clamped to
///    [`NUMERIC_MIN_PX`]..[`NUMERIC_MAX_PX`]. `Min`, not `Fixed`, on purpose: a result set
///    with no text column at all (`select count(*)`) would otherwise leave the grid's
///    right edge empty, and `sid_ui`'s resolver hands the leftover to `Min` columns
///    precisely when nothing grows.
/// 3. An **unbounded** column becomes `Grow`, floored the same way into
///    [`TEXT_MIN_PX`]..[`TEXT_MAX_PX`], with its **weight set to its content length** —
///    so a `description` beside a `city` takes the larger share of a wide window instead
///    of the two splitting it evenly and both being wrong.
/// 4. No columns in, no widths out: a statement that returned no result set plans nothing
///    rather than a phantom column.
fn plan_result_columns(columns: &[DbColumn], rows: &[Row]) -> Vec<ColumnWidth> {
    columns
        .iter()
        .enumerate()
        .map(|(ix, column)| {
            let widest_value = rows
                .iter()
                .filter_map(|row| row.values.get(ix))
                .map(|v| v.chars().count())
                .max()
                .unwrap_or(0);
            let chars = column.name.chars().count().max(widest_value) as f32;
            let ideal = chars * CELL_CHAR_PX + CELL_CHROME_PX;
            if is_bounded(&column.ty) {
                ColumnWidth::Min(ideal.clamp(NUMERIC_MIN_PX, NUMERIC_MAX_PX))
            } else {
                ColumnWidth::grow()
                    .min_width(ideal.clamp(TEXT_MIN_PX, TEXT_MAX_PX))
                    // Weight is a ratio, so the unit does not matter — only that a column
                    // holding twice the text asks for twice the slack. Floored at 1 so a
                    // column of empty strings still counts as a grower.
                    .weight(chars.max(1.0))
            }
        })
        .collect()
}

/// One grid cell's text, collapsed onto a single line.
///
/// A result value is whatever the driver rendered, and plenty of them carry newlines: a
/// `sqlite_master.sql` row is a whole `CREATE TABLE` statement, a `jsonb` column is a
/// pretty-printed document, a `text` column is a pasted log. A grid row is one line tall,
/// so an unmodified multi-line value paints straight through the rows beneath it and is
/// clipped mid-glyph — which is what the fill-width columns made impossible to miss.
///
/// Runs of whitespace (including the newline) become one space, and the ends are trimmed.
/// Nothing is lost: the untouched value is one click away in the view popover, the
/// clipboard copy takes the original, and CSV export writes from `last_page` and never
/// sees this.
fn single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(c);
    }
    out
}

// ---- inc-3: the results grid's view state (sort + filter, both pure) --------------------

/// The literal a driver renders a SQL `NULL` as.
///
/// Not a guess: `sid_core::db::Row`'s contract is "values rendered to display strings,
/// `NULL` for NULL", and both adapters implement exactly that (`postgres::render`'s
/// `Ok(None)` arm and `sqlite::render_sqlite_value`'s `Value::Null` arm).
///
/// **The limitation this carries:** a `text` column holding the four characters `NULL`
/// is indistinguishable from an absent value *at this layer* — the distinction was
/// already thrown away by the driver, before the grid ever saw the page. Sorting is
/// therefore the wrong place to fix it; the fix, if it is ever wanted, is a typed
/// nullability flag on [`Row`], which is a change to the adapter contract.
const NULL_TEXT: &str = "NULL";

/// A results-grid sort: which column, and which way.
///
/// `None` (an absent `ResultSort`) is a real state, not "unset": it means **the order
/// the engine returned**, which is what the third click of the header cycle restores
/// and the only order that reflects an `ORDER BY` the user wrote themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResultSort {
    col_ix: usize,
    descending: bool,
}

/// Order two non-NULL cells of a column of type `ty`.
///
/// Every value in a result page is a **display string** the driver already rendered, so
/// there is no typed value left to compare — the column's declared [`ColumnType`] is
/// the only evidence of what the text means, and this is where that evidence is spent.
/// Sorting the strings directly is what makes `"10"` sort before `"9"`.
fn cell_cmp(a: &str, b: &str, ty: &ColumnType) -> Ordering {
    match ty {
        ColumnType::Integer | ColumnType::Float => numeric_cmp(a, b),
        ColumnType::Bool => bool_cmp(a, b),
        // `Bytes` renders as `0x…`, which orders sensibly as text (same length ⇒ same
        // order as the bytes). `Null` columns are all-NULL and never reach here.
        // `Other` — uuid, json, timestamptz, every unmapped Postgres type — is text
        // because text is the only ordering sid can honestly claim for a type it does
        // not recognise; ISO-8601 timestamps happen to sort correctly that way anyway.
        ColumnType::Text | ColumnType::Bytes | ColumnType::Null | ColumnType::Other(_) => {
            text_cmp(a, b)
        }
    }
}

/// Numeric order, with an exact integer path first.
///
/// `i128` before `f64` because a Postgres `int8`/`numeric` can carry values an `f64`
/// cannot tell apart (anything past 2^53), and two neighbouring bignums comparing
/// `Equal` would silently leave them in their original order while claiming to be
/// sorted. A value neither parse accepts — a `money` column's `$1.00`, a driver
/// placeholder — falls back to [`text_cmp`] so the comparator stays a *total* order
/// rather than collapsing every such pair to `Equal`. `NaN` (which no driver should
/// emit) compares `Equal`, the only answer `partial_cmp` allows.
fn numeric_cmp(a: &str, b: &str) -> Ordering {
    if let (Ok(x), Ok(y)) = (a.trim().parse::<i128>(), b.trim().parse::<i128>()) {
        return x.cmp(&y);
    }
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => text_cmp(a, b),
    }
}

/// Case-insensitive order, with a case-sensitive tiebreak.
///
/// Byte order alone puts every capitalised value ahead of every lowercase one, which
/// reads as "not sorted" on a name column. Case-insensitive *alone* leaves `"ABC"` and
/// `"abc"` equal and therefore in arbitrary relative order; the tiebreak is what makes
/// this a total order, so the same page always sorts the same way.
fn text_cmp(a: &str, b: &str) -> Ordering {
    match a.to_lowercase().cmp(&b.to_lowercase()) {
        Ordering::Equal => a.cmp(b),
        ordered => ordered,
    }
}

/// `false` before `true`, in every spelling a driver produces.
///
/// Postgres renders `true`/`false`. SQLite has no boolean type at all, so a column
/// *declared* `BOOLEAN` (which `rusqlite_type_to_column_type` maps to
/// [`ColumnType::Bool`]) arrives as `1`/`0`. Both must order the same way, and neither
/// can be left to [`text_cmp`], where `"f" < "t"` and `"no" < "yes"` are right only by
/// accident of the alphabet. Anything unrecognised falls back to text.
fn bool_cmp(a: &str, b: &str) -> Ordering {
    match (bool_rank(a), bool_rank(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => text_cmp(a, b),
    }
}

/// `0` for a false-ish rendering, `1` for a true-ish one, `None` for neither.
fn bool_rank(value: &str) -> Option<u8> {
    match value.trim().to_ascii_lowercase().as_str() {
        "false" | "f" | "0" | "no" => Some(0),
        "true" | "t" | "1" | "yes" => Some(1),
        _ => None,
    }
}

/// One cell as the comparator sees it: `None` for a SQL NULL **or** a cell the row
/// does not have at all (a ragged row is a driver bug, and an absent value is as
/// absent as an explicit one). An empty string is a *value* and comes back `Some("")`.
fn sortable_cell(row: &Row, col_ix: usize) -> Option<&str> {
    row.values
        .get(col_ix)
        .map(String::as_str)
        .filter(|v| *v != NULL_TEXT)
}

/// Order two rows by one column, with **NULLs last in both directions**.
///
/// The direction flip is applied to the value comparison only, never to the NULL
/// placement — which is the whole reason this is a function rather than a
/// `sort_by_key` plus a `.reverse()`. Postgres's own default is NULLS LAST ascending
/// and NULLS FIRST descending; sid does not follow it, because a person sorts a grid
/// to bring the interesting extreme to the top, and a screenful of empty cells there
/// is the one thing they were not looking for.
fn row_cmp(a: &Row, b: &Row, col_ix: usize, ty: &ColumnType, descending: bool) -> Ordering {
    match (sortable_cell(a, col_ix), sortable_cell(b, col_ix)) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let ordered = cell_cmp(x, y, ty);
            if descending {
                ordered.reverse()
            } else {
                ordered
            }
        }
    }
}

/// Whether any of `row`'s cells contains `needle`, which must already be lowercased
/// and trimmed (see [`visible_rows`], which does that once per pass rather than once
/// per cell). An empty needle matches everything.
fn row_matches(row: &Row, needle: &str) -> bool {
    needle.is_empty() || row.values.iter().any(|v| v.to_lowercase().contains(needle))
}

/// The rows the grid actually shows: `rows` narrowed by `query`, then ordered by
/// `sort`.
///
/// # The decision this encodes
///
/// **Sorting and filtering happen client-side, over the page that was fetched — not
/// over the result set.** The alternative was to re-issue the query with an `ORDER BY`
/// / `WHERE` appended, which is the only ordering that would be true of the whole
/// result set. It was rejected: it means rewriting SQL the user wrote (`query_paged`
/// already documents how badly its own single-`SELECT` subquery wrapper copes with
/// anything else), it costs a round trip per header click, it cannot express "sort by
/// the third column of this ad-hoc expression" without naming an alias that may not
/// exist, and it is impossible for the redb browse engine, which has no query language
/// at all. A page sort is cheap, instant, works identically on all three engines, and
/// is *honest* as long as the user is told what it covers — which is
/// [`page_view_caveat`]'s job.
///
/// Filter first, then order what survives, so the count and the first row agree.
/// The sort is **stable** (`sort_by`, not `sort_unstable_by`): rows with equal keys
/// keep the order the engine returned, which is what makes re-applying the same sort a
/// no-op and the third header click a genuine "back to how it arrived".
///
/// A `col_ix` past the end of the page is ignored rather than panicking — the column
/// plan is rebuilt with every page, so a sort left over from a wider one is a state
/// that really occurs.
fn visible_rows(
    column_types: &[ColumnType],
    rows: &[Row],
    query: &str,
    sort: Option<ResultSort>,
) -> Vec<Row> {
    let needle = query.trim().to_lowercase();
    let mut out: Vec<Row> = rows
        .iter()
        .filter(|row| row_matches(row, &needle))
        .cloned()
        .collect();
    if let Some(ResultSort { col_ix, descending }) = sort
        && let Some(ty) = column_types.get(col_ix)
    {
        out.sort_by(|a, b| row_cmp(a, b, col_ix, ty, descending));
    }
    out
}

/// The one-line caveat a page-local view has to carry, or `None` when it does not.
///
/// [`visible_rows`] sorts and filters the *fetched page*. When that page is the whole
/// result set — `has_more` is false, there is no next cursor — the page view IS the
/// result-set view and a warning would be crying wolf on every small query. It is only
/// when rows exist that the grid has never seen that "sorted" and "filtered" become
/// claims narrower than they look, and the user has to be told which one is narrow.
fn page_view_caveat(sorted: bool, filtered: bool, has_more: bool) -> Option<&'static str> {
    if !has_more {
        return None;
    }
    match (sorted, filtered) {
        (true, true) => Some(
            "sorted and filtered within this page only — the result set has more rows, \
             fetch them to order or search across all of it",
        ),
        (true, false) => Some(
            "sorted within this page only — the result set has more rows, fetch them for \
             a complete ordering",
        ),
        (false, true) => Some(
            "filtered within this page only — the result set has more rows, fetch them to \
             search across all of it",
        ),
        (false, false) => None,
    }
}

// ---- inc-3: EXPLAIN -----------------------------------------------------------------

/// A plan the user asked for, while it is on screen.
///
/// Held beside `last_page` rather than replacing it: the plan is a detour, and closing
/// it puts the results the user was looking at back exactly as they were, with their
/// sort and filter intact.
struct PlanView {
    /// The engine's own plan keyword (`ExplainSupport::keyword`), so the panel says
    /// which dialect's plan this is — `EXPLAIN` and `EXPLAIN QUERY PLAN` read very
    /// differently and confusing them wastes real time.
    keyword: SharedString,
    /// The plan, one entry per printable line — see [`plan_lines`].
    lines: Vec<String>,
}

/// Flatten an `EXPLAIN` result page into printable plan lines.
///
/// # Why a plan does not go in the results grid
///
/// Postgres returns one `QUERY PLAN` text column whose rows already carry the tree's
/// indentation — `"  ->  Seq Scan on orders"` — and that indentation *is* the tree.
/// The grid's `single_line` collapses runs of whitespace (it has to: a cell is one row
/// tall and a `CREATE TABLE` statement in a cell paints through everything under it),
/// which would flatten every plan into an unreadable single-level list. So the plan
/// gets its own monospace pane, and this function is the seam between the engine's
/// page and that pane.
///
/// A row with several columns — SQLite's `EXPLAIN QUERY PLAN` returns `id`, `parent`,
/// `notused`, `detail` — is joined rather than reduced to its "interesting" column.
/// `detail` is the readable part, but which of an engine's plan columns a reader is
/// allowed to see is not sid's call to make.
fn plan_lines(page: &QueryPage) -> Vec<String> {
    page.rows.iter().map(|row| row.values.join("  ")).collect()
}

/// Backs the results grid. Constructed empty by `ensure_query_widgets`, then mutated in
/// place (`set_page`) whenever a query completes — see the `results` field doc comment
/// for why it's never rebuilt.
struct ResultDelegate {
    /// The columns and the width each one declared, resized to the live viewport by
    /// [`FillTable`]. Rebuilt from [`plan_result_columns`] on every page, because unlike
    /// every other table in sid this one's *schema* changes with the data.
    columns: FillColumns,
    /// The page exactly as the engine returned it — never rendered directly. Kept so
    /// the filter and the sort are both *views* over it: clearing the filter restores
    /// every row without a round trip, and the third header click restores the
    /// engine's own order (see [`ResultSort`]). Same `all_*`/display split
    /// `systems_tab`'s `ProcessesDelegate` uses.
    all_rows: Vec<Row>,
    /// The declared type of each column, parallel to `columns` — the only evidence the
    /// comparators have about what a display string means (see [`cell_cmp`]).
    column_types: Vec<ColumnType>,
    /// The filtered + sorted display set. This is what `rows_count`/`render_td` read.
    rows: Vec<Row>,
    /// The live filter text, cached so [`Self::recompute`] can re-apply it after a
    /// sort or a new page.
    query: String,
    /// The active sort, or `None` for the engine's own order.
    sort: Option<ResultSort>,
    /// Handle back to the owning [`AppState`], used only by D2's `view` click (open
    /// the popover on `AppState.db.cell_view`) and copy-notice (`AppState.db.notice`).
    /// A raw `div().on_click` inside `render_td` only ever gets `&mut App` at click
    /// time (see `gpui::div::InteractiveElement::on_click`), not `AppState` — this weak
    /// handle is what lets the cell reach back into it. `None` only in the brief window
    /// before `ensure_query_widgets` sets it (never observed mid-render — the table is
    /// built and given a handle in the same call).
    app: Option<WeakEntity<AppState>>,
}

impl ResultDelegate {
    fn empty() -> Self {
        Self {
            columns: FillColumns::new([]),
            all_rows: Vec::new(),
            column_types: Vec::new(),
            rows: Vec::new(),
            query: String::new(),
            sort: None,
            app: None,
        }
    }

    /// Replace the displayed page — and, with it, the whole column declaration. The
    /// widths come from [`plan_result_columns`]; [`FillTable`]'s viewport probe resolves
    /// them against the grid's real width on the next frame.
    ///
    /// Every column is declared `.sortable()`, which is what makes
    /// [`sid_ui::sortable_th`]'s header cell live — see [`Self::perform_sort`] for the
    /// ordering it applies and [`visible_rows`] for what a sort does and does not cover.
    ///
    /// # What survives a new page
    ///
    /// The **filter** always does: it is a view preference the user typed, and dropping
    /// it on every `next page` would make paging through a search impossible.
    ///
    /// The **sort** survives only when the new page has the same columns, by name and
    /// position. A `ResultSort` is an *index*, so carrying it across a page with a
    /// different shape would silently re-point it at an unrelated column — the exact
    /// class of bug the generation guards exist to prevent, one layer down. Same
    /// columns means this is the next page of the same query, where keeping the sort is
    /// what the user expects.
    fn set_page(&mut self, page: QueryPage) {
        let same_shape = self.column_types.len() == page.columns.len()
            && (0..self.columns.len())
                .all(|ix| self.columns.column(ix).name.as_ref() == page.columns[ix].name.as_str());
        if !same_shape {
            self.sort = None;
        }
        let plan = plan_result_columns(&page.columns, &page.rows);
        self.columns = FillColumns::new(page.columns.iter().zip(plan).map(|(c, width)| {
            (
                Column::new(c.name.clone(), c.name.clone()).sortable(),
                width,
            )
        }));
        // Re-assert the surviving sort on the freshly declared columns, or the header
        // chevron would come back blank on a page that is still sorted.
        if let Some(ResultSort { col_ix, descending }) = self.sort {
            self.columns.apply_sort(
                col_ix,
                if descending {
                    ColumnSort::Descending
                } else {
                    ColumnSort::Ascending
                },
            );
        }
        self.column_types = page.columns.into_iter().map(|c| c.ty).collect();
        self.all_rows = page.rows;
        self.recompute();
    }

    /// Set the filter text and re-derive the display set.
    fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.recompute();
    }

    /// Re-derive the display set from the page, the filter and the sort. The one place
    /// `rows` is written, so the three inputs can never fall out of step.
    fn recompute(&mut self) {
        self.rows = visible_rows(&self.column_types, &self.all_rows, &self.query, self.sort);
    }

    /// How many of the page's rows the filter is currently hiding — `(shown, total)`,
    /// for the toolbar's count.
    fn counts(&self) -> (usize, usize) {
        (self.rows.len(), self.all_rows.len())
    }
}

impl FillTableDelegate for ResultDelegate {
    fn fill_columns(&mut self) -> &mut FillColumns {
        &mut self.columns
    }
}

impl TableDelegate for ResultDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        self.columns.column(col_ix)
    }

    /// Sort on a click anywhere in the header cell, not only on the chevron
    /// (`sid_ui::table::sortable_th`).
    ///
    /// The migration that adopted `sortable_th` here left the columns *not* sortable
    /// and wrote down why: a result page is `Vec<Row>` of display strings, so a sort
    /// needed three answers first — is `"10" < "9"`, where does `NULL` go, and does a
    /// click sort this page or re-issue the query with an `ORDER BY`. Those are
    /// answered now, in [`cell_cmp`], [`row_cmp`] and [`visible_rows`] respectively,
    /// each with tests; the seam it left is simply used.
    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        sortable_th(col_ix, self.columns.column(col_ix), cx)
    }

    /// Re-order the page by `col_ix`.
    ///
    /// `ColumnSort::Default` — the third click of upstream's cycle — clears the sort
    /// rather than keeping the previous direction (which is what `systems_tab` does
    /// with its live process list, because "insertion order" means nothing there).
    /// Here it means something exact and useful: **the order the engine returned**,
    /// which is the order an `ORDER BY` in the user's own SQL produced. Throwing that
    /// away would make the user's own ordering unrecoverable without re-running.
    ///
    /// `apply_sort` mirrors the new state onto the delegate's columns so the header
    /// chevron survives the next `TableState::refresh` — which every viewport change
    /// triggers on a [`FillTable`]. See `FillColumns::apply_sort`.
    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = match sort {
            ColumnSort::Ascending => Some(ResultSort {
                col_ix,
                descending: false,
            }),
            ColumnSort::Descending => Some(ResultSort {
                col_ix,
                descending: true,
            }),
            ColumnSort::Default => None,
        };
        self.columns.apply_sort(col_ix, sort);
        self.recompute();
        cx.notify();
    }

    /// D2: the whole cell copies its text to the clipboard on click; a cell over
    /// [`CELL_VIEW_THRESHOLD`] chars also gets an expand button opening the read-only view
    /// popover.
    ///
    /// The expand button is now a real [`IconButton`], which consumes its own click — so
    /// unlike the bare `view` word it replaces, opening the popover no longer also copies
    /// the cell (`sid_ui::button`'s `consume`).
    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let t = theme::active(cx);
        let (fg, selection) = (t.fg, t.selection);
        let text = self.rows[row_ix]
            .values
            .get(col_ix)
            .cloned()
            .unwrap_or_default();
        let column_name = self.columns.column(col_ix).name.to_string();
        let cell_ix = row_ix * 4096 + col_ix;

        let copy_text = text.clone();
        let copy_app = self.app.clone();
        let view_button = (text.chars().count() > CELL_VIEW_THRESHOLD).then(|| {
            let view_app = self.app.clone();
            let view_text = text.clone();
            let view_column = column_name.clone();
            IconButton::new(
                ("db-cell-view", cell_ix),
                Icon::Maximize,
                "view the whole value",
            )
            .small()
            .on_click(move |_ev, _window, cx| {
                let Some(app) = &view_app else { return };
                let _ = app.update(cx, |state, cx| {
                    state.db.cell_view = Some(CellView {
                        column: view_column.clone(),
                        text: view_text.clone(),
                    });
                    cx.notify();
                });
            })
        });

        h_flex()
            .id(("db-cell", cell_ix))
            .w_full()
            .min_w_0()
            .gap_1()
            .px_2()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(selection)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(fg))
                    .child(single_line(&text)),
            )
            .children(view_button)
            .on_click(move |_ev, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                if let Some(app) = &copy_app {
                    let _ = app.update(cx, |state, cx| {
                        state.db.notice = Some("copied cell to clipboard".to_string());
                        cx.notify();
                    });
                }
            })
    }
}

// ---- D1: schema tree (pure `SchemaInfo -> tree-rows` transform) --------------------------

/// One renderable row of the schema tree — either a table header (expand/collapse +
/// click-to-insert-SQL) or one of its columns (only present while that table is
/// expanded). Pure data, no rendering — `schema_tree_rows` below is the one place
/// `SchemaInfo` becomes a flat, orderable list the tree view can `uniform_list`/`Vec`
/// over; kept separate from rendering so it's unit-testable without a `Window` (D1's
/// TDD requirement).
#[derive(Clone, Debug, PartialEq, Eq)]
enum SchemaRow {
    Table {
        display_name: String,
        expanded: bool,
    },
    Column {
        name: String,
    },
}

/// `schema` flattened into `SchemaRow`s, in table order, expanding each table present
/// in `expanded` (keyed by [`table_display_name`]) into its columns immediately after
/// its header row.
fn schema_tree_rows(schema: &SchemaInfo, expanded: &HashSet<String>) -> Vec<SchemaRow> {
    let mut rows = Vec::with_capacity(schema.tables.len());
    for table in &schema.tables {
        let display_name = table_display_name(table);
        let is_expanded = expanded.contains(&display_name);
        rows.push(SchemaRow::Table {
            display_name,
            expanded: is_expanded,
        });
        if is_expanded {
            rows.extend(table.columns.iter().map(|c| SchemaRow::Column {
                name: c.name.clone(),
            }));
        }
    }
    rows
}

// ---- connections panel: folder grouping (pure `connections -> panel rows` transform) ----

/// One renderable row of the connections panel — a collapsible folder header, or a
/// connection nested under one (or sitting at the top level, when ungrouped). Pure data,
/// mirroring [`SchemaRow`]'s split from rendering — [`group_connections`] is the one
/// place the composed connection list becomes this flat, orderable row list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ConnRow {
    Folder {
        name: String,
        expanded: bool,
        count: usize,
    },
    /// A connection's id — the panel row re-looks this up in `self.db.connections` at
    /// render time (rather than cloning the whole `Attributed<DbConnection>` in here) so
    /// this stays a plain identity list, matching how `active_id`/`armed_delete` already
    /// key rows by id rather than by index.
    Connection { id: String },
}

/// Group `connections` by [`DbConnection::folder`] (one flat level — see that field's
/// own doc comment) into the connections panel's row list: every ungrouped connection
/// (`folder` absent, or present-but-blank) stays at the top level first — Murphy's
/// "None → ungrouped top level" — followed by named folders in alphabetical order, each
/// a collapsible header (collapsed when its name is in `collapsed`) with its members
/// immediately after when expanded. Within a group, connections keep their incoming
/// (store) order. Pure logic, no `AppState`/GPUI — the folder-grouping TDD target.
fn group_connections(
    connections: &[Attributed<DbConnection>],
    collapsed: &HashSet<String>,
) -> Vec<ConnRow> {
    let mut folders: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut ungrouped: Vec<String> = Vec::new();
    for a in connections {
        match a.item.folder.as_deref() {
            Some(f) if !f.is_empty() => folders.entry(f).or_default().push(a.item.id.clone()),
            _ => ungrouped.push(a.item.id.clone()),
        }
    }

    let mut rows: Vec<ConnRow> = ungrouped
        .into_iter()
        .map(|id| ConnRow::Connection { id })
        .collect();
    for (name, ids) in folders {
        let expanded = !collapsed.contains(name);
        rows.push(ConnRow::Folder {
            name: name.to_string(),
            expanded,
            count: ids.len(),
        });
        if expanded {
            rows.extend(ids.into_iter().map(|id| ConnRow::Connection { id }));
        }
    }
    rows
}

/// The state mark one connection row shows, from the four facts the tab knows about it.
///
/// A saved connection is not a session, so "live" here means what it can mean: sid is
/// holding an **open client** for this row (`DbTabState::client`/`client_for`), which is
/// exactly the state that makes the next Run skip the connect. Kept pure — the four
/// booleans are all the render path has, and the precedence between them is the only
/// thing worth getting right:
///
/// - **live wins over busy**: a query running against an already-open connection is a
///   connected connection doing work, not one that is still dialling.
/// - **busy wins over failed**: a retry in flight supersedes the error it is retrying.
/// - **failed and busy are only ever the *active* row's** — `schema_error` and the
///   in-flight flags are single-slot state belonging to the current selection, so
///   attributing either to a row that is not selected would light up the wrong dot.
fn connection_dot(live: bool, active: bool, busy: bool, errored: bool) -> ConnectionState {
    match (live, active, busy, errored) {
        (true, ..) => ConnectionState::Live,
        (false, true, true, _) => ConnectionState::Connecting,
        (false, true, false, true) => ConnectionState::Failed,
        _ => ConnectionState::Offline,
    }
}

/// `schema.table` for Postgres (non-empty schema), or the bare table name for SQLite
/// and the redb browse engine (no schema namespace). Doubles as the tree row's expanded
/// key and the identifier `SELECT * FROM <table_display_name>` inserts.
///
/// `pub(crate)` — `db_diagram::DiagramView` joins [`sid_core::db::ForeignKey`]/
/// `primary_keys` edges (qualified the same way, per that type's doc comment) to table
/// boxes by this exact key, so the diagram reuses this helper rather than recomputing
/// the rule.
pub(crate) fn table_display_name(table: &TableInfo) -> String {
    match table.schema.as_deref() {
        Some(s) if !s.is_empty() => format!("{s}.{}", table.name),
        _ => table.name.clone(),
    }
}

/// Quote an introspected identifier for interpolation into generated SQL, unless every
/// dot-segment is already a plain identifier (`[A-Za-z_][A-Za-z0-9_]*`). ANSI style —
/// wrap in `"` with internal `"` doubled — valid for both Postgres and SQLite, sid's two
/// SQL engines. Splitting on `.` keeps `schema.table` display names (see
/// [`table_display_name`]) emitting the correct `"schema"."table"` form; a SQLite table
/// whose *name* literally contains a dot mis-splits (already unrepresentable in the
/// display key), but every segment is still quoted, so a hostile name (`x"; DROP …`)
/// can never escape the identifier position — worst case is a syntax error, never a
/// second statement.
fn quote_ident(ident: &str) -> String {
    fn plain(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    if ident.split('.').all(plain) {
        return ident.to_string();
    }
    ident
        .split('.')
        .map(|seg| {
            if plain(seg) {
                seg.to_string()
            } else {
                format!("\"{}\"", seg.replace('"', "\"\""))
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Task 2's `WHERE` filter scaffold — the diagram's column-row click seeds the editor
/// with this, trailing space included, for the user to complete. Identifiers pass
/// through [`quote_ident`]: plain names stay bare (readable editor SQL); anything else
/// is ANSI-quoted so an introspected name can't smuggle SQL into the scaffold.
fn where_filter_scaffold(table: &str, column: &str) -> String {
    format!(
        "SELECT * FROM {} WHERE {} = ",
        quote_ident(table),
        quote_ident(column)
    )
}

// ---- shared chrome ------------------------------------------------------------------

/// The header strip of one left-rail panel: an uppercase section label with its count,
/// and whatever controls the caller adds, right-aligned.
///
/// Not [`sid_ui::Card`]: a card's body is a fixed `v_flex`, and all three panels here need
/// a `flex_1` scrolling list under the header. What the card *does* own — the label's
/// wording and type — comes from `sid_ui::card::header_text` and `section_label`, so the
/// three headers cannot drift apart.
fn panel_header(theme: &Theme, title: &str, count: Option<usize>) -> gpui::Div {
    h_flex()
        .justify_between()
        .gap_1()
        .px_2()
        .py_1()
        .flex_none()
        .hairline_b(theme)
        .child(
            div()
                .section_label(theme)
                .child(sid_ui::card::header_text(title, count)),
        )
}

/// An inline failure notice: the registry's error glyph, then the message.
///
/// The same shape `systems_tab.rs` uses (that copy is file-private, so this is a second
/// spelling of four lines rather than an import — a candidate for `sid-ui` proper).
/// Replaces this tab's three literal `✗` prefixes, each of which was drawn by whatever
/// the text font had at whatever weight.
fn error_line(theme: &Theme, message: String) -> impl IntoElement + use<> {
    h_flex()
        .gap_1p5()
        .py_1()
        .text_xs()
        .text_color(rgb(theme.danger))
        .child(Icon::Error.small())
        .child(message)
}

/// An advisory line: the same shape as [`error_line`] in `muted` rather than `danger`.
///
/// Deliberately not the danger colour. A page-local sort ([`page_view_caveat`]) is not
/// a failure — nothing went wrong and nothing needs fixing — it is a statement about
/// what the numbers on screen mean. Painting it red would train the eye to skip the
/// real errors that share this slot.
fn caveat_line(theme: &Theme, message: &'static str) -> impl IntoElement + use<> {
    h_flex()
        .gap_1p5()
        .py_1()
        .text_xs()
        .text_color(rgb(theme.muted))
        .child(Icon::Info.small())
        .child(message)
}

impl DbTabState {
    /// Build the DB tab state and load its initial connection list for `scope`. A read
    /// failure here is swallowed (matches `AppState::new`'s host-list bootstrap
    /// contract) — `AppState::new` calls `refresh_db` again right after construction
    /// wiring, which surfaces any error through the shared error line.
    pub(crate) fn new(store: &Store, scope: &Scope, filters: ViewFilters) -> Self {
        let mut state = Self {
            registry: Rc::new(DbRegistry::new()),
            connections: Vec::new(),
            active_id: None,
            armed_delete: None,
            form: None,
            _form_subscription: None,
            sql: None,
            _sql_subscription: None,
            results: None,
            client: None,
            client_for: None,
            running: false,
            status: QueryStatus::Idle,
            last_sql: None,
            next_cursor: None,
            result_filter: None,
            _result_filter_sub: None,
            plan: None,
            last_page: None,
            schema: None,
            schema_graph: None,
            schema_loading: false,
            schema_generation: 0,
            query_generation: 0,
            schema_error: None,
            schema_expanded: HashSet::new(),
            diagram: None,
            cell_view: None,
            notice: None,
            export_menu_open: false,
            history: Vec::new(),
            collapsed_folders: HashSet::new(),
            conn_focus: None,
            renaming: None,
            folder_editing: None,
        };
        let _ = state.refresh(store, scope, filters);
        state
    }

    /// Re-query the composed connection list for `scope` + `filters`. Returns an error
    /// message on failure (the caller — `AppState::refresh_db` — owns the shared error
    /// line, so this stays store-agnostic about where the message lands). Any refresh
    /// changes the row set, so a pending delete confirmation is disarmed.
    fn refresh(&mut self, store: &Store, scope: &Scope, filters: ViewFilters) -> Option<String> {
        self.armed_delete = None;
        match store.read_connections(scope, filters) {
            Ok(list) => {
                self.connections = list;
                None
            }
            Err(e) => {
                self.connections = Vec::new();
                Some(e.to_string())
            }
        }
    }
}

impl AppState {
    /// Re-query the DB tab's connection list for the current scope + filters and
    /// surface any error through the shared error line (mirrors `AppState::refresh`).
    pub(crate) fn refresh_db(&mut self) {
        self.error = self.db.refresh(&self.store, &self.scope, self.filters);
    }

    pub(crate) fn db_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_query_widgets(window, cx);

        let t = theme::active(cx).clone();
        // The saved-connection picker lives in the unified left panel (`connection_panel`,
        // built inside `query_pane`, stacked above the schema tree) — DBeaver-style, per
        // Murphy: "connections on the left, like dbeaver" (an earlier pass had put this
        // on a right-edge rail — that was a misread and has been reverted). This top
        // strip is now just the tab's shared error line (still needed: promote/demote/
        // delete/rename/folder-edit failures all land in `self.error`), collapsing to
        // nothing when there is none rather than reserving dead space.
        let error_banner = self
            .error
            .clone()
            .map(|e| div().px_4().hairline_b(&t).child(error_line(&t, e)));

        v_flex()
            .flex_1()
            .children(error_banner)
            .child(self.query_pane(cx))
            .children(self.cell_view_overlay(window, cx))
            .into_any_element()
    }

    /// D2's `view` popover — `None` when nothing is being viewed. Mirrors
    /// `session.rs`'s `preview_overlay` (`anchored`/`deferred` pin a viewport-sized,
    /// occluding backdrop at the window origin, painted above everything else). Built
    /// here — inside the DB tab's own returned tree — rather than composited in
    /// `app.rs` (like `AppState.form`/`AppState.db.form`) so this slice's `app.rs`
    /// footprint stays at zero: `Anchored`'s `position_mode` defaults to `Window`, so
    /// `.position(point(px(0.), px(0.)))` still pins to the window origin regardless of
    /// how deep in the tree this element sits, and `deferred` defers its paint until
    /// after all ancestors — nesting depth doesn't affect the result.
    fn cell_view_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let t = theme::active(cx);
        let (surface, border, fg) = (t.surface, t.border, t.fg);
        let cell = self.db.cell_view.clone()?;
        let viewport = window.viewport_size();

        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(rgba(0x000000a8))
                        .child(
                            div()
                                .w(px(640.))
                                .h(px(400.))
                                .flex()
                                .flex_col()
                                .bg(rgb(surface))
                                .border_1()
                                .border_color(rgb(border))
                                .rounded_md()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .px_3()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(rgb(border))
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(fg))
                                                .child(cell.column.clone()),
                                        )
                                        .child(
                                            IconButton::new(
                                                "db-cell-view-close",
                                                Icon::Close,
                                                "close",
                                            )
                                            .small()
                                            .on_click(
                                                cx.listener(
                                                    |this, _ev: &ClickEvent, _window, cx| {
                                                        this.db.cell_view = None;
                                                        cx.notify();
                                                    },
                                                ),
                                            ),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("db-cell-view-body")
                                        .flex_1()
                                        .overflow_y_scroll()
                                        .p_3()
                                        .text_sm()
                                        .font_family(MONO)
                                        .text_color(rgb(fg))
                                        .child(cell.text.clone()),
                                ),
                        ),
                ),
            )
            .with_priority(1),
        )
    }

    /// Lazily build the SQL editor + results table on first paint of the DB tab.
    /// Idempotent (checked every render) — cheap after the first call. Needs `window`
    /// for `InputState::new`/`TableState::new`, which is why this can't happen in
    /// `DbTabState::new` (constructed before any window exists).
    fn ensure_query_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.db.conn_focus.is_none() {
            self.db.conn_focus = Some(cx.focus_handle());
        }
        if self.db.sql.is_some() {
            return;
        }
        let sql = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql")
                .line_number(true)
                .rows(8)
                .default_value(DEMO_SQL)
        });
        // `subscribe_in` (not `subscribe`) so `on_sql_event` gets a `&mut Window` — it
        // now needs one to open the connect-time password prompt via `run_query`.
        self.db._sql_subscription = Some(cx.subscribe_in(&sql, window, Self::on_sql_event));
        self.db.sql = Some(sql);
        // inc-3: the results filter. `TextInput` has no change callback, so observing
        // the `cx.notify()` it makes on every edit is the wiring — see
        // `network_tab.rs`'s "Filtering" doc section for why this pattern rather than
        // a callback.
        let filter = cx.new(|cx| TextInput::new(cx, "filter results…"));
        self.db._result_filter_sub = Some(cx.observe(&filter, |this: &mut Self, _filter, cx| {
            this.apply_result_filter(cx);
        }));
        self.db.result_filter = Some(filter);
        // D2: hand the results table's delegate a weak handle back to `AppState` so a
        // cell's `view` click (which only sees `&mut App`, not `AppState` — see
        // `ResultDelegate::app`'s doc comment) can open the view popover.
        let app = cx.weak_entity();
        self.db.results = Some(cx.new(|cx| {
            TableState::new(
                ResultDelegate {
                    app: Some(app),
                    ..ResultDelegate::empty()
                },
                window,
                cx,
            )
        }));
    }

    /// Push the results filter box's current text into the grid's delegate.
    ///
    /// No re-query: the filter is a view over the page already fetched (see
    /// [`visible_rows`]). `cx.notify()` alone is enough — only the row *count*
    /// changed, and `TableState::refresh` would additionally discard the measured
    /// column bounds and cost a full-window relayout per keystroke (the cost
    /// `systems_tab`'s `rows_changed` documents).
    fn apply_result_filter(&mut self, cx: &mut Context<Self>) {
        let query = self
            .db
            .result_filter
            .as_ref()
            .map(|f| f.read(cx).content().to_string())
            .unwrap_or_default();
        if let Some(results) = self.db.results.clone() {
            results.update(cx, |state, cx| {
                state.delegate_mut().set_query(&query);
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Ctrl/Cmd-Enter in the SQL editor runs the query. Plain Enter inserts a newline
    /// (handled inside `InputState` itself — multi-line/code-editor mode) and is not
    /// acted on here.
    fn on_sql_event(
        &mut self,
        _sql: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { secondary: true } = event {
            self.run_query(window, cx);
        }
    }

    /// The SQL editor + Run/next-page controls + status line + results table, below the
    /// connection picker. Always rendered; Run/next-page are no-ops (surfaced as a
    /// status message) with no active connection rather than being conditionally absent.
    fn query_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let active_label: SharedString = match &self.db.active_id {
            Some(id) => self
                .db
                .connections
                .iter()
                .find(|a| &a.item.id == id)
                .map(|a| {
                    if a.item.name.is_empty() {
                        a.item.id.clone()
                    } else {
                        a.item.name.clone()
                    }
                })
                .unwrap_or_else(|| id.clone())
                .into(),
            None => "no connection selected".into(),
        };

        // How much of the fetched page the filter is letting through, and whether a
        // sort is active — both live in the delegate, which is the only thing that
        // knows what is actually on screen.
        let (shown, total, is_sorted) = self
            .db
            .results
            .as_ref()
            .map(|t| {
                let delegate = t.read(cx).delegate();
                let (shown, total) = delegate.counts();
                (shown, total, delegate.sort.is_some())
            })
            .unwrap_or((0, 0, false));
        let is_filtered = shown != total;

        // The successful-run summary reads as the toolbar's count (`"340 rows · 12 ms"`,
        // the shape `Toolbar::count_label` exists for); only a *failure* still needs its
        // own line below the editor, where it can be as long as the driver made it.
        // Under a filter the count grows a numerator — `"12 of 340 rows"` — because a
        // grid showing 12 rows over a label saying 340 is a grid that looks broken.
        let (count_label, error_text): (Option<SharedString>, Option<String>) =
            match &self.db.status {
                QueryStatus::Idle => (None, None),
                QueryStatus::Err(e) => (None, Some(e.clone())),
                QueryStatus::Ok { duration_ms, .. } => {
                    // While a plan is up the grid is not on screen, so counting *its*
                    // rows would report on something invisible — and "0 rows" beside a
                    // plan that ran fine reads as a failure. Count the plan instead.
                    let label = match &self.db.plan {
                        Some(plan) => format!(
                            "{} · {duration_ms} ms",
                            sid_ui::toolbar::count_label(plan.lines.len(), "plan line")
                        ),
                        None => {
                            let rows = sid_ui::toolbar::count_label(total, "row");
                            if is_filtered {
                                format!("{shown} of {rows} · {duration_ms} ms")
                            } else {
                                format!("{rows} · {duration_ms} ms")
                            }
                        }
                    };
                    (Some(label.into()), None)
                }
            };
        let has_more = matches!(&self.db.status, QueryStatus::Ok { has_more: true, .. });
        // The documented limitation of a client-side page view, surfaced where the
        // user can act on it rather than only in `visible_rows`'s doc comment.
        let caveat = page_view_caveat(is_sorted, is_filtered, has_more);

        let next_page = has_more.then(|| {
            Button::new("db-next-page", "next page")
                .size(QUERY_ACTION_SIZE)
                .icon(Icon::ChevronRight)
                .tooltip("fetch the next page of this result set")
                .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                    this.next_page(cx);
                }))
        });

        let sql_editor = self.db.sql.clone().map(|sql| {
            div()
                .h(px(140.))
                .rounded_md()
                .elevation(Elevation::Well, &t)
                .child(Input::new(&sql))
        });

        let notice = self.db.notice.clone().map(|n| div().hint_text(&t).child(n));

        let editor_and_results = v_flex()
            .flex_1()
            // A flex child's default minimum is content-sized: without this the results
            // grid pushes the pane wider than the window instead of scrolling inside it.
            .min_w(px(0.))
            .gap_2()
            .child(
                Toolbar::new()
                    // The connection label and the results filter share the toolbar's
                    // left slot: the filter narrows what the count beside it counts,
                    // so the two belong on the same line, and a second toolbar strip
                    // just for a search box would be more chrome than content.
                    .filter(
                        h_flex()
                            .gap_3()
                            .child(div().flex_none().hint_text(&t).child(active_label))
                            .child(
                                // Capped, not filling: a 1200px-wide filter field is
                                // as wrong as the ribbon table it sits above used to
                                // be (`systems_tab`'s toolbar makes the same call).
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .max_w(px(280.))
                                    .children(self.db.result_filter.clone()),
                            ),
                    )
                    .when_some(count_label, |bar, label| bar.count_label(label))
                    .when_some(next_page, |bar, button| bar.action(button))
                    .action(self.explain_button(cx))
                    .action(
                        Button::new("db-run", "Run")
                            .primary()
                            .size(QUERY_ACTION_SIZE)
                            .loading(self.db.running)
                            .tooltip("run the query (Ctrl-Enter)")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                this.run_query(window, cx);
                            })),
                    )
                    // Far right, after Run (Murphy: "download as csv should be on the
                    // far right") — the generic export control (Task 1).
                    .action(self.export_control(cx)),
            )
            .children(sql_editor)
            .children(error_text.map(|e| error_line(&t, e)))
            .children(caveat.map(|c| caveat_line(&t, c)))
            .children(notice)
            .child(self.results_area(cx));

        // Deliberately not `h_flex`: that centres its children on the cross axis, and
        // this row's children must *stretch* to it. A content-height query column gives
        // the results grid nothing to be `flex_1` of, and a zero-height grid paints its
        // header and not one row — the same failure mode `sid_ui::table`'s `TABLE_CHROME`
        // documents, reached from the other direction.
        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .gap_2()
            .p_3()
            .hairline_t(&t)
            .child(self.left_panel(cx))
            .child(editor_and_results)
    }

    /// The engine behind the active selection, or `None` with nothing selected.
    fn active_kind(&self) -> Option<DbKind> {
        let id = self.db.active_id.as_deref()?;
        self.db
            .connections
            .iter()
            .find(|a| a.item.id == id)
            .map(|a| a.item.kind)
    }

    /// What the active engine can tell us about a plan.
    ///
    /// Read off the *factory* (`registry.client(kind)`), not an open client:
    /// [`sid_core::db::DbClient::explain_support`] is a classification that needs no
    /// connection, which is the whole reason it exists — the button has to know
    /// whether to enable itself before anything is dialled.
    fn explain_support(&self) -> Option<sid_core::db::ExplainSupport> {
        self.active_kind()
            .map(|kind| self.db.registry.client(kind).explain_support())
    }

    /// The "Explain" control.
    ///
    /// Present in every state, never hidden — a control that appears and disappears
    /// with the selected engine teaches nothing. When the engine cannot explain, the
    /// button is disabled and its tooltip carries the engine's **own** reason
    /// verbatim ("the sid store browser reads a table by name — there is no query to
    /// plan"), so "why is this greyed out" has an answer on hover instead of in the
    /// source. `Button::disabled` installs no click handler at all, so inert is
    /// structural here rather than a guard inside the listener.
    fn explain_button(&self, cx: &mut Context<Self>) -> Button {
        let support = self.explain_support();
        let (disabled, tooltip): (bool, SharedString) = match support {
            None => (true, "select a connection first".into()),
            Some(s) => match (s.keyword(), s.reason()) {
                (Some(keyword), _) => (false, format!("show the query plan ({keyword})").into()),
                (None, Some(reason)) => (true, reason.into()),
                // Unreachable: `ExplainSupport` has exactly one of the two, and
                // `support_and_keyword_and_reason_agree` pins that for every engine.
                (None, None) => (true, "no query plan available".into()),
            },
        };
        Button::new("db-explain", "Explain")
            .size(QUERY_ACTION_SIZE)
            .icon(Icon::Info)
            .disabled(disabled || self.db.running)
            .tooltip(tooltip)
            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                this.explain_query(cx);
            }))
    }

    /// The plan pane — what [`Self::results_area`] shows instead of the grid while a
    /// plan is up.
    ///
    /// Monospace, one line per plan row, in a scrolling well. Monospace and
    /// *unprocessed*: a Postgres plan's leading spaces are its tree structure (see
    /// [`plan_lines`]), and a proportional face would misalign the columns SQLite's
    /// `EXPLAIN QUERY PLAN` puts in its `detail` strings.
    fn plan_pane(&self, plan: &PlanView, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx).clone();
        let header = h_flex()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .flex_none()
            .hairline_b(&t)
            .child(
                div()
                    .section_label(&t)
                    .child(sid_ui::card::header_text("QUERY PLAN", None)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().hint_text(&t).child(plan.keyword.clone()))
                    .child(
                        IconButton::new("db-plan-close", Icon::Close, "back to the results")
                            .small()
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.db.plan = None;
                                cx.notify();
                            })),
                    ),
            );

        let body: AnyElement = if plan.lines.is_empty() {
            div()
                .p_3()
                .hint_text(&t)
                .child("the engine returned an empty plan for this statement")
                .into_any_element()
        } else {
            v_flex()
                .id("db-plan-body")
                .flex_1()
                .min_h(px(0.))
                .overflow_scroll()
                .p_3()
                .gap_0p5()
                .children(plan.lines.iter().map(|line| {
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_mono(&t)
                        .text_color(rgb(t.fg))
                        .child(line.clone())
                }))
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .rounded_md()
            .elevation(Elevation::Well, &t)
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The results grid, or — with no connection chosen yet — the tab's empty state.
    ///
    /// The grid is a [`FillTable`]: its columns are resized to the width this pane
    /// actually got, so a 2000px window shows 2000px of data instead of a 140px-per-column
    /// ribbon with the rest of the screen black (`sid_ui::table`).
    fn results_area(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // A plan takes the whole area while it is up. The grid keeps its page, sort
        // and filter underneath, so closing the plan is a genuine "back", not a
        // re-render from scratch.
        if let Some(plan) = self.db.plan.take() {
            let element = self.plan_pane(&plan, cx);
            self.db.plan = Some(plan);
            return element;
        }
        if self.db.active_id.is_none() {
            let empty = if self.db.connections.is_empty() {
                EmptyState::new("no database connections yet")
                    .guidance("add one to browse its schema, run queries and export the results")
                    .icon(Icon::Dashboard)
                    .action(
                        Button::new("db-empty-add", "add a connection")
                            .primary()
                            .icon(Icon::Add)
                            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                this.open_add_db_form(window, cx);
                            })),
                    )
            } else {
                EmptyState::new("no connection selected")
                    .guidance("pick a connection on the left to load its schema and run queries")
                    .icon(Icon::Dashboard)
                    .action(
                        Button::new("db-empty-add", "add a connection")
                            .icon(Icon::Add)
                            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                this.open_add_db_form(window, cx);
                            })),
                    )
            };
            return div()
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .child(empty)
                .into_any_element();
        }
        // Selected, but nothing run yet. The grid at this point is a header strip with no
        // columns over the library's own generic "no data" illustration — which says
        // nothing about *why* it is empty. One line of guidance does; the Run button it
        // points at is already on the toolbar above.
        if self.db.last_page.is_none() && !self.db.running {
            return div()
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .child(
                    EmptyState::new("no results yet")
                        .guidance(
                            "write a query above and Run it, or click a table in the schema tree",
                        )
                        .icon(Icon::Terminal),
                )
                .into_any_element();
        }
        match self.db.results.clone() {
            Some(table) => div()
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .child(FillTable::new(&table).stripe(true))
                .into_any_element(),
            None => div().flex_1().into_any_element(),
        }
    }

    /// The "⭳ Export ▾" control: a button that toggles [`DbTabState::export_menu_open`],
    /// plus (when open) a small dropdown listing [`ExportFormat::ALL`]. Reuses the
    /// `anchored`/`deferred` primitives [`Self::cell_view_overlay`] is built from (see
    /// that method's doc comment) so the menu paints above the editor/results below it
    /// in the tab's child order, rather than being clipped by them — but anchors at the
    /// button's own flow position (`Corner::TopRight`, no explicit `.position()`) instead
    /// of a window-pinned point, since this is a small trigger-attached menu, not a
    /// full-viewport modal.
    fn export_control(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let button = Button::new("db-export-open", "Export")
            .size(QUERY_ACTION_SIZE)
            .tooltip("export the results now on screen")
            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                this.db.export_menu_open = !this.db.export_menu_open;
                cx.notify();
            }));

        let menu = self.db.export_menu_open.then(|| {
            deferred(
                anchored()
                    .anchor(Corner::TopRight)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        v_flex()
                            .id("db-export-menu")
                            .occlude()
                            .mt_1()
                            .min_w(px(180.))
                            .rounded_md()
                            .elevation(Elevation::Surface, &t)
                            .p_1()
                            .children(ExportFormat::ALL.iter().enumerate().map(|(ix, fmt)| {
                                let fmt = *fmt;
                                UiRow::new(("db-export-item", ix))
                                    .child(div().text_xs().child(fmt.label()))
                                    .on_click(cx.listener(
                                        move |this, _ev: &ClickEvent, _window, cx| {
                                            this.export(fmt, cx);
                                        },
                                    ))
                            })),
                    ),
            )
            .with_priority(1)
        });

        div().relative().child(button).children(menu)
    }

    /// Run `format`'s export and close the menu — the one call site every export
    /// format's action routes through. A new format is one [`ExportFormat`] variant plus
    /// one arm here.
    fn export(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        self.db.export_menu_open = false;
        match format {
            ExportFormat::Csv => self.export_csv(cx),
        }
    }

    /// The unified LEFT panel (DBeaver-style, Murphy: "connections on the left, like
    /// dbeaver"): the saved-connections list ([`Self::connection_panel`]) on top, the
    /// active connection's schema tree ([`Self::schema_tree_panel`], claiming most of
    /// the remaining vertical space) below it, and D4's fixed-height query-history panel
    /// at the bottom. One column, three stacked sections — a second side-by-side column
    /// would crowd the tab.
    fn left_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        v_flex()
            // 280 rather than the old 260: a connection row now carries a status dot and
            // an origin chip beside its name, and the DSN subtitle under it was already
            // the first thing to truncate. The 20px comes out of a query pane that has
            // ~1700px at the capture width.
            .w(px(280.))
            .flex_none()
            .h_full()
            .gap_2()
            .child(self.connection_panel(cx))
            .child(self.schema_tree_panel(cx))
            .child(self.history_panel(cx))
    }

    /// D1: the schema tree panel — a `⟳` refresh header over a flat, indented list of
    /// tables (click name -> insert `SELECT * FROM <table>`; click chevron -> expand to
    /// show columns). Pure-from-cache: reads `self.db.schema`/`schema_expanded` only,
    /// never touches the runtime itself (that's `refresh_schema`'s job).
    fn schema_tree_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let table_count = self.db.schema.as_ref().map(|s| s.tables.len());
        let header = panel_header(&t, "SCHEMA", table_count)
            .child(self.diagram_button(cx))
            .child(
                IconButton::new("db-schema-refresh", Icon::Refresh, "reload the schema")
                    .small()
                    .loading(self.db.schema_loading)
                    .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                        this.refresh_schema(window, cx);
                    })),
            );

        let body: AnyElement = if self.db.schema_loading && self.db.schema.is_none() {
            div()
                .p_2()
                .hint_text(&t)
                .child("loading schema…")
                .into_any_element()
        } else if let Some(err) = &self.db.schema_error {
            div()
                .px_2()
                .child(error_line(&t, err.clone()))
                .into_any_element()
        } else {
            let rows = match &self.db.schema {
                Some(schema) => schema_tree_rows(schema, &self.db.schema_expanded),
                None => Vec::new(),
            };
            if rows.is_empty() {
                div()
                    .p_2()
                    .hint_text(&t)
                    .child("no schema loaded — select a connection")
                    .into_any_element()
            } else {
                List::scrolling("db-schema-tree-body")
                    .px_1()
                    .children(
                        rows.into_iter()
                            .enumerate()
                            .map(|(ix, row)| self.schema_tree_row(ix, row, cx)),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .flex_1()
            .min_h(px(0.))
            .rounded_md()
            .elevation(Elevation::Well, &t)
            .child(header)
            .child(body)
    }

    /// "diagram" — opens the Access-style relationships pop-out window (see
    /// [`Self::open_diagram_window`]). Enabled (brand-colored, clickable) only once a
    /// schema is cached for the active connection; otherwise rendered dim and inert
    /// rather than hidden, matching this tab's convention of always-present, sometimes
    /// no-op controls (see `query_pane`'s doc comment on Run/next-page).
    fn diagram_button(&self, cx: &mut Context<Self>) -> Button {
        // `Button::disabled` installs no click handler at all, so "inert until a schema
        // is cached" is structural here rather than an `if` around the listener.
        Button::new("db-diagram-open", "diagram")
            .small()
            .ghost()
            .disabled(self.db.schema.is_none())
            .tooltip("open the relationships diagram in its own window")
            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                this.open_diagram_window(window, cx);
            }))
    }

    /// Open the relationships diagram in its own OS window — a snapshot of the cached
    /// [`SchemaInfo`] + [`SchemaGraph`] handed to a fresh [`DiagramView`] entity.
    /// Synchronous: sid is a single [`gpui::Application`] and `Context` derefs to
    /// `App`, so `cx.open_window` opens a second top-level window in the same process
    /// (no second instance, no subprocess) right here in the click handler. Cribs the
    /// window-bootstrap shape from `main.rs` exactly — `Root::new` must be the window's
    /// first layer and the theme bridge must run before anything paints, or
    /// gpui-component's widgets panic reaching for a `Root` ancestor. A snapshot means
    /// the pop-out goes stale if the schema changes later; re-opening it re-reads
    /// whatever is cached then (acceptable for v1 — noted in the module's plan).
    ///
    /// Also hands the new [`DiagramView`] a [`WeakEntity`] back to this `AppState` and an
    /// [`gpui::AnyWindowHandle`] for *this* (the main) window — the diagram's click-
    /// through (Task 2: click a table/column to jump back to the main SQL editor) needs
    /// both. Entities are app-global in GPUI, so `weak.update(cx, ...)` reaches this
    /// `AppState` from the diagram window's own `Context` with no extra plumbing; the
    /// window handle is only needed because the SQL `InputState`'s mutators
    /// (`set_value`/`set_cursor_position`) take a `&mut Window` and use it for
    /// window-scoped bookkeeping (focus, cursor blink) — handing them the *diagram*
    /// window's `Window` there would register that bookkeeping against the wrong OS
    /// window. `AnyWindowHandle::update` (see [`DiagramView::navigate_to_table`]) resolves
    /// that by handing back the *main* window's real `Window` when the click fires.
    ///
    /// The snapshot is no longer one-shot: the new view's handle is kept in
    /// `DbTabState::diagram` so [`Self::push_schema_to_diagram`] can hand it a fresh one
    /// when a schema fetch completes — which is what makes the pop-out's own ⟳ (and the
    /// main panel's) update it in place.
    fn open_diagram_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(schema) = self.db.schema.clone() else {
            return;
        };
        let graph = self.db.schema_graph.clone().unwrap_or_default();
        let connection_id = self.db.active_id.clone();
        let connection_label = self
            .db
            .active_id
            .as_deref()
            .and_then(|id| self.db.connections.iter().find(|a| a.item.id == id))
            .map(|a| {
                if a.item.name.is_empty() {
                    a.item.id.clone()
                } else {
                    a.item.name.clone()
                }
            })
            .unwrap_or_else(|| "connection".to_string());
        let title = format!("sid — relationships · {connection_label}");
        let main_window = window.window_handle();
        let app = cx.entity().downgrade();

        // Built here rather than inside the window closure so the handle can be kept:
        // gpui entities are app-global, not window-owned, and the window's `Root` still
        // takes the only strong reference a moment later.
        let view = cx.new(|_cx| DiagramView::new(connection_id, schema, graph, app, main_window));
        self.db.diagram = Some(view.downgrade());

        let bounds = Bounds::centered(None, size(px(1000.), px(700.)), cx);
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                // Same app_id as the main window (see main.rs) so window tooling
                // groups the diagram pop-out with sid.
                app_id: Some("sid".into()),
                ..Default::default()
            },
            move |window, cx| {
                // Same sync as main.rs's startup window — the sid `Theme` global is
                // process-wide, so this pop-out follows whatever the user has active.
                sid_ui::bridge::sync(Some(window), cx);
                cx.new(|cx| Root::new(view, window, cx))
            },
        );
    }

    /// One [`SchemaRow`]'s rendering — a table header (chevron toggles expand, name
    /// inserts `SELECT * FROM <table>`) or an indented column leaf.
    fn schema_tree_row(&self, ix: usize, row: SchemaRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx).clone();
        match row {
            SchemaRow::Table {
                display_name,
                expanded,
            } => {
                let chevron_name = display_name.clone();
                let insert_name = display_name.clone();
                let (chevron, hint) = match expanded {
                    true => (Icon::ChevronDown, "hide this table's columns"),
                    false => (Icon::ChevronRight, "show this table's columns"),
                };
                UiRow::new(("db-schema-table", ix))
                    // Tree rows are the densest list in the app — three panels share one
                    // 280px column — so they take the row language at the tighter of the
                    // system's two vertical rhythms.
                    .py_0p5()
                    .px_1()
                    .leading(
                        IconButton::new(("db-schema-toggle", ix), chevron, hint)
                            .small()
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                                this.toggle_schema_table(&chevron_name, cx);
                            })),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(t.fg))
                            .child(display_name),
                    )
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                        this.insert_select_star(&insert_name, window, cx);
                    }))
                    .into_any_element()
            }
            // Inert by design: a column leaf has no behaviour, and `Row` paints no hover
            // on a row that does nothing (`sid_ui::list`'s "an inert row promises
            // nothing").
            // Indented past the table name above it, not merely past the chevron: at the
            // same x the two levels read as one flat list.
            SchemaRow::Column { name } => UiRow::new(("db-schema-col", ix))
                .py_0p5()
                .pl_12()
                .pr_2()
                .child(div().truncate().hint_text(&t).child(name))
                .into_any_element(),
        }
    }

    /// D4: the query-history panel — most-recent-first, click an entry to reload it
    /// (unmodified) into the SQL editor.
    fn history_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let entries = self.db.history.clone();
        let header = panel_header(&t, "HISTORY", Some(entries.len()));

        let body: AnyElement = if entries.is_empty() {
            div()
                .p_2()
                .hint_text(&t)
                .child("no queries run yet")
                .into_any_element()
        } else {
            List::scrolling("db-history-body")
                .px_1()
                .children(entries.into_iter().enumerate().map(|(ix, sql)| {
                    let full = sql.clone();
                    let label: SharedString = if sql.chars().count() > 34 {
                        let head: String = sql.chars().take(34).collect();
                        format!("{head}…").into()
                    } else {
                        sql.clone().into()
                    };
                    UiRow::new(("db-history", ix))
                        .py_0p5()
                        .px_2()
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(t.fg))
                                .child(label),
                        )
                        .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                            this.reload_history_entry(&full, window, cx);
                        }))
                }))
                .into_any_element()
        };

        v_flex()
            .h(px(160.))
            .flex_none()
            .rounded_md()
            .elevation(Elevation::Well, &t)
            .child(header)
            .child(body)
    }

    /// The DBeaver-style connections list — the top section of the unified left panel
    /// ([`Self::left_panel`]), stacked directly above the active connection's schema
    /// tree (Murphy: "connections on the left, like dbeaver"; an earlier pass had this
    /// on a right-edge rail — reverted). Groups the composed connection list by
    /// [`DbConnection::folder`] via [`group_connections`] under a small
    /// `connections · N` / `+` header. Also the F2 target: focused on every row click
    /// (see [`Self::render_connection_row`]) so F2 with no text field focused reaches
    /// [`Self::begin_rename_active`] — the double-click-a-name path (also wired in
    /// `render_connection_row`) needs no focus of its own.
    fn connection_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let count = self.db.connections.len();
        let header = panel_header(&t, "CONNECTIONS", Some(count)).child(
            IconButton::new("db-conn-add", Icon::Add, "add a connection")
                .small()
                .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                    this.open_add_db_form(window, cx);
                })),
        );

        let rows = group_connections(&self.db.connections, &self.db.collapsed_folders);
        let body: AnyElement = if rows.is_empty() {
            div()
                .p_2()
                .hint_text(&t)
                .child("no connections yet")
                .into_any_element()
        } else {
            List::scrolling("db-conn-body")
                .px_1()
                .children(
                    rows.into_iter()
                        .enumerate()
                        .map(|(ix, row)| self.connection_panel_row(ix, row, cx)),
                )
                .into_any_element()
        };

        let focus_handle = self.db.conn_focus.clone();
        v_flex()
            .id("db-conn-panel")
            .w_full()
            .h(px(240.))
            .flex_none()
            .rounded_md()
            .elevation(Elevation::Well, &t)
            .when_some(focus_handle, |el, fh| {
                el.track_focus(&fh).on_key_down(cx.listener(
                    |this, ev: &KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "f2" {
                            this.begin_rename_active(window, cx);
                        }
                    },
                ))
            })
            .child(header)
            .child(body)
    }

    /// One [`ConnRow`]'s rendering: a folder header (click toggles collapse) or a
    /// connection looked up by id. A stale id (deleted mid-render, between
    /// `group_connections` snapshotting the list and this call) renders nothing —
    /// `refresh_db` drops it from the row list on the very next paint.
    fn connection_panel_row(&self, ix: usize, row: ConnRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx).clone();
        match row {
            ConnRow::Folder {
                name,
                expanded,
                count,
            } => {
                let toggle_name = name.clone();
                let chevron = match expanded {
                    true => Icon::ChevronDown,
                    false => Icon::ChevronRight,
                };
                UiRow::new(("db-folder", ix))
                    .py_1()
                    .px_2()
                    .leading(chevron.small().text_color(rgb(t.muted)))
                    .child(div().truncate().hint_text(&t).child(name))
                    .meta(Badge::count(count))
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.toggle_conn_folder(&toggle_name, cx);
                    }))
                    .into_any_element()
            }
            ConnRow::Connection { id } => {
                match self.db.connections.iter().find(|a| a.item.id == id) {
                    Some(a) => self.render_connection_row(ix, a, cx),
                    None => div().into_any_element(),
                }
            }
        }
    }

    /// One connection's row in the panel: a state dot, its name (a live rename
    /// [`TextInput`] in place, mid-rename) with its origin chip, a DSN subtitle (a live
    /// folder-edit [`TextInput`] in place, mid-folder-edit), and the
    /// promote/demote/edit/folder/delete action strip.
    ///
    /// # Why the actions are a third line rather than `Row`'s action slot
    ///
    /// [`UiRow`]'s slot order — mark, content, metadata, engagement — assumes a row as
    /// wide as a table. This one lives in a 280px rail: a dot, a name, an origin chip and
    /// five 24px controls on one line leaves the name about 20px, and the DSN under it
    /// less. So the row keeps `Row`'s box, hover, selection and click, and stacks
    /// name / DSN / actions inside its content slot. The alternative — moving four of the
    /// five actions into a right-click menu — is a real design, but it needs the
    /// container-owned context menu `sid_ui::list` documents, and that is a different
    /// commit.
    fn render_connection_row(
        &self,
        ix: usize,
        a: &Attributed<DbConnection>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme::active(cx).clone();
        let conn = a.item.clone();
        let display_name: SharedString = if conn.name.is_empty() {
            conn.id.clone().into()
        } else {
            conn.name.clone().into()
        };
        let subtitle: SharedString = format!("{} · {}", conn.kind.label(), conn.dsn).into();
        let is_active = self.db.active_id.as_deref() == Some(conn.id.as_str());
        let click_id = conn.id.clone();
        let origin = a.origin.clone();
        let armed = delete_click_executes(
            self.db.armed_delete.as_ref(),
            &(conn.id.clone(), origin.clone()),
        );
        let state = connection_dot(
            self.db.client.is_some() && self.db.client_for.as_deref() == Some(conn.id.as_str()),
            is_active,
            self.db.running || self.db.schema_loading,
            self.db.schema_error.is_some(),
        );

        // Promote: workspace-origin rows only.
        let promote = can_promote(&origin).then(|| {
            let id = conn.id.clone();
            let origin = origin.clone();
            IconButton::new(
                ("db-promote", ix),
                Icon::ChevronUp,
                "promote this connection to the global store",
            )
            .small()
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                this.promote_db_row(&id, &origin, cx);
            }))
        });

        // Demote: global-origin rows while a workspace scope is active.
        let demote = can_demote(&origin, &self.scope).then(|| {
            let id = conn.id.clone();
            IconButton::new(
                ("db-demote", ix),
                Icon::ChevronDown,
                "demote this connection into the active workspace",
            )
            .small()
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                this.demote_db_row(&id, cx);
            }))
        });

        // Edit: opens the form prefilled with this row's record.
        let edit = {
            let conn = conn.clone();
            let origin = origin.clone();
            IconButton::new(("db-edit", ix), Icon::Rename, "edit this connection")
                .small()
                .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                    this.open_edit_db_form(conn.clone(), origin.clone(), window, cx);
                }))
        };

        // Folder: opens the minimal inline folder-assignment editor (Task 2's "row
        // hover-menu → small input" — see `Self::begin_folder_edit`).
        let folder_btn = {
            let id = conn.id.clone();
            let origin = origin.clone();
            let current = conn.folder.clone();
            IconButton::new(
                ("db-folder-edit", ix),
                Icon::Folder,
                "put this row in a folder",
            )
            .small()
            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                this.begin_folder_edit(&id, &origin, current.as_deref(), window, cx);
            }))
        };

        // Delete: two-click confirm — the first click arms this row, the second deletes
        // from the row's origin layer (and its secret from the keyring). `ConfirmButton`
        // renders the arm; the arm itself stays `DbTabState::armed_delete`, which is keyed
        // by (id, origin) — `ConfirmArm` requires a `Copy` key and this one is not.
        let delete = {
            let id = conn.id.clone();
            let origin = origin.clone();
            let secret_ref = conn.secret_ref.clone();
            ConfirmButton::new(("db-delete", ix), "delete")
                .armed(armed)
                .icon(Icon::Trash)
                .tooltip("delete this connection and its stored secret")
                .on_press(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                    let key = (id.clone(), origin.clone());
                    if delete_click_executes(this.db.armed_delete.as_ref(), &key) {
                        this.delete_db_row(&id, &origin, secret_ref.as_deref(), cx);
                    } else {
                        this.db.armed_delete = Some(key);
                        cx.notify();
                    }
                }))
        };

        // Name area — the live rename `TextInput` in place of the label while this row
        // is mid-rename (Enter commits, Escape cancels — bound directly on the wrapper
        // since `TextInput` itself claims neither key, same technique
        // `DbConnForm::handle_key_down` uses for Tab); otherwise the plain
        // double-click-armed label.
        let is_renaming = self.db.renaming.as_ref().is_some_and(|r| r.id == conn.id);
        let name_area: AnyElement = if is_renaming {
            let input = self.db.renaming.as_ref().unwrap().input.clone();
            div()
                .id(("db-conn-rename", ix))
                .flex_1()
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                    match ev.keystroke.key.as_str() {
                        "enter" => {
                            cx.stop_propagation();
                            this.commit_rename(cx);
                        }
                        "escape" => {
                            cx.stop_propagation();
                            this.cancel_rename(cx);
                        }
                        _ => {}
                    }
                }))
                .child(input)
                .into_any_element()
        } else {
            let name_id = conn.id.clone();
            let name_origin = origin.clone();
            let name_text = display_name.clone();
            div()
                .id(("db-conn-name", ix))
                .truncate()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(if is_active { t.fg_strong } else { t.fg }))
                .child(display_name.clone())
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    // Double-click (VS Code convention) starts the inline rename; a
                    // single click here also fires the row's own `on_click` below
                    // (selecting it) — harmless, same convention the action strip uses.
                    if ev.click_count() >= 2 {
                        this.begin_rename(&name_id, &name_origin, &name_text, window, cx);
                    }
                }))
                .into_any_element()
        };

        // Subtitle area — the folder-edit `TextInput` in place of the DSN subtitle
        // while this row is mid-folder-edit; otherwise the plain subtitle.
        let is_folder_editing = self
            .db
            .folder_editing
            .as_ref()
            .is_some_and(|f| f.id == conn.id);
        let subtitle_area: AnyElement = if is_folder_editing {
            let input = self.db.folder_editing.as_ref().unwrap().input.clone();
            div()
                .id(("db-conn-folder-input", ix))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                    match ev.keystroke.key.as_str() {
                        "enter" => {
                            cx.stop_propagation();
                            this.commit_folder_edit(cx);
                        }
                        "escape" => {
                            cx.stop_propagation();
                            this.cancel_folder_edit(cx);
                        }
                        _ => {}
                    }
                }))
                .child(input)
                .into_any_element()
        } else {
            div()
                .truncate()
                .font_family(MONO)
                .hint_text(&t)
                .child(subtitle)
                .into_any_element()
        };

        UiRow::new(("db-conn", ix))
            .selected(is_active)
            .py_1p5()
            .px_2()
            .leading(StatusDot::new(("db-conn-dot", ix), state))
            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                if this.db.active_id.as_deref() != Some(click_id.as_str()) {
                    // Switching connections: drop the previous connection's schema
                    // immediately (rather than leaving it up until the new fetch
                    // resolves) so the tree never shows a stale, wrong-connection
                    // schema mid-load — D1's "on connect" trigger.
                    this.db.schema = None;
                    this.db.schema_graph = None;
                    this.db.schema_error = None;
                    this.db.schema_expanded.clear();
                    // ...and invalidate everything still in flight for the previous
                    // connection: bumping both generations makes any pending
                    // fetch_schema/run_query completion a guarded no-op, so its result
                    // can never land under this newly-selected connection (bug-hunt
                    // round D, HIGH). The query pane resets with it — results, status,
                    // paging cursor and export cache all belonged to the old
                    // connection.
                    this.db.schema_generation += 1;
                    this.db.query_generation += 1;
                    this.db.schema_loading = false;
                    this.db.running = false;
                    this.db.status = QueryStatus::Idle;
                    this.db.last_sql = None;
                    this.db.next_cursor = None;
                    this.db.last_page = None;
                    // A plan describes one engine's execution of one statement — it is
                    // as connection-scoped as the rows beside it.
                    this.db.plan = None;
                    if let Some(results) = this.db.results.clone() {
                        results.update(cx, |state, cx| {
                            state.delegate_mut().set_page(QueryPage {
                                columns: Vec::new(),
                                rows: Vec::new(),
                                next_cursor: None,
                                duration_ms: 0,
                            });
                            state.refresh(cx);
                            cx.notify();
                        });
                    }
                }
                this.db.active_id = Some(click_id.clone());
                // Selecting a row is also this panel's one focus entry point — F2
                // afterwards renames whatever just got selected (`begin_rename_active`).
                // But a nested control's own click fires *before* this row-level one and
                // bubbles up to here: the name's double-click (`begin_rename`), the folder
                // button (`begin_folder_edit`), and the ✎ button (`open_edit_db_form`)
                // each grab focus for their freshly-opened input/form — so only claim
                // panel focus when none of those started, or this handler would steal it
                // straight back and the inline editors would open unfocused.
                let opening_editor = this.db.renaming.is_some()
                    || this.db.folder_editing.is_some()
                    || this.db.form.is_some();
                if !opening_editor && let Some(fh) = this.db.conn_focus.clone() {
                    window.focus(&fh);
                }
                this.refresh_schema(window, cx);
                cx.notify();
            }))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(name_area))
                            .child(self.db_scope_chip(a)),
                    )
                    .child(subtitle_area)
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_1()
                            .children(promote)
                            .children(demote)
                            .child(folder_btn)
                            .child(edit)
                            .child(delete),
                    ),
            )
            .into_any_element()
    }

    /// Folder-header click (folders/grouping) — flip `name` between collapsed/expanded
    /// in the connections panel. Presence-in-`collapsed_folders` encodes "collapsed"
    /// (see that field's doc comment).
    fn toggle_conn_folder(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.db.collapsed_folders.remove(name) {
            self.db.collapsed_folders.insert(name.to_string());
        }
        cx.notify();
    }

    /// The origin chip for a connection's layer.
    ///
    /// Replaces this tab's hand-rolled `db_origin_badge`, whose workspace rows were
    /// coloured `success` — green for "this lives in a workspace", which the design system
    /// reads as a *state*, not an origin. [`ScopeChip`] separates the two layers by weight
    /// instead of hue (`sid_ui::scope_chip`), and carries the `· dup` mark the lossless
    /// store needs.
    fn db_scope_chip(&self, a: &Attributed<DbConnection>) -> ScopeChip {
        let chip = match &a.origin {
            Scope::Global => ScopeChip::global(),
            Scope::Workspace(id) => ScopeChip::workspace(
                self.scopes
                    .iter()
                    .find(|c| matches!(&c.scope, Scope::Workspace(w) if w == id))
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| "workspace".into()),
            ),
        };
        chip.duplicate(a.duplicate)
    }

    // ---- connections panel: inline rename / folder edit (Tasks 2-3) -------------------

    /// F2 (panel focused, see [`Self::connection_panel`]) — rename whichever connection
    /// is currently `active_id`. A no-op with nothing selected or the row since gone
    /// (rather than an error) — F2 with no selection is a plausible fumble, not a
    /// mistake worth surfacing.
    fn begin_rename_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.db.active_id.clone() else {
            return;
        };
        let Some(a) = self
            .db
            .connections
            .iter()
            .find(|a| a.item.id == id)
            .cloned()
        else {
            return;
        };
        self.begin_rename(&a.item.id, &a.origin, &a.item.name, window, cx);
    }

    /// Enter VS Code-style inline rename for connection `id`/`origin`, seeded with
    /// `current_name` (or `id` if the display name is empty — matches how the row
    /// itself falls back). Replaces any rename/folder-edit already in progress — only
    /// one inline edit is live at a time.
    fn begin_rename(
        &mut self,
        id: &str,
        origin: &Scope,
        current_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.db.folder_editing = None;
        let seed = if current_name.is_empty() {
            id.to_string()
        } else {
            current_name.to_string()
        };
        let input = cx.new(|cx| {
            let mut t = TextInput::new(cx, "name");
            t.set_content(seed, cx);
            t
        });
        input.read(cx).focus(window);
        self.db.renaming = Some(RenameState {
            id: id.to_string(),
            origin: origin.clone(),
            input,
        });
        cx.notify();
    }

    /// Enter commits the in-progress rename via [`Store::rename_connection`]. An empty
    /// (post-trim) name stays in rename mode with an error rather than silently
    /// reverting — the user's edit isn't lost.
    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.db.renaming else {
            return;
        };
        let new_name = state.input.read(cx).content().trim().to_string();
        if new_name.is_empty() {
            self.error = Some("name must not be empty".to_string());
            cx.notify();
            return;
        }
        let RenameState { id, origin, .. } = self.db.renaming.take().expect("checked above");
        match self.store.rename_connection(&origin, &id, &new_name) {
            Ok(()) => self.refresh_db(),
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// Escape discards the in-progress rename, leaving the stored name untouched.
    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.db.renaming = None;
        cx.notify();
    }

    /// folder (folders/grouping) — enter the minimal inline folder-assignment editor for
    /// connection `id`/`origin`, seeded with its `current` folder (blank when
    /// ungrouped). Replaces any rename/folder-edit already in progress.
    fn begin_folder_edit(
        &mut self,
        id: &str,
        origin: &Scope,
        current: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.db.renaming = None;
        let input = cx.new(|cx| {
            let mut t = TextInput::new(cx, "folder (blank = none)");
            if let Some(f) = current {
                t.set_content(f.to_string(), cx);
            }
            t
        });
        input.read(cx).focus(window);
        self.db.folder_editing = Some(FolderEditState {
            id: id.to_string(),
            origin: origin.clone(),
            input,
        });
        cx.notify();
    }

    /// Enter commits the in-progress folder edit via [`Store::set_connection_folder`] —
    /// a blank (post-trim) value clears the folder, moving the connection back to the
    /// panel's ungrouped top level.
    fn commit_folder_edit(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.db.folder_editing else {
            return;
        };
        let raw = state.input.read(cx).content().trim().to_string();
        let folder = (!raw.is_empty()).then_some(raw);
        let FolderEditState { id, origin, .. } =
            self.db.folder_editing.take().expect("checked above");
        match self.store.set_connection_folder(&origin, &id, folder) {
            Ok(()) => self.refresh_db(),
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// Escape discards the in-progress folder edit, leaving the stored folder untouched.
    fn cancel_folder_edit(&mut self, cx: &mut Context<Self>) {
        self.db.folder_editing = None;
        cx.notify();
    }

    // ---- add/edit form (W4) ----------------------------------------------------------

    /// Open the empty add form, preselecting `save to:` from the persisted
    /// [`sid_store::Settings::default_scope`]. Mirrors `AppState::open_add_form`.
    pub(crate) fn open_add_db_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_scope = self
            .store
            .settings()
            .map(|s| s.default_scope)
            .unwrap_or_default();
        let workspace = self.active_workspace();
        let registry = self.db.registry.clone();
        let degraded = self.secrets_degraded;
        let form =
            cx.new(|cx| DbConnForm::new_add(cx, registry, workspace, default_scope, degraded));
        self.open_db_form(form, window, cx);
    }

    /// ✎ Open the edit form prefilled with `conn`, writing back into `origin` on save.
    pub(crate) fn open_edit_db_form(
        &mut self,
        conn: DbConnection,
        origin: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.db.armed_delete = None;
        let workspace = self.active_workspace();
        let registry = self.db.registry.clone();
        let degraded = self.secrets_degraded;
        let form =
            cx.new(|cx| DbConnForm::new_edit(cx, registry, conn, origin, workspace, degraded));
        self.open_db_form(form, window, cx);
    }

    fn open_db_form(
        &mut self,
        form: Entity<DbConnForm>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        form.read(cx).focus_first(window, cx);
        // `subscribe_in` (not `subscribe`) so the handler gets a `&mut Window` and can
        // refocus `root_focus` on close — otherwise closing the form (Escape/Cancel)
        // leaves keyboard focus on a now-unrendered element and silently kills all key
        // dispatch until the next mouse click (same class of bug the host form fixed).
        self.db._form_subscription = Some(cx.subscribe_in(&form, window, Self::on_db_form_event));
        self.db.form = Some(form);
        cx.notify();
    }

    pub(crate) fn close_db_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.db.form = None;
        self.db._form_subscription = None;
        window.focus(&self.root_focus);
        cx.notify();
    }

    fn on_db_form_event(
        &mut self,
        form: &Entity<DbConnForm>,
        event: &DbConnFormEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DbConnFormEvent::Cancel => self.close_db_form(window, cx),
            DbConnFormEvent::Submit(submission) => match self.perform_db_submit(submission) {
                Ok(post_warning) => {
                    self.close_db_form(window, cx);
                    self.refresh_db();
                    if post_warning.is_some() {
                        self.error = post_warning;
                    }
                    cx.notify();
                }
                // Guard/secret/store failures land in the form's error line; the form
                // stays open so nothing typed is lost.
                Err(msg) => form.update(cx, |f, cx| f.set_error(msg, cx)),
            },
        }
    }

    /// Run a submission end-to-end: add-mode guard → stage the secret plan → write the
    /// connection → delete any superseded secret. Returns a non-fatal warning to
    /// surface after success. Mirrors `AppState::perform_submit` exactly.
    fn perform_db_submit(&self, submission: &Submission) -> Result<Option<String>, String> {
        let is_edit = submission.old.is_some();
        let target_holds = self
            .layer_holds_id(&submission.target, &submission.connection.id)
            .map_err(|e| e.to_string())?;
        add_guard(is_edit, target_holds, &self.layer_label(&submission.target))?;

        let has_password_field = self
            .db
            .registry
            .descriptor(submission.connection.kind)
            .map(|d| {
                d.connection_fields()
                    .iter()
                    .any(|f| matches!(f.kind, sid_core::db::ConnFieldKind::Password))
            })
            .unwrap_or(false);
        let plan = plan_secret(
            submission.old.as_ref(),
            has_password_field,
            submission.secret.is_some(),
        );
        let staged = stage_secret(
            self.secrets.as_ref(),
            &plan,
            &submission.connection.name,
            submission.secret.as_deref(),
        )?;

        let mut connection = submission.connection.clone();
        connection.secret_ref = staged.secret_ref.clone();
        if let Err(e) = self.store.write_connection(&connection, &submission.target) {
            // Roll back a freshly minted secret so a failed write never orphans one.
            if staged.minted
                && let Some(id) = &staged.secret_ref
            {
                let _ = self.secrets.delete(&SecretId::new(id.clone()));
            }
            return Err(e.to_string());
        }

        // Only after the write is durable is the superseded secret deleted.
        let mut post_warning = None;
        if let Some(old_id) = &staged.delete_after_write
            && let Err(e) = self.secrets.delete(&SecretId::new(old_id.clone()))
        {
            post_warning = Some(format!("saved, but deleting the old secret failed: {e}"));
        }
        Ok(post_warning)
    }

    /// Whether `target`'s **own layer** already holds `id` (the add-mode guard's
    /// question). Reads the layer directly — mirrors `AppState::layer_holds_alias`.
    fn layer_holds_id(&self, target: &Scope, id: &str) -> sid_store::Result<bool> {
        match target {
            Scope::Global => Ok(self.store.global().get_connection(id)?.is_some()),
            Scope::Workspace(_) => {
                let filters = ViewFilters {
                    collapse_duplicates: false,
                    hide_global: true,
                };
                let conns = self.store.read_connections(target, filters)?;
                Ok(conns.iter().any(|a| a.item.id == id))
            }
        }
    }

    // ---- row actions (W4) -------------------------------------------------------------

    /// ✕ (second click) Remove the record from **its origin layer**, then its secret
    /// from the keyring.
    fn delete_db_row(
        &mut self,
        id: &str,
        origin: &Scope,
        secret_ref: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.db.armed_delete = None;
        match self.store.delete_connection(id, origin) {
            Ok(_removed) => {
                let mut post_warning = None;
                if let Some(secret_id) = secret_ref
                    && let Err(e) = self.secrets.delete(&SecretId::new(secret_id))
                {
                    post_warning = Some(format!(
                        "connection deleted, but deleting its secret failed: {e}"
                    ));
                }
                self.refresh_db();
                if post_warning.is_some() {
                    self.error = post_warning;
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// ⤒ Move a workspace-origin record up to global.
    fn promote_db_row(&mut self, id: &str, origin: &Scope, cx: &mut Context<Self>) {
        self.db.armed_delete = None;
        let Scope::Workspace(ws_id) = origin else {
            return;
        };
        match self.store.promote_connection(id, ws_id) {
            Ok(()) => self.refresh_db(),
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// ⤓ Move a global-origin record down into the active workspace.
    fn demote_db_row(&mut self, id: &str, cx: &mut Context<Self>) {
        self.db.armed_delete = None;
        let Scope::Workspace(ws_id) = self.scope.clone() else {
            return;
        };
        match self.store.demote_connection(id, &ws_id) {
            Ok(()) => self.refresh_db(),
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    // ---- SQL editor + results (W5) -----------------------------------------------

    /// ▶ Run (or Ctrl/Cmd-Enter in the editor): resolve the active connection's secret,
    /// reuse (or open) its client, and fetch the first page. No-ops into a status
    /// message when nothing is selected/typed rather than disabling the button — keeps
    /// the click handler unconditional (simpler than threading `can_run` through render).
    ///
    /// Round-D §A.4: a dangling `secret_ref` on a connection whose engine needs a
    /// password (Postgres) opens the connect-time password prompt instead of failing
    /// outright — see [`needs_password_prompt`] and
    /// [`AppState::on_password_prompt_event`](crate::app::AppState::on_password_prompt_event).
    /// `pub(crate)` so that handler can retry this directly.
    pub(crate) fn run_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.db.running {
            return;
        }
        let Some(id) = self.db.active_id.clone() else {
            self.db.status = QueryStatus::Err("select a connection first".into());
            cx.notify();
            return;
        };
        let Some(conn) = self
            .db
            .connections
            .iter()
            .find(|a| a.item.id == id)
            .map(|a| a.item.clone())
        else {
            self.db.status = QueryStatus::Err("selected connection no longer exists".into());
            cx.notify();
            return;
        };
        let Some(sql_entity) = self.db.sql.clone() else {
            return;
        };
        let sql = sql_entity.read(cx).value().to_string();
        if sql.trim().is_empty() {
            self.db.status = QueryStatus::Err("SQL is empty".into());
            cx.notify();
            return;
        }
        let secret_result = resolve_db_secret(self.secrets.as_ref(), conn.secret_ref.as_deref());
        if needs_password_prompt(conn.kind, &secret_result) {
            let secret_ref = conn
                .secret_ref
                .clone()
                .expect("needs_password_prompt only fires on a dangling (thus Some) secret_ref");
            let label: SharedString = conn.name.clone().into();
            self.open_password_prompt(
                label,
                crate::app::PendingSecretPrompt::Db {
                    secret_ref,
                    retry: DbRetry::RunQuery,
                },
                window,
                cx,
            );
            return;
        }
        let secret = match secret_result {
            Ok(s) => s,
            Err(e) => {
                self.db.status = QueryStatus::Err(e);
                cx.notify();
                return;
            }
        };

        // Reuse the already-open client only if it belongs to this exact connection —
        // the active connection may have changed since the last run.
        let cached = if self.db.client_for.as_deref() == Some(id.as_str()) {
            self.db.client.clone()
        } else {
            None
        };
        let factory = self.db.registry.client(conn.kind);

        self.db.running = true;
        self.db.next_cursor = None;
        // Running a statement puts the results back on screen: a plan for the previous
        // statement left up beside fresh rows would be describing the wrong query.
        self.db.plan = None;
        self.db.last_sql = Some(sql.clone());
        push_history(&mut self.db.history, sql.clone(), HISTORY_CAP);
        self.db.query_generation += 1;
        let generation = self.db.query_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = run_first_page(factory, conn, secret, cached, sql).await;
            let _ = this.update(cx, |this, cx| {
                if this.db.query_generation != generation {
                    // Stale: the user switched connections (or started a newer run)
                    // while this query was in flight — its result must not land under
                    // the current selection.
                    return;
                }
                this.db.running = false;
                match outcome {
                    Ok((client, page)) => {
                        this.db.client = Some(client);
                        this.db.client_for = Some(id);
                        this.apply_query_page(&page, cx);
                    }
                    Err(e) => this.db.status = QueryStatus::Err(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Explain: run the editor's statement through the active engine's plan syntax and
    /// show the result in the plan pane.
    ///
    /// Guarded by `query_generation` exactly as `run_query` is — a plan is a query
    /// result like any other, and one arriving under a connection the user has since
    /// switched away from would be a plan for a different database.
    ///
    /// # Why this one does not open the password prompt
    ///
    /// `run_query`/`refresh_schema` pause for the connect-time prompt on a dangling
    /// Postgres `secret_ref` (round-D §A.4). Explain reports the error instead. The
    /// retry is dispatched through `crate::app::PendingSecretPrompt::Db`'s [`DbRetry`],
    /// whose match arm lives in `app.rs` — a file this slice does not own — so a third
    /// variant would be a cross-file change for a case with a one-step workaround: Run
    /// prompts, and Explain works from then on.
    fn explain_query(&mut self, cx: &mut Context<Self>) {
        if self.db.running {
            return;
        }
        let Some(support) = self.explain_support() else {
            self.db.status = QueryStatus::Err("select a connection first".into());
            cx.notify();
            return;
        };
        let Some(keyword) = support.keyword() else {
            // Belt and braces: the button is already disabled in this state, so
            // reaching here means a keyboard path found it anyway. Say the same thing
            // the tooltip does.
            self.db.status = QueryStatus::Err(
                support
                    .reason()
                    .unwrap_or("this engine has no query planner")
                    .to_string(),
            );
            cx.notify();
            return;
        };
        let Some(id) = self.db.active_id.clone() else {
            return;
        };
        let Some(conn) = self
            .db
            .connections
            .iter()
            .find(|a| a.item.id == id)
            .map(|a| a.item.clone())
        else {
            self.db.status = QueryStatus::Err("selected connection no longer exists".into());
            cx.notify();
            return;
        };
        let Some(sql_entity) = self.db.sql.clone() else {
            return;
        };
        let sql = sql_entity.read(cx).value().to_string();
        if sql.trim().is_empty() {
            self.db.status = QueryStatus::Err("SQL is empty".into());
            cx.notify();
            return;
        }
        let secret = match resolve_db_secret(self.secrets.as_ref(), conn.secret_ref.as_deref()) {
            Ok(s) => s,
            Err(e) => {
                self.db.status = QueryStatus::Err(e);
                cx.notify();
                return;
            }
        };
        let cached = if self.db.client_for.as_deref() == Some(id.as_str()) {
            self.db.client.clone()
        } else {
            None
        };
        let factory = self.db.registry.client(conn.kind);

        self.db.running = true;
        self.db.query_generation += 1;
        let generation = self.db.query_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = run_explain(factory, conn, secret, cached, sql).await;
            let _ = this.update(cx, |this, cx| {
                if this.db.query_generation != generation {
                    // Stale — see `run_query`'s identical guard.
                    return;
                }
                this.db.running = false;
                match outcome {
                    Ok((client, page)) => {
                        this.db.client = Some(client);
                        this.db.client_for = Some(id);
                        this.db.plan = Some(PlanView {
                            keyword: keyword.into(),
                            lines: plan_lines(&page),
                        });
                        // A plan is not a result set: it must not become what Export
                        // writes, nor reset the paging cursor of the results still
                        // sitting behind it.
                        this.db.status = QueryStatus::Ok {
                            duration_ms: page.duration_ms,
                            has_more: false,
                        };
                    }
                    Err(e) => this.db.status = QueryStatus::Err(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ⭳ next page: repeat `last_sql` against the cached client with `next_cursor`.
    fn next_page(&mut self, cx: &mut Context<Self>) {
        if self.db.running {
            return;
        }
        let (Some(cursor), Some(sql), Some(client)) = (
            self.db.next_cursor,
            self.db.last_sql.clone(),
            self.db.client.clone(),
        ) else {
            return;
        };

        self.db.running = true;
        // Same reason as `run_query`: fetching rows means the grid is what the user is
        // looking at, so the plan pane steps aside.
        self.db.plan = None;
        self.db.query_generation += 1;
        let generation = self.db.query_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime()
                .spawn(async move { client.query_paged(&sql, Some(cursor), PAGE_SIZE).await });
            let outcome = match handle.await {
                Ok(Ok(page)) => Ok(page),
                Ok(Err(e)) => Err(e.to_string()),
                Err(join_err) => Err(format!("query task panicked: {join_err}")),
            };
            let _ = this.update(cx, |this, cx| {
                if this.db.query_generation != generation {
                    // Stale: superseded by a connection switch or a newer run — see
                    // `run_query`'s identical guard.
                    return;
                }
                this.db.running = false;
                match outcome {
                    Ok(page) => this.apply_query_page(&page, cx),
                    Err(e) => this.db.status = QueryStatus::Err(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Apply a completed page to the status line + results table. The table's delegate
    /// is mutated in place and `refresh`ed (recomputes column layout) — see the
    /// `results` field's doc comment for why it's never rebuilt.
    fn apply_query_page(&mut self, page: &QueryPage, cx: &mut Context<Self>) {
        self.db.status = QueryStatus::Ok {
            duration_ms: page.duration_ms,
            has_more: page.next_cursor.is_some(),
        };
        self.db.next_cursor = page.next_cursor;
        // D3 (CSV export) exports whatever page is currently on screen — cache it here,
        // the one place a page becomes "current", rather than re-deriving it from the
        // table delegate at export time.
        self.db.last_page = Some(page.clone());
        if let Some(results) = self.db.results.clone() {
            results.update(cx, |state, cx| {
                state.delegate_mut().set_page(page.clone());
                state.refresh(cx);
                cx.notify();
            });
        }
    }

    /// D1: kick off a schema refresh for the active connection on the shared runtime
    /// (never inline in render). Reuses the already-open client the same way
    /// `run_query` does — connecting twice for one connection would be wasteful and
    /// could surprise a single-connection-limited engine (e.g. a locked SQLite file).
    ///
    /// Round-D §A.4: same dangling-ref-on-Postgres prompt treatment as `run_query` —
    /// see that method's doc comment. `pub(crate)` for the same reason.
    pub(crate) fn refresh_schema(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.db.active_id.clone() else {
            return;
        };
        let Some(conn) = self
            .db
            .connections
            .iter()
            .find(|a| a.item.id == id)
            .map(|a| a.item.clone())
        else {
            return;
        };
        let secret_result = resolve_db_secret(self.secrets.as_ref(), conn.secret_ref.as_deref());
        if needs_password_prompt(conn.kind, &secret_result) {
            let secret_ref = conn
                .secret_ref
                .clone()
                .expect("needs_password_prompt only fires on a dangling (thus Some) secret_ref");
            let label: SharedString = conn.name.clone().into();
            self.open_password_prompt(
                label,
                crate::app::PendingSecretPrompt::Db {
                    secret_ref,
                    retry: DbRetry::RefreshSchema,
                },
                window,
                cx,
            );
            return;
        }
        let secret = match secret_result {
            Ok(s) => s,
            Err(e) => {
                self.db.schema_error = Some(e);
                cx.notify();
                return;
            }
        };
        let cached = if self.db.client_for.as_deref() == Some(id.as_str()) {
            self.db.client.clone()
        } else {
            None
        };
        let factory = self.db.registry.client(conn.kind);

        self.db.schema_loading = true;
        self.db.schema_error = None;
        self.db.schema_generation += 1;
        let generation = self.db.schema_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = fetch_schema(factory, conn, secret, cached).await;
            let _ = this.update(cx, |this, cx| {
                if this.db.schema_generation != generation {
                    // Stale: the user selected another connection (which bumped the
                    // generation and blanked the tree) or a newer refresh superseded
                    // this one — applying would overwrite the newer selection's schema
                    // with a wrong-connection one (bug-hunt round D, HIGH).
                    return;
                }
                this.db.schema_loading = false;
                match outcome {
                    Ok((client, schema, graph)) => {
                        this.db.client = Some(client);
                        this.db.client_for = Some(id);
                        this.db.schema = Some(schema);
                        this.db.schema_graph = Some(graph);
                        this.push_schema_to_diagram(cx);
                    }
                    Err(e) => this.db.schema_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Hand a freshly fetched schema to the relationships pop-out, if one is open.
    ///
    /// Push, not pull. The pop-out cannot fetch for itself: `DbTabState::schema` and
    /// `schema_graph` are private to this module, and [`Self::refresh_schema`] spawns
    /// and detaches, so there is no moment at which the other window could read a result
    /// it asked for. This side, in the completion, has both the data and the `&mut App`
    /// needed to reach an entity in another window — so it does the handing over, and
    /// the pop-out stays a passive renderer with no async of its own.
    ///
    /// A closed pop-out leaves a dead [`WeakEntity`]; `update` returns `Err` and this is
    /// a no-op. [`DiagramView::reload`] does the connection-id check, since it is the
    /// side that knows which connection its window is titled for.
    fn push_schema_to_diagram(&self, cx: &mut Context<Self>) {
        let (Some(view), Some(schema), Some(graph)) =
            (&self.db.diagram, &self.db.schema, &self.db.schema_graph)
        else {
            return;
        };
        let connection_id = self.db.active_id.clone();
        let _ = view.update(cx, |view, cx| {
            view.reload(connection_id.as_deref(), schema, graph, cx);
        });
    }

    /// D1: chevron-click — toggle one table's expanded state (shows/hides its columns).
    fn toggle_schema_table(&mut self, display_name: &str, cx: &mut Context<Self>) {
        if !self.db.schema_expanded.remove(display_name) {
            self.db.schema_expanded.insert(display_name.to_string());
        }
        cx.notify();
    }

    /// D1: name-click — replace the editor contents with `SELECT * FROM <table>`.
    fn insert_select_star(
        &mut self,
        display_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sql_entity) = self.db.sql.clone() else {
            return;
        };
        let stmt = format!("SELECT * FROM {}", quote_ident(display_name));
        sql_entity.update(cx, |state, cx| {
            state.set_value(stmt, window, cx);
        });
        cx.notify();
    }

    /// Task 2 — diagram click-through, table NAME click: set the editor to
    /// `SELECT * FROM <table>` (same scaffold [`Self::insert_select_star`] builds) and
    /// run it immediately. `pub(crate)` — called from `db_diagram.rs` across the
    /// diagram's OS window via the `WeakEntity<AppState>`/`AnyWindowHandle` pair
    /// [`Self::open_diagram_window`] hands the diagram (see that method's doc comment).
    pub(crate) fn diagram_open_table(
        &mut self,
        table: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.insert_select_star(table, window, cx);
        self.run_query(window, cx);
    }

    /// Task 2 — diagram click-through, COLUMN row click: seed (not run) the editor with
    /// a `WHERE` filter scaffold for `table`/`column`, cursor parked at the end (after
    /// the trailing space) so the user can type the value straight away, and surface a
    /// notice explaining the scaffold. `set_value` alone would leave the cursor at
    /// offset 0 for a multi-line/code-editor `InputState` (see its doc comment) — hence
    /// the explicit `set_cursor_position` follow-up here, which `insert_select_star`
    /// doesn't need (that scaffold has nothing left for the user to type).
    pub(crate) fn diagram_open_column_filter(
        &mut self,
        table: &str,
        column: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sql_entity) = self.db.sql.clone() else {
            return;
        };
        let stmt = where_filter_scaffold(table, column);
        let end = Position::new(0, stmt.chars().count() as u32);
        sql_entity.update(cx, |state, cx| {
            state.set_value(stmt, window, cx);
            state.set_cursor_position(end, window, cx);
        });
        self.db.notice = Some("filter scaffold from diagram — complete the value and Run".into());
        cx.notify();
    }

    /// D4: history-entry click — reload that exact SQL text into the editor
    /// (unmodified; doesn't re-run it).
    fn reload_history_entry(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sql_entity) = self.db.sql.clone() else {
            return;
        };
        sql_entity.update(cx, |state, cx| {
            state.set_value(sql.to_string(), window, cx);
        });
        cx.notify();
    }

    /// D3: export the currently-displayed page to `~/Downloads/<conn>-<n>.csv`, guarding
    /// CSV formula injection (see [`csv_escape_field`]'s doc comment — this is the
    /// security-load-bearing piece of this increment). Reports success/failure as a
    /// transient `db.notice`, mirroring how `db.status` reports query results.
    fn export_csv(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.db.last_page.clone() else {
            self.db.notice = Some("nothing to export — run a query first".to_string());
            cx.notify();
            return;
        };
        let conn_label = self
            .db
            .active_id
            .as_deref()
            .and_then(|id| self.db.connections.iter().find(|a| a.item.id == id))
            .map(|a| a.item.name.clone())
            .unwrap_or_else(|| "query".to_string());

        let csv = page_to_csv(&page);
        self.db.notice = Some(match write_csv_export(&conn_label, &csv) {
            Ok(path) => format!("✓ exported to {}", path.display()),
            Err(e) => format!("✗ export failed: {e}"),
        });
        cx.notify();
    }
}

/// The connect-or-reuse-then-schema body of [`AppState::refresh_schema`] — the D1
/// counterpart of [`run_first_page`], split out for the same readability reason.
///
/// Also fetches [`SchemaGraph`] (FK edges + primary keys) in the same round trip — the
/// diagram view's data contract — since both calls need the identical open-or-reuse
/// client and there is no reason to connect twice for one schema refresh.
async fn fetch_schema(
    factory: Arc<dyn DbClient>,
    conn: DbConnection,
    secret: Option<String>,
    cached: Option<Arc<dyn DbClient>>,
) -> Result<(Arc<dyn DbClient>, SchemaInfo, SchemaGraph), String> {
    let handle = ssh_runtime().spawn(async move {
        let client = match cached {
            Some(c) => c,
            None => {
                let params = OpenParams {
                    kind: conn.kind,
                    dsn: conn.dsn.clone(),
                    password: secret,
                    sqlite_mode: None,
                };
                factory.open(params).await?
            }
        };
        let schema = client.schema_introspect().await?;
        let graph = client.schema_graph().await?;
        Ok::<_, DbError>((client, schema, graph))
    });
    match handle.await {
        Ok(Ok(triple)) => Ok(triple),
        Ok(Err(e)) => Err(e.to_string()),
        Err(join_err) => Err(format!("schema task panicked: {join_err}")),
    }
}

/// The connect-or-reuse-then-first-page body of [`AppState::run_query`], split out so
/// the `cx.spawn` future in that method stays readable. Runs on `session::ssh_runtime()`
/// (see this module's doc comment) since both `tokio-postgres` and `rusqlite` need an
/// ambient Tokio context; gpui's own executor provides none.
async fn run_first_page(
    factory: Arc<dyn DbClient>,
    conn: DbConnection,
    secret: Option<String>,
    cached: Option<Arc<dyn DbClient>>,
    sql: String,
) -> Result<(Arc<dyn DbClient>, QueryPage), String> {
    let handle = ssh_runtime().spawn(async move {
        let client = match cached {
            Some(c) => c,
            None => {
                let params = OpenParams {
                    kind: conn.kind,
                    dsn: conn.dsn.clone(),
                    password: secret,
                    // A saved connection has no persisted SQLite mode (`DbConnection`
                    // carries none); `sqlite.rs` treats `None` as `OpenExisting` — the
                    // safe, non-destructive default for re-opening a file that (per the
                    // add/edit form) was already created or picked. Ignored by
                    // Postgres/Redb.
                    sqlite_mode: None,
                };
                factory.open(params).await?
            }
        };
        let page = client.query_paged(&sql, None, PAGE_SIZE).await?;
        Ok::<_, DbError>((client, page))
    });
    match handle.await {
        Ok(Ok(pair)) => Ok(pair),
        Ok(Err(e)) => Err(e.to_string()),
        Err(join_err) => Err(format!("query task panicked: {join_err}")),
    }
}

/// The connect-or-reuse-then-explain body of [`AppState::explain_query`] — the same
/// shape as [`run_first_page`], calling [`sid_core::db::DbClient::explain`] instead of
/// `query_paged`. The statement is described, never executed (see
/// [`sid_core::db::ExplainSupport`]).
async fn run_explain(
    factory: Arc<dyn DbClient>,
    conn: DbConnection,
    secret: Option<String>,
    cached: Option<Arc<dyn DbClient>>,
    sql: String,
) -> Result<(Arc<dyn DbClient>, QueryPage), String> {
    let handle = ssh_runtime().spawn(async move {
        let client = match cached {
            Some(c) => c,
            None => {
                let params = OpenParams {
                    kind: conn.kind,
                    dsn: conn.dsn.clone(),
                    password: secret,
                    sqlite_mode: None,
                };
                factory.open(params).await?
            }
        };
        let page = client.explain(&sql).await?;
        Ok::<_, DbError>((client, page))
    });
    match handle.await {
        Ok(Ok(pair)) => Ok(pair),
        Ok(Err(e)) => Err(e.to_string()),
        Err(join_err) => Err(format!("explain task panicked: {join_err}")),
    }
}

/// Fetch the secret backing `secret_ref`, if any — the DB-connection mirror of
/// `ssh_connect::resolve_secret`. No ref → `Ok(None)` (fine for SQLite/Redb, or a
/// Postgres connection with no password). A *dangling* ref (recorded but missing from
/// the keyring) is always an error: the connection was configured to need a secret we
/// can no longer deliver.
fn resolve_db_secret(
    secrets: &dyn SecretStore,
    secret_ref: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(secret_ref) = secret_ref else {
        return Ok(None);
    };
    let id = SecretId::new(secret_ref.to_string());
    let bytes = secrets
        .get(&id)
        .map_err(|e| format!("secret lookup for {secret_ref:?} failed: {e}"))?;
    match bytes {
        Some(b) => String::from_utf8(b)
            .map(Some)
            .map_err(|_| "stored secret is not valid UTF-8".to_string()),
        None => Err(format!(
            "dangling secret_ref {secret_ref:?} — no secret in the keyring"
        )),
    }
}

/// Which DB action a connect-time password prompt (round-D §A.4) should retry once its
/// password lands in the secret store — see [`AppState::run_query`]/
/// [`AppState::refresh_schema`] and `crate::app::PendingSecretPrompt::Db`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbRetry {
    RunQuery,
    RefreshSchema,
}

/// Whether a DB action should pause for the connect-time password prompt instead of
/// surfacing [`resolve_db_secret`]'s error outright: only when the lookup failed (a
/// dangling `secret_ref`) **and** the connection's engine actually authenticates with a
/// password. `Sqlite`/`Redb` never do (a dangling ref there is a plain configuration
/// error, not a prompt-fixable one) — only `Postgres` does today.
pub(crate) fn needs_password_prompt(kind: DbKind, secret: &Result<Option<String>, String>) -> bool {
    kind == DbKind::Postgres && secret.is_err()
}

// ---- D4: query history (ring cap + consecutive dedup) -------------------------------------

/// Push `sql` onto `history` (most-recent-last), capping length at `cap` by dropping the
/// oldest entry, and skipping the push entirely if `sql` is identical to the current
/// most-recent entry (consecutive-dedup — re-running the same query shouldn't spam the
/// list). Pure logic, no `AppState`/GPUI — D4's TDD target.
fn push_history(history: &mut Vec<String>, sql: String, cap: usize) {
    if history.last() == Some(&sql) {
        return;
    }
    history.push(sql);
    if history.len() > cap {
        history.remove(0);
    }
}

// ---- D3: CSV export (security-load-bearing) ------------------------------------------------

/// Escape one CSV field against both RFC-4180 structural characters *and* formula
/// injection (CWE-1236 / OWASP "CSV Injection"): if a spreadsheet app opens this file and
/// a field's first character is `=`, `+`, `-`, `@`, a tab, or a CR, that app may parse the
/// field as a formula and execute it (e.g. an attacker-controlled row value like
/// `=cmd|'/C calc'!A1` launching a program on open). Any such field gets a leading `'`
/// prefix first — spreadsheet apps render a leading apostrophe as "force text" and never
/// execute what follows — *then* the (possibly now-prefixed) field is RFC-4180 quoted if
/// it contains a `"`, `,`, or newline.
fn csv_escape_field(field: &str) -> String {
    let needs_formula_guard = matches!(
        field.chars().next(),
        Some('=' | '+' | '-' | '@' | '\t' | '\r')
    );
    let guarded = if needs_formula_guard {
        format!("'{field}")
    } else {
        field.to_string()
    };
    let needs_quoting = guarded.contains(['"', ',', '\n', '\r']);
    if needs_quoting {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

/// Render a whole [`QueryPage`] (header row of column names + data rows) as CSV text,
/// `\r\n`-terminated per RFC 4180, escaping every field via [`csv_escape_field`].
fn page_to_csv(page: &QueryPage) -> String {
    let mut out = String::new();
    let header = page
        .columns
        .iter()
        .map(|c| csv_escape_field(&c.name))
        .collect::<Vec<_>>()
        .join(",");
    out.push_str(&header);
    out.push_str("\r\n");
    for row in &page.rows {
        let line = row
            .values
            .iter()
            .map(|v| csv_escape_field(v))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out
}

/// `$HOME/Downloads` — a local duplicate of `session.rs`'s private `downloads_dir()` (that
/// module is off-limits for edits this slice, so its helper can't be reused directly; both
/// intentionally avoid the `dirs` crate for one env-var read).
fn downloads_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join("Downloads")
}

/// Strip everything but alphanumerics/`-`/`_` from a connection name before it becomes
/// part of a filename — the same path-traversal-defense shape as `session.rs`'s
/// `safe_local_name`, applied here to keep a connection named e.g. `prod/../../etc` (or
/// containing spaces/slashes) from producing a path that escapes `~/Downloads` or breaks
/// the shell when the user later opens it.
fn sanitize_filename_component(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "query".to_string()
    } else {
        cleaned
    }
}

/// The first `~/Downloads/<conn>-<n>.csv` (n = 1, 2, 3, …) that doesn't already exist —
/// so repeated exports for the same connection accumulate rather than clobber.
fn next_csv_export_path(dir: &Path, conn_label: &str) -> PathBuf {
    let stem = sanitize_filename_component(conn_label);
    let mut n = 1u32;
    loop {
        let candidate = dir.join(format!("{stem}-{n}.csv"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Write `csv` to the next free export path for `conn_label` under `~/Downloads`,
/// creating the directory if needed. Returns the path written on success.
fn write_csv_export(conn_label: &str, csv: &str) -> Result<PathBuf, String> {
    let dir = downloads_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let path = next_csv_export_path(&dir, conn_label);
    fs::write(&path, csv).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod result_column_plan_tests {
    use sid_ui::table::resolve_widths;

    use super::*;

    /// The grid's own reserve, so a test can assert against the width the columns
    /// actually get. Mirrors `FillColumns::sync`, which is what runs in the app.
    fn resolved(columns: &[DbColumn], rows: &[Row], viewport: f32) -> Vec<f32> {
        resolve_widths(
            &plan_result_columns(columns, rows),
            viewport - sid_ui::table::TABLE_CHROME,
        )
    }

    fn col(name: &str, ty: ColumnType) -> DbColumn {
        DbColumn {
            name: name.to_string(),
            ty,
        }
    }

    fn row(values: &[&str]) -> Row {
        Row {
            values: values.iter().map(|v| v.to_string()).collect(),
        }
    }

    /// A realistic mixed page: a numeric key, a uuid, a short label, and a long note.
    fn mixed() -> (Vec<DbColumn>, Vec<Row>) {
        (
            vec![
                col("id", ColumnType::Integer),
                col("external_id", ColumnType::Other("uuid".into())),
                col("city", ColumnType::Text),
                col("note", ColumnType::Text),
            ],
            vec![
                row(&[
                    "1",
                    "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
                    "Phoenix",
                    "shipped late, customer called twice about the missing pallet",
                ]),
                row(&[
                    "2",
                    "9c858901-8a57-4791-81fe-4c455b099bc9",
                    "Austin",
                    "fine",
                ]),
            ],
        )
    }

    #[test]
    fn a_statement_with_no_result_set_plans_no_columns() {
        // `create table …` and friends come back with zero columns. The grid must plan
        // nothing rather than a phantom column filling the pane.
        assert!(plan_result_columns(&[], &[]).is_empty());
        assert!(plan_result_columns(&[], &[row(&[])]).is_empty());
    }

    #[test]
    fn a_uuid_column_is_floored_wide_enough_to_show_a_whole_uuid() {
        // The headline case: 36 characters at the old fixed 140px was a guaranteed
        // truncation on every row. The floor alone — before any leftover — has to cover
        // it, so a cramped window truncates every other column before this one.
        let (columns, rows) = mixed();
        let plan = plan_result_columns(&columns, &rows);
        let uuid_floor = plan[1].floor();
        assert!(
            uuid_floor >= 36.0 * CELL_CHAR_PX,
            "a uuid needs {}px of text, the floor is {uuid_floor}",
            36.0 * CELL_CHAR_PX
        );
    }

    #[test]
    fn a_numeric_column_stays_compact_while_the_text_columns_take_the_room() {
        // "Narrow numeric columns stay compact" is the other half of the fill-width
        // promise: reclaiming 1300px is no good if it goes to the `id` column.
        let (columns, rows) = mixed();
        let widths = resolved(&columns, &rows, 2000.);
        assert!(
            widths[0] <= NUMERIC_MAX_PX,
            "id took {}px of a 2000px grid",
            widths[0]
        );
        assert!(widths[3] > 500., "note only got {}px", widths[3]);
    }

    #[test]
    fn the_widest_text_column_takes_the_largest_share() {
        // Weight follows content: `note` holds an order of magnitude more text than
        // `city`, so an even split would leave one wrapped and the other mostly padding.
        let (columns, rows) = mixed();
        let widths = resolved(&columns, &rows, 2000.);
        assert!(
            widths[3] > widths[2],
            "note {} vs city {}",
            widths[3],
            widths[2]
        );
    }

    #[test]
    fn the_planned_columns_fill_the_grid_with_no_dead_space() {
        // The property the whole migration exists for, at the width the capture is taken
        // at and either side of it.
        let (columns, rows) = mixed();
        for viewport in [900., 1440., 2000., 3440.] {
            let total: f32 = resolved(&columns, &rows, viewport).iter().sum();
            assert!(
                (total - (viewport - sid_ui::table::TABLE_CHROME)).abs() < 0.01,
                "{viewport}px: columns total {total}"
            );
        }
    }

    #[test]
    fn a_long_header_widens_the_narrow_column_under_it() {
        // A count column holding two-digit values still has to show its own name, or the
        // header is the thing that truncates.
        let columns = vec![col("orders_placed_last_quarter", ColumnType::Integer)];
        let rows = vec![row(&["12"])];
        let plan = plan_result_columns(&columns, &rows);
        assert!(
            plan[0].floor() > NUMERIC_MIN_PX,
            "a 26-character header got the bare minimum {}",
            plan[0].floor()
        );
    }

    #[test]
    fn a_bounded_column_is_never_floored_past_its_cap() {
        // A 60-digit bignum is representable; letting it claim 450px before any text
        // column has had a pixel is not.
        let columns = vec![col("n", ColumnType::Integer)];
        let rows = vec![row(&["9".repeat(60).as_str()])];
        assert_eq!(
            plan_result_columns(&columns, &rows)[0],
            ColumnWidth::Min(NUMERIC_MAX_PX)
        );
    }

    #[test]
    fn an_unbounded_column_is_never_floored_past_its_cap() {
        // One JSON blob column must not push its siblings into horizontal scroll: it is
        // a `Grow`, so a wide window still gives it everything spare.
        let columns = vec![col("doc", ColumnType::Text), col("id", ColumnType::Integer)];
        let rows = vec![row(&["x".repeat(4000).as_str(), "1"])];
        assert_eq!(
            plan_result_columns(&columns, &rows)[0].floor(),
            TEXT_MAX_PX,
            "capped at the readable maximum"
        );
    }

    #[test]
    fn an_all_numeric_result_still_fills_the_grid() {
        // `select count(*)` has no grower at all. Bounded columns are `Min`, not `Fixed`,
        // exactly so this case fills instead of leaving 1800px of black beside a number.
        let columns = vec![col("count", ColumnType::Integer)];
        let rows = vec![row(&["3"])];
        let widths = resolved(&columns, &rows, 2000.);
        assert!(
            (widths[0] - (2000. - sid_ui::table::TABLE_CHROME)).abs() < 0.01,
            "a lone count column resolved to {}px",
            widths[0]
        );
    }

    #[test]
    fn an_empty_result_set_with_columns_still_plans_from_its_headers() {
        // `select * from t where false`: no rows to measure, but the headers still have
        // to render untruncated.
        let columns = vec![
            col("id", ColumnType::Integer),
            col("customer_reference", ColumnType::Text),
        ];
        let plan = plan_result_columns(&columns, &[]);
        assert_eq!(plan.len(), 2);
        assert!(plan[0].floor() >= NUMERIC_MIN_PX);
        assert!(plan[1].floor() >= TEXT_MIN_PX);
    }

    #[test]
    fn a_driver_specific_type_is_treated_as_unbounded() {
        // `Other` is where `uuid`, `json`, `timestamptz` and every unmapped Postgres type
        // land. Guessing "narrow" for a type sid does not recognise is how a uuid column
        // ends up clipped.
        for ty in [
            ColumnType::Text,
            ColumnType::Bytes,
            ColumnType::Other("jsonb".into()),
        ] {
            assert!(!is_bounded(&ty), "{ty:?} should grow");
        }
        for ty in [
            ColumnType::Integer,
            ColumnType::Float,
            ColumnType::Bool,
            ColumnType::Null,
        ] {
            assert!(is_bounded(&ty), "{ty:?} should stay compact");
        }
    }

    #[test]
    fn a_multi_line_value_is_flattened_onto_the_cell_s_one_line() {
        // `sqlite_master.sql` is the case that made this visible: a whole CREATE TABLE
        // statement in a 32px row, painting through every row under it.
        assert_eq!(
            single_line("CREATE TABLE t (\n  id INTEGER,\n  name TEXT\n)"),
            "CREATE TABLE t ( id INTEGER, name TEXT )"
        );
        assert_eq!(single_line("a\r\n\tb"), "a b");
    }

    #[test]
    fn a_single_line_value_survives_flattening_unchanged() {
        assert_eq!(single_line("Phoenix"), "Phoenix");
        assert_eq!(single_line("R. Runner"), "R. Runner");
        assert_eq!(single_line(""), "");
        assert_eq!(single_line("   "), "", "whitespace-only collapses to empty");
        assert_eq!(single_line("  padded  "), "padded", "ends are trimmed");
    }

    #[test]
    fn a_column_shorter_than_the_rest_of_the_row_does_not_panic() {
        // Ragged rows are a driver bug, not a crash: the plan reads values through `get`.
        let columns = vec![col("a", ColumnType::Text), col("b", ColumnType::Text)];
        assert_eq!(
            plan_result_columns(&columns, &[row(&["only-one"])]).len(),
            2
        );
    }
}

#[cfg(test)]
mod connection_dot_tests {
    use super::*;

    #[test]
    fn an_open_client_reads_as_connected_even_while_a_query_runs() {
        // Live beats busy: a query running against an already-open connection is not a
        // connection that is still dialling.
        assert_eq!(
            connection_dot(true, true, true, false),
            ConnectionState::Live
        );
        assert_eq!(
            connection_dot(true, false, false, false),
            ConnectionState::Live
        );
    }

    #[test]
    fn the_selected_row_shows_the_work_in_flight() {
        assert_eq!(
            connection_dot(false, true, true, false),
            ConnectionState::Connecting
        );
    }

    #[test]
    fn a_retry_in_flight_supersedes_the_error_it_is_retrying() {
        assert_eq!(
            connection_dot(false, true, true, true),
            ConnectionState::Connecting
        );
        assert_eq!(
            connection_dot(false, true, false, true),
            ConnectionState::Failed
        );
    }

    #[test]
    fn an_unselected_row_never_wears_the_selection_s_state() {
        // `schema_error`, `running` and `schema_loading` are single-slot fields belonging
        // to the active connection. Painting them on every row would light up the whole
        // list on one failure.
        for busy in [true, false] {
            for errored in [true, false] {
                assert_eq!(
                    connection_dot(false, false, busy, errored),
                    ConnectionState::Offline,
                    "busy={busy} errored={errored}"
                );
            }
        }
    }
}

#[cfg(test)]
mod query_secret_tests {
    use sid_secrets::MemorySecretStore;

    use super::*;

    #[test]
    fn no_ref_resolves_to_no_secret() {
        let secrets = MemorySecretStore::default();
        assert_eq!(resolve_db_secret(&secrets, None), Ok(None));
    }

    #[test]
    fn present_ref_resolves_to_its_bytes() {
        let secrets = MemorySecretStore::default();
        secrets.put(&SecretId::new("db-a"), b"hunter2").unwrap();
        assert_eq!(
            resolve_db_secret(&secrets, Some("db-a")),
            Ok(Some("hunter2".to_string()))
        );
    }

    #[test]
    fn dangling_ref_is_an_error() {
        let secrets = MemorySecretStore::default();
        assert!(resolve_db_secret(&secrets, Some("db-missing")).is_err());
    }

    // ---- needs_password_prompt (round-D §A.4) ------------------------------------

    #[test]
    fn postgres_dangling_ref_needs_a_prompt() {
        let dangling: Result<Option<String>, String> = Err("dangling secret_ref".into());
        assert!(needs_password_prompt(DbKind::Postgres, &dangling));
    }

    #[test]
    fn postgres_with_a_resolved_secret_never_prompts() {
        assert!(!needs_password_prompt(
            DbKind::Postgres,
            &Ok(Some("hunter2".to_string()))
        ));
        assert!(!needs_password_prompt(DbKind::Postgres, &Ok(None)));
    }

    #[test]
    fn sqlite_and_redb_never_prompt_even_on_a_dangling_ref() {
        let dangling: Result<Option<String>, String> = Err("dangling secret_ref".into());
        assert!(!needs_password_prompt(DbKind::Sqlite, &dangling));
        assert!(!needs_password_prompt(DbKind::Redb, &dangling));
    }
}

#[cfg(test)]
mod csv_export_tests {
    use sid_core::db::{Column as DbColumn, ColumnType};

    use super::*;

    /// D3's load-bearing test: a cell value crafted to launch a program if a
    /// spreadsheet app naively opens the export (CVE-class CSV/formula injection) must
    /// round-trip as inert, quoted, apostrophe-prefixed text — never a bare formula.
    #[test]
    fn formula_injection_payload_is_neutralized() {
        let payload = "=cmd|'/C calc'!A1";
        let escaped = csv_escape_field(payload);
        assert!(
            !escaped.starts_with('='),
            "escaped field must not start with '=': {escaped:?}"
        );
        // A leading apostrophe is enough on its own to force every mainstream
        // spreadsheet app to treat the cell as text rather than evaluate it — the
        // payload has no `"`/`,`/newline, so RFC-4180 quoting doesn't additionally
        // kick in. The whole thing must decode back to exactly `'` + payload.
        assert_eq!(escaped, format!("'{payload}"));
    }

    #[test]
    fn each_formula_lead_character_is_guarded() {
        for lead in ['=', '+', '-', '@', '\t', '\r'] {
            let field = format!("{lead}rest");
            let escaped = csv_escape_field(&field);
            let unquoted = escaped.trim_matches('"');
            assert!(
                unquoted.starts_with('\''),
                "lead {lead:?} not guarded: {escaped:?}"
            );
        }
    }

    #[test]
    fn plain_field_is_untouched() {
        assert_eq!(csv_escape_field("hello"), "hello");
        assert_eq!(csv_escape_field(""), "");
    }

    #[test]
    fn comma_and_quote_and_newline_trigger_rfc4180_quoting() {
        assert_eq!(csv_escape_field("a,b"), "\"a,b\"");
        assert_eq!(csv_escape_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_escape_field("a\nb"), "\"a\nb\"");
    }

    #[test]
    fn page_to_csv_renders_header_and_rows_crlf_terminated() {
        let page = QueryPage {
            columns: vec![
                DbColumn {
                    name: "id".into(),
                    ty: ColumnType::Integer,
                },
                DbColumn {
                    name: "note".into(),
                    ty: ColumnType::Text,
                },
            ],
            rows: vec![Row {
                values: vec!["1".into(), "=evil()".into()],
            }],
            next_cursor: None,
            duration_ms: 0,
        };
        let csv = page_to_csv(&page);
        assert_eq!(csv, "id,note\r\n1,'=evil()\r\n");
    }

    #[test]
    fn sanitize_filename_component_strips_traversal_and_separators() {
        assert_eq!(sanitize_filename_component("prod"), "prod");
        // `.` and `/` both fall outside the alnum/-/_ allowlist, so `..`/`/` collapse to
        // underscores too — no traversal-meaningful character survives at all, which is
        // stricter (and simpler to reason about) than merely blocking `..` sequences.
        assert_eq!(
            sanitize_filename_component("../../etc/passwd"),
            "______etc_passwd"
        );
        assert_eq!(sanitize_filename_component("my db 1"), "my_db_1");
        assert_eq!(sanitize_filename_component(""), "query");
    }

    #[test]
    fn next_csv_export_path_increments_past_existing_files() {
        let dir = std::env::temp_dir().join(format!(
            "sid-db-csv-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("demo-1.csv"), "x").unwrap();
        fs::write(dir.join("demo-2.csv"), "x").unwrap();
        let next = next_csv_export_path(&dir, "demo");
        assert_eq!(next, dir.join("demo-3.csv"));
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn pushes_grow_the_list_in_order() {
        let mut history = Vec::new();
        push_history(&mut history, "select 1".to_string(), 50);
        push_history(&mut history, "select 2".to_string(), 50);
        assert_eq!(
            history,
            vec!["select 1".to_string(), "select 2".to_string()]
        );
    }

    #[test]
    fn consecutive_duplicate_is_not_pushed_again() {
        let mut history = Vec::new();
        push_history(&mut history, "select 1".to_string(), 50);
        push_history(&mut history, "select 1".to_string(), 50);
        assert_eq!(history, vec!["select 1".to_string()]);
    }

    #[test]
    fn non_consecutive_duplicate_is_pushed_again() {
        let mut history = Vec::new();
        push_history(&mut history, "select 1".to_string(), 50);
        push_history(&mut history, "select 2".to_string(), 50);
        push_history(&mut history, "select 1".to_string(), 50);
        assert_eq!(
            history,
            vec![
                "select 1".to_string(),
                "select 2".to_string(),
                "select 1".to_string()
            ]
        );
    }

    #[test]
    fn ring_caps_at_capacity_dropping_oldest() {
        let mut history = Vec::new();
        for i in 0..5 {
            push_history(&mut history, format!("select {i}"), 3);
        }
        assert_eq!(
            history,
            vec![
                "select 2".to_string(),
                "select 3".to_string(),
                "select 4".to_string(),
            ]
        );
    }
}

#[cfg(test)]
mod schema_tree_tests {
    use sid_core::db::{Column as DbColumn, ColumnType};

    use super::*;

    fn table(schema: Option<&str>, name: &str, cols: &[&str]) -> TableInfo {
        TableInfo {
            schema: schema.map(str::to_string),
            name: name.to_string(),
            columns: cols
                .iter()
                .map(|c| DbColumn {
                    name: c.to_string(),
                    ty: ColumnType::Text,
                })
                .collect(),
        }
    }

    #[test]
    fn collapsed_tables_render_as_headers_only() {
        let schema = SchemaInfo {
            tables: vec![
                table(None, "users", &["id", "name"]),
                table(None, "orders", &["id"]),
            ],
        };
        let rows = schema_tree_rows(&schema, &HashSet::new());
        assert_eq!(
            rows,
            vec![
                SchemaRow::Table {
                    display_name: "users".to_string(),
                    expanded: false
                },
                SchemaRow::Table {
                    display_name: "orders".to_string(),
                    expanded: false
                },
            ]
        );
    }

    #[test]
    fn expanded_table_inserts_its_columns_right_after_its_header() {
        let schema = SchemaInfo {
            tables: vec![
                table(None, "users", &["id", "name"]),
                table(None, "orders", &["id"]),
            ],
        };
        let mut expanded = HashSet::new();
        expanded.insert("users".to_string());
        let rows = schema_tree_rows(&schema, &expanded);
        assert_eq!(
            rows,
            vec![
                SchemaRow::Table {
                    display_name: "users".to_string(),
                    expanded: true
                },
                SchemaRow::Column {
                    name: "id".to_string()
                },
                SchemaRow::Column {
                    name: "name".to_string()
                },
                SchemaRow::Table {
                    display_name: "orders".to_string(),
                    expanded: false
                },
            ]
        );
    }

    #[test]
    fn postgres_schema_qualified_name_uses_schema_dot_table() {
        let table = table(Some("public"), "users", &[]);
        assert_eq!(table_display_name(&table), "public.users");
    }

    #[test]
    fn sqlite_table_with_no_schema_uses_bare_name() {
        let table = table(None, "users", &[]);
        assert_eq!(table_display_name(&table), "users");
    }
}

#[cfg(test)]
mod diagram_scaffold_tests {
    use super::*;

    /// Task 2's TDD target: the `WHERE` filter scaffold, trailing space included. Plain
    /// identifiers (including a dotted `schema.table`) stay bare for readable editor
    /// SQL; anything else gets ANSI quoting via [`quote_ident`].
    #[test]
    fn builds_a_where_scaffold_with_a_trailing_space() {
        assert_eq!(
            where_filter_scaffold("public.users", "user_id"),
            "SELECT * FROM public.users WHERE user_id = "
        );
        assert_eq!(
            where_filter_scaffold("public.users", "user id"),
            "SELECT * FROM public.users WHERE \"user id\" = "
        );
    }

    /// Security-load-bearing: an introspected name from a hostile database cannot break
    /// out of the identifier position in generated SQL — quotes are doubled, so the
    /// payload stays one (syntactically doomed) identifier, never a second statement.
    #[test]
    fn quote_ident_defuses_hostile_introspected_names() {
        assert_eq!(
            quote_ident(r#"users"; DROP TABLE x;--"#),
            r#""users""; DROP TABLE x;--""#
        );
        assert_eq!(quote_ident("order items"), r#""order items""#);
        assert_eq!(quote_ident("public.users"), "public.users");
        assert_eq!(quote_ident("weird schema.users"), r#""weird schema".users"#);
        assert_eq!(quote_ident("_ok123"), "_ok123");
    }
}

#[cfg(test)]
mod folder_grouping_tests {
    use sid_core::db::DbKind;

    use super::*;

    fn conn(id: &str, folder: Option<&str>) -> Attributed<DbConnection> {
        Attributed {
            item: DbConnection {
                id: id.to_string(),
                dsn: "d".to_string(),
                secret_ref: None,
                kind: DbKind::Postgres,
                name: id.to_string(),
                folder: folder.map(str::to_string),
            },
            origin: Scope::Global,
            duplicate: false,
        }
    }

    #[test]
    fn all_ungrouped_connections_stay_in_incoming_order() {
        let conns = vec![conn("b", None), conn("a", None)];
        let rows = group_connections(&conns, &HashSet::new());
        assert_eq!(
            rows,
            vec![
                ConnRow::Connection { id: "b".into() },
                ConnRow::Connection { id: "a".into() },
            ]
        );
    }

    /// A present-but-empty `folder` (e.g. a legacy record, or a folder-edit committed
    /// as an all-whitespace string that only got trimmed at the UI layer) is normalized
    /// to ungrouped rather than becoming a nameless folder header.
    #[test]
    fn a_blank_folder_string_is_treated_as_ungrouped() {
        let conns = vec![conn("a", Some(""))];
        let rows = group_connections(&conns, &HashSet::new());
        assert_eq!(rows, vec![ConnRow::Connection { id: "a".into() }]);
    }

    /// Murphy's "None → ungrouped top level": ungrouped connections lead the row list,
    /// ahead of every folder, regardless of insertion order.
    #[test]
    fn ungrouped_connections_come_before_folders() {
        let conns = vec![conn("in-folder", Some("acme")), conn("top-level", None)];
        let rows = group_connections(&conns, &HashSet::new());
        assert_eq!(
            rows,
            vec![
                ConnRow::Connection {
                    id: "top-level".into()
                },
                ConnRow::Folder {
                    name: "acme".into(),
                    expanded: true,
                    count: 1
                },
                ConnRow::Connection {
                    id: "in-folder".into()
                },
            ]
        );
    }

    #[test]
    fn folders_are_sorted_alphabetically() {
        let conns = vec![conn("z", Some("zeta")), conn("a", Some("alpha"))];
        let rows = group_connections(&conns, &HashSet::new());
        assert_eq!(
            rows,
            vec![
                ConnRow::Folder {
                    name: "alpha".into(),
                    expanded: true,
                    count: 1
                },
                ConnRow::Connection { id: "a".into() },
                ConnRow::Folder {
                    name: "zeta".into(),
                    expanded: true,
                    count: 1
                },
                ConnRow::Connection { id: "z".into() },
            ]
        );
    }

    /// Collapsing a folder (Task 2's "collapsible folder headers") hides its members
    /// but keeps the header itself (with its member count) visible.
    #[test]
    fn a_collapsed_folder_hides_its_members_but_keeps_its_header() {
        let conns = vec![conn("a", Some("acme")), conn("b", Some("acme"))];
        let mut collapsed = HashSet::new();
        collapsed.insert("acme".to_string());
        let rows = group_connections(&conns, &collapsed);
        assert_eq!(
            rows,
            vec![ConnRow::Folder {
                name: "acme".into(),
                expanded: false,
                count: 2
            }]
        );
    }
}
#[cfg(test)]
mod result_view_tests {
    use super::*;

    /// A column's *name* plays no part in sorting or filtering — only its declared
    /// type does — so these fixtures are type lists, not `DbColumn` lists.
    fn col(_name: &str, ty: ColumnType) -> ColumnType {
        ty
    }

    fn row(values: &[&str]) -> Row {
        Row {
            values: values.iter().map(|v| v.to_string()).collect(),
        }
    }

    /// The displayed values of column `ix`, in display order.
    fn column_of(rows: &[Row], ix: usize) -> Vec<String> {
        rows.iter()
            .map(|r| r.values.get(ix).cloned().unwrap_or_default())
            .collect()
    }

    fn sorted(columns: &[ColumnType], rows: &[Row], col_ix: usize, descending: bool) -> Vec<Row> {
        visible_rows(columns, rows, "", Some(ResultSort { col_ix, descending }))
    }

    // ---- ordering: numbers are numbers -------------------------------------------

    #[test]
    fn an_integer_column_orders_numerically_not_lexicographically() {
        // The reason a page of display strings cannot just be `sort()`ed: every value
        // in the grid is text, and as text "10" sorts before "9".
        let columns = vec![col("n", ColumnType::Integer)];
        let rows = vec![row(&["9"]), row(&["10"]), row(&["-2"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 0),
            vec!["-2", "9", "10"]
        );
    }

    #[test]
    fn a_float_column_orders_numerically() {
        let columns = vec![col("f", ColumnType::Float)];
        let rows = vec![row(&["9.5"]), row(&["10.25"]), row(&["-0.5"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 0),
            vec!["-0.5", "9.5", "10.25"]
        );
    }

    #[test]
    fn a_bignum_past_f64_precision_still_orders_exactly() {
        // Postgres `numeric`/`int8` can carry values an f64 cannot distinguish.
        // Parsing as i128 first is what keeps two neighbouring bignums from
        // collapsing to "equal" and silently keeping their original order.
        let columns = vec![col("n", ColumnType::Integer)];
        let rows = vec![
            row(&["9007199254740993"]),
            row(&["9007199254740992"]),
            row(&["9007199254740994"]),
        ];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 0),
            vec!["9007199254740992", "9007199254740993", "9007199254740994"]
        );
    }

    #[test]
    fn a_numeric_column_holding_an_unparseable_value_falls_back_to_text() {
        // A driver can render something that is not a bare number into a column it
        // typed as numeric (a Postgres `money`, an out-of-range literal, a
        // driver-specific placeholder). The order must stay total and deterministic
        // rather than collapsing every such pair to "equal".
        let columns = vec![col("n", ColumnType::Integer)];
        let rows = vec![row(&["b"]), row(&["a"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 0),
            vec!["a", "b"]
        );
    }

    // ---- ordering: text ------------------------------------------------------------

    #[test]
    fn a_text_column_orders_case_insensitively() {
        // Byte order would put every capitalised value ahead of every lowercase one,
        // which reads as "not sorted" to anyone looking at a name column.
        let columns = vec![col("t", ColumnType::Text)];
        let rows = vec![row(&["banana"]), row(&["Apple"]), row(&["cherry"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 0),
            vec!["Apple", "banana", "cherry"]
        );
    }

    #[test]
    fn text_ordering_is_total_so_a_case_only_difference_still_has_an_answer() {
        // Case-insensitive alone makes "ABC" and "abc" equal, which leaves their
        // relative order to whatever the sort happened to do. The case-sensitive
        // tiebreak makes the comparator a total order.
        let columns = vec![col("t", ColumnType::Text)];
        let rows = vec![row(&["abc"]), row(&["ABC"])];
        let asc = column_of(&sorted(&columns, &rows, 0, false), 0);
        assert_eq!(asc, vec!["ABC", "abc"]);
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, true), 0),
            vec!["abc", "ABC"]
        );
    }

    #[test]
    fn a_driver_specific_column_orders_as_text() {
        // `Other` is where uuid/json/timestamptz land. Text is the only ordering sid
        // can honestly claim for a type it does not recognise.
        let columns = vec![col("u", ColumnType::Other("uuid".into()))];
        let rows = vec![row(&["b"]), row(&["a"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 0),
            vec!["a", "b"]
        );
    }

    // ---- ordering: booleans --------------------------------------------------------

    #[test]
    fn a_bool_column_orders_false_before_true_in_every_spelling() {
        // Postgres renders `true`/`false`; SQLite has no boolean type at all, so a
        // column *declared* BOOLEAN arrives as `1`/`0`. Both have to order the same
        // way, and neither may fall back to text (where "false" < "true" is right by
        // accident and "0" < "1" is right by accident, but "f" < "t" and "no" < "yes"
        // are not guaranteed to keep meaning anything).
        for (f, t) in [("false", "true"), ("0", "1"), ("f", "t"), ("no", "yes")] {
            let columns = vec![col("b", ColumnType::Bool)];
            let rows = vec![row(&[t]), row(&[f])];
            assert_eq!(
                column_of(&sorted(&columns, &rows, 0, false), 0),
                vec![f, t],
                "{f}/{t}"
            );
        }
    }

    // ---- NULL placement ------------------------------------------------------------

    #[test]
    fn nulls_sort_last_ascending() {
        let columns = vec![col("n", ColumnType::Integer)];
        let rows = vec![row(&["2"]), row(&["NULL"]), row(&["1"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 0),
            vec!["1", "2", "NULL"]
        );
    }

    #[test]
    fn nulls_sort_last_descending_too() {
        // The decision: NULLs are always last, in BOTH directions — not Postgres's
        // "NULLS LAST ascending, NULLS FIRST descending". A user sorts a grid to bring
        // the interesting extreme to the top; a screen of empty cells there is the one
        // thing they were not looking for.
        let columns = vec![col("n", ColumnType::Integer)];
        let rows = vec![row(&["2"]), row(&["NULL"]), row(&["1"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, true), 0),
            vec!["2", "1", "NULL"]
        );
    }

    #[test]
    fn a_row_missing_the_sort_column_sorts_as_null() {
        // Ragged rows are a driver bug, not a crash — and an absent value is as
        // absent as an explicit one.
        let columns = vec![col("a", ColumnType::Text), col("b", ColumnType::Text)];
        let rows = vec![row(&["x"]), row(&["y", "b"]), row(&["z", "a"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 1, false), 0),
            vec!["z", "y", "x"]
        );
    }

    #[test]
    fn an_empty_string_is_a_value_and_not_a_null() {
        // `''` and NULL are different facts about a row and must not be merged: the
        // empty string is an ordinary value that sorts first, NULL is absence and
        // sorts last.
        let columns = vec![col("t", ColumnType::Text)];
        let rows = vec![row(&["NULL"]), row(&["b"]), row(&[""])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 0),
            vec!["", "b", "NULL"]
        );
    }

    // ---- stability -----------------------------------------------------------------

    #[test]
    fn rows_with_equal_keys_keep_the_order_the_driver_returned() {
        // A stable sort is what makes the grid's third click ("back to unsorted")
        // meaningful and what stops rows from shuffling under the cursor when the
        // same sort is re-applied.
        let columns = vec![col("k", ColumnType::Text), col("id", ColumnType::Integer)];
        let rows = vec![
            row(&["same", "1"]),
            row(&["same", "2"]),
            row(&["same", "3"]),
        ];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, false), 1),
            vec!["1", "2", "3"]
        );
        assert_eq!(
            column_of(&sorted(&columns, &rows, 0, true), 1),
            vec!["1", "2", "3"]
        );
    }

    #[test]
    fn no_sort_hands_back_the_page_exactly_as_the_driver_returned_it() {
        // What the third click restores. "Unsorted" has to mean the engine's own
        // order — the order that an ORDER BY in the user's SQL produced — not some
        // arbitrary fallback.
        let columns = vec![col("n", ColumnType::Integer)];
        let rows = vec![row(&["3"]), row(&["1"]), row(&["2"])];
        assert_eq!(
            column_of(&visible_rows(&columns, &rows, "", None), 0),
            vec!["3", "1", "2"]
        );
    }

    #[test]
    fn a_sort_column_past_the_end_of_the_page_is_ignored() {
        // The column plan is rebuilt with every page. A sort index left over from a
        // wider page must degrade to "unsorted", never panic.
        let columns = vec![col("n", ColumnType::Integer)];
        let rows = vec![row(&["3"]), row(&["1"])];
        assert_eq!(
            column_of(&sorted(&columns, &rows, 7, false), 0),
            vec!["3", "1"]
        );
    }

    // ---- filtering -------------------------------------------------------------------

    #[test]
    fn an_empty_or_blank_query_shows_every_row() {
        let columns = vec![col("t", ColumnType::Text)];
        let rows = vec![row(&["a"]), row(&["b"])];
        assert_eq!(visible_rows(&columns, &rows, "", None).len(), 2);
        assert_eq!(visible_rows(&columns, &rows, "   ", None).len(), 2);
    }

    #[test]
    fn the_filter_is_case_insensitive_and_matches_any_cell() {
        // One box over the whole row, not per column: the question a user asks a
        // result grid is "where is Phoenix", not "which column is Phoenix in".
        let columns = vec![col("city", ColumnType::Text), col("note", ColumnType::Text)];
        let rows = vec![
            row(&["Phoenix", "on time"]),
            row(&["Austin", "shipped to PHOENIX by mistake"]),
            row(&["Springfield", "on time"]),
        ];
        assert_eq!(visible_rows(&columns, &rows, "phoenix", None).len(), 2);
        assert_eq!(visible_rows(&columns, &rows, "  PHOENIX ", None).len(), 2);
    }

    #[test]
    fn a_query_that_matches_nothing_shows_no_rows() {
        // Not "everything", which is the failure mode of a filter that treats an
        // unmatched query as no filter at all.
        let columns = vec![col("t", ColumnType::Text)];
        let rows = vec![row(&["a"]), row(&["b"])];
        assert!(visible_rows(&columns, &rows, "zzz", None).is_empty());
    }

    #[test]
    fn filtering_and_sorting_compose() {
        // Filter first, then order what survives — so the grid's count and its first
        // row agree with each other.
        let columns = vec![col("city", ColumnType::Text), col("n", ColumnType::Integer)];
        let rows = vec![
            row(&["Phoenix", "3"]),
            row(&["Austin", "1"]),
            row(&["Phoenix", "2"]),
        ];
        let out = visible_rows(
            &columns,
            &rows,
            "phoenix",
            Some(ResultSort {
                col_ix: 1,
                descending: false,
            }),
        );
        assert_eq!(column_of(&out, 1), vec!["2", "3"]);
    }

    // ---- the honesty line --------------------------------------------------------------

    #[test]
    fn a_complete_result_set_carries_no_caveat() {
        // The whole result set is on screen, so a page sort IS the sort. Saying
        // otherwise would be a warning that cries wolf on every small query.
        assert_eq!(page_view_caveat(true, true, false), None);
        assert_eq!(page_view_caveat(true, false, false), None);
        assert_eq!(page_view_caveat(false, true, false), None);
    }

    #[test]
    fn an_untouched_partial_result_set_carries_no_caveat() {
        // More pages exist, but the user has not asked for an ordering or a subset —
        // there is nothing to be wrong about yet.
        assert_eq!(page_view_caveat(false, false, true), None);
    }

    #[test]
    fn a_partial_result_set_names_exactly_what_is_page_local() {
        // The documented limitation, surfaced where the user can see it rather than
        // only in a doc comment.
        let sorted_only = page_view_caveat(true, false, true).expect("a caveat");
        assert!(sorted_only.contains("sorted"), "{sorted_only:?}");
        assert!(!sorted_only.contains("filtered"), "{sorted_only:?}");

        let filtered_only = page_view_caveat(false, true, true).expect("a caveat");
        assert!(filtered_only.contains("filtered"), "{filtered_only:?}");
        assert!(!filtered_only.contains("sorted"), "{filtered_only:?}");

        let both = page_view_caveat(true, true, true).expect("a caveat");
        assert!(
            both.contains("sorted") && both.contains("filtered"),
            "{both:?}"
        );
    }

    #[test]
    fn every_caveat_says_it_is_about_this_page() {
        // Whatever the wording, the word that makes it true has to be there.
        for (sorted, filtered) in [(true, false), (false, true), (true, true)] {
            let caveat = page_view_caveat(sorted, filtered, true).expect("a caveat");
            assert!(
                caveat.contains("page"),
                "{sorted}/{filtered}: {caveat:?} never says which rows it means"
            );
        }
    }
}
#[cfg(test)]
mod plan_view_tests {
    use super::*;

    fn page(columns: &[&str], rows: &[&[&str]]) -> QueryPage {
        QueryPage {
            columns: columns
                .iter()
                .map(|c| DbColumn {
                    name: c.to_string(),
                    ty: ColumnType::Text,
                })
                .collect(),
            rows: rows
                .iter()
                .map(|r| Row {
                    values: r.iter().map(|v| v.to_string()).collect(),
                })
                .collect(),
            next_cursor: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn a_single_column_plan_keeps_each_row_verbatim() {
        // Postgres returns one `QUERY PLAN` column whose rows already carry the
        // tree's indentation. That indentation IS the tree — this is the entire
        // reason a plan does not go through the results grid, where `single_line`
        // would collapse it.
        let plan = page(
            &["QUERY PLAN"],
            &[
                &["Hash Join  (cost=1.09..2.20 rows=3 width=68)"],
                &["  Hash Cond: (o.customer_id = c.id)"],
                &["  ->  Seq Scan on orders o  (cost=0.00..1.03 rows=3 width=40)"],
            ],
        );
        assert_eq!(
            plan_lines(&plan),
            vec![
                "Hash Join  (cost=1.09..2.20 rows=3 width=68)".to_string(),
                "  Hash Cond: (o.customer_id = c.id)".to_string(),
                "  ->  Seq Scan on orders o  (cost=0.00..1.03 rows=3 width=40)".to_string(),
            ]
        );
    }

    #[test]
    fn a_multi_column_plan_joins_its_columns_rather_than_dropping_any() {
        // SQLite's `EXPLAIN QUERY PLAN` returns id/parent/notused/detail. `detail` is
        // the readable part, but sid is not in the business of deciding which of an
        // engine's plan columns the user is allowed to see.
        let plan = page(
            &["id", "parent", "notused", "detail"],
            &[&["3", "0", "0", "SCAN customers"]],
        );
        assert_eq!(plan_lines(&plan), vec!["3  0  0  SCAN customers"]);
    }

    #[test]
    fn an_empty_plan_produces_no_lines() {
        // An engine can legitimately return nothing (a statement with no plan). The
        // pane shows its own "no plan" hint rather than a blank scroll area.
        assert!(plan_lines(&page(&["QUERY PLAN"], &[])).is_empty());
        assert!(plan_lines(&page(&[], &[])).is_empty());
    }

    #[test]
    fn a_plan_row_with_no_values_is_still_a_line() {
        // A blank line in a plan is a blank line, not a row to skip — dropping it
        // would silently re-flow the tree.
        assert_eq!(plan_lines(&page(&["QUERY PLAN"], &[&[""]])), vec![""]);
    }
}
