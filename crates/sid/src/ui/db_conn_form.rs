//! Database connection add/edit form (W4): the write side of the Database tab.
//!
//! [`DbConnForm`] mirrors [`super::host_form::HostForm`]'s shape (own modal panel,
//! never touches the store/keyring directly — it validates and emits
//! [`DbConnFormEvent::Submit`], leaving the guard/secret/write to the owner in
//! `db_tab.rs`) but is written fresh rather than importing from `host_form.rs`,
//! matching that module's own "kept local so `ui` stays self-contained" convention.
//!
//! The field set is **descriptor-driven**: [`DbConnForm::new_add`]/[`DbConnForm::new_edit`]
//! ask the [`DbRegistry`] for the chosen [`DbKind`]'s [`DbClientDescriptor::connection_fields`]
//! and build one widget per [`ConnField`] — a masked [`TextInput`] for `Password`, a
//! segmented pill row for `Choice`/`Bool`, a plain [`TextInput`] otherwise. Redb has no
//! descriptor (a synthetic, always-present connection, never a form choice — see
//! `db_registry.rs`), so it never appears in the engine selector.
//!
//! Two deliberate simplifications vs. the host form: the engine (`DbKind`) is **locked**
//! in edit mode (switching engines mid-edit would reshape the DSN/field set entirely —
//! out of scope, same as the host form locking its alias), and there is no `SecretSlot`
//! enum — a connection has at most one secret (a password), and since the engine can't
//! change on edit, the old and new secret "slot" are always the same one.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    SharedString, Window, actions, div, prelude::*, px, rgb,
};
use sid_core::db::{ConnField, ConnFieldKind, DbKind};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{DbConnection, DefaultScope, Scope};

use super::TextInput;
use crate::db_registry::DbRegistry;

// Dark-theme palette, aligned with `app.rs`/`host_form.rs`. Kept local so `ui` stays
// self-contained (same convention as `host_form.rs`).
const PANEL_BG: u32 = 0x1d1d20;
const FIELD_BG: u32 = 0x121215;
const BORDER: u32 = 0x2c2c30;
const FIELD_BORDER: u32 = 0x33343a;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_BG: u32 = 0x33343a;
const ACTIVE_FG: u32 = 0xffffff;
const BRAND: u32 = 0x5a9ad0;
const DANGER: u32 = 0xd08a8a;

actions!(
    db_conn_form,
    [
        /// Dismiss the form without saving (bound to `escape`).
        DbFormCancel,
        /// Validate and submit the form (bound to `enter`).
        DbFormSubmit,
    ]
);

/// Which layer the `save to:` selector points at. Local copy of the host form's
/// `SaveTarget` — small enough that duplicating it beats importing across an otherwise
/// self-contained module boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveTarget {
    Workspace,
    Global,
}

/// Add a new connection, or edit an existing one in place.
pub(crate) enum FormMode {
    /// Fresh record; the add-mode guard applies.
    Add,
    /// Upsert of `original` into its `origin` layer; the engine and id are locked.
    Edit {
        /// The record as it was when the form opened (carries the old dsn/secret_ref).
        original: DbConnection,
        /// The layer the record was read from — edits always write back here.
        origin: Scope,
    },
}

/// Events the form emits to its owner.
pub(crate) enum DbConnFormEvent {
    Cancel,
    /// A locally-validated submission. The owner performs the add-mode guard, the
    /// secret lifecycle, and the store write.
    Submit(Box<Submission>),
}

/// A validated form submission.
#[derive(Debug, Clone)]
pub(crate) struct Submission {
    /// The validated connection. `secret_ref` is `None` here — the owner assigns it
    /// from the staged secret plan before writing.
    pub connection: DbConnection,
    /// The layer to write into.
    pub target: Scope,
    /// The original record when editing (source of the old `secret_ref`).
    pub old: Option<DbConnection>,
    /// Secret text entered this session (the password field), if any. Only ever
    /// forwarded to the [`SecretStore`]; never written to the DSN/config.
    pub secret: Option<String>,
}

