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
use gpui_component::input::{Input, InputEvent, InputState, Position};
use gpui_component::table::{Column, Table, TableDelegate, TableState};
use gpui_component::{Root, Theme, ThemeMode};
use sid_core::db::{
    DbClient, DbError, OpenParams, PageCursor, QueryPage, Row, SchemaGraph, SchemaInfo, TableInfo,
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

// DANGER matches app.rs's palette (used by the delete action's armed state).
const DANGER: u32 = 0xd08a8a;

// Dark-theme palette, aligned with `app.rs`. Kept local so `ui` stays self-contained
// (same convention as `host_form.rs`).
const BG: u32 = 0x161618;
const BORDER: u32 = 0x2c2c30;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_BG: u32 = 0x33343a;
const ACTIVE_FG: u32 = 0xffffff;
const BRAND: u32 = 0x5a9ad0;
const WS_FG: u32 = 0xa98bd0;
/// Monospace family for the DSN subtitle; matches `app.rs`'s host rows.
const MONO: &str = "DejaVu Sans Mono";

// W5: query pane palette (the editor/results border+fill), matching `db_conn_form.rs`'s
// field styling so the tab reads as one surface.
const FIELD_BG: u32 = 0x121215;
const FIELD_BORDER: u32 = 0x33343a;

/// Seeded into the SQL editor on first paint — works unmodified against every engine
/// (SQLite, Postgres, and the redb browse engine all accept a bare `select 1;`), so it
/// isn't tied to the demo SQLite connection's schema.
const DEMO_SQL: &str = "select 1;";

/// Rows per `query_paged` call. Small enough to make the "⭳ next page" control
/// exercisable by hand against the demo seed without a huge fixture table.
const PAGE_SIZE: u32 = 100;

// ---- increment 2: schema tree / cell copy-view / CSV export / history --------------------

/// A result cell longer than this (in `char`s) gets a `👁 view` affordance opening the
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
    /// triggers (connection switch, re-fetch). Feeds the "⧉ diagram" pop-out window
    /// (`db_diagram::DiagramView`); `None` before the first successful fetch, same as
    /// `schema`.
    schema_graph: Option<SchemaGraph>,
    /// True while a `schema_introspect` task is in flight — guards re-entrant
    /// selection/⟳ clicks the same way `running` guards Run.
    schema_loading: bool,
    schema_error: Option<String>,
    /// Which tables are expanded (columns visible), keyed by [`table_display_name`].
    /// Cleared whenever the active connection changes or the schema is re-fetched.
    schema_expanded: HashSet<String>,

    // ---- D2: cell copy / view -------------------------------------------------------
    /// The `👁 view` popover's contents, if open.
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

    // ---- selector-right: folders / inline rename ------------------------------------
    /// Which folders are collapsed in the connection rail, keyed by folder name.
    /// Presence encodes "collapsed" (mirrors `schema_expanded`'s presence-encodes-state
    /// convention, inverted: there, presence means expanded — here it means collapsed,
    /// since a freshly-created folder should default to expanded/visible).
    collapsed_folders: HashSet<String>,
    /// The connection rail's own focus handle — lazily created by `ensure_query_widgets`
    /// (needs `Context`, unlike `DbTabState::new`). Focused on every row click so a
    /// subsequent F2 (with no text field focused) reaches [`AppState::begin_rename_active`]
    /// via the rail's `on_key_down`.
    rail_focus: Option<FocusHandle>,
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

/// The `👁 view` popover's contents (D2) — the column a long cell came from, and its
/// full (untruncated) text. Read-only; mirrors `session.rs`'s `Preview`/`PreviewContent`
/// shape but simpler (no oversize/binary cases — grid cells are always the display
/// strings `DbClient` already rendered to text).
#[derive(Clone, Debug, PartialEq, Eq)]
struct CellView {
    column: String,
    text: String,
}

/// Outcome of the last query run, driving the query pane's status line.
enum QueryStatus {
    Idle,
    Err(String),
    Ok {
        rows: usize,
        duration_ms: u64,
        has_more: bool,
    },
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

/// Backs the results [`Table`]. Constructed empty by `ensure_query_widgets`, then
/// mutated in place (`set_page`) whenever a query completes — see the `results` field
/// doc comment for why it's never rebuilt.
struct ResultDelegate {
    columns: Vec<Column>,
    rows: Vec<Row>,
    /// Handle back to the owning [`AppState`], used only by D2's `👁 view` click (open
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
            columns: Vec::new(),
            rows: Vec::new(),
            app: None,
        }
    }

    fn set_page(&mut self, page: QueryPage) {
        self.columns = page
            .columns
            .iter()
            .map(|c| Column::new(c.name.clone(), c.name.clone()).width(px(140.)))
            .collect();
        self.rows = page.rows;
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
        &self.columns[col_ix]
    }

    /// D2: the whole cell copies its text to the clipboard on click; a cell over
    /// [`CELL_VIEW_THRESHOLD`] chars also gets a `👁` button opening the read-only view
    /// popover. The `👁` click sits inside the cell's own click area, so it fires the
    /// copy handler too (harmless — the same convention `app.rs`'s row action buttons
    /// use: "a click here also fires the row's on_click... which is harmless").
    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self.rows[row_ix]
            .values
            .get(col_ix)
            .cloned()
            .unwrap_or_default();
        let column_name = self
            .columns
            .get(col_ix)
            .map(|c| c.name.to_string())
            .unwrap_or_default();
        let cell_ix = row_ix * 4096 + col_ix;

        let copy_text = text.clone();
        let copy_app = self.app.clone();
        let view_button = (text.chars().count() > CELL_VIEW_THRESHOLD).then(|| {
            let view_app = self.app.clone();
            let view_text = text.clone();
            let view_column = column_name.clone();
            div()
                .id(("db-cell-view", cell_ix))
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_color(rgb(BRAND))
                .hover(|s| s.bg(rgb(ACTIVE_BG)))
                .child("👁")
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

        div()
            .id(("db-cell", cell_ix))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ACTIVE_BG)))
            .child(div().flex_1().text_xs().text_color(rgb(FG)).child(text))
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

