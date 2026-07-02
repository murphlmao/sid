//! Host add/edit form modal (A6): the write side of the SSH tab.
//!
//! [`HostForm`] is a gpui entity rendered as a centered panel (the dimmed backdrop and
//! `deferred`/`anchored` overlay live in `app.rs`). It owns one [`TextInput`] per field,
//! an auth-method segmented selector, and the `save to:` layer selector. It never touches
//! the store or the keyring itself — on Save it validates and emits
//! [`HostFormEvent::Submit`]; the owner (`AppState`) runs the add-mode guard, the
//! secret lifecycle, and the write, and pushes any store error back into the form's
//! error line via [`HostForm::set_error`].
//!
//! The decision logic is deliberately extracted into plain functions — [`validate`],
//! [`add_guard`], [`plan_secret`], [`stage_secret`], [`preselect`] — so the critical
//! paths (validation, the attributive add-guard, and the keyring lifecycle) are
//! unit-tested without gpui; rendering is observation-gated.

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString, Window,
    actions, div, prelude::*, px, rgb,
};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{AuthMethod, DefaultScope, Host, Scope};

use super::TextInput;
use super::text_input::next_focus_index;

// Dark-theme palette, aligned with `app.rs`. Kept local so `ui` stays self-contained.
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
    host_form,
    [
        /// Dismiss the form without saving (bound to `escape`).
        FormCancel,
        /// Validate and submit the form (bound to `enter`).
        FormSubmit,
    ]
);

/// Which auth method the segmented selector has chosen. UI-side mirror of
/// [`AuthMethod`] minus the data payload (the key path lives in its own input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthChoice {
    /// Use the running SSH agent.
    Agent,
    /// Public-key auth (key path + optional passphrase).
    Key,
    /// Password auth.
    Password,
}

/// Which layer the `save to:` selector points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveTarget {
    /// The active workspace's committed `.sid/config.toml`.
    Workspace,
    /// The machine-global redb store.
    Global,
}

/// Add a new host, or edit an existing one in place.
pub enum FormMode {
    /// Fresh record; the add-mode guard applies.
    Add,
    /// Upsert of `original` into its `origin` layer; alias locked (rename out of scope).
    Edit {
        /// The record as it was when the form opened (carries the old auth/secret_ref).
        original: Host,
        /// The layer the record was read from — edits always write back here.
        origin: Scope,
    },
}

/// Events the form emits to its owner.
pub enum HostFormEvent {
    /// Dismiss without saving.
    Cancel,
    /// A locally-validated submission. The owner performs the add-mode guard, the
    /// secret lifecycle, and the store write. Boxed: the payload dwarfs `Cancel`.
    Submit(Box<Submission>),
}

/// A validated form submission.
#[derive(Debug, Clone)]
pub struct Submission {
    /// The validated host. `secret_ref` is `None` here — the owner assigns it from the
    /// staged secret plan before writing.
    pub host: Host,
    /// The layer to write into.
    pub target: Scope,
    /// The original record when editing (source of the old auth + `secret_ref`).
    pub old: Option<Host>,
    /// Secret text entered this session (password or key passphrase), if any. Only ever
    /// forwarded to the [`SecretStore`]; never written to config.
    pub secret: Option<String>,
}