/// One collected engine field's live widget.
enum FieldWidget {
    Text(Entity<TextInput>),
    /// A segmented pill control — used for both `Choice` and `Bool` fields (a `Bool`
    /// is just a two-option choice, so it reuses this rather than a fourth variant).
    Choice {
        options: Vec<String>,
        selected: usize,
    },
}

/// The database connection add/edit form.
pub struct DbConnForm {
    mode: FormMode,
    registry: Rc<DbRegistry>,
    name: Entity<TextInput>,
    kind: DbKind,
    fields: Vec<(ConnField, FieldWidget)>,
    /// The selected save target; `None` = nothing preselected (the `Ask` default).
    save_to: Option<SaveTarget>,
    /// The active workspace scope + its display label, if one is focused.
    workspace: Option<(Scope, SharedString)>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl DbConnForm {
    /// An empty add form defaulting to the first registered engine with a descriptor
    /// (Postgres). `default_scope` drives the `save to:` preselection.
    pub fn new_add(
        cx: &mut Context<Self>,
        registry: Rc<DbRegistry>,
        workspace: Option<(Scope, SharedString)>,
        default_scope: DefaultScope,
    ) -> Self {
        let workspace_active = workspace.is_some();
        let mut form = Self::new_inner(cx, registry, workspace, DbKind::Postgres, None, None);
        form.save_to = preselect(default_scope, workspace_active);
        form
    }

    /// An edit form prefilled from `original`, writing back into `origin` on save. The
    /// engine is locked to `original.kind`; its fields are prefilled via the
    /// descriptor's `dsn_to_field_values` (best-effort — a stored secret is never read
    /// back, so the password field always starts empty; see `field_row`).
    pub fn new_edit(
        cx: &mut Context<Self>,
        registry: Rc<DbRegistry>,
        original: DbConnection,
        origin: Scope,
        workspace: Option<(Scope, SharedString)>,
    ) -> Self {
        let kind = original.kind;
        let prefill = registry
            .descriptor(kind)
            .map(|d| d.dsn_to_field_values(&original.dsn))
            .unwrap_or_default();
        let mut form = Self::new_inner(
            cx,
            registry,
            workspace,
            kind,
            Some(original.name.as_str()),
            Some(&prefill),
        );
        form.save_to = Some(match &origin {
            Scope::Global => SaveTarget::Global,
            Scope::Workspace(_) => SaveTarget::Workspace,
        });
        form.mode = FormMode::Edit { original, origin };
        form
    }

    fn new_inner(
        cx: &mut Context<Self>,
        registry: Rc<DbRegistry>,
        workspace: Option<(Scope, SharedString)>,
        kind: DbKind,
        name_prefill: Option<&str>,
        field_prefill: Option<&BTreeMap<String, String>>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut input = TextInput::new(cx, "name — a short label");
            if let Some(v) = name_prefill {
                input.set_content(v.to_string(), cx);
            }
            input
        });
        let fields = Self::build_fields(&registry, kind, cx, field_prefill);
        Self {
            mode: FormMode::Add,
            registry,
            name,
            kind,
            fields,
            save_to: None,
            workspace,
            error: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Build one widget per field the engine's descriptor declares. A `prefill` map
    /// (from `dsn_to_field_values`) seeds every widget except `Password` — a stored
    /// secret is never read back into the UI (same rule as the host form): an empty
    /// masked field on an edit means "keep the existing secret" (see [`plan_secret`]).
    fn build_fields(
        registry: &DbRegistry,
        kind: DbKind,
        cx: &mut Context<Self>,
        prefill: Option<&BTreeMap<String, String>>,
    ) -> Vec<(ConnField, FieldWidget)> {
        let Some(descriptor) = registry.descriptor(kind) else {
            return Vec::new();
        };
        descriptor
            .connection_fields()
            .into_iter()
            .map(|field| {
                let value = prefill
                    .and_then(|p| p.get(&field.key).cloned())
                    .or_else(|| field.default.clone());
                let widget = match &field.kind {
                    ConnFieldKind::Password => FieldWidget::Text(
                        cx.new(|cx| TextInput::new_masked(cx, field.label.clone())),
                    ),
                    ConnFieldKind::Choice { options } => {
                        let selected = value
                            .as_deref()
                            .and_then(|v| options.iter().position(|o| o == v))
                            .unwrap_or(0);
                        FieldWidget::Choice {
                            options: options.clone(),
                            selected,
                        }
                    }
                    ConnFieldKind::Bool => {
                        let options = vec!["false".to_string(), "true".to_string()];
                        let selected = usize::from(value.as_deref() == Some("true"));
                        FieldWidget::Choice { options, selected }
                    }
                    ConnFieldKind::Text | ConnFieldKind::Port | ConnFieldKind::Path => {
                        FieldWidget::Text(cx.new(|cx| {
                            let mut input = TextInput::new(cx, field.label.clone());
                            if let Some(v) = &value {
                                input.set_content(v.clone(), cx);
                            }
                            input
                        }))
                    }
                };
                (field, widget)
            })
            .collect()
    }

    /// Focus the name field — the first editable field in both modes.
    pub fn focus_first(&self, window: &mut Window, cx: &App) {
        self.name.read(cx).focus(window);
    }

    /// Surface an owner-side failure (guard/secret/store) in the form's error line.
    pub fn set_error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(msg.into());
        cx.notify();
    }

