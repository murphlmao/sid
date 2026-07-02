//! Reusable GPUI UI elements for sid.
//!
//! The single-line [`TextInput`] (P3.2 spearhead) and the [`host_form::HostForm`]
//! modal built on it. The input's actions are declared here and bound once via
//! [`init`], scoped to the `TextInput` key context so they never collide with other
//! bindings; the form's `escape`/`enter` bindings are scoped to `HostForm` the same way.

pub mod db_conn_form;
pub mod db_tab;
pub mod host_form;
pub mod network_tab;
pub mod session;
mod text_input;

pub use session::{SessionStatus, SshSession};
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
    ]);
}
