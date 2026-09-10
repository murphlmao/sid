//! Host add/edit form modal (A6): the write side of the SSH tab.
//!
//! [`HostForm`] is a gpui entity rendered as a [`sid_ui::Modal`] panel (the scrim that
//! centres it is `sid_ui::modal::overlay`, mounted by `app.rs`). It owns one
//! [`TextInput`] per field, an auth-method [`SegmentedControl`], and the `save to:`
//! layer selector built from [`sid_ui::Row`]s. It never touches
//! the store or the keyring itself — on Save it validates and emits
//! [`HostFormEvent::Submit`]; the owner (`AppState`) runs the add-mode guard, the
//! secret lifecycle, and the write, and pushes any store error back into the form's
//! error line via [`HostForm::set_error`].
//!
//! The decision logic is deliberately extracted into plain functions — [`validate`],
//! [`add_guard`], [`plan_secret`], [`stage_secret`], [`preselect`] — so the critical
//! paths (validation, the attributive add-guard, and the keyring lifecycle) are
//! unit-tested without gpui; rendering is observation-gated.

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, Global,
    PathPromptOptions, SharedString, Window, actions, div, prelude::*, px, rgb,
};
use sid_core::keys::{IdentityScan as _, KeyCandidate, PreferredAuth, preferred, preferred_auth};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{AuthMethod, DefaultScope, Host, Scope};

use sid_ui::theme::{self, Theme};
use sid_ui::{
    Button, Elevation, Icon, InputState, Modal, Row, SegmentSelect, SegmentedControl,
    StyledExt as _, TextInput, Toast, Typography as _, caveat_line, h_flex, v_flex,
};

actions!(
    host_form,
    [
        /// Dismiss the form without saving (bound to `escape`).
        FormCancel,
        /// Validate and submit the form (bound to `enter`).
        FormSubmit,
    ]
);

/// The `port` field's place in the tab order, and the `auth` selector's with it.
///
/// The fields are numbered by hand from 1 (`alias`, `user`, `host`, `port`, `key path`,
/// `passphrase`, `password`) because gpui sorts tab stops by index first and paint order
/// only within an index — so a control that does not name one sits at 0 and jumps the
/// whole form. Only the two indices something else has to agree with are named.
const PORT_TAB_INDEX: isize = 4;

/// The `save to:` rows' place: after every field, because that is where they render and
/// because choosing a layer is the last decision the form asks for.
const SAVE_TO_TAB_INDEX: isize = 8;

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

// ---- opening an add form with something already in it (issue #2) --------------------

/// What an add form should open holding, plus what to do once it has been saved.
///
/// Quick-connect's *"that host isn't saved — add it"* path: everything the user already
/// typed into the filter box goes straight into the fields, so the form is a
/// confirmation rather than a re-type.
pub(crate) struct AddPrefill {
    /// The record's proposed name.
    pub alias: String,
    /// The login.
    pub user: String,
    /// The hostname or address.
    pub host: String,
    /// The port.
    pub port: u16,
    /// Run once the owner has had its chance to write the record — see [`AfterSave`].
    pub after_save: Option<AfterSave>,
}

/// The follow-up an opener attaches to a prefilled add: the submitted host and the layer
/// it was aimed at.
///
/// A closure rather than a flag on [`Submission`], so the form stays as ignorant of the
/// store as it has always been (see this module's opening comment). Only the opener knows
/// what a saved record is *for* — quick-connect uses this to dial the connection it just
/// talked the user into saving. It runs on the turn of the effect loop *after* the submit
/// event, so by the time it fires the record is either written or refused; it is expected
/// to re-check the store rather than assume.
pub(crate) type AfterSave = Rc<dyn Fn(&Host, &Scope, &mut Window, &mut App)>;

/// A one-shot handoff for the next [`HostForm::new_add`].
///
/// **Why a global and not a parameter.** Every add-connection entry point funnels through
/// `AppState::open_add_form`, which builds the form and owns both it and its event
/// subscription — all three private to `app.rs`, which is outside this change's blast
/// radius. A gpui `Global` is the framework's own answer to exactly this shape: one value,
/// set at one call site and consumed at another, on the single main thread, with no
/// second owner and no lifetime to thread through.
///
/// It is **taken**, not read, so a prefill can never bleed into a later form: a plain
/// `+ add connection` click that follows a cancelled quick-connect add opens empty.
/// Collapse this into an `open_add_form(prefill)` parameter once `app.rs` is free.
#[derive(Default)]
struct PendingAdd(Option<AddPrefill>);