    /// Switch the engine (add mode only) and rebuild the field list from scratch —
    /// entered values for the old engine's fields don't carry over, since a different
    /// engine's fields have no defined mapping from them.
    fn set_kind(&mut self, kind: DbKind, cx: &mut Context<Self>) {
        if self.kind == kind || !matches!(self.mode, FormMode::Add) {
            return;
        }
        self.kind = kind;
        self.fields = Self::build_fields(&self.registry, kind, cx, None);
        cx.notify();
    }

    /// The concrete layer a save would write into. Edits always target their origin;
    /// adds follow the `save to:` selection (`None` until the user chooses).
    fn target_scope(&self) -> Option<Scope> {
        if let FormMode::Edit { origin, .. } = &self.mode {
            return Some(origin.clone());
        }
        match self.save_to? {
            SaveTarget::Global => Some(Scope::Global),
            SaveTarget::Workspace => self.workspace.as_ref().map(|(scope, _)| scope.clone()),
        }
    }

    /// The password text the user typed this session, if any.
    fn entered_secret(&self, cx: &App) -> Option<String> {
        self.fields.iter().find_map(|(field, widget)| {
            if !matches!(field.kind, ConnFieldKind::Password) {
                return None;
            }
            let FieldWidget::Text(input) = widget else {
                return None;
            };
            let input = input.read(cx);
            (!input.is_empty()).then(|| input.content().to_string())
        })
    }

    /// Collect every field's current value, keyed by `ConnField::key` — the shape
    /// `DbClientDescriptor::assemble_params`/`validate_port_fields` expect.
    fn collect_values(&self, cx: &App) -> BTreeMap<String, String> {
        self.fields
            .iter()
            .map(|(field, widget)| {
                let value = match widget {
                    FieldWidget::Text(input) => input.read(cx).content().to_string(),
                    FieldWidget::Choice { options, selected } => {
                        options.get(*selected).cloned().unwrap_or_default()
                    }
                };
                (field.key.clone(), value)
            })
            .collect()
    }

