//! The encrypted-file secret vault's unlock/create modal.
//!
//! `// ponytail:` v1 simplification (per the secret-backends plan): this only ever
//! appears **once, at startup**, when `sid_secrets::resolve_secret_store` picks the
//! encrypted-file backend as effective. It is not threaded as a lazy per-call-site
//! retry — every other `SecretStore` call site (host connect, the host/db forms, …)
//! already surfaces a plain [`sid_secrets::SecretError`] through its existing error
//! line, and `SecretError::Locked`'s `Display` reads fine there on its own. If the user
//! cancels this modal, the vault just stays locked for the rest of the session (every
//! subsequent secret op returns `Locked`) rather than nagging on every attempt — no
//! retry loop is threaded through `host_form.rs`/`session.rs`/etc.
//!
//! Also a deliberate v1 gap: this modal has no `key_context`/keyboard actions (no
//! Escape-to-cancel, no Enter-to-submit) — only the Cancel/Unlock/Create buttons are
//! wired, via a plain `on_click` the same way `HostForm`'s buttons are. Wiring
//! `escape`/`enter` would need a `KeyBinding` registered in `ui::init` (`ui/mod.rs`),
//! which is out of this change's file-ownership scope; the fields themselves are still
//! fully usable (typing, backspace, mouse selection all work — they're the same
//! `TextInput` used everywhere else in `sid`).

use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Window, div, prelude::*,
    px, rgb,
};
use sid_secrets::EncryptedFileStore;

use super::TextInput;

// Dark-theme palette, aligned with `app.rs`/`host_form.rs`. Kept local so `ui` stays
// self-contained.
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

/// Which side of the encrypted-file vault's lifecycle this prompt is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretUnlockMode {
    /// A vault file already exists — enter its passphrase.
    Unlock,
    /// No vault file exists yet — choose (and confirm) a new passphrase.
    Create,
}

/// Events the modal emits to its owner (`AppState`).
pub enum SecretUnlockEvent {
    /// Dismiss without unlocking/creating.
    Cancel,
    /// The vault is now unlocked (or freshly created) — the owner closes the modal.
    Done,
}

/// The unlock/create modal for [`EncryptedFileStore`].
pub struct SecretUnlockModal {
    handle: Arc<EncryptedFileStore>,
    mode: SecretUnlockMode,
    passphrase: Entity<TextInput>,
    /// Only rendered/read in [`SecretUnlockMode::Create`] — must match `passphrase`.
    confirm: Entity<TextInput>,
    error: Option<SharedString>,
}

impl SecretUnlockModal {
    /// A fresh modal over `handle`, in `mode`.
    pub fn new(
        cx: &mut Context<Self>,
        handle: Arc<EncryptedFileStore>,
        mode: SecretUnlockMode,
    ) -> Self {
        Self {
            handle,
            mode,
            passphrase: cx.new(|cx| TextInput::new_masked(cx, "passphrase")),
            confirm: cx.new(|cx| TextInput::new_masked(cx, "confirm passphrase")),
            error: None,
        }
    }

    /// Move keyboard focus into the passphrase field. Called once, right after the
    /// modal entity is created (see `AppState::open_secret_unlock`).
    pub fn focus_first(&self, window: &mut Window, cx: &App) {
        self.passphrase.read(cx).focus(window);
    }

    /// Surface an owner- or self-detected failure in the modal's error line.
    pub fn set_error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(msg.into());
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let passphrase = self.passphrase.read(cx).content().to_string();
        let result: Result<(), String> = match self.mode {
            SecretUnlockMode::Unlock => validate_unlock(&passphrase)
                .and_then(|p| self.handle.unlock(&p).map_err(|e| e.to_string())),
            SecretUnlockMode::Create => {
                let confirm = self.confirm.read(cx).content().to_string();
                validate_create(&passphrase, &confirm)
                    .and_then(|p| self.handle.create(&p).map_err(|e| e.to_string()))
            }
        };
        match result {
            Ok(()) => cx.emit(SecretUnlockEvent::Done),
            Err(msg) => self.set_error(msg, cx),
        }
    }

    fn field(label: &'static str, input: &Entity<TextInput>) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(rgb(FG_DIM)).child(label))
            .child(input.clone())
    }

    fn buttons(
        &self,
        submit_label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap_2()
            .child(
                div()
                    .id("secret-unlock-cancel")
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
                        cx.emit(SecretUnlockEvent::Cancel);
                    })),
            )
            .child(
                div()
                    .id("secret-unlock-submit")
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(ACTIVE_BG))
                    .border_1()
                    .border_color(rgb(BRAND))
                    .text_color(rgb(ACTIVE_FG))
                    .child(submit_label)
                    .on_click(cx.listener(|this, _ev, _window, cx| this.submit(cx))),
            )
    }
}

impl EventEmitter<SecretUnlockEvent> for SecretUnlockModal {}

impl Render for SecretUnlockModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, hint, submit_label): (&str, &str, &'static str) = match self.mode {
            SecretUnlockMode::Unlock => (
                "Unlock secret vault",
                "the encrypted-file backend protects your stored passwords and key \
                 passphrases — enter its passphrase to use them this session.",
                "Unlock",
            ),
            SecretUnlockMode::Create => (
                "Create secret vault",
                "no OS keyring is in use, so sid needs a passphrase to protect the \
                 encrypted-file vault that will hold passwords and key passphrases \
                 instead.",
                "Create",
            ),
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .w(px(420.))
            .p_4()
            .rounded_lg()
            .bg(rgb(PANEL_BG))
            .border_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(FG))
            .child(div().text_sm().text_color(rgb(FG)).child(title.to_string()))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(hint.to_string()),
            )
            .child(Self::field("passphrase", &self.passphrase))
            .when(matches!(self.mode, SecretUnlockMode::Create), |el| {
                el.child(Self::field("confirm passphrase", &self.confirm))
            })
            .when_some(self.error.clone(), |el, err| {
                el.child(div().text_sm().text_color(rgb(DANGER)).child(err))
            })
            .child(self.buttons(submit_label, cx))
    }
}

// ---------------------------------------------------------------------------
// Pure decision logic (unit-tested without gpui)
// ---------------------------------------------------------------------------

/// Validate an unlock attempt's raw input before it ever touches the vault.
pub(crate) fn validate_unlock(passphrase: &str) -> Result<String, String> {
    if passphrase.is_empty() {
        Err("enter the vault passphrase".into())
    } else {
        Ok(passphrase.to_string())
    }
}

/// Validate a create attempt: both fields non-empty and matching.
pub(crate) fn validate_create(passphrase: &str, confirm: &str) -> Result<String, String> {
    if passphrase.is_empty() {
        Err("choose a passphrase".into())
    } else if passphrase != confirm {
        Err("passphrases do not match".into())
    } else {
        Ok(passphrase.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_unlock_rejects_empty() {
        assert!(validate_unlock("").is_err());
    }

    #[test]
    fn validate_unlock_accepts_nonempty() {
        assert_eq!(validate_unlock("hunter2").unwrap(), "hunter2");
    }

    #[test]
    fn validate_create_rejects_empty_passphrase() {
        let err = validate_create("", "").unwrap_err();
        assert!(err.contains("choose"), "{err}");
    }

    #[test]
    fn validate_create_rejects_mismatch() {
        let err = validate_create("hunter2", "hunter3").unwrap_err();
        assert!(err.contains("match"), "{err}");
    }

    #[test]
    fn validate_create_accepts_matching_nonempty() {
        assert_eq!(validate_create("hunter2", "hunter2").unwrap(), "hunter2");
    }
}