/// The host add/edit form.
pub struct HostForm {
    mode: FormMode,
    alias: Entity<TextInput>,
    user: Entity<TextInput>,
    host: Entity<TextInput>,
    port: Entity<TextInput>,
    key_path: Entity<TextInput>,
    passphrase: Entity<TextInput>,
    password: Entity<TextInput>,
    auth: AuthChoice,
    /// The selected save target; `None` = nothing preselected (the `Ask` default).
    save_to: Option<SaveTarget>,
    /// The active workspace scope + its display label, if one is focused. Enables the
    /// `workspace` save target and names it in messages.
    workspace: Option<(Scope, SharedString)>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl HostForm {
    /// An empty add form. `default_scope` drives the `save to:` preselection
    /// ([`preselect`]); the dialog itself always shows.
    pub fn new_add(
        cx: &mut Context<Self>,
        workspace: Option<(Scope, SharedString)>,
        default_scope: DefaultScope,
    ) -> Self {
        let workspace_active = workspace.is_some();
        let mut form = Self::new_inner(cx, workspace, None);
        form.save_to = preselect(default_scope, workspace_active);
        form
    }

    /// An edit form prefilled from `original`, writing back into `origin` on save.
    /// The alias is locked (rename is out of scope for P3.2).
    pub fn new_edit(
        cx: &mut Context<Self>,
        original: Host,
        origin: Scope,
        workspace: Option<(Scope, SharedString)>,
    ) -> Self {
        let mut form = Self::new_inner(cx, workspace, Some(&original));
        form.save_to = Some(match &origin {
            Scope::Global => SaveTarget::Global,
            Scope::Workspace(_) => SaveTarget::Workspace,
        });
        form.mode = FormMode::Edit { original, origin };
        form
    }

    fn new_inner(
        cx: &mut Context<Self>,
        workspace: Option<(Scope, SharedString)>,
        prefill: Option<&Host>,
    ) -> Self {
        let mk = |cx: &mut Context<Self>, placeholder: &str, value: Option<String>| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| {
                let mut input = TextInput::new(cx, placeholder);
                if let Some(v) = value {
                    input.set_content(v, cx);
                }
                input
            })
        };
        let mk_masked = |cx: &mut Context<Self>, placeholder: &str| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| TextInput::new_masked(cx, placeholder))
        };

        // A stored secret is never read back into the UI: an empty masked field on an
        // edit means "keep the existing secret" (see `plan_secret`).
        let has_stored_secret = prefill.is_some_and(|h| h.secret_ref.is_some());
        let password_hint = if has_stored_secret {
            "leave empty to keep the stored secret"
        } else {
            "password — stored in the OS keyring"
        };
        let passphrase_hint = if has_stored_secret {
            "leave empty to keep the stored secret"
        } else {
            "passphrase (optional) — stored in the OS keyring"
        };

        let (auth, key_path_value) = match prefill.map(|h| &h.auth) {
            None | Some(AuthMethod::Agent) => (AuthChoice::Agent, None),
            Some(AuthMethod::Password) => (AuthChoice::Password, None),
            Some(AuthMethod::Key { path }) => (AuthChoice::Key, Some(path.clone())),
        };

        Self {
            alias: mk(
                cx,
                "alias — unique short name",
                prefill.map(|h| h.alias.clone()),
            ),
            user: mk(cx, "user", prefill.map(|h| h.user.clone())),
            host: mk(cx, "hostname or address", prefill.map(|h| h.host.clone())),
            port: mk(
                cx,
                "port",
                Some(
                    prefill
                        .map(|h| h.port.to_string())
                        .unwrap_or_else(|| "22".into()),
                ),
            ),
            key_path: mk(cx, "~/.ssh/id_ed25519", key_path_value),
            passphrase: mk_masked(cx, passphrase_hint),
            password: mk_masked(cx, password_hint),
            auth,
            save_to: None,
            workspace,
            mode: FormMode::Add,
            error: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Focus the first editable field: alias when adding, user when editing (the alias
    /// is locked in edit mode).
    pub fn focus_first(&self, window: &mut Window, cx: &App) {
        let target = match &self.mode {
            FormMode::Add => &self.alias,
            FormMode::Edit { .. } => &self.user,
        };
        target.read(cx).focus(window);
    }

    /// Surface an owner-side failure (guard/secret/store) in the form's error line.
    pub fn set_error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(msg.into());
        cx.notify();
    }

    /// Switch the auth segment, clearing secret fields that no longer apply so stale
    /// secret text never lingers in memory or leaks across methods.
    fn set_auth(&mut self, choice: AuthChoice, cx: &mut Context<Self>) {
        if self.auth == choice {
            return;
        }
        self.auth = choice;
        match choice {
            AuthChoice::Agent => {
                self.password.update(cx, |i, cx| i.reset(cx));
                self.passphrase.update(cx, |i, cx| i.reset(cx));
            }
            AuthChoice::Key => self.password.update(cx, |i, cx| i.reset(cx)),
            AuthChoice::Password => self.passphrase.update(cx, |i, cx| i.reset(cx)),
        }
        cx.notify();
    }

    /// The text fields currently on screen, in render order. Tracks the auth-method
    /// switch (`set_auth`) and the add-vs-edit alias row so Tab/Shift+Tab only ever
    /// visits what's actually rendered. Segmented selectors, the save-to picker, and
    /// the buttons are not text inputs and are excluded from v1's cycle.
    fn focusable_fields(&self) -> Vec<Entity<TextInput>> {
        let mut fields = Vec::with_capacity(6);
        if matches!(self.mode, FormMode::Add) {
            fields.push(self.alias.clone());
        }
        fields.push(self.user.clone());
        fields.push(self.host.clone());
        fields.push(self.port.clone());
        match self.auth {
            AuthChoice::Agent => {}
            AuthChoice::Key => {
                fields.push(self.key_path.clone());
                fields.push(self.passphrase.clone());
            }
            AuthChoice::Password => fields.push(self.password.clone()),
        }
        fields
    }

    /// Move focus to the next (or, `backwards`, previous) currently-rendered text
    /// field, wrapping around at either end. Called from the Tab/Shift+Tab key
    /// handler below; a field with no focus (e.g. the form container itself right
    /// after opening) lands on the first field going forward, or the last going
    /// backward, rather than skipping one.
    fn cycle_focus(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.focusable_fields();
        if fields.is_empty() {
            return;
        }
        let current = fields
            .iter()
            .position(|field| field.read(cx).focus_handle(cx).is_focused(window));
        let target = match current {
            Some(ix) => next_focus_index(ix, fields.len(), backwards),
            None if backwards => fields.len() - 1,
            None => 0,
        };
        fields[target].read(cx).focus(window);
    }

    /// Intercept Tab/Shift+Tab on the bubble phase before it can reach the focused
    /// field's IME/text-insertion path — `stop_propagation` here is what keeps a
    /// literal tab character from ever landing in the input (see `TextInput`'s doc
    /// comment on why typed content only ever arrives via the input-method protocol).
    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key != "tab" {
            return;
        }
        let backwards = event.keystroke.modifiers.shift;
        cx.stop_propagation();
        self.cycle_focus(backwards, window, cx);
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

    /// The secret text the current auth selection carries, if the user typed one.
    fn entered_secret(&self, cx: &App) -> Option<String> {
        let field = match self.auth {
            AuthChoice::Agent => return None,
            AuthChoice::Password => &self.password,
            AuthChoice::Key => &self.passphrase,
        };
        let input = field.read(cx);
        (!input.is_empty()).then(|| input.content().to_string())
    }

    /// Validate and emit [`HostFormEvent::Submit`]; on a validation miss, show the
    /// message and stay open.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let alias = match &self.mode {
            FormMode::Add => self.alias.read(cx).content().to_string(),
            FormMode::Edit { original, .. } => original.alias.clone(),
        };
        let input = FormInput {
            alias,
            user: self.user.read(cx).content().to_string(),
            host: self.host.read(cx).content().to_string(),
            port: self.port.read(cx).content().to_string(),
            auth: self.auth,
            key_path: self.key_path.read(cx).content().to_string(),
        };
        let host = match validate(&input) {
            Ok(host) => host,
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
        let secret = self.entered_secret(cx);
        let old = match &self.mode {
            FormMode::Add => None,
            FormMode::Edit { original, .. } => Some(original.clone()),
        };
        self.error = None;
        cx.emit(HostFormEvent::Submit(Box::new(Submission {
            host,
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

    /// The alias row in edit mode: static text, visibly locked.
    fn locked_alias(&self, alias: &str) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::field_label("alias — locked while editing"))
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
                    .child(SharedString::from(alias.to_string())),
            )
    }

    fn auth_selector(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let segments = [
            ("auth-agent", "agent", AuthChoice::Agent),
            ("auth-key", "key", AuthChoice::Key),
            ("auth-password", "password", AuthChoice::Password),
        ];
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::field_label("auth"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .children(segments.map(|(id, label, choice)| {
                        let active = self.auth == choice;
                        div()
                            .id(id)
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(rgb(if active { ACTIVE_BG } else { FIELD_BG }))
                            .border_1()
                            .border_color(rgb(if active { BRAND } else { FIELD_BORDER }))
                            .text_color(rgb(if active { ACTIVE_FG } else { FG_DIM }))
                            .child(label)
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                this.set_auth(choice, cx);
                            }))
                    })),
            )
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
            "save to — fixed while editing (use ⤒/⤓ to move a host)".into()
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
                    .id("form-cancel")
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
                    .on_click(cx.listener(|_this, _ev, _window, cx| {
                        cx.emit(HostFormEvent::Cancel);
                    })),
            )
            .child(
                div()
                    .id("form-save")
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
                    .on_click(cx.listener(|this, _ev, _window, cx| this.submit(cx))),
            )
    }
}