    /// Validate and emit [`DbConnFormEvent::Submit`]; on a validation miss, show the
    /// message and stay open.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let name = match validate_name(self.name.read(cx).content()) {
            Ok(name) => name,
            Err(msg) => {
                self.error = Some(msg.into());
                cx.notify();
                return;
            }
        };
        let Some(descriptor) = self.registry.descriptor(self.kind) else {
            self.error = Some("internal: no descriptor for this engine".into());
            cx.notify();
            return;
        };
        let values = self.collect_values(cx);
        let raw_fields: Vec<ConnField> = self.fields.iter().map(|(f, _)| f.clone()).collect();
        if let Err(msg) = validate_port_fields(&raw_fields, &values) {
            self.error = Some(msg.into());
            cx.notify();
            return;
        }
        let params = match descriptor.assemble_params(&values) {
            Ok(p) => p,
            Err(msg) => {
                self.error = Some(msg.into());
                cx.notify();
                return;
            }
        };
        let Some(target) = self.target_scope() else {
            self.error = Some("choose where to save: workspace or global".into());
            cx.notify();
            return;
        };
        let id = match &self.mode {
            FormMode::Add => name.clone(),
            FormMode::Edit { original, .. } => original.id.clone(),
        };
        let connection = DbConnection {
            id,
            dsn: params.dsn,
            secret_ref: None,
            kind: self.kind,
            name,
        };
        let secret = self.entered_secret(cx);
        let old = match &self.mode {
            FormMode::Add => None,
            FormMode::Edit { original, .. } => Some(original.clone()),
        };
        self.error = None;
        cx.emit(DbConnFormEvent::Submit(Box::new(Submission {
            connection,
            target,
            old,
            secret,
        })));
        cx.notify();
    }

    // ---- render pieces ------------------------------------------------------

    fn field_label(text: impl Into<SharedString>) -> impl IntoElement {
        div().text_xs().text_color(rgb(FG_DIM)).child(text.into())
    }

    fn field(&self, label: &'static str, input: &Entity<TextInput>) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::field_label(label))
            .child(input.clone())
    }

    /// The engine row in edit mode: static text, visibly locked (see the module doc's
    /// "engine locked in edit" note).
    fn locked_kind(&self) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::field_label("engine — locked while editing"))
            .child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .rounded_md()
                    .bg(rgb(FIELD_BG))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_sm()
                    .text_color(rgb(FG_DIM))
                    .child(self.kind.label()),
            )
    }

    fn kind_selector(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let kinds: Vec<DbKind> = self
            .registry
            .kinds()
            .into_iter()
            .filter(|k| self.registry.descriptor(*k).is_some())
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::field_label("engine"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .children(kinds.into_iter().enumerate().map(|(ix, k)| {
                        let active = self.kind == k;
                        div()
                            .id(("db-kind", ix))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(rgb(if active { ACTIVE_BG } else { FIELD_BG }))
                            .border_1()
                            .border_color(rgb(if active { BRAND } else { FIELD_BORDER }))
                            .text_color(rgb(if active { ACTIVE_FG } else { FG_DIM }))
                            .child(k.label())
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                                this.set_kind(k, cx);
                            }))
                    })),
            )
    }

    /// Render one descriptor field: a plain input, or a segmented pill row for
    /// `Choice`/`Bool` fields.
    fn field_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let (field, widget) = &self.fields[ix];
        let label = field.label.clone();
        match widget {
            FieldWidget::Text(input) => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(Self::field_label(label))
                .child(input.clone())
                .into_any_element(),
            FieldWidget::Choice { options, selected } => {
                let selected = *selected;
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Self::field_label(label))
                    .child(div().flex().flex_row().gap_1().children(
                        options.iter().cloned().enumerate().map(|(opt_ix, opt)| {
                            let active = opt_ix == selected;
                            div()
                                .id(("field-choice", ix * 1000 + opt_ix))
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .bg(rgb(if active { ACTIVE_BG } else { FIELD_BG }))
                                .border_1()
                                .border_color(rgb(if active { BRAND } else { FIELD_BORDER }))
                                .text_color(rgb(if active { ACTIVE_FG } else { FG_DIM }))
                                .child(opt)
                                .on_click(cx.listener(move |this, _ev, _window, cx| {
                                    if let Some((_, FieldWidget::Choice { selected, .. })) =
                                        this.fields.get_mut(ix)
                                    {
                                        *selected = opt_ix;
                                    }
                                    cx.notify();
                                }))
                        }),
                    ))
                    .into_any_element()
            }
        }
    }

    fn save_to_selector(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let locked = matches!(self.mode, FormMode::Edit { .. });
        let ws_active = self.workspace.is_some();

        let option = |id: &'static str,
                      title: &'static str,
                      note: &'static str,
                      target: SaveTarget,
                      enabled: bool,
                      selected: bool,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(rgb(if selected { BRAND } else { FIELD_BORDER }))
                .bg(rgb(if selected { ACTIVE_BG } else { FIELD_BG }))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(if selected { BRAND } else { FG_DIM }))
                        .child(if selected { "●" } else { "○" }),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(if enabled { FG } else { FG_DIM }))
                        .child(title),
                )
                .child(div().text_xs().text_color(rgb(FG_DIM)).child(note))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .on_click(cx.listener(move |this, _ev, _window, cx| {
                            this.save_to = Some(target);
                            cx.notify();
                        }))
                })
        };

        let label: SharedString = if locked {
            "save to — fixed while editing (use ⤒/⤓ to move a connection)".into()
        } else {
            "save to:".into()
        };

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::field_label(label))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(option(
                        "save-workspace",
                        "workspace",
                        "— .sid/ · travels with git",
                        SaveTarget::Workspace,
                        ws_active && !locked,
                        self.save_to == Some(SaveTarget::Workspace),
                        cx,
                    ))
                    .child(option(
                        "save-global",
                        "global",
                        "— everywhere · never lost",
                        SaveTarget::Global,
                        !locked,
                        self.save_to == Some(SaveTarget::Global),
                        cx,
                    )),
            )
    }

    fn buttons(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap_2()
            .child(
                div()
                    .id("db-form-cancel")
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(FIELD_BG))
                    .border_1()
                    .border_color(rgb(FIELD_BORDER))
                    .text_color(rgb(FG_DIM))
                    .child("Cancel")
                    .on_click(cx.listener(|_this, _ev: &ClickEvent, _window, cx| {
                        cx.emit(DbConnFormEvent::Cancel);
                    })),
            )
            .child(
                div()
                    .id("db-form-save")
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(ACTIVE_BG))
                    .border_1()
                    .border_color(rgb(BRAND))
                    .text_color(rgb(ACTIVE_FG))
                    .child("Save")
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| this.submit(cx))),
            )
    }
}

