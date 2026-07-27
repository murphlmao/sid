//! sid's screens: one module per tab, plus the modals and the single-line
//! [`TextInput`] they share.
//!
//! Shared *widgets* and the semantic tokens live in the `sid-ui` crate, not here — see
//! that crate's docs and `docs/design/2026-07-26-ui-overhaul-plan.md`. The theme module
//! these screens read (`sid_ui::theme`) moved there with it.
//!
//! The input's actions are declared here and bound once via [`init`], scoped to the
//! `TextInput` key context so they never collide with other bindings; the form's
//! `escape`/`enter` bindings are scoped to `HostForm` the same way.

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
mod text_input;
pub mod workspaces_tab;

pub use session::{SessionStatus, SshSession, SshSessionEvent};
pub use text_input::TextInput;

use gpui::{App, KeyBinding, Keystroke, actions};

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

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectAll,
        SelectToHome,
        SelectToEnd,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

/// The key context the input's bindings are scoped to. Must match the `key_context`
/// set in [`TextInput`]'s `render`.
const CONTEXT: &str = "TextInput";

/// Register the [`TextInput`] keybindings. Call once from `main`, before opening the
/// window. Every binding is scoped to the `TextInput` context so app-level shortcuts
/// added later (in other contexts) do not clash.
///
/// Cross-platform note: we bind both `cmd-` and `ctrl-` for clipboard/select-all so the
/// element works on Linux/Wayland now (ctrl) without needing a rebind on macOS later
/// (cmd). This is the one deliberate seam the CLAUDE.md "accommodate, don't solve" rule
/// allows for an input element that is otherwise platform-agnostic.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("ctrl-left", WordLeft, Some(CONTEXT)),
        KeyBinding::new("ctrl-right", WordRight, Some(CONTEXT)),
        KeyBinding::new("alt-left", WordLeft, Some(CONTEXT)),
        KeyBinding::new("alt-right", WordRight, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectToHome, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectToEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
        KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
        KeyBinding::new("ctrl-x", Cut, Some(CONTEXT)),
        KeyBinding::new("cmd-x", Cut, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some(CONTEXT)),
        // Host-form bindings, scoped to its own key context. They sit on an ancestor of
        // the focused TextInput, so they fire from any field inside the form.
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
