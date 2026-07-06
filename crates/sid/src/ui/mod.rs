//! Reusable GPUI UI elements for sid.
//!
//! The single-line [`TextInput`] (P3.2 spearhead) and the [`host_form::HostForm`]
//! modal built on it. The input's actions are declared here and bound once via
//! [`init`], scoped to the `TextInput` key context so they never collide with other
//! bindings; the form's `escape`/`enter` bindings are scoped to `HostForm` the same way.

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
pub mod theme;

pub use session::{SessionStatus, SshSession, SshSessionEvent};
pub use text_input::TextInput;

use gpui::{App, KeyBinding, actions};

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