impl EventEmitter<DbConnFormEvent> for DbConnForm {}

impl Focusable for DbConnForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DbConnForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match &self.mode {
            FormMode::Add => "Add connection",
            FormMode::Edit { .. } => "Edit connection",
        };
        let field_rows: Vec<AnyElement> = (0..self.fields.len())
            .map(|ix| self.field_row(ix, cx))
            .collect();

        div()
            .key_context("DbConnForm")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_this, _: &DbFormCancel, _window, cx| {
                cx.emit(DbConnFormEvent::Cancel);
            }))
            .on_action(cx.listener(|this, _: &DbFormSubmit, _window, cx| this.submit(cx)))
            .flex()
            .flex_col()
            .gap_3()
            .w(px(460.))
            .p_4()
            .rounded_lg()
            .bg(rgb(PANEL_BG))
            .border_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(FG))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(div().flex_1().text_sm().text_color(rgb(FG)).child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(FG_DIM))
                            .child("esc cancels · enter saves"),
                    ),
            )
            .child(self.field("name", &self.name))
            .child(match &self.mode {
                FormMode::Add => self.kind_selector(cx).into_any_element(),
                FormMode::Edit { .. } => self.locked_kind().into_any_element(),
            })
            .children(field_rows)
            .child(self.save_to_selector(cx))
            .when_some(self.error.clone(), |el, err| {
                el.child(div().text_sm().text_color(rgb(DANGER)).child(err))
            })
            .child(self.buttons(cx))
    }
}

// ---------------------------------------------------------------------------
// Pure decision logic (unit-tested without gpui)
// ---------------------------------------------------------------------------

/// Validate the connection name: non-empty, trimmed. Doubles as the record's `id`
/// on add (see `submit`) — mirrors the host form's alias in spirit, but `DbConnection`
/// keeps `id`/`name` as separate fields, so a later rename only ever touches `name`.
pub(crate) fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("name must not be empty".into());
    }
    Ok(name.to_string())
}