impl EventEmitter<HostFormEvent> for HostForm {}

impl Focusable for HostForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HostForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match &self.mode {
            FormMode::Add => "Add host",
            FormMode::Edit { .. } => "Edit host",
        };

        div()
            .key_context("HostForm")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_this, _: &FormCancel, _window, cx| {
                cx.emit(HostFormEvent::Cancel);
            }))
            .on_action(cx.listener(|this, _: &FormSubmit, _window, cx| this.submit(cx)))
            .on_key_down(cx.listener(Self::handle_key_down))
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
            .child(match &self.mode {
                FormMode::Add => self.field("alias", &self.alias).into_any_element(),
                FormMode::Edit { original, .. } => {
                    self.locked_alias(&original.alias).into_any_element()
                }
            })
            .child(self.field("user", &self.user))
            .child(self.field("host", &self.host))
            .child(self.field("port", &self.port))
            .child(self.auth_selector(cx))
            .when(self.auth == AuthChoice::Key, |el| {
                el.child(self.field("key path", &self.key_path))
                    .child(self.field("passphrase", &self.passphrase))
            })
            .when(self.auth == AuthChoice::Password, |el| {
                el.child(self.field("password", &self.password))
            })
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

/// Raw field values gathered from the inputs, before validation.
pub(crate) struct FormInput {
    pub alias: String,
    pub user: String,
    pub host: String,
    pub port: String,
    pub auth: AuthChoice,
    pub key_path: String,
}