impl Global for PendingAdd {}

/// Queue `prefill` for the next add form that opens.
pub(crate) fn queue_add_prefill(cx: &mut App, prefill: AddPrefill) {
    cx.set_global(PendingAdd(Some(prefill)));
}

/// Take whatever was queued, leaving nothing behind for the form after this one.
fn take_add_prefill(cx: &mut App) -> Option<AddPrefill> {
    cx.has_global::<PendingAdd>()
        .then(|| cx.global_mut::<PendingAdd>().0.take())
        .flatten()
}

/// What this machine can authenticate with, as the form needs it: the keys on disk, where
/// they were found, and whether an agent is actually there.
struct MachineIdentities {
    dir: PathBuf,
    keys: Vec<KeyCandidate>,
    agent: bool,
}

/// Ask the machine what it has. The one place in this crate that names a concrete
/// [`sid_core::keys::IdentityScan`] implementation — composition-root work, done here
/// because `app.rs` (which constructs this form) is out of reach; move the call there and
/// pass the result in when it is not.
///
/// Every failure collapses to "nothing found", because there is nothing else the form
/// could usefully do with one: no `$HOME`, no `~/.ssh`, and an unreadable `~/.ssh` all
/// produce the same screen — a picker that says where it looked and a field to type a
/// path into.
fn scan_identities() -> MachineIdentities {
    let Ok(local) = sid_ssh::keyscan::LocalIdentities::from_env() else {
        return MachineIdentities {
            dir: PathBuf::new(),
            keys: Vec::new(),
            agent: false,
        };
    };
    MachineIdentities {
        dir: local.key_dir(),
        keys: local.keys().unwrap_or_default(),
        agent: local.agent_available(),
    }
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
    alias: Entity<InputState>,
    user: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    key_path: Entity<InputState>,
    passphrase: Entity<InputState>,
    password: Entity<InputState>,
    auth: AuthChoice,
    /// The selected save target; `None` = nothing preselected (the `Ask` default).
    save_to: Option<SaveTarget>,
    /// The active workspace scope + its display label, if one is focused. Enables the
    /// `workspace` save target and names it in messages.
    workspace: Option<(Scope, SharedString)>,
    error: Option<SharedString>,
    /// What this machine can authenticate with (issue #2) — the key picker's contents,
    /// and the fact that decides whether agent auth is a sane default at all.
    identities: MachineIdentities,
    /// The opener's follow-up, if it asked for one. Kept (not taken) across a failed
    /// submit so a corrected re-save still runs it.
    after_save: Option<AfterSave>,
    focus_handle: FocusHandle,
}

impl HostForm {
    /// An empty add form. `default_scope` drives the `save to:` preselection
    /// ([`preselect`]); the dialog itself always shows. `secrets_degraded` is
    /// `AppState::secrets_degraded` (round-D §A.5) — memory-only backend swaps the
    /// password field's helper copy to say so, since it means a password entered here
    /// won't outlive this session.
    /// A prefill queued by [`queue_add_prefill`] (quick-connect's "add the thing you just
    /// typed") fills the fields and attaches its follow-up.
    pub fn new_add(
        window: &mut Window,
        cx: &mut Context<Self>,
        workspace: Option<(Scope, SharedString)>,
        default_scope: DefaultScope,
        secrets_degraded: bool,
    ) -> Self {
        let workspace_active = workspace.is_some();
        let prefill = take_add_prefill(cx);
        let seed = prefill.as_ref().map(|p| Host {
            alias: p.alias.clone(),
            user: p.user.clone(),
            host: p.host.clone(),
            port: p.port,
            secret_ref: None,
            auth: AuthMethod::Agent,
            folder: None,
        });
        let mut form = Self::new_inner(window, cx, workspace, seed.as_ref(), secrets_degraded);
        form.save_to = preselect(default_scope, workspace_active);
        form.after_save = prefill.and_then(|p| p.after_save);
        // Issue #2's root cause, fixed where a record is *born*: `AuthMethod::Agent` is
        // the `#[default]`, so on a machine with no agent — the normal case for a
        // desktop launch, which inherits no `SSH_AUTH_SOCK` — every host sid has ever
        // added was created pointing at something that was never there, and only said so
        // at connect time. With a key already on disk, the key is the honest default.
        if preferred_auth(form.identities.agent, !form.identities.keys.is_empty())
            == PreferredAuth::Key
        {
            form.auth = AuthChoice::Key;
        }
        form
    }