/// Range-check every `Port`-kind field's value. `DbClientDescriptor::assemble_params`
/// only checks non-emptiness (via its own `require` helper) — the 1..=65535 range the
/// field's doc comment promises is the form's job, not the descriptor's.
pub(crate) fn validate_port_fields(
    fields: &[ConnField],
    values: &BTreeMap<String, String>,
) -> Result<(), String> {
    for field in fields {
        if !matches!(field.kind, ConnFieldKind::Port) {
            continue;
        }
        let Some(raw) = values.get(&field.key) else {
            continue;
        };
        if raw.is_empty() {
            continue; // non-emptiness is the descriptor's job.
        }
        let port: u32 = raw
            .trim()
            .parse()
            .map_err(|_| format!("{} must be a number in 1-65535", field.label))?;
        if !(1..=65535).contains(&port) {
            return Err(format!("{} must be a number in 1-65535", field.label));
        }
    }
    Ok(())
}

/// The attributive add-mode guard: an *add* into a layer that already holds the
/// connection's id is refused (nothing is ever silently clobbered); only an explicit
/// edit upserts. Mirrors the host form's `add_guard` exactly.
pub(crate) fn add_guard(
    is_edit: bool,
    target_holds_id: bool,
    target_label: &str,
) -> Result<(), String> {
    if !is_edit && target_holds_id {
        Err(format!(
            "a connection with this name exists in {target_label} — edit it instead"
        ))
    } else {
        Ok(())
    }
}

/// Which `save to:` option an add form preselects. Identical logic to the host form's
/// `preselect` (duplicated per this module's own self-contained convention).
pub(crate) fn preselect(default_scope: DefaultScope, workspace_active: bool) -> Option<SaveTarget> {
    match default_scope {
        DefaultScope::Ask => None,
        DefaultScope::Global => Some(SaveTarget::Global),
        DefaultScope::Workspace => workspace_active.then_some(SaveTarget::Workspace),
    }
}

/// The keyring consequence of a save. No `SecretSlot` enum here (unlike the host
/// form): a connection carries at most one secret kind (a password), and the engine —
/// so whether that slot even exists — is locked on edit, so old vs. new can never
/// disagree about which slot is in play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SecretPlan {
    /// The new record stores no secret; delete the old id if one existed.
    Clear { delete_old: Option<String> },
    /// Keep the existing `secret_ref` untouched (edit with the masked field left
    /// empty).
    Keep(String),
    /// Put the newly entered secret under a freshly minted id; delete the old id (if
    /// any) after a successful write.
    Mint { delete_old: Option<String> },
}

/// Decide what a save does to the keyring. `old` is the pre-edit record (`None` when
/// adding); `has_password_field` is whether the current engine's fields include a
/// `Password` slot at all; `secret_entered` is whether the user typed into it.
pub(crate) fn plan_secret(
    old: Option<&DbConnection>,
    has_password_field: bool,
    secret_entered: bool,
) -> SecretPlan {
    let old_ref = old.and_then(|c| c.secret_ref.clone());
    if !has_password_field {
        return SecretPlan::Clear {
            delete_old: old_ref,
        };
    }
    if secret_entered {
        return SecretPlan::Mint {
            delete_old: old_ref,
        };
    }
    match old_ref {
        Some(id) => SecretPlan::Keep(id),
        None => SecretPlan::Clear { delete_old: None },
    }
}

/// Mint an opaque keyring id: `db-<name>-<unix_nanos>`. Nanosecond timestamps keep
/// same-name records in different layers from colliding.
pub(crate) fn mint_secret_id(name: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("db-{name}-{nanos}")
}

/// The result of staging a [`SecretPlan`] against the keyring, ready for the store
/// write.
#[derive(Debug)]
pub(crate) struct StagedSecret {
    /// The `secret_ref` the written connection should carry.
    pub secret_ref: Option<String>,
    /// A superseded keyring id to delete only *after* the store write succeeds, so a
    /// failed write never loses the old secret.
    pub delete_after_write: Option<String>,
    /// Whether a fresh id was minted (and must be rolled back if the write fails).
    pub minted: bool,
}