/// Validate raw field values into a writable [`Host`] (with `secret_ref: None` — the
/// secret lifecycle assigns it later). Rules: alias/user/host non-empty; port parses
/// into 1–65535; key path non-empty when auth is `Key`.
pub(crate) fn validate(input: &FormInput) -> Result<Host, String> {
    let alias = input.alias.trim();
    if alias.is_empty() {
        return Err("alias must not be empty".into());
    }
    let user = input.user.trim();
    if user.is_empty() {
        return Err("user must not be empty".into());
    }
    let host = input.host.trim();
    if host.is_empty() {
        return Err("host must not be empty".into());
    }
    let port: u16 = input
        .port
        .trim()
        .parse()
        .ok()
        .filter(|p| *p >= 1)
        .ok_or_else(|| "port must be a number in 1–65535".to_string())?;
    let auth = match input.auth {
        AuthChoice::Agent => AuthMethod::Agent,
        AuthChoice::Password => AuthMethod::Password,
        AuthChoice::Key => {
            let path = input.key_path.trim();
            if path.is_empty() {
                return Err("key path must not be empty for key auth".into());
            }
            AuthMethod::Key { path: path.into() }
        }
    };
    Ok(Host {
        alias: alias.into(),
        user: user.into(),
        host: host.into(),
        port,
        secret_ref: None,
        auth,
    })
}

/// The attributive add-mode guard: an *add* into a layer that already holds the alias is
/// refused (nothing is ever silently clobbered); only an explicit edit upserts.
/// `target_label` names the offending layer in the message (e.g. `⌂ global`).
pub(crate) fn add_guard(
    is_edit: bool,
    target_holds_alias: bool,
    target_label: &str,
) -> Result<(), String> {
    if !is_edit && target_holds_alias {
        Err(format!("alias exists in {target_label} — edit it instead"))
    } else {
        Ok(())
    }
}