    /// An edit form prefilled from `original`, writing back into `origin` on save.
    /// The alias is locked (rename is out of scope for P3.2). See [`Self::new_add`] for
    /// `secrets_degraded`.
    pub fn new_edit(
        window: &mut Window,
        cx: &mut Context<Self>,
        original: Host,
        origin: Scope,
        workspace: Option<(Scope, SharedString)>,
        secrets_degraded: bool,
    ) -> Self {
        let mut form = Self::new_inner(window, cx, workspace, Some(&original), secrets_degraded);
        form.save_to = Some(match &origin {
            Scope::Global => SaveTarget::Global,
            Scope::Workspace(_) => SaveTarget::Workspace,
        });
        form.mode = FormMode::Edit { original, origin };
        form
    }

    fn new_inner(
        window: &mut Window,
        cx: &mut Context<Self>,
        workspace: Option<(Scope, SharedString)>,
        prefill: Option<&Host>,
        secrets_degraded: bool,
    ) -> Self {
        let mk = |window: &mut Window,
                  cx: &mut Context<Self>,
                  placeholder: &str,
                  value: Option<String>| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(placeholder);
                if let Some(v) = value {
                    input.set_value(v, window, cx);
                }
                input
            })
        };
        let mk_masked = |window: &mut Window, cx: &mut Context<Self>, placeholder: &str| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(placeholder)
            })
        };

        // A stored secret is never read back into the UI: an empty masked field on an
        // edit means "keep the existing secret" (see `plan_secret`). The password
        // field's hint additionally distinguishes a degraded (memory) backend (round-D
        // §A.5) — the passphrase field doesn't, since a `Key` auth's passphrase is
        // always optional and never triggers the connect-time prompt regardless of
        // backend health (see `ssh_connect::needs_password_prompt`'s doc comment).
        let has_stored_secret = prefill.is_some_and(|h| h.secret_ref.is_some());
        let password_hint = if has_stored_secret {
            "leave empty to keep the stored secret"
        } else if secrets_degraded {
            "no OS keyring — passwords last this session only; you'll be asked at connect"
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

        // Issue #2, second half: *"the default key path be fine"*. A record that already
        // names a key keeps it; everything else opens with the conventional one this
        // machine actually has (`id_ed25519`, then `id_rsa`, then whatever else the scan
        // turned up — `sid_core::keys::preferred`). Filling it in even for an agent- or
        // password-auth record is deliberate: switching the segment to `key` then costs
        // nothing, which is exactly the move a failed agent connect asks for.
        let identities = scan_identities();
        let key_path_value = key_path_value
            .or_else(|| preferred(&identities.keys).map(|c| c.path.display().to_string()));

        Self {
            alias: mk(
                window,
                cx,
                "alias — unique short name",
                prefill.map(|h| h.alias.clone()),
            ),
            user: mk(window, cx, "user", prefill.map(|h| h.user.clone())),
            host: mk(
                window,
                cx,
                "hostname or address",
                prefill.map(|h| h.host.clone()),
            ),
            port: mk(
                window,
                cx,
                "port",
                Some(
                    prefill
                        .map(|h| h.port.to_string())
                        .unwrap_or_else(|| "22".into()),
                ),
            ),
            key_path: mk(window, cx, "~/.ssh/id_ed25519", key_path_value),
            passphrase: mk_masked(window, cx, passphrase_hint),
            password: mk_masked(window, cx, password_hint),
            auth,
            save_to: None,
            workspace,
            mode: FormMode::Add,
            error: None,
            identities,
            after_save: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Focus the first editable field: alias when adding, user when editing (the alias
    /// is locked in edit mode).
    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        let target = match &self.mode {
            FormMode::Add => &self.alias,
            FormMode::Edit { .. } => &self.user,
        };
        target.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Surface an owner-side failure (guard/secret/store) in the form's error line.
    pub fn set_error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(msg.into());
        cx.notify();
    }

    /// Switch the auth segment, clearing secret fields that no longer apply so stale
    /// secret text never lingers in memory or leaks across methods.
    fn set_auth(&mut self, choice: AuthChoice, window: &mut Window, cx: &mut Context<Self>) {
        if self.auth == choice {
            return;
        }
        self.auth = choice;
        match choice {
            AuthChoice::Agent => {
                self.password
                    .update(cx, |i, cx| i.set_value("", window, cx));
                self.passphrase
                    .update(cx, |i, cx| i.set_value("", window, cx));
            }
            AuthChoice::Key => self
                .password
                .update(cx, |i, cx| i.set_value("", window, cx)),
            AuthChoice::Password => self
                .passphrase
                .update(cx, |i, cx| i.set_value("", window, cx)),
        }
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

    /// The secret text the current auth selection carries, if the user typed one.
    fn entered_secret(&self, cx: &App) -> Option<String> {
        let field = match self.auth {
            AuthChoice::Agent => return None,
            AuthChoice::Password => &self.password,
            AuthChoice::Key => &self.passphrase,
        };
        let input = field.read(cx);
        let value = input.value();
        (!value.is_empty()).then(|| value.to_string())
    }

    /// Point the key-path field at `path` — the picker's and the file dialog's one write
    /// into the form.
    fn set_key_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.key_path
            .update(cx, |input, cx| input.set_value(path, window, cx));
        self.error = None;
        cx.notify();
    }

    /// Open the platform file dialog on a key the scan did not turn up — issue #2's
    /// *"let the user select a key if we can't find any despite them saying a key
    /// exists"*.
    ///
    /// A dialog is not guaranteed to exist (on Linux it is a desktop portal, which a bare
    /// compositor may not run), so a failure is answered with the fallback that always
    /// works rather than with the portal's error: the field above takes a typed path.
    fn browse_for_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use this key".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let chosen = picked.await;
            this.update_in(cx, |form, window, cx| match chosen {
                Ok(Ok(Some(paths))) => {
                    if let Some(path) = paths.first() {
                        form.set_key_path(path.display().to_string(), window, cx);
                    }
                }
                // Dismissed. Nothing to say.
                Ok(Ok(None)) => {}
                _ => form.set_error(
                    "no file picker is available here — type the key's path into the \
                     field above",
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Validate and emit [`HostFormEvent::Submit`]; on a validation miss, show the
    /// message and stay open.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let alias = match &self.mode {
            FormMode::Add => self.alias.read(cx).value().to_string(),
            FormMode::Edit { original, .. } => original.alias.clone(),
        };
        let input = FormInput {
            alias,
            user: self.user.read(cx).value().to_string(),
            host: self.host.read(cx).value().to_string(),
            port: self.port.read(cx).value().to_string(),
            auth: self.auth,
            key_path: self.key_path.read(cx).value().to_string(),
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
        let saved = host.clone();
        let layer = target.clone();
        cx.emit(HostFormEvent::Submit(Box::new(Submission {
            host,
            target,
            old,
            secret,
        })));
        // The opener's follow-up, one turn of the effect loop later. `Window::defer`
        // queues behind the `Emit` above, so the owner has already run the add-mode
        // guard, staged the secret and written (or refused) the record by the time this
        // fires — and it survives this form being dropped by that same handler, which a
        // subscription on this entity would not.
        if let Some(after_save) = self.after_save.clone() {
            window.defer(cx, move |window, cx| after_save(&saved, &layer, window, cx));
        }
        cx.notify();
    }

    // ---- render pieces ------------------------------------------------------

    /// A field's caption. `Meta` (12px, muted): this is orientation above the value,
    /// not a section header — those are UPPERCASE `Label`.
    fn field_label(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        div().text_meta(&theme).child(text.into())
    }

    fn field(
        &self,
        label: &'static str,
        input: &Entity<InputState>,
        tab_index: isize,
        cx: &App,
    ) -> impl IntoElement + use<> {
        v_flex()
            .gap_1()
            .child(Self::field_label(label, cx))
            .child(TextInput::new(input).tab_index(tab_index))
    }

    /// The alias row in edit mode: static text in an input-shaped recess, so it reads as
    /// a field that cannot be typed in rather than as a stray line of prose.
    fn locked_alias(&self, alias: &str, cx: &App) -> impl IntoElement + use<> {
        let theme = theme::active(cx).clone();
        v_flex()
            .gap_1()
            .child(Self::field_label("alias — locked while editing", cx))
            .child(
                div()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .elevation(Elevation::Well, &theme)
                    .text_body(&theme)
                    .text_color(rgb(theme.muted))
                    .child(SharedString::from(alias.to_string())),
            )
    }

    fn auth_selector(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // The `h_flex` wrapper is what keeps the control hugging its three chips: a flex
        // column stretches its children across the cross axis, which left the segmented
        // track spanning the whole panel with two-thirds of it empty.
        v_flex().gap_1().child(Self::field_label("auth", cx)).child(
            h_flex().child(
                SegmentedControl::new("host-form-auth")
                    .segments(AUTH_SEGMENTS.map(|(label, _)| label))
                    // Shares the port field's index and renders after it, so gpui orders
                    // it there by paint order. Left at the default 0 the whole strip
                    // would sort ahead of `alias`.
                    .tab_index(PORT_TAB_INDEX)
                    .selected(auth_index(self.auth))
                    .on_select(cx.listener(|this, ev: &SegmentSelect, window, cx| {
                        this.set_auth(auth_at(ev.index), window, cx);
                    })),
            ),
        )
    }

    /// The key picker: every private key this machine actually has, as a chip that fills
    /// the path field, plus a way to reach one the scan never saw.
    ///
    /// Issue #2 asked for two things here and they are different requests. *"Autoscan for
    /// a key"* is the common case and is already answered above the picker — the path
    /// field opens filled in. This is the other one: *"let the user select a key if we
    /// can't find any despite them saying a key exists"*. So the row is present whether
    /// or not the scan found anything, and when it found nothing it says **where** it
    /// looked — a picker that just renders empty leaves the user unable to tell a broken
    /// scan from an empty `~/.ssh`.
    ///
    /// Nothing here reads a key. The chips are file names off the port's candidates (see
    /// `sid_core::keys`' privacy invariant), and clicking one only writes a path into a
    /// text field.
    fn key_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = theme::active(cx).clone();
        let current = self.key_path.read(cx).value().trim().to_string();
        let dir = self.identities.dir.display().to_string();

        let browse = Button::new("host-form-key-browse", "browse…")
            .small()
            .icon(Icon::Folder)
            .tooltip("pick a key file this scan didn't find")
            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                this.browse_for_key(window, cx);
            }));

        let found = match self.identities.keys.is_empty() {
            true => div()
                .child(caveat_line(format!("no keys found in {dir}")))
                .into_any_element(),
            false => {
                let mut chips = h_flex().gap_1().flex_wrap();
                for (index, candidate) in self.identities.keys.iter().enumerate() {
                    let path = candidate.path.display().to_string();
                    let chip = Button::new(("host-form-key", index), candidate.name.clone())
                        .small()
                        .tooltip(path.clone())
                        .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                            this.set_key_path(path.clone(), window, cx);
                        }));
                    // Which key is chosen still has to be answerable without reading
                    // the path, but an accent fill was the loudest mark in the whole
                    // form for a fact this quiet — a selected key, not an action to
                    // take. Neutral solid (the default fill, one rung above the card)
                    // with an accent hairline says "this one" without shouting;
                    // unselected drops to a neutral outline instead of matching that
                    // fill, so the chosen chip is still the only filled one.
                    let chosen = candidate.path.display().to_string() == current;
                    chips = chips.child(match chosen {
                        true => chip.border_color(rgb(theme.accent)),
                        false => chip.ghost(),
                    });
                }
                chips.into_any_element()
            }
        };

        v_flex()
            .gap_1()
            .child(Self::field_label(
                match self.identities.keys.is_empty() {
                    true => "keys on this machine".to_string(),
                    false => format!("keys in {dir}"),
                },
                cx,
            ))
            .child(found)
            .child(
                h_flex()
                    .gap_2()
                    .child(browse)
                    .child(div().text_meta(&theme).child("or type a path above")),
            )
    }

    fn save_to_selector(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let locked = matches!(self.mode, FormMode::Edit { .. });
        let ws_active = self.workspace.is_some();
        let theme = theme::active(cx).clone();

        let option = |id: &'static str,
                      title: &'static str,
                      note: &'static str,
                      target: SaveTarget,
                      enabled: bool,
                      selected: bool,
                      theme: &Theme,
                      cx: &mut Context<Self>| {
            let ink = if enabled { theme.fg } else { theme.faint };
            Row::new(id)
                .selected(selected)
                // The picker is a required choice inside a modal, so it has to be
                // reachable from the keyboard — it was the one control in this form that
                // Tab could not get to. Last in the order, where it renders.
                .tab_index(SAVE_TO_TAB_INDEX)
                .selected(selected)
                .leading(radio_mark(selected, enabled, theme))
                // Title and note in the *same* slot: `Row`'s meta slot is right-anchored,
                // which parked "— .sid/ · travels with git" 250px from the word it
                // qualifies. The design system's own rule — a label's action never lives
                // a screen-width away — reads on this scale too.
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().text_body(theme).text_color(rgb(ink)).child(title))
                        .child(div().text_meta(theme).child(note)),
                )
                // A disabled option installs no click handler at all, so `Row` renders
                // it inert — no pointer, no hover fill, nothing promising it will react.
                .when(enabled, |row| {
                    row.on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
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

        let workspace_note =
            workspace_option_note(ws_active).unwrap_or("— .sid/ · travels with git");

        v_flex().gap_1().child(Self::field_label(label, cx)).child(
            v_flex()
                .gap_0p5()
                .child(option(
                    "save-workspace",
                    "workspace",
                    workspace_note,
                    SaveTarget::Workspace,
                    ws_active && !locked,
                    self.save_to == Some(SaveTarget::Workspace),
                    &theme,
                    cx,
                ))
                .child(option(
                    "save-global",
                    "global",
                    "— everywhere · never lost",
                    SaveTarget::Global,
                    !locked,
                    self.save_to == Some(SaveTarget::Global),
                    &theme,
                    cx,
                )),
        )
    }
}

