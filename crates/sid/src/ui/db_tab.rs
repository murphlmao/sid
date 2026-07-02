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

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, SharedString,
    Subscription, Window, div, prelude::*, px, rgb, uniform_list,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::table::{Column, Table, TableDelegate, TableState};
use sid_core::db::{DbClient, DbError, OpenParams, PageCursor, QueryPage, Row};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{Attributed, DbConnection, Scope, Store, ViewFilters};

use crate::app::{AppState, can_demote, can_promote, delete_click_executes};
use crate::db_registry::DbRegistry;
use crate::ui::db_conn_form::{
    DbConnForm, DbConnFormEvent, Submission, add_guard, plan_secret, stage_secret,
};
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
}

impl ResultDelegate {
    fn empty() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
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
        div().px_2().text_xs().text_color(rgb(FG)).child(text)
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
            .into_any_element()
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
        self.db.results = Some(cx.new(|cx| TableState::new(ResultDelegate::empty(), window, cx)));
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

        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(rgb(BORDER))
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
                    ),
            )
            .children(sql_editor)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(status_color))
                    .child(status_text),
            )
            .children(results_table)
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
                this.db.active_id = Some(click_id.clone());
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
        if let Some(results) = self.db.results.clone() {
            results.update(cx, |state, cx| {
                state.delegate_mut().set_page(page.clone());
                state.refresh(cx);
                cx.notify();
            });
        }
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