/// Which `save to:` option an add form preselects. `Ask` preselects nothing; a
/// `Workspace` default falls back to no preselection when no workspace is active
/// (a disabled option cannot be preselected). The dialog itself always shows.
pub(crate) fn preselect(default_scope: DefaultScope, workspace_active: bool) -> Option<SaveTarget> {
    match default_scope {
        DefaultScope::Ask => None,
        DefaultScope::Global => Some(SaveTarget::Global),
        DefaultScope::Workspace => workspace_active.then_some(SaveTarget::Workspace),
    }
}

/// Which secret an [`AuthMethod`] stores in the keyring. Distinguishing the slots keeps
/// an old *password* from silently becoming a key *passphrase* when the auth method
/// changes but the masked field is left empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecretSlot {
    /// No stored secret (agent auth).
    None,
    /// The login password.
    Password,
    /// The private key's passphrase.
    KeyPassphrase,
}

/// The slot `auth` keeps its secret in.
pub(crate) fn secret_slot(auth: &AuthMethod) -> SecretSlot {
    match auth {
        AuthMethod::Agent => SecretSlot::None,
        AuthMethod::Password => SecretSlot::Password,
        AuthMethod::Key { .. } => SecretSlot::KeyPassphrase,
    }
}

/// The keyring consequence of a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SecretPlan {
    /// The new record stores no secret; delete the old id if one existed.
    Clear {
        /// The superseded keyring id to delete (after a successful write).
        delete_old: Option<String>,
    },
    /// Keep the existing `secret_ref` untouched (edit with the masked field left empty
    /// and the same secret slot).
    Keep(String),
    /// Put the newly entered secret under a freshly minted id; delete the old id (if
    /// any) after a successful write.
    Mint {
        /// The superseded keyring id to delete (after a successful write).
        delete_old: Option<String>,
    },
}

/// Decide what a save does to the keyring. `old` is the pre-edit record (`None` when
/// adding); `secret_entered` is whether the user typed into the relevant masked field.
pub(crate) fn plan_secret(
    old: Option<&Host>,
    new_auth: &AuthMethod,
    secret_entered: bool,
) -> SecretPlan {
    let old_ref = old.and_then(|h| h.secret_ref.clone());
    let old_slot = old
        .map(|h| secret_slot(&h.auth))
        .unwrap_or(SecretSlot::None);
    let new_slot = secret_slot(new_auth);

    if new_slot == SecretSlot::None {
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
        Some(id) if old_slot == new_slot => SecretPlan::Keep(id),
        other => SecretPlan::Clear { delete_old: other },
    }
}

/// Mint an opaque keyring id: `ssh-<alias>-<unix_nanos>`. Nanosecond timestamps keep
/// same-alias records in different layers from colliding.
pub(crate) fn mint_secret_id(alias: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("ssh-{alias}-{nanos}")
}

/// The result of staging a [`SecretPlan`] against the keyring, ready for the store
/// write.
#[derive(Debug)]
pub(crate) struct StagedSecret {
    /// The `secret_ref` the written host should carry.
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
    alias: &str,
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
            let id = mint_secret_id(alias);
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

    fn input(alias: &str, user: &str, host: &str, port: &str) -> FormInput {
        FormInput {
            alias: alias.into(),
            user: user.into(),
            host: host.into(),
            port: port.into(),
            auth: AuthChoice::Agent,
            key_path: String::new(),
        }
    }

    fn host(alias: &str, secret_ref: Option<&str>, auth: AuthMethod) -> Host {
        Host {
            alias: alias.into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: secret_ref.map(Into::into),
            auth,
        }
    }

    // ---- validate -----------------------------------------------------------

    #[test]
    fn validate_accepts_and_trims_agent_host() {
        let h = validate(&input("  web ", " deploy ", " 10.0.0.1 ", " 22 ")).unwrap();
        assert_eq!(h.alias, "web");
        assert_eq!(h.user, "deploy");
        assert_eq!(h.host, "10.0.0.1");
        assert_eq!(h.port, 22);
        assert_eq!(h.auth, AuthMethod::Agent);
        assert_eq!(h.secret_ref, None);
    }

    #[test]
    fn validate_rejects_empty_alias() {
        let err = validate(&input("  ", "u", "h", "22")).unwrap_err();
        assert!(err.contains("alias"), "{err}");
    }

