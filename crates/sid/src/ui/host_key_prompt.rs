//! Unknown-host-key confirmation (security review M1).
//!
//! `sid-ssh` used to learn an unknown host key into sid's known_hosts the moment it saw
//! it, so a first-contact man-in-the-middle was undetectable — and it removed the only
//! speed bump in front of a redirected saved password (H2). It now refuses the
//! handshake with [`SshError::HostKeyUnknown`](sid_core::ssh::SshError::HostKeyUnknown),
//! and this modal is where the user decides:
//!
//! 1. the session's connect fails with that verdict; the session keeps what it needs to
//!    re-dial and fires `SshSessionEvent::UnknownHostKey` (see `ui::session`);
//! 2. `AppState::on_session_event` hands it to [`AppState::open_host_key_prompt`];
//! 3. **Trust and connect** re-dials with that fingerprint approved — the adapter learns
//!    the key only if the server presents exactly that key again, so a key swapped
//!    between the two dials is reported afresh rather than trusted; **Cancel** (or Esc,
//!    or the close button) fails the session with "host key not trusted".
//!
//! Enter is deliberately *not* bound: trusting a key must be a choice, never the reflex
//! of confirming whatever dialog is in front of you. A changed key (`HostKeyMismatch`)
//! never reaches this modal — it stays fatal.
//!
//! Lives as a second `impl AppState` block (the `ui::db_tab`/`ui::config_editor` shape)
//! so `app.rs` carries one field, one event arm, one overlay child and one
//! `blocking_modal_open` term. Prompts arriving while one is open (two connects to new
//! hosts at once) wait their turn in [`PromptQueue`].

use std::collections::VecDeque;

use gpui::{
    ClickEvent, Context, FocusHandle, IntoElement, SharedString, WeakEntity, Window, actions, div,
    prelude::*,
};
use sid_core::ssh::UnknownHostKey;
use sid_ui::{Button, Modal, Toast, Typography as _, modal, theme, v_flex};

use crate::app::AppState;
use crate::ui::session::SshSession;

actions!(
    host_key_prompt,
    [
        /// Refuse the key (bound to `escape`). There is intentionally no submit action.
        HostKeyPromptCancel,
    ]
);

/// The `key_context` the Escape binding is scoped to (see `ui::init`).
pub(crate) const HOST_KEY_PROMPT_CONTEXT: &str = "HostKeyPrompt";

/// One unanswered unknown-key verdict.
pub(crate) struct PendingHostKey {
    /// The session waiting on the answer. Weak: closing the tab must not be held up by,
    /// or resurrected through, a prompt.
    session: WeakEntity<SshSession>,
    key: UnknownHostKey,
    /// Keyboard focus while the prompt is up, so Esc reaches the binding.
    focus: FocusHandle,
}

/// One prompt on screen, the rest in arrival order.
#[derive(Debug)]
pub(crate) struct PromptQueue<T> {
    open: Option<T>,
    waiting: VecDeque<T>,
}

impl<T> Default for PromptQueue<T> {
    fn default() -> Self {
        Self {
            open: None,
            waiting: VecDeque::new(),
        }
    }
}

impl<T> PromptQueue<T> {
    /// Add a prompt. Returns `true` if it went straight on screen.
    pub(crate) fn push(&mut self, item: T) -> bool {
        if self.open.is_none() {
            self.open = Some(item);
            true
        } else {
            self.waiting.push_back(item);
            false
        }
    }

    /// Take the on-screen prompt (it was answered) and bring the next one up.
    pub(crate) fn answer(&mut self) -> Option<T> {
        let answered = self.open.take();
        self.open = self.waiting.pop_front();
        answered
    }

    pub(crate) fn current(&self) -> Option<&T> {
        self.open.as_ref()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }
}

/// `AppState`'s one field for this modal.
pub(crate) type HostKeyPrompts = PromptQueue<PendingHostKey>;

