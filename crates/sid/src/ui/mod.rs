//! sid's screens: one module per tab, plus the modals they share.
//!
//! Shared *widgets* — including the single-line text field, `sid_ui::TextInput`/
//! `SearchInput` — and the semantic tokens live in the `sid-ui` crate, not here — see
//! that crate's docs and `docs/design/2026-07-26-ui-overhaul-plan.md`. The theme module
//! these screens read (`sid_ui::theme`) moved there with it.
//!
//! Every field's own std-editing chords (ctrl-backspace, ctrl-shift-arrows, Tab) come
//! from `gpui_component::input::InputState`, which `sid_ui::TextInput` wraps — nothing
//! here binds them. What *is* declared here and bound once via [`init`] are the
//! form-level `escape`/`enter` bindings, each scoped to its own key context (`HostForm`,
//! `DbConnForm`, `PasswordPrompt`, …) on an ancestor of the focused field.

pub mod command_palette;
pub mod config_editor;
pub mod db_conn_form;
pub mod db_diagram;
pub mod db_tab;
pub mod host_form;
pub mod network_tab;
pub mod password_prompt;
pub mod session;
pub mod settings_tab;
pub mod ssh_home;
pub mod systems_tab;
pub mod workspaces_tab;

pub use session::{SessionStatus, SshSession, SshSessionEvent};

use gpui::{App, KeyBinding, Keystroke};

/// Does this keystroke mean "submit the single-line field I am in"?
///
/// The two idioms sid uses for Enter-to-submit are a form-wide key context (see the
/// `HostForm`/`DbConnForm`/`PasswordPrompt` bindings in [`init`]) and, for a lone field
/// with a button beside it, an `on_key_down` on the wrapper. This is the decision the
/// second kind shares, in one place — it was open-coded per site, and the two sites that
/// forgot to code it at all were the SFTP go-to-path field and the System tab's
/// "pin a file…" input, whose own doc comment claimed Enter worked.
///
/// **Plain Enter only.** A modified Enter belongs to whatever else is listening — a
/// terminal wanting a literal newline, a future "save and add another" — and a field that
/// swallowed every chord would make those unreachable without anyone noticing.
pub(crate) fn is_field_submit(keystroke: &Keystroke) -> bool {
    let m = &keystroke.modifiers;
    keystroke.key == "enter" && !m.control && !m.alt && !m.shift && !m.platform && !m.function
}

/// Register the form-level keybindings. Call once from `main`, before opening the
/// window.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        // Host-form bindings, scoped to its own key context. They sit on an ancestor of
        // the focused field, so they fire from any field inside the form.
        KeyBinding::new("escape", host_form::FormCancel, Some("HostForm")),
        KeyBinding::new("enter", host_form::FormSubmit, Some("HostForm")),
        // DB connection form bindings (W4), scoped the same way as the host form's.
        KeyBinding::new("escape", db_conn_form::DbFormCancel, Some("DbConnForm")),
        KeyBinding::new("enter", db_conn_form::DbFormSubmit, Some("DbConnForm")),
        // Connect-time password prompt (round-D §A.4), scoped the same way.
        KeyBinding::new(
            "escape",
            password_prompt::PasswordPromptCancel,
            Some("PasswordPrompt"),
        ),
        KeyBinding::new(
            "enter",
            password_prompt::PasswordPromptSubmit,
            Some("PasswordPrompt"),
        ),
        // SSH home-tree inline rename / folder-edit bindings (ssh-v3), scoped to the
        // row wrapper's own key context so Enter/Esc commit/cancel the in-place edit no
        // matter which nested `TextInput` has focus — same ancestor-context trick the
        // host form uses.
        KeyBinding::new(
            "escape",
            ssh_home::InlineEditCancel,
            Some(ssh_home::INLINE_EDIT_CONTEXT),
        ),
        KeyBinding::new(
            "enter",
            ssh_home::InlineEditCommit,
            Some(ssh_home::INLINE_EDIT_CONTEXT),
        ),
        // Quick-connect box: Enter fires the connect, same as clicking Go.
        KeyBinding::new(
            "enter",
            ssh_home::QuickConnectGo,
            Some(ssh_home::QUICK_CONNECT_CONTEXT),
        ),
        // Config-file editor modal (round-e §D), scoped the same way — the multi-line
        // gpui-component `Input` inside it propagates an unhandled Escape (see that
        // crate's `InputState::escape`) up to this ancestor context.
        KeyBinding::new(
            "escape",
            config_editor::ConfigEditorCancel,
            Some("ConfigEditor"),
        ),
    ]);
}

#[cfg(test)]
mod submit_key_tests {
    use super::*;
    use gpui::Modifiers;

    fn stroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_string(),
            key_char: None,
        }
    }

    #[test]
    fn a_plain_enter_submits() {
        assert!(is_field_submit(&stroke("enter", Modifiers::none())));
    }

    #[test]
    fn any_other_key_does_not() {
        for key in ["escape", "tab", "a", "space", "return"] {
            assert!(
                !is_field_submit(&stroke(key, Modifiers::none())),
                "{key} should not submit"
            );
        }
    }

    #[test]
    fn a_modified_enter_belongs_to_someone_else() {
        // Each modifier on its own: a field that ate every Enter chord would quietly
        // make the chord unavailable to anything else that wanted it.
        let modified = [
            ("control", Modifiers::control()),
            ("alt", Modifiers::alt()),
            ("shift", Modifiers::shift()),
            ("platform", Modifiers::command()),
        ];
        for (name, modifiers) in modified {
            assert!(
                !is_field_submit(&stroke("enter", modifiers)),
                "{name}-enter should not submit"
            );
        }
    }
}