// ---- selector-right: folder grouping (pure `connections -> rail rows` transform) --------

/// One renderable row of the connection rail — a collapsible folder header, or a
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
    /// A connection's id — the rail row re-looks this up in `self.db.connections` at
    /// render time (rather than cloning the whole `Attributed<DbConnection>` in here) so
    /// this stays a plain identity list, matching how `active_id`/`armed_delete` already
    /// key rows by id rather than by index.
    Connection { id: String },
}

/// Group `connections` by [`DbConnection::folder`] (one flat level — see that field's
/// own doc comment) into the connection rail's row list: every ungrouped connection
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
            last_page: None,
            schema: None,
            schema_graph: None,
            schema_loading: false,
            schema_error: None,
            schema_expanded: HashSet::new(),
            cell_view: None,
            notice: None,
            export_menu_open: false,
            history: Vec::new(),
            collapsed_folders: HashSet::new(),
            rail_focus: None,
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

        // The saved-connection picker moved to the right-edge rail (`connection_rail`,
        // built inside `query_pane`) — Murphy: "the database selector should be on the
        // right side for the connection i want to start". This top strip is now just
        // the tab's shared error line (still needed: promote/demote/delete/rename/
        // folder-edit failures all land in `self.error`), collapsing to nothing when
        // there is none rather than reserving dead space.
        let error_line = self.error.clone().map(|e| {
            div()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .text_sm()
                .text_color(rgb(DANGER))
                .child(format!("error: {e}"))
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .children(error_line)
            .child(self.query_pane(cx))
            .children(self.cell_view_overlay(window, cx))
            .into_any_element()
    }

    /// D2's `👁 view` popover — `None` when nothing is being viewed. Mirrors
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
                                .bg(rgb(BG))
                                .border_1()
                                .border_color(rgb(BORDER))
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
                                        .border_color(rgb(BORDER))
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(FG))
                                                .child(cell.column.clone()),
                                        )
                                        .child(
                                            div()
                                                .id("db-cell-view-close")
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .cursor_pointer()
                                                .text_color(rgb(FG_DIM))
                                                .hover(|s| s.bg(rgb(ACTIVE_BG)))
                                                .child("✕ close")
                                                .on_click(cx.listener(
                                                    |this, _ev: &ClickEvent, _window, cx| {
                                                        this.db.cell_view = None;
                                                        cx.notify();
                                                    },
                                                )),
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
                                        .text_color(rgb(FG))
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
        if self.db.rail_focus.is_none() {
            self.db.rail_focus = Some(cx.focus_handle());
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
        self.db._sql_subscription = Some(cx.subscribe(&sql, Self::on_sql_event));
        self.db.sql = Some(sql);
        // D2: hand the results table's delegate a weak handle back to `AppState` so a
        // cell's `👁 view` click (which only sees `&mut App`, not `AppState` — see
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

    /// Ctrl/Cmd-Enter in the SQL editor runs the query. Plain Enter inserts a newline
    /// (handled inside `InputState` itself — multi-line/code-editor mode) and is not
    /// acted on here.
    fn on_sql_event(
        &mut self,
        _sql: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { secondary: true } = event {
            self.run_query(cx);
        }
    }

    /// The SQL editor + Run/next-page controls + status line + results table, below the
    /// connection picker. Always rendered; Run/next-page are no-ops (surfaced as a
    /// status message) with no active connection rather than being conditionally absent.
    fn query_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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

        let (status_text, status_color): (SharedString, u32) = match &self.db.status {
            QueryStatus::Idle => ("".into(), FG_DIM),
            QueryStatus::Err(e) => (format!("✗ {e}").into(), DANGER),
            QueryStatus::Ok {
                rows, duration_ms, ..
            } => (format!("✓ {rows} rows · {duration_ms} ms").into(), FG_DIM),
        };
        let has_more = matches!(&self.db.status, QueryStatus::Ok { has_more: true, .. });
        let run_label = if self.db.running { "…" } else { "▶ Run" };

        let next_page = has_more.then(|| {
            div()
                .id("db-next-page")
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(BRAND))
                .hover(|s| s.bg(rgb(ACTIVE_BG)))
                .child("⭳ next page")
                .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                    this.next_page(cx);
                }))
        });

        let sql_editor = self.db.sql.clone().map(|sql| {
            div()
                .h(px(140.))
                .rounded_md()
                .border_1()
                .border_color(rgb(FIELD_BORDER))
                .bg(rgb(FIELD_BG))
                .child(Input::new(&sql))
        });
        let results_table = self
            .db
            .results
            .clone()
            .map(|t| div().flex_1().w_full().child(Table::new(&t).stripe(true)));

        let notice = self
            .db
            .notice
            .clone()
            .map(|n| div().text_xs().text_color(rgb(FG_DIM)).child(n));

        let editor_and_results = div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(FG_DIM))
                            .child(active_label),
                    )
                    .children(next_page)
                    .child(
                        div()
                            .id("db-run")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .text_color(rgb(ACTIVE_FG))
                            .bg(rgb(BRAND))
                            .hover(|s| s.opacity(0.85))
                            .child(run_label)
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.run_query(cx);
                            })),
                    )
                    // Far right, after Run (Murphy: "download as csv should be on the
                    // far right") — the generic export control (Task 1).
                    .child(self.export_control(cx)),
            )
            .children(sql_editor)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(status_color))
                    .child(status_text),
            )
            .children(notice)
            .children(results_table);

        div()
            .flex()
            .flex_row()
            .flex_1()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(self.left_sidebar(cx))
            .child(editor_and_results)
            .child(self.connection_rail(cx))
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
        let button = div()
            .id("db-export-open")
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .text_color(rgb(BRAND))
            .hover(|s| s.bg(rgb(ACTIVE_BG)))
            .child("⭳ Export ▾")
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
                        div()
                            .id("db-export-menu")
                            .occlude()
                            .mt_1()
                            .min_w(px(180.))
                            .flex()
                            .flex_col()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(BG))
                            .py_1()
                            .children(ExportFormat::ALL.iter().enumerate().map(|(ix, fmt)| {
                                let fmt = *fmt;
                                div()
                                    .id(("db-export-item", ix))
                                    .px_3()
                                    .py_1()
                                    .text_xs()
                                    .cursor_pointer()
                                    .text_color(rgb(FG))
                                    .hover(|s| s.bg(rgb(ACTIVE_BG)))
                                    .child(fmt.label())
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

    /// D1 + D4's left sidebar: the schema tree (claims most of the vertical space) atop
    /// a fixed-height query-history panel — both live beside the editor/results per the
    /// plan, sharing one column since a third side-by-side column would crowd the tab.
    fn left_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .w(px(220.))
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.schema_tree_panel(cx))
            .child(self.history_panel(cx))
    }

    /// D1: the schema tree panel — a `⟳` refresh header over a flat, indented list of
    /// tables (click name -> insert `SELECT * FROM <table>`; click chevron -> expand to
    /// show columns). Pure-from-cache: reads `self.db.schema`/`schema_expanded` only,
    /// never touches the runtime itself (that's `refresh_schema`'s job).
    fn schema_tree_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(div().text_xs().text_color(rgb(FG_DIM)).child("schema"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(self.diagram_button(cx))
                    .child(
                        div()
                            .id("db-schema-refresh")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(BRAND))
                            .hover(|s| s.bg(rgb(ACTIVE_BG)))
                            .child(if self.db.schema_loading { "…" } else { "⟳" })
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.refresh_schema(cx);
                            })),
                    ),
            );

        let body: AnyElement = if self.db.schema_loading && self.db.schema.is_none() {
            div()
                .p_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child("loading schema…")
                .into_any_element()
        } else if let Some(err) = &self.db.schema_error {
            div()
                .p_2()
                .text_xs()
                .text_color(rgb(DANGER))
                .child(format!("✗ {err}"))
                .into_any_element()
        } else {
            let rows = match &self.db.schema {
                Some(schema) => schema_tree_rows(schema, &self.db.schema_expanded),
                None => Vec::new(),
            };
            if rows.is_empty() {
                div()
                    .p_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child("no schema loaded — select a connection")
                    .into_any_element()
            } else {
                div()
                    .id("db-schema-tree-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .children(
                        rows.into_iter()
                            .enumerate()
                            .map(|(ix, row)| self.schema_tree_row(ix, row, cx)),
                    )
                    .into_any_element()
            }
        };

        div()
            .flex_1()
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(rgb(FIELD_BORDER))
            .bg(rgb(FIELD_BG))
            .child(header)
            .child(body)
    }

    /// "⧉ diagram" — opens the Access-style relationships pop-out window (see
    /// [`Self::open_diagram_window`]). Enabled (brand-colored, clickable) only once a
    /// schema is cached for the active connection; otherwise rendered dim and inert
    /// rather than hidden, matching this tab's convention of always-present, sometimes
    /// no-op controls (see `query_pane`'s doc comment on Run/next-page).
    fn diagram_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.db.schema.is_some();
        let button = div()
            .id("db-diagram-open")
            .px_1()
            .rounded_sm()
            .text_color(rgb(if enabled { BRAND } else { FG_DIM }))
            .child("⧉ diagram");
        if enabled {
            button
                .cursor_pointer()
                .hover(|s| s.bg(rgb(ACTIVE_BG)))
                .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                    this.open_diagram_window(window, cx);
                }))
                .into_any_element()
        } else {
            button.into_any_element()
        }
    }

    /// Open the relationships diagram in its own OS window — a snapshot of the cached
    /// [`SchemaInfo`] + [`SchemaGraph`] handed to a fresh [`DiagramView`] entity.
    /// Synchronous: sid is a single [`gpui::Application`] and `Context` derefs to
    /// `App`, so `cx.open_window` opens a second top-level window in the same process
    /// (no second instance, no subprocess) right here in the click handler. Cribs the
    /// window-bootstrap shape from `main.rs` exactly — `Root::new` must be the window's
    /// first layer and `Theme::change` must run before anything paints, or
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
    fn open_diagram_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(schema) = self.db.schema.clone() else {
            return;
        };
        let graph = self.db.schema_graph.clone().unwrap_or_default();
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

        let bounds = Bounds::centered(None, size(px(1000.), px(700.)), cx);
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                Theme::change(ThemeMode::Dark, Some(window), cx);
                let view = cx.new(|_cx| DiagramView::new(schema, graph, app, main_window));
                cx.new(|cx| Root::new(view, window, cx))
            },
        );
    }

    /// One [`SchemaRow`]'s rendering — a table header (chevron toggles expand, name
    /// inserts `SELECT * FROM <table>`) or an indented column leaf.
    fn schema_tree_row(&self, ix: usize, row: SchemaRow, cx: &mut Context<Self>) -> AnyElement {
        match row {
            SchemaRow::Table {
                display_name,
                expanded,
            } => {
                let chevron_name = display_name.clone();
                let insert_name = display_name.clone();
                div()
                    .id(("db-schema-table", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .id(("db-schema-toggle", ix))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(FG_DIM))
                            .child(if expanded { "▾" } else { "▸" })
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                                this.toggle_schema_table(&chevron_name, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("db-schema-name", ix))
                            .flex_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(FG))
                            .hover(|s| s.bg(rgb(ACTIVE_BG)))
                            .child(display_name.clone())
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                                this.insert_select_star(&insert_name, window, cx);
                            })),
                    )
                    .into_any_element()
            }
            SchemaRow::Column { name } => div()
                .id(("db-schema-col", ix))
                .pl_6()
                .pr_2()
                .py_1()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(name)
                .into_any_element(),
        }
    }

    /// D4: the query-history panel — most-recent-first, click an entry to reload it
    /// (unmodified) into the SQL editor.
    fn history_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let entries = self.db.history.clone();
        let header = div()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .text_color(rgb(FG_DIM))
            .child("history");

        let body: AnyElement = if entries.is_empty() {
            div()
                .p_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child("no queries run yet")
                .into_any_element()
        } else {
            div()
                .id("db-history-body")
                .flex()
                .flex_col()
                .flex_1()
                .overflow_y_scroll()
                .children(entries.into_iter().enumerate().map(|(ix, sql)| {
                    let full = sql.clone();
                    let label: SharedString = if sql.chars().count() > 34 {
                        let head: String = sql.chars().take(34).collect();
                        format!("{head}…").into()
                    } else {
                        sql.clone().into()
                    };
                    div()
                        .id(("db-history", ix))
                        .px_2()
                        .py_1()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(FG))
                        .hover(|s| s.bg(rgb(ACTIVE_BG)))
                        .child(label)
                        .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                            this.reload_history_entry(&full, window, cx);
                        }))
                }))
                .into_any_element()
        };

        div()
            .h(px(160.))
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(rgb(FIELD_BORDER))
            .bg(rgb(FIELD_BG))
            .child(header)
            .child(body)
    }

    /// The DBeaver-style connection selector, moved to the right edge (Murphy: "the
    /// database selector should be on the right side for the connection i want to
    /// start"). Groups the composed connection list by [`DbConnection::folder`] via
    /// [`group_connections`] under a small `connections · N` / `＋` header. Also the
    /// F2 target: focused on every row click (see [`Self::render_connection_row`]) so
    /// F2 with no text field focused reaches [`Self::begin_rename_active`] — the
    /// double-click-a-name path (also wired in `render_connection_row`) needs no focus
    /// of its own.
    fn connection_rail(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let count = self.db.connections.len();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(format!("connections · {count}")),
            )
            .child(
                div()
                    .id("db-rail-add")
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(BRAND))
                    .hover(|s| s.bg(rgb(ACTIVE_BG)))
                    .child("＋")
                    .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                        this.open_add_db_form(window, cx);
                    })),
            );

        let rows = group_connections(&self.db.connections, &self.db.collapsed_folders);
        let body: AnyElement = if rows.is_empty() {
            div()
                .p_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child("no connections yet")
                .into_any_element()
        } else {
            div()
                .id("db-rail-body")
                .flex()
                .flex_col()
                .flex_1()
                .overflow_y_scroll()
                .children(
                    rows.into_iter()
                        .enumerate()
                        .map(|(ix, row)| self.connection_rail_row(ix, row, cx)),
                )
                .into_any_element()
        };

        let focus_handle = self.db.rail_focus.clone();
        div()
            .id("db-rail")
            .w(px(240.))
            .h_full()
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(rgb(FIELD_BORDER))
            .bg(rgb(FIELD_BG))
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
    fn connection_rail_row(&self, ix: usize, row: ConnRow, cx: &mut Context<Self>) -> AnyElement {
        match row {
            ConnRow::Folder {
                name,
                expanded,
                count,
            } => {
                let toggle_name = name.clone();
                div()
                    .id(("db-folder", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(ACTIVE_BG)))
                    .child(div().text_xs().text_color(rgb(FG_DIM)).child(if expanded {
                        "▾"
                    } else {
                        "▸"
                    }))
                    .child(div().flex_1().text_xs().text_color(rgb(FG_DIM)).child(name))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(FG_DIM))
                            .child(count.to_string()),
                    )
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

    /// One connection's row in the rail: its name (a live rename [`TextInput`] in
    /// place, mid-rename) plus origin badge and `★` active marker, a DSN subtitle (a
    /// live folder-edit [`TextInput`] in place, mid-folder-edit), and the
    /// promote/demote/edit/📁/delete action strip. Structurally the pre-selector-move
    /// row (W3's `db_connection_row`), restacked into the rail's narrower 240px column
    /// and extended with the rename/folder affordances.
    fn render_connection_row(
        &self,
        ix: usize,
        a: &Attributed<DbConnection>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let conn = a.item.clone();
        let display_name: SharedString = if conn.name.is_empty() {
            conn.id.clone().into()
        } else {
            conn.name.clone().into()
        };
        let subtitle: SharedString = format!("{} · {}", conn.kind.label(), conn.dsn).into();
        let (badge, badge_color) = self.db_origin_badge(a);
        let is_active = self.db.active_id.as_deref() == Some(conn.id.as_str());
        let click_id = conn.id.clone();
        let origin = a.origin.clone();
        let armed = delete_click_executes(
            self.db.armed_delete.as_ref(),
            &(conn.id.clone(), origin.clone()),
        );

        // Small text-button factory for the row's action strip. Mirrors `app.rs`'s
        // `host_row::action` closure exactly. Note: these buttons sit inside the row's
        // own click-to-select area — a click here also fires the row's `on_click`
        // (selecting it), which is harmless (selection isn't destructive).
        let action = |id: (&'static str, usize), label: SharedString, color: u32| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(color))
                .hover(|s| s.bg(rgb(ACTIVE_BG)))
                .child(label)
        };

        // ⤒ promote: workspace-origin rows only.
        let promote = can_promote(&origin).then(|| {
            let id = conn.id.clone();
            let origin = origin.clone();
            action(("db-promote", ix), "⤒".into(), FG_DIM).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| {
                    this.promote_db_row(&id, &origin, cx);
                },
            ))
        });

        // ⤓ demote: global-origin rows while a workspace scope is active.
        let demote = can_demote(&origin, &self.scope).then(|| {
            let id = conn.id.clone();
            action(("db-demote", ix), "⤓".into(), FG_DIM).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| {
                    this.demote_db_row(&id, cx);
                },
            ))
        });

        // ✎ edit: opens the form prefilled with this row's record.
        let edit = {
            let conn = conn.clone();
            let origin = origin.clone();
            action(("db-edit", ix), "✎".into(), FG_DIM).on_click(cx.listener(
                move |this, _ev: &ClickEvent, window, cx| {
                    this.open_edit_db_form(conn.clone(), origin.clone(), window, cx);
                },
            ))
        };

        // 📁 folder: opens the minimal inline folder-assignment editor (Task 2's "row
        // hover-menu → small input" — see `Self::begin_folder_edit`).
        let folder_btn = {
            let id = conn.id.clone();
            let origin = origin.clone();
            let current = conn.folder.clone();
            action(("db-folder-edit", ix), "📁".into(), FG_DIM).on_click(cx.listener(
                move |this, _ev: &ClickEvent, window, cx| {
                    this.begin_folder_edit(&id, &origin, current.as_deref(), window, cx);
                },
            ))
        };

        // ✕ delete: two-click confirm — the first click arms this row, the second
        // deletes from the row's origin layer (and its secret from the keyring).
        let delete = {
            let id = conn.id.clone();
            let origin = origin.clone();
            let secret_ref = conn.secret_ref.clone();
            let (label, color) = if armed {
                ("✕ confirm?", DANGER)
            } else {
                ("✕", FG_DIM)
            };
            action(("db-delete", ix), label.into(), color).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| {
                    let key = (id.clone(), origin.clone());
                    if delete_click_executes(this.db.armed_delete.as_ref(), &key) {
                        this.delete_db_row(&id, &origin, secret_ref.as_deref(), cx);
                    } else {
                        this.db.armed_delete = Some(key);
                        cx.notify();
                    }
                },
            ))
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
                .flex_1()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(if is_active { ACTIVE_FG } else { FG }))
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
                .font_family(MONO)
                .text_xs()
                .text_color(rgb(FG_DIM))
                .child(subtitle)
                .into_any_element()
        };

        div()
            .id(("db-conn", ix))
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .px_2()
            .py_2()
            .cursor_pointer()
            .bg(rgb(if is_active { ACTIVE_BG } else { BG }))
            .border_b_1()
            .border_color(rgb(BORDER))
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
                }
                this.db.active_id = Some(click_id.clone());
                // Selecting a row is also this rail's one focus entry point — F2
                // afterwards renames whatever just got selected (`begin_rename_active`).
                if let Some(fh) = this.db.rail_focus.clone() {
                    window.focus(&fh);
                }
                this.refresh_schema(cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(name_area)
                    .child(div().text_xs().text_color(rgb(badge_color)).child(badge))
                    .when(is_active, |el| {
                        el.child(div().text_xs().text_color(rgb(BRAND)).child("★"))
                    }),
            )
            .child(subtitle_area)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .children(promote)
                    .children(demote)
                    .child(folder_btn)
                    .child(edit)
                    .child(delete),
            )
            .into_any_element()
    }

    /// Folder-header click (folders/grouping) — flip `name` between collapsed/expanded
    /// in the rail. Presence-in-`collapsed_folders` encodes "collapsed" (see that
    /// field's doc comment).
    fn toggle_conn_folder(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.db.collapsed_folders.remove(name) {
            self.db.collapsed_folders.insert(name.to_string());
        }
        cx.notify();
    }

    /// Badge label + color for a connection's origin layer — the `DbConnection` mirror
    /// of `AppState::origin_badge`.
    fn db_origin_badge(&self, a: &Attributed<DbConnection>) -> (SharedString, u32) {
        let (mut label, color): (SharedString, u32) = match &a.origin {
            Scope::Global => ("⌂ global".into(), BRAND),
            Scope::Workspace(id) => {
                let name = self
                    .scopes
                    .iter()
                    .find(|c| matches!(&c.scope, Scope::Workspace(w) if w == id))
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| "workspace".into());
                (name, WS_FG)
            }
        };
        if a.duplicate {
            label = format!("{label} · dup").into();
        }
        (label, color)
    }

    // ---- selector-right: inline rename / folder edit (Tasks 2-3) ---------------------

    /// F2 (rail focused, see [`Self::connection_rail`]) — rename whichever connection
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

    /// 📁 (folders/grouping) — enter the minimal inline folder-assignment editor for
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
    /// rail's ungrouped top level.
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
        let form = cx.new(|cx| DbConnForm::new_add(cx, registry, workspace, default_scope));
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
        let form = cx.new(|cx| DbConnForm::new_edit(cx, registry, conn, origin, workspace));
        self.open_db_form(form, window, cx);
    }

    fn open_db_form(
        &mut self,
        form: Entity<DbConnForm>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        form.read(cx).focus_first(window, cx);
        self.db._form_subscription = Some(cx.subscribe(&form, Self::on_db_form_event));
        self.db.form = Some(form);
        cx.notify();
    }

    pub(crate) fn close_db_form(&mut self, cx: &mut Context<Self>) {
        self.db.form = None;
        self.db._form_subscription = None;
        cx.notify();
    }

    fn on_db_form_event(
        &mut self,
        form: Entity<DbConnForm>,
        event: &DbConnFormEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            DbConnFormEvent::Cancel => self.close_db_form(cx),
            DbConnFormEvent::Submit(submission) => match self.perform_db_submit(submission) {
                Ok(post_warning) => {
                    self.close_db_form(cx);
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
    fn run_query(&mut self, cx: &mut Context<Self>) {
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
        let secret = match resolve_db_secret(self.secrets.as_ref(), conn.secret_ref.as_deref()) {
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
        self.db.last_sql = Some(sql.clone());
        push_history(&mut self.db.history, sql.clone(), HISTORY_CAP);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = run_first_page(factory, conn, secret, cached, sql).await;
            let _ = this.update(cx, |this, cx| {
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
            rows: page.rows.len(),
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
    fn refresh_schema(&mut self, cx: &mut Context<Self>) {
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
        let secret = match resolve_db_secret(self.secrets.as_ref(), conn.secret_ref.as_deref()) {
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
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = fetch_schema(factory, conn, secret, cached).await;
            let _ = this.update(cx, |this, cx| {
                this.db.schema_loading = false;
                match outcome {
                    Ok((client, schema, graph)) => {
                        this.db.client = Some(client);
                        this.db.client_for = Some(id);
                        this.db.schema = Some(schema);
                        this.db.schema_graph = Some(graph);
                    }
                    Err(e) => this.db.schema_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
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
        self.run_query(cx);
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