    #[test]
    fn validate_rejects_empty_user() {
        let err = validate(&input("a", "", "h", "22")).unwrap_err();
        assert!(err.contains("user"), "{err}");
    }

    #[test]
    fn validate_rejects_empty_host() {
        let err = validate(&input("a", "u", " ", "22")).unwrap_err();
        assert!(err.contains("host"), "{err}");
    }

    #[test]
    fn validate_port_bounds() {
        assert_eq!(validate(&input("a", "u", "h", "1")).unwrap().port, 1);
        assert_eq!(
            validate(&input("a", "u", "h", "65535")).unwrap().port,
            65535
        );
        for bad in ["0", "65536", "-1", "abc", "", "2.2"] {
            let err = validate(&input("a", "u", "h", bad)).unwrap_err();
            assert!(err.contains("port"), "{bad} → {err}");
        }
    }

    #[test]
    fn validate_key_auth_requires_key_path() {
        let mut i = input("a", "u", "h", "22");
        i.auth = AuthChoice::Key;
        let err = validate(&i).unwrap_err();
        assert!(err.contains("key path"), "{err}");

        i.key_path = " ~/.ssh/id_ed25519 ".into();
        let h = validate(&i).unwrap();
        assert_eq!(
            h.auth,
            AuthMethod::Key {
                path: "~/.ssh/id_ed25519".into()
            }
        );
    }

    #[test]
    fn validate_password_auth_maps_through() {
        let mut i = input("a", "u", "h", "22");
        i.auth = AuthChoice::Password;
        assert_eq!(validate(&i).unwrap().auth, AuthMethod::Password);
    }

    // ---- add-mode guard -------------------------------------------------------

    #[test]
    fn add_guard_rejects_add_into_occupied_layer_with_named_layer() {
        let err = add_guard(false, true, "⌂ global").unwrap_err();
        assert_eq!(err, "alias exists in ⌂ global — edit it instead");
    }

    #[test]
    fn add_guard_allows_add_into_free_layer() {
        assert!(add_guard(false, false, "⌂ global").is_ok());
    }

    #[test]
    fn add_guard_allows_edit_upsert() {
        assert!(add_guard(true, true, "⌂ global").is_ok());
    }

    // ---- save-to preselection ---------------------------------------------------

    #[test]
    fn preselect_ask_selects_nothing() {
        assert_eq!(preselect(DefaultScope::Ask, true), None);
        assert_eq!(preselect(DefaultScope::Ask, false), None);
    }

    #[test]
    fn preselect_global_always_selects_global() {
        assert_eq!(
            preselect(DefaultScope::Global, false),
            Some(SaveTarget::Global)
        );
        assert_eq!(
            preselect(DefaultScope::Global, true),
            Some(SaveTarget::Global)
        );
    }

    #[test]
    fn preselect_workspace_needs_an_active_workspace() {
        assert_eq!(
            preselect(DefaultScope::Workspace, true),
            Some(SaveTarget::Workspace)
        );
        assert_eq!(preselect(DefaultScope::Workspace, false), None);
    }

    // ---- secret plan ------------------------------------------------------------

    #[test]
    fn plan_add_agent_stores_nothing() {
        assert_eq!(
            plan_secret(None, &AuthMethod::Agent, false),
            SecretPlan::Clear { delete_old: None }
        );
    }

    #[test]
    fn plan_add_password_entered_mints() {
        assert_eq!(
            plan_secret(None, &AuthMethod::Password, true),
            SecretPlan::Mint { delete_old: None }
        );
    }

    #[test]
    fn plan_add_password_not_entered_stores_nothing() {
        // Password auth with no stored password is allowed (prompt at connect time).
        assert_eq!(
            plan_secret(None, &AuthMethod::Password, false),
            SecretPlan::Clear { delete_old: None }
        );
    }

    #[test]
    fn plan_edit_away_from_secret_deletes_old() {
        let old = host("a", Some("ssh-a-1"), AuthMethod::Password);
        assert_eq!(
            plan_secret(Some(&old), &AuthMethod::Agent, false),
            SecretPlan::Clear {
                delete_old: Some("ssh-a-1".into())
            }
        );
    }

