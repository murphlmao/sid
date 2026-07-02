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

use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, Entity, FontWeight, IntoElement, SharedString, Subscription,
    Window, div, prelude::*, rgb, uniform_list,
};
use sid_secrets::SecretId;
use sid_store::{Attributed, DbConnection, Scope, Store, ViewFilters};

use crate::app::{AppState, can_demote, can_promote, delete_click_executes};
use crate::db_registry::DbRegistry;
use crate::ui::db_conn_form::{
    DbConnForm, DbConnFormEvent, Submission, add_guard, plan_secret, stage_secret,
};

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

    pub(crate) fn db_tab(&self, cx: &mut Context<Self>) -> AnyElement {
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
                .flex_1(),
            )
            .into_any_element()
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
}