/// The auth segments, in render order. One list: the labels the selector shows and the
/// choices they map to cannot drift apart into "clicking `key` selects password".
const AUTH_SEGMENTS: [(&str, AuthChoice); 3] = [
    ("agent", AuthChoice::Agent),
    ("key", AuthChoice::Key),
    ("password", AuthChoice::Password),
];

/// Which segment is lit for `choice`.
fn auth_index(choice: AuthChoice) -> usize {
    AUTH_SEGMENTS
        .iter()
        .position(|(_, c)| *c == choice)
        .unwrap_or(0)
}

/// Which choice segment `index` means. An out-of-range index (impossible unless the
/// control and this list disagree) falls back to `Agent` — the method that stores no
/// secret, so a drift can never silently arm a password field.
fn auth_at(index: usize) -> AuthChoice {
    AUTH_SEGMENTS
        .get(index)
        .map_or(AuthChoice::Agent, |(_, choice)| *choice)
}

/// Why the `workspace` option is disabled, if it is. `None` while a workspace is
/// focused, when the option's own everyday description ("— .sid/ · travels with
/// git") stays put — a greyed row with no reason read as broken, not "add a
/// workspace first".
fn workspace_option_note(ws_active: bool) -> Option<&'static str> {
    (!ws_active).then_some("— no workspace focused")
}