/// Execute the write-side half of a [`SecretPlan`]: mint + put for [`SecretPlan::Mint`]
/// (requires `secret`), pass-through for `Keep`/`Clear`. Old-id deletion is deferred to
/// the caller via [`StagedSecret::delete_after_write`].
pub(crate) fn stage_secret(
    secrets: &dyn SecretStore,
    plan: &SecretPlan,
    name: &str,
    secret: Option<&str>,
) -> Result<StagedSecret, String> {
    match plan {
        SecretPlan::Clear { delete_old } => Ok(StagedSecret {
            secret_ref: None,
            delete_after_write: delete_old.clone(),
            minted: false,
        }),
        SecretPlan::Keep(id) => Ok(StagedSecret {
            secret_ref: Some(id.clone()),
            delete_after_write: None,
            minted: false,
        }),
        SecretPlan::Mint { delete_old } => {
            let secret =
                secret.ok_or_else(|| "internal: mint plan without a secret".to_string())?;
            let id = mint_secret_id(name);
            secrets
                .put(&SecretId::new(id.clone()), secret.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(StagedSecret {
                secret_ref: Some(id),
                delete_after_write: delete_old.clone(),
                minted: true,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_secrets::keyring::{FakeKeyring, KeyringStore};

    fn conn(id: &str, secret_ref: Option<&str>) -> DbConnection {
        DbConnection {
            id: id.into(),
            dsn: "d".into(),
            secret_ref: secret_ref.map(Into::into),
            kind: DbKind::Postgres,
            name: id.into(),
        }
    }

    fn port_field(key: &str) -> ConnField {
        ConnField::new(key, key, ConnFieldKind::Port)
    }

    // ---- validate_name --------------------------------------------------------------

    #[test]
    fn validate_name_trims_and_accepts() {
        assert_eq!(validate_name("  prod db  ").unwrap(), "prod db");
    }

    #[test]
    fn validate_name_rejects_empty() {
        assert!(validate_name("   ").is_err());
    }

    // ---- validate_port_fields --------------------------------------------------------

    #[test]
    fn validate_port_fields_accepts_in_range() {
        let fields = vec![port_field("port")];
        let values = BTreeMap::from([("port".to_string(), "5432".to_string())]);
        assert!(validate_port_fields(&fields, &values).is_ok());
    }

    #[test]
    fn validate_port_fields_rejects_out_of_range() {
        let fields = vec![port_field("port")];
        for bad in ["0", "65536", "-1", "abc"] {
            let values = BTreeMap::from([("port".to_string(), bad.to_string())]);
            assert!(
                validate_port_fields(&fields, &values).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn validate_port_fields_ignores_missing_or_empty() {
        let fields = vec![port_field("port")];
        assert!(validate_port_fields(&fields, &BTreeMap::new()).is_ok());
        let values = BTreeMap::from([("port".to_string(), String::new())]);
        assert!(validate_port_fields(&fields, &values).is_ok());
    }

    #[test]
    fn validate_port_fields_ignores_non_port_kinds() {
        let fields = vec![ConnField::new("host", "Host", ConnFieldKind::Text)];
        let values = BTreeMap::from([("host".to_string(), "not-a-number".to_string())]);
        assert!(validate_port_fields(&fields, &values).is_ok());
    }

    // ---- add_guard --------------------------------------------------------------------

    #[test]
    fn add_guard_refuses_add_into_a_layer_that_holds_the_id() {
        assert!(add_guard(false, true, "⌂ global").is_err());
    }

    #[test]
    fn add_guard_allows_add_into_a_free_layer() {
        assert!(add_guard(false, false, "⌂ global").is_ok());
    }

    #[test]
    fn add_guard_always_allows_edit() {
        assert!(add_guard(true, true, "⌂ global").is_ok());
    }

    // ---- plan_secret --------------------------------------------------------------------

    #[test]
    fn plan_no_password_field_clears_any_old_secret() {
        let old = conn("a", Some("db-a-1"));
        assert_eq!(
            plan_secret(Some(&old), false, false),
            SecretPlan::Clear {
                delete_old: Some("db-a-1".into())
            }
        );
    }

    #[test]
    fn plan_add_with_password_field_and_entered_secret_mints() {
        assert_eq!(
            plan_secret(None, true, true),
            SecretPlan::Mint { delete_old: None }
        );
    }

    #[test]
    fn plan_edit_replacing_secret_mints_and_deletes_old() {
        let old = conn("a", Some("db-a-1"));
        assert_eq!(
            plan_secret(Some(&old), true, true),
            SecretPlan::Mint {
                delete_old: Some("db-a-1".into())
            }
        );
    }

    #[test]
    fn plan_edit_untouched_password_field_keeps_the_old_secret() {
        let old = conn("a", Some("db-a-1"));
        assert_eq!(
            plan_secret(Some(&old), true, false),
            SecretPlan::Keep("db-a-1".into())
        );
    }

    #[test]
    fn plan_add_with_password_field_but_nothing_entered_clears() {
        assert_eq!(
            plan_secret(None, true, false),
            SecretPlan::Clear { delete_old: None }
        );
    }

    // ---- mint_secret_id -----------------------------------------------------------------

    #[test]
    fn mint_ids_are_unique_across_calls() {
        assert_ne!(mint_secret_id("db"), mint_secret_id("db"));
    }

    // ---- staging against the (fake) keyring ----------------------------------------------

    #[test]
    fn stage_mint_puts_bytes_under_fresh_id() {
        let secrets = KeyringStore::with_backend(FakeKeyring::default());
        let plan = SecretPlan::Mint {
            delete_old: Some("db-a-old".into()),
        };
        let staged = stage_secret(&secrets, &plan, "a", Some("hunter2")).unwrap();
        let id = staged.secret_ref.expect("minted ref");
        assert!(id.starts_with("db-a-"), "{id}");
        assert!(staged.minted);
        assert_eq!(staged.delete_after_write.as_deref(), Some("db-a-old"));
        assert_eq!(
            secrets.get(&SecretId::new(id)).unwrap().as_deref(),
            Some(&b"hunter2"[..])
        );
    }

    #[test]
    fn stage_mint_without_secret_is_an_internal_error() {
        let secrets = KeyringStore::with_backend(FakeKeyring::default());
        let plan = SecretPlan::Mint { delete_old: None };
        assert!(stage_secret(&secrets, &plan, "a", None).is_err());
        assert!(secrets.list_ids().unwrap().is_empty());
    }

    #[test]
    fn stage_keep_touches_nothing() {
        let secrets = KeyringStore::with_backend(FakeKeyring::default());
        secrets.put(&SecretId::new("db-a-1"), b"old").unwrap();
        let staged = stage_secret(&secrets, &SecretPlan::Keep("db-a-1".into()), "a", None).unwrap();
        assert_eq!(staged.secret_ref.as_deref(), Some("db-a-1"));
        assert_eq!(staged.delete_after_write, None);
        assert!(!staged.minted);
        assert_eq!(
            secrets.get(&SecretId::new("db-a-1")).unwrap().as_deref(),
            Some(&b"old"[..])
        );
    }

    #[test]
    fn stage_clear_defers_the_delete_to_after_the_write() {
        let secrets = KeyringStore::with_backend(FakeKeyring::default());
        secrets.put(&SecretId::new("db-a-1"), b"old").unwrap();
        let plan = SecretPlan::Clear {
            delete_old: Some("db-a-1".into()),
        };
        let staged = stage_secret(&secrets, &plan, "a", None).unwrap();
        assert_eq!(staged.secret_ref, None);
        assert_eq!(staged.delete_after_write.as_deref(), Some("db-a-1"));
        // The old secret must still exist — it is only deleted after a successful write.
        assert!(secrets.get(&SecretId::new("db-a-1")).unwrap().is_some());
    }
}
