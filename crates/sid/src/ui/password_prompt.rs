//! Connect-time password prompt (round-D §A.4).
//!
//! Replaces the old encrypted-file vault's startup unlock/create modal (deleted along
//! with this crate's wiring to `sid_secrets::file::EncryptedFileStore` — see
//! `sid_secrets::resolve`'s module doc). The new model is **keyring → memory**: when a
//! connect attempt (an SSH host, a DB connection) needs a password but the secret store
//! has nothing concrete for it — no OS keyring persisting it, or a dangling
//! `secret_ref` — this modal asks for it right then, once, instead of failing outright.
//!
//! This modal never persists anything itself. It only ever hands the plaintext back to
//! its owner (`AppState::on_password_prompt_event`) exactly once, on submit; that
//! caller decides whether to `secrets.put` it under a pre-existing `secret_ref` (so the
//! rest of the session remembers it) or use it as a pure one-shot. Never logged, never
//! written to config from here or there.
//!
//! See `crate::ssh_connect::needs_password_prompt` / `crate::ui::db_tab::needs_password_prompt`
//! for the pure decisions that trigger opening this modal.

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Window, actions, div, prelude::*,
};

use super::TextInput;
use sid_ui::{Button, Modal, Toast};

actions!(
    password_prompt,
    [
        /// Dismiss without connecting (bound to `escape`).
        PasswordPromptCancel,
        /// Submit the entered password (bound to `enter`).
        PasswordPromptSubmit,
    ]
);

/// Events the modal emits to its owner (`AppState`).
pub enum PasswordPromptEvent {
    /// Dismiss without a password — the triggering connect/query attempt stays failed.
    Cancel,
    /// The password as typed, handed back exactly once. Never logged; this modal keeps
    /// no copy of it past this point (its field is dropped along with the modal on
    /// close).
    Submit(String),
}

/// The connect-time password prompt.
pub struct PasswordPromptModal {
    /// What the prompt is for — e.g. `user@host` or a DB connection's name — shown as
    /// "password for {label}".
    label: SharedString,
    password: Entity<TextInput>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl PasswordPromptModal {
    pub fn new(cx: &mut Context<Self>, label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            password: cx.new(|cx| TextInput::new_masked(cx, "password")),
            error: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Move keyboard focus into the password field. Called once, right after the modal
    /// entity is created (see `AppState::open_password_prompt`).
    pub fn focus_first(&self, window: &mut Window, cx: &App) {
        self.password.read(cx).focus(window);
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let password = self.password.read(cx).content().to_string();
        match validate_password(&password) {
            Ok(password) => cx.emit(PasswordPromptEvent::Submit(password)),
            Err(msg) => {
                self.error = Some(msg.into());
                cx.notify();
            }
        }
    }
}

impl EventEmitter<PasswordPromptEvent> for PasswordPromptModal {}

impl Focusable for PasswordPromptModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PasswordPromptModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = format!("password for {}", self.label).into();

        // The key context, the focus handle and the two actions stay on a wrapper around
        // the panel: `Modal` is a plain element with no lifecycle, and Escape/Enter are
        // this entity's own bindings (see `sid_ui::modal`'s "what the panel does not
        // own").
        div()
            .key_context("PasswordPrompt")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_this, _: &PasswordPromptCancel, _window, cx| {
                cx.emit(PasswordPromptEvent::Cancel);
            }))
            .on_action(cx.listener(|this, _: &PasswordPromptSubmit, _window, cx| this.submit(cx)))
            .child(
                Modal::new("password-prompt", title)
                    // The forms' width, not the old 380: a narrower panel drops the
                    // footer's `Enter connects` hint (`sid_ui::modal::shows_key_hint`),
                    // and every modal being the same size is worth more than 80px.
                    .submit_hint("connects")
                    .on_dismiss(cx.listener(|_this, _ev: &ClickEvent, _window, cx| {
                        cx.emit(PasswordPromptEvent::Cancel);
                    }))
                    // Why the prompt exists at all, stated before the field rather than
                    // as a caption under it: what the user types here does not persist.
                    .child(Toast::info(
                        "no OS keyring — this password is used once and held only \
                         for this session",
                    ))
                    .child(self.password.clone())
                    .when_some(self.error.clone(), |modal, err| {
                        modal.child(Toast::danger(err))
                    })
                    .footer(
                        Button::new("password-prompt-cancel", "Cancel")
                            .ghost()
                            .on_click(cx.listener(|_this, _ev: &ClickEvent, _window, cx| {
                                cx.emit(PasswordPromptEvent::Cancel);
                            })),
                    )
                    .footer(
                        Button::new("password-prompt-submit", "Connect")
                            .primary()
                            .on_click(
                                cx.listener(|this, _ev: &ClickEvent, _window, cx| this.submit(cx)),
                            ),
                    ),
            )
    }
}

// ---------------------------------------------------------------------------
// Pure decision logic (unit-tested without gpui)
// ---------------------------------------------------------------------------

/// Validate the raw field value before it's ever emitted: non-empty. Kept as a free
/// function so it's unit-tested without gpui, same convention as
/// `host_form::validate`/`secret_unlock`'s (now-deleted) `validate_unlock`.
pub(crate) fn validate_password(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        Err("enter the password".into())
    } else {
        Ok(raw.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_password_rejects_empty() {
        assert!(validate_password("").is_err());
    }

    #[test]
    fn validate_password_accepts_nonempty() {
        assert_eq!(validate_password("hunter2").unwrap(), "hunter2");
    }
}
