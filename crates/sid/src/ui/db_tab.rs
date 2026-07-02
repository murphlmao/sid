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

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, Entity, FontWeight, IntoElement,
    SharedString, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions,
    anchored, deferred, div, point, prelude::*, px, rgb, rgba, size, uniform_list,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::table::{Column, Table, TableDelegate, TableState};
use gpui_component::{Root, Theme, ThemeMode};
use sid_core::db::{
    DbClient, DbError, OpenParams, PageCursor, QueryPage, Row, SchemaGraph, SchemaInfo, TableInfo,
};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{Attributed, DbConnection, Scope, Store, ViewFilters};

use crate::app::{AppState, can_demote, can_promote, delete_click_executes};
use crate::db_registry::DbRegistry;
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
const ROW_ALT: u32 = 0x1c1c20;
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

    // ---- D4: query history -----------------------------------------------------------
    /// Most-recent-first ring of run queries, capped at [`HISTORY_CAP`] with
    /// consecutive-duplicate suppression — see [`push_history`].
    history: Vec<String>,
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
            history: Vec::new(),
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

        let count = self.db.connections.len();
        let sub: SharedString = match &self.error {
            Some(e) => format!("error: {e}").into(),
            None => format!("{count} connections · union of this scope, deduped").into(),
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
                    .child(div().flex_1().text_sm().text_color(rgb(FG_DIM)).child(sub))
                    .child(
                        div()
                            .id("db-add")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .text_color(rgb(BRAND))
                            .hover(|s| s.bg(rgb(ACTIVE_BG)))
                            .child("＋ Add connection")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                this.open_add_db_form(window, cx);
                            })),
                    ),
            )
            .child(
                uniform_list(
                    "db-connections",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _win, cx| {
                        range
                            .map(|ix| this.db_connection_row(ix, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                // Fixed height (was `flex_1`, W3) — the query pane below now claims the
                // rest of the tab's vertical space.
                .h(px(220.)),
            )
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
                            .id("db-export-csv")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .text_color(rgb(BRAND))
                            .hover(|s| s.bg(rgb(ACTIVE_BG)))
                            .child("⭳ CSV")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.export_csv(cx);
                            })),
                    )
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
                    ),
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
                .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                    this.open_diagram_window(cx);
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
    fn open_diagram_window(&mut self, cx: &mut Context<Self>) {
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
                let view = cx.new(|_cx| DiagramView::new(schema, graph));
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

    fn db_connection_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let a = &self.db.connections[ix];
        let conn = a.item.clone();
        let display_name: SharedString = if conn.name.is_empty() {
            conn.id.clone().into()
        } else {
            conn.name.clone().into()
        };
        let subtitle: SharedString = format!("{} · {}", conn.kind.label(), conn.dsn).into();
        let (badge, badge_color) = self.db_origin_badge(a);
        let alt = ix % 2 == 1;
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

        div()
            .id(("db-conn", ix))
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .px_4()
            .py_2()
            .cursor_pointer()
            .bg(rgb(if is_active {
                ACTIVE_BG
            } else if alt {
                ROW_ALT
            } else {
                BG
            }))
            .border_b_1()
            .border_color(rgb(BORDER))
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
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
                this.refresh_schema(cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(if is_active { ACTIVE_FG } else { FG }))
                            .child(display_name),
                    )
                    .child(div().text_xs().text_color(rgb(badge_color)).child(badge))
                    .child(div().flex_1())
                    .when(is_active, |el| {
                        el.child(div().text_xs().text_color(rgb(BRAND)).child("★ active"))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .children(promote)
                            .children(demote)
                            .child(edit)
                            .child(delete),
                    ),
            )
            .child(
                div()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(subtitle),
            )
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
        let stmt = format!("SELECT * FROM {display_name}");
        sql_entity.update(cx, |state, cx| {
            state.set_value(stmt, window, cx);
        });
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