/// The save-to picker's radio mark: a ring that gains a filled core when chosen.
///
/// Drawn rather than glyphed. The `●`/`○` pair this replaces renders in whatever the
/// ambient font happens to have, at whatever weight, and in most families the two are
/// visibly different sizes — so the picker jittered as the selection moved.
fn radio_mark(selected: bool, enabled: bool, theme: &Theme) -> impl IntoElement + use<> {
    let edge = match (selected, enabled) {
        (true, _) => theme.accent,
        // Was inverted: the choosable, unselected ring read fainter (`border`) than
        // the disabled one (`faint`), so a genuinely clickable option looked less
        // present than one you cannot click.
        (false, true) => theme.muted,
        (false, false) => theme.border,
    };
    div()
        .size_3()
        .rounded_full()
        .border_1()
        .border_color(rgb(edge))
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |mark| {
            mark.child(div().size_1p5().rounded_full().bg(rgb(theme.accent)))
        })
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

        // The key context, the focus handle and the three key handlers stay on a wrapper
        // around the panel: `sid_ui::Modal` is a plain element with no lifecycle, and
        // Escape/Enter/Tab are this entity's own bindings (see `sid_ui::modal`'s "what
        // the panel does not own").
        div()
            .key_context("HostForm")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_this, _: &FormCancel, _window, cx| {
                cx.emit(HostFormEvent::Cancel);
            }))
            .on_action(cx.listener(|this, _: &FormSubmit, window, cx| this.submit(window, cx)))
            .child(
                Modal::new("host-form", title)
                    .submit_hint("saves")
                    .on_dismiss(cx.listener(|_this, _ev: &ClickEvent, _window, cx| {
                        cx.emit(HostFormEvent::Cancel);
                    }))
                    .child(match &self.mode {
                        FormMode::Add => self.field("alias", &self.alias, 1, cx).into_any_element(),
                        FormMode::Edit { original, .. } => {
                            self.locked_alias(&original.alias, cx).into_any_element()
                        }
                    })
                    .child(self.field("user", &self.user, 2, cx))
                    .child(self.field("host", &self.host, 3, cx))
                    .child(
                        v_flex().gap_1().child(Self::field_label("port", cx)).child(
                            TextInput::new(&self.port)
                                .fixed(px(90.))
                                .tab_index(PORT_TAB_INDEX),
                        ),
                    )
                    .child(self.auth_selector(cx))
                    .when(self.auth == AuthChoice::Key, |modal| {
                        modal
                            .child(self.field("key path", &self.key_path, 5, cx))
                            .child(self.key_picker(cx))
                            .child(self.field("passphrase", &self.passphrase, 6, cx))
                    })
                    .when(self.auth == AuthChoice::Password, |modal| {
                        modal.child(self.field("password", &self.password, 7, cx))
                    })
                    .child(self.save_to_selector(cx))
                    .when_some(self.error.clone(), |modal, err| {
                        modal.child(Toast::danger(err))
                    })
                    .footer(Button::new("host-form-cancel", "Cancel").ghost().on_click(
                        cx.listener(|_this, _ev: &ClickEvent, _window, cx| {
                            cx.emit(HostFormEvent::Cancel);
                        }),
                    ))
                    .footer(Button::new("host-form-save", "Save").primary().on_click(
                        cx.listener(|this, _ev: &ClickEvent, window, cx| this.submit(window, cx)),
                    )),
            )
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
        folder: None,
    })
}