    #[test]
    fn plan_edit_same_slot_empty_field_keeps_old() {
        let old = host("a", Some("ssh-a-1"), AuthMethod::Password);
        assert_eq!(
            plan_secret(Some(&old), &AuthMethod::Password, false),
            SecretPlan::Keep("ssh-a-1".into())
        );
        let old_key = host("a", Some("ssh-a-2"), AuthMethod::Key { path: "p".into() });
        assert_eq!(
            plan_secret(Some(&old_key), &AuthMethod::Key { path: "q".into() }, false),
            SecretPlan::Keep("ssh-a-2".into())
        );
    }

    #[test]
    fn plan_edit_replacing_secret_mints_and_deletes_old() {
        let old = host("a", Some("ssh-a-1"), AuthMethod::Password);
        assert_eq!(
            plan_secret(Some(&old), &AuthMethod::Password, true),
            SecretPlan::Mint {
                delete_old: Some("ssh-a-1".into())
            }
        );
    }

    #[test]
    fn plan_edit_slot_change_never_reuses_the_old_secret() {
        // A stored *password* must not silently become a key *passphrase*.
        let old = host("a", Some("ssh-a-1"), AuthMethod::Password);
        assert_eq!(
            plan_secret(Some(&old), &AuthMethod::Key { path: "p".into() }, false),
            SecretPlan::Clear {
                delete_old: Some("ssh-a-1".into())
            }
        );
    }

    #[test]
    fn plan_edit_without_old_ref_same_slot_stores_nothing() {
        let old = host("a", None, AuthMethod::Password);
        assert_eq!(
            plan_secret(Some(&old), &AuthMethod::Password, false),
            SecretPlan::Clear { delete_old: None }
        );
    }

    // ---- minting -----------------------------------------------------------------

    #[test]
    fn mint_id_carries_alias_prefix_and_nanos() {
        let id = mint_secret_id("web");
        let suffix = id.strip_prefix("ssh-web-").expect("prefix");
        assert!(suffix.parse::<u128>().is_ok(), "{id}");
    }

    #[test]
    fn mint_ids_are_unique_across_calls() {
        assert_ne!(mint_secret_id("web"), mint_secret_id("web"));
    }

    // ---- staging against the (fake) keyring ----------------------------------------

    #[test]
    fn stage_mint_puts_bytes_under_fresh_id() {
        let secrets = KeyringStore::with_backend(FakeKeyring::default());
        let plan = SecretPlan::Mint {
            delete_old: Some("ssh-a-old".into()),
        };
        let staged = stage_secret(&secrets, &plan, "a", Some("hunter2")).unwrap();
        let id = staged.secret_ref.expect("minted ref");
        assert!(id.starts_with("ssh-a-"), "{id}");
        assert!(staged.minted);
        assert_eq!(staged.delete_after_write.as_deref(), Some("ssh-a-old"));
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
        secrets.put(&SecretId::new("ssh-a-1"), b"old").unwrap();
        let staged =
            stage_secret(&secrets, &SecretPlan::Keep("ssh-a-1".into()), "a", None).unwrap();
        assert_eq!(staged.secret_ref.as_deref(), Some("ssh-a-1"));
        assert_eq!(staged.delete_after_write, None);
        assert!(!staged.minted);
        assert_eq!(
            secrets.get(&SecretId::new("ssh-a-1")).unwrap().as_deref(),
            Some(&b"old"[..])
        );
    }

    #[test]
    fn stage_clear_defers_the_delete_to_after_the_write() {
        let secrets = KeyringStore::with_backend(FakeKeyring::default());
        secrets.put(&SecretId::new("ssh-a-1"), b"old").unwrap();
        let plan = SecretPlan::Clear {
            delete_old: Some("ssh-a-1".into()),
        };
        let staged = stage_secret(&secrets, &plan, "a", None).unwrap();
        assert_eq!(staged.secret_ref, None);
        assert_eq!(staged.delete_after_write.as_deref(), Some("ssh-a-1"));
        // The old secret must still exist — it is only deleted after a successful write.
        assert!(secrets.get(&SecretId::new("ssh-a-1")).unwrap().is_some());
    }
}