/// The modal's explanatory line: what is being asked, and how to check it.
pub(crate) fn prompt_message(key: &UnknownHostKey) -> String {
    format!(
        "sid has never connected to {} before, so it cannot tell this server from an \
         impostor. Compare the fingerprint below with one obtained another way (the \
         server's admin, or `ssh-keygen -lf` on its host key) before trusting it.",
        key.host
    )
}

impl AppState {
    /// Queue (or show) the confirmation for `key`, which `session`'s connect stopped
    /// on. Called from `on_session_event`.
    pub(crate) fn open_host_key_prompt(
        &mut self,
        session: WeakEntity<SshSession>,
        key: UnknownHostKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = PendingHostKey {
            session,
            key,
            focus: cx.focus_handle(),
        };
        let focus = pending.focus.clone();
        if self.host_key_prompt.push(pending) {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// Answer the on-screen prompt: re-dial trusting its fingerprint, or fail the
    /// session. Then show the next waiting prompt, or hand focus back to `root_focus`.
    fn answer_host_key(&mut self, trust: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(answered) = self.host_key_prompt.answer() else {
            return;
        };
        if let Some(session) = answered.session.upgrade() {
            session.update(cx, |session, cx| {
                if trust {
                    session.trust_host_key(cx);
                } else {
                    session.reject_host_key(cx);
                }
            });
        }
        match self.host_key_prompt.current() {
            Some(next) => window.focus(&next.focus, cx),
            None => window.focus(&self.root_focus, cx),
        }
        cx.notify();
    }

    /// The scrim + panel, or `None` when no prompt is open.
    pub(crate) fn host_key_prompt_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let pending = self.host_key_prompt.current()?;
        let t = theme::active(cx).clone();
        let key = &pending.key;
        let title: SharedString = format!("Unknown host key — {}", key.host).into();

        let fingerprint = v_flex()
            .gap_1()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(gpui::rgb(t.border))
            .bg(gpui::rgb(t.well))
            .child(div().text_meta(&t).child(key.key_type.clone()))
            .child(
                div()
                    .text_mono(&t)
                    // A fingerprint is 50 characters of base64 that must be read whole;
                    // wrap rather than clip it at narrow widths.
                    .whitespace_normal()
                    .child(key.fingerprint.clone()),
            );

        let panel = Modal::new("host-key-prompt", title)
            .on_dismiss(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.answer_host_key(false, window, cx);
            }))
            .child(Toast::warning(prompt_message(key)))
            .child(fingerprint)
            .footer(
                Button::new("host-key-prompt-cancel", "Cancel")
                    .ghost()
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.answer_host_key(false, window, cx);
                    })),
            )
            .footer(
                Button::new("host-key-prompt-trust", "Trust and connect")
                    .primary()
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.answer_host_key(true, window, cx);
                    })),
            );

        Some(modal::overlay(
            window,
            div()
                .key_context(HOST_KEY_PROMPT_CONTEXT)
                .track_focus(&pending.focus)
                .on_action(cx.listener(|this, _: &HostKeyPromptCancel, window, cx| {
                    this.answer_host_key(false, window, cx);
                }))
                .child(panel),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_prompt_shows_and_later_ones_wait_in_order() {
        let mut q = PromptQueue::default();
        assert!(q.push("a"));
        assert!(!q.push("b"));
        assert!(!q.push("c"));
        assert_eq!(q.current(), Some(&"a"));
        assert_eq!(q.answer(), Some("a"));
        assert_eq!(q.current(), Some(&"b"));
        assert_eq!(q.answer(), Some("b"));
        assert_eq!(q.answer(), Some("c"));
        assert!(!q.is_open());
        assert_eq!(q.answer(), None);
    }

    #[test]
    fn the_message_names_the_host_and_how_to_check_the_fingerprint() {
        let key = UnknownHostKey {
            host: "[db.example]:2222".into(),
            key_type: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        };
        let msg = prompt_message(&key);
        assert!(msg.contains("[db.example]:2222"));
        assert!(msg.contains("ssh-keygen -lf"));
    }
}