/// The attributive add-mode guard: an *add* into a layer that already holds the alias is
/// refused (nothing is ever silently clobbered); only an explicit edit upserts.
/// `target_label` names the offending layer in the message (e.g. `global`).
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

    #[test]
    fn a_focused_workspace_leaves_the_option_alone() {
        assert_eq!(workspace_option_note(true), None);
    }

    #[test]
    fn no_focused_workspace_explains_why_its_disabled() {
        assert_eq!(workspace_option_note(false), Some("— no workspace focused"));
    }

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
            folder: None,
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

    // ---- auth segments ----------------------------------------------------------

    #[test]
    fn every_auth_choice_round_trips_through_its_segment() {
        // The failure this prevents: the segment list and the choice list drifting, so
        // clicking `key` selects password auth. Both directions, exhaustively.
        for (ix, (_, choice)) in AUTH_SEGMENTS.iter().enumerate() {
            assert_eq!(auth_index(*choice), ix, "{choice:?}");
            assert_eq!(auth_at(ix), *choice, "index {ix}");
        }
    }

    #[test]
    fn the_segments_are_labelled_in_render_order() {
        assert_eq!(
            AUTH_SEGMENTS.map(|(label, _)| label),
            ["agent", "key", "password"]
        );
    }

    #[test]
    fn an_out_of_range_segment_falls_back_to_the_secretless_method() {
        // Unreachable unless the control and the list disagree — and if they ever do,
        // the safe landing is the method that stores nothing in the keyring.
        assert_eq!(auth_at(AUTH_SEGMENTS.len()), AuthChoice::Agent);
        assert_eq!(auth_at(usize::MAX), AuthChoice::Agent);
    }

    // ---- add-mode guard -------------------------------------------------------

    #[test]
    fn add_guard_rejects_add_into_occupied_layer_with_named_layer() {
        let err = add_guard(false, true, "global").unwrap_err();
        assert_eq!(err, "alias exists in global — edit it instead");
    }

    #[test]
    fn add_guard_allows_add_into_free_layer() {
        assert!(add_guard(false, false, "global").is_ok());
    }

    #[test]
    fn add_guard_allows_edit_upsert() {
        assert!(add_guard(true, true, "global").is_ok());
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
