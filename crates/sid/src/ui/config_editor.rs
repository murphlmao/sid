//! Config-file editor modal (Round E §D): the Systems tab's "click a pinned/curated
//! config file -> edit it in place" flow.
//!
//! Reuses the SQL editor's gpui-component `Input`/`InputState` machinery
//! (`ui::db_tab`'s `ensure_query_widgets`) for the multi-line buffer. On open:
//! - the file is read off the render thread (`session::ssh_runtime()`) and gated
//!   through [`gate_loaded_bytes`] — at most 1 MiB, must be valid UTF-8, mirroring
//!   `session.rs`'s SFTP preview gate; a rejection shows a read-only notice instead of
//!   an editor (same shape as that preview's `PreviewContent::Notice`);
//! - writability is probed once, up front, via `OpenOptions::new().write(true).open`
//!   (never truncating) — a failed probe renders the editor read-only with an amber
//!   "needs root" banner and disables Save. No sudo/pkexec escalation in v1.
//!
//! Save writes the buffer to a sibling temp file, copies the *original* file's
//! permissions onto it (`fs::metadata` → `set_permissions`), then renames it over the
//! original — so a `sudoedit`-managed `/etc` file comes back with the same mode bits it
//! had before, not whatever `fs::write` would have defaulted to.
//!
//! The modal is built inside the Systems tab's own returned tree (`systems_tab.rs`'s
//! `AppState::systems_tab` appends `config_editor_overlay`'s output), the same
//! zero-`app.rs`-footprint shape `ui::db_tab`'s `cell_view_overlay` and `ui::session`'s
//! `preview_overlay` use — so this track never touches `app.rs`. Esc (`ConfigEditorCancel`,
//! bound in `ui::mod::init`) and the close button both refocus `AppState::root_focus` —
//! the dangling-focus bug `ui::db_tab`'s `close_db_form` fixed the same way. Closing a
//! dirty buffer asks nothing in v1 — edits are simply discarded.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClickEvent, Context, Entity, IntoElement, SharedString, Subscription, Window,
    actions, anchored, deferred, div, point, prelude::*, px, rgb, rgba,
};
use gpui_component::input::{Input, InputEvent, InputState};

use crate::app::AppState;
use crate::ui::session::ssh_runtime;
use crate::ui::theme;

/// Load cap for a config file opened in the editor: 1 MiB — the same value as
/// `session.rs`'s `PREVIEW_MAX_BYTES` (private to that module, so redeclared here
/// rather than importing it).
const CONFIG_MAX_BYTES: u64 = 1024 * 1024;

actions!(config_editor, [ConfigEditorCancel]);

/// What the modal body shows, depending on how the open-time load went.
enum ConfigEditorBody {
    /// The async read/gate/writability-probe is still in flight.
    Loading,
    /// The load gate rejected the file (too large, not UTF-8, or a read error) — shown
    /// as a read-only notice, no editor, no save. Mirrors `session.rs`'s
    /// `PreviewContent::Notice`.
    Notice(String),
    /// Loaded successfully: the multi-line editor, whether the writability probe
    /// (taken once, at open time) succeeded, and whether the buffer has unsaved edits.
    Editor {
        input: Entity<InputState>,
        _input_sub: Subscription,
        writable: bool,
        dirty: bool,
    },
}

/// One open config-file editor. Lives on `SystemsTabState::editor` (`ui::systems_tab`)
/// — `None` when nothing is open.
pub(crate) struct ConfigEditorState {
    path: PathBuf,
    body: ConfigEditorBody,
    /// Set only by a failed save; cleared on the next edit or successful save.
    save_error: Option<String>,
    /// True while a save is in flight — guards a second click and disables Save.
    saving: bool,
}

impl AppState {
    /// Open `path` in the config-file editor modal — see the module doc comment for
    /// the load/gate/writability-probe flow. Superseding calls (opening a second file
    /// before the first's load lands) are handled by the path check in the completion
    /// closure below.
    pub(crate) fn open_config_editor(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.systems.editor = Some(ConfigEditorState {
            path: path.clone(),
            body: ConfigEditorBody::Loading,
            save_error: None,
            saving: false,
        });
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let probe_path = path.clone();
            let handle = ssh_runtime()
                .spawn(async move { (probe_writable(&probe_path), read_and_gate(&probe_path)) });
            let outcome = handle.await;
            let _ = this.update_in(cx, |state, window, cx| {
                let Some(editor) = state.systems.editor.as_mut() else {
                    return;
                };
                if editor.path != path {
                    return; // superseded by a newer `open_config_editor` call
                }
                match outcome {
                    Ok((writable, Ok(text))) => {
                        let input = cx.new(|cx| {
                            InputState::new(window, cx)
                                .code_editor("")
                                .line_number(true)
                                .default_value(text)
                        });
                        let sub =
                            cx.subscribe_in(&input, window, AppState::on_config_editor_input_event);
                        input.update(cx, |state, cx| state.focus(window, cx));
                        editor.body = ConfigEditorBody::Editor {
                            input,
                            _input_sub: sub,
                            writable,
                            dirty: false,
                        };
                    }
                    Ok((_, Err(notice))) => editor.body = ConfigEditorBody::Notice(notice),
                    Err(join_err) => {
                        editor.body =
                            ConfigEditorBody::Notice(format!("load task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Esc (`ConfigEditorCancel`) / the close button: discard and close. v1 never asks
    /// about unsaved changes even if dirty — see the module doc comment.
    pub(crate) fn close_config_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.systems.editor = None;
        // The dangling-focus bug: refocus `root_focus` so keyboard dispatch doesn't die
        // the instant this modal's tree stops rendering — same fix `db_tab.rs`'s
        // `close_db_form` applies.
        window.focus(&self.root_focus);
        cx.notify();
    }

    /// Marks the open buffer dirty on the first edit. Wired via `cx.subscribe_in` in
    /// `open_config_editor` (mirrors `ui::db_tab`'s `on_sql_event`).
    fn on_config_editor_input_event(
        &mut self,
        _input: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        if let Some(editor) = self.systems.editor.as_mut()
            && let ConfigEditorBody::Editor { dirty, .. } = &mut editor.body
        {
            *dirty = true;
        }
        cx.notify();
    }

    /// Save: write the buffer to a sibling temp file, copy the original's permissions
    /// onto it, then rename it over the original (see [`save_preserving_permissions`]).
    /// A no-op if there's nothing to save (not in `Editor` state, read-only, clean, or a
    /// save is already in flight) — mirrors the button's own disabled condition.
    fn save_config_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.systems.editor.as_mut() else {
            return;
        };
        let ConfigEditorBody::Editor {
            input,
            writable,
            dirty,
            ..
        } = &editor.body
        else {
            return;
        };
        if !*writable || !*dirty || editor.saving {
            return;
        }
        let path = editor.path.clone();
        let contents = input.read(cx).value().to_string();
        editor.saving = true;
        editor.save_error = None;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let save_path = path.clone();
            let handle = ssh_runtime()
                .spawn(async move { save_preserving_permissions(&save_path, &contents) });
            let outcome = handle.await;
            let _ = this.update_in(cx, |state, _window, cx| {
                let Some(editor) = state.systems.editor.as_mut() else {
                    return;
                };
                if editor.path != path {
                    return;
                }
                editor.saving = false;
                match outcome {
                    Ok(Ok(())) => {
                        if let ConfigEditorBody::Editor { dirty, .. } = &mut editor.body {
                            *dirty = false;
                        }
                    }
                    Ok(Err(e)) => editor.save_error = Some(e.to_string()),
                    Err(join_err) => {
                        editor.save_error = Some(format!("save task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The editor modal overlay — `None` when nothing is open. Mirrors `db_tab.rs`'s
    /// `cell_view_overlay` / `session.rs`'s `preview_overlay`: an `anchored`/`deferred`
    /// viewport-sized occluding backdrop, sized "most of the viewport" per the spec.
    pub(crate) fn config_editor_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let theme = theme::active(cx).clone();
        let editor = self.systems.editor.as_ref()?;
        let viewport = window.viewport_size();
        let path = editor.path.clone();
        let file_name: SharedString = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
            .into();
        let full_path: SharedString = path.display().to_string().into();
        let saving = editor.saving;
        let save_error = editor.save_error.clone();

        let (dirty, writable, is_editor, body): (bool, bool, bool, AnyElement) = match &editor.body
        {
            ConfigEditorBody::Loading => (
                false,
                true,
                false,
                div()
                    .flex_1()
                    .p_4()
                    .text_sm()
                    .text_color(rgb(theme.muted))
                    .child("loading…")
                    .into_any_element(),
            ),
            ConfigEditorBody::Notice(msg) => (
                false,
                false,
                false,
                div()
                    .flex_1()
                    .p_4()
                    .text_sm()
                    .text_color(rgb(theme.muted))
                    .child(msg.clone())
                    .into_any_element(),
            ),
            ConfigEditorBody::Editor {
                input,
                writable,
                dirty,
                ..
            } => (
                *dirty,
                *writable,
                true,
                div()
                    .flex_1()
                    .m_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.well))
                    .overflow_hidden()
                    .child(Input::new(input).disabled(!*writable))
                    .into_any_element(),
            ),
        };

        let title: SharedString = if dirty {
            format!("{file_name} •").into()
        } else {
            file_name
        };

        let banner = (is_editor && !writable).then(|| {
            div()
                .mx_3()
                .mt_3()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(rgba(0xe8b04a26))
                .text_sm()
                .text_color(rgb(theme.warning))
                .child("read-only — needs root; edit with sudoedit")
        });

        let save_error_line = save_error.map(|e| {
            div()
                .mx_3()
                .mt_2()
                .text_xs()
                .text_color(rgb(theme.danger))
                .child(format!("save failed: {e}"))
        });

        let can_save = is_editor && writable && dirty && !saving;
        let save_label = if saving { "saving…" } else { "save" };

        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("config-editor-backdrop")
                        .key_context("ConfigEditor")
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(rgba(0x000000a8))
                        .on_action(cx.listener(|this, _: &ConfigEditorCancel, window, cx| {
                            this.close_config_editor(window, cx);
                        }))
                        .child(
                            div()
                                .w(viewport.width * 0.88)
                                .h(viewport.height * 0.86)
                                .flex()
                                .flex_col()
                                .bg(rgb(theme.surface))
                                .border_1()
                                .border_color(rgb(theme.border))
                                .rounded_md()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .px_3()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(rgb(theme.border))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(theme.fg_strong))
                                                        .child(title),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(theme.muted))
                                                        .child(full_path),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap_2()
                                                .when(can_save, |el| {
                                                    el.child(
                                                        div()
                                                            .id("config-editor-save")
                                                            .px_3()
                                                            .py_1()
                                                            .rounded_md()
                                                            .text_sm()
                                                            .cursor_pointer()
                                                            .text_color(rgb(theme.accent))
                                                            .hover(|s| s.bg(rgb(theme.selection)))
                                                            .child(save_label)
                                                            .on_click(cx.listener(
                                                                |this,
                                                                 _: &ClickEvent,
                                                                 window,
                                                                 cx| {
                                                                    this.save_config_editor(
                                                                        window, cx,
                                                                    );
                                                                },
                                                            )),
                                                    )
                                                })
                                                .child(
                                                    div()
                                                        .id("config-editor-close")
                                                        .px_2()
                                                        .py_1()
                                                        .rounded_md()
                                                        .cursor_pointer()
                                                        .text_sm()
                                                        .text_color(rgb(theme.muted))
                                                        .hover(|s| s.bg(rgb(theme.selection)))
                                                        .child("close")
                                                        .on_click(cx.listener(
                                                            |this, _: &ClickEvent, window, cx| {
                                                                this.close_config_editor(
                                                                    window, cx,
                                                                );
                                                            },
                                                        )),
                                                ),
                                        ),
                                )
                                .children(banner)
                                .child(body)
                                .children(save_error_line),
                        ),
                ),
            )
            .with_priority(1),
        )
    }
}

// ---- pure helpers (unit-tested) ---------------------------------------------------

/// Probe whether `path` is writable by the current user, without truncating or
/// otherwise modifying it: `OpenOptions::write(true).open` only succeeds if the file
/// can be opened for writing, and the handle is dropped immediately.
fn probe_writable(path: &Path) -> bool {
    fs::OpenOptions::new().write(true).open(path).is_ok()
}

/// Read `path` and gate it through [`gate_loaded_bytes`] — the I/O glue
/// `open_config_editor` runs on `ssh_runtime()`, off the render thread.
fn read_and_gate(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    gate_loaded_bytes(bytes)
}

/// The size/UTF-8 load gate — mirrors `session.rs`'s SFTP preview gate
/// (`PREVIEW_MAX_BYTES` + the `String::from_utf8` check in `SshSession::view`). Pure
/// over already-read bytes, so it's unit-tested without touching the filesystem.
fn gate_loaded_bytes(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() as u64 > CONFIG_MAX_BYTES {
        return Err("too large to edit (> 1 MiB) — view it with a pager instead".into());
    }
    String::from_utf8(bytes).map_err(|_| "binary file — cannot edit as text".into())
}

/// A private sibling path to write new content to before the atomic rename:
/// `.<name>.sid-tmp-<pid>`, in the same directory as `path` — same filesystem, so the
/// final rename is a metadata-only operation, never a cross-device copy.
fn sibling_temp_path(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    dir.join(format!(".{name}.sid-tmp-{}", std::process::id()))
}

/// Save `contents` over `path`: write to a sibling temp file, copy `path`'s *current*
/// permissions onto it, then rename it into place. The permission copy — not a blind
/// overwrite — is the whole point: a `sudoedit`-managed `/etc` file must come back with
/// the same mode bits it had before, whatever they were (including a read-only mode;
/// this function itself does no writability gating — that's `probe_writable`'s job,
/// enforced by the UI before a save is ever attempted). Cleans up the temp file if any
/// step after it's written fails.
fn save_preserving_permissions(path: &Path, contents: &str) -> io::Result<()> {
    let perms = fs::metadata(path)?.permissions();
    let tmp = sibling_temp_path(path);
    let result = (|| {
        fs::write(&tmp, contents.as_bytes())?;
        fs::set_permissions(&tmp, perms)?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_loaded_bytes_accepts_utf8_under_cap() {
        let got = gate_loaded_bytes(b"host example.com\nuser root\n".to_vec()).unwrap();
        assert_eq!(got, "host example.com\nuser root\n");
    }

    #[test]
    fn gate_loaded_bytes_rejects_over_cap() {
        let bytes = vec![b'a'; CONFIG_MAX_BYTES as usize + 1];
        let err = gate_loaded_bytes(bytes).unwrap_err();
        assert!(err.contains("too large"), "unexpected message: {err}");
    }

    #[test]
    fn gate_loaded_bytes_accepts_exactly_the_cap() {
        let bytes = vec![b'a'; CONFIG_MAX_BYTES as usize];
        assert!(gate_loaded_bytes(bytes).is_ok());
    }

    #[test]
    fn gate_loaded_bytes_rejects_invalid_utf8() {
        let bytes = vec![0xff, 0xfe, 0xfd];
        let err = gate_loaded_bytes(bytes).unwrap_err();
        assert!(err.contains("binary"), "unexpected message: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn save_preserving_permissions_keeps_mode_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sshd_config");
        fs::write(&path, "PermitRootLogin no\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        save_preserving_permissions(&path, "PermitRootLogin yes\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "PermitRootLogin yes\n");
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode bits must survive the save");
    }

    /// The readonly case: even a file whose *own* mode has no write bit set saves fine
    /// (the containing directory is writable, so the temp-file + rename succeeds) —
    /// and comes back with that same readonly mode, not some default.
    #[cfg(unix)]
    #[test]
    fn save_preserving_permissions_keeps_readonly_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly.conf");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();

        save_preserving_permissions(&path, "new\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o444, "readonly mode must survive the save");
    }

    #[cfg(unix)]
    #[test]
    fn probe_writable_reflects_the_mode_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, "x").unwrap();
        assert!(probe_writable(&path), "owner-writable file");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(!probe_writable(&path), "readonly file");
    }

    #[test]
    fn sibling_temp_path_is_hidden_and_same_directory() {
        let tmp = sibling_temp_path(Path::new("/etc/hosts"));
        assert_eq!(tmp.parent(), Some(Path::new("/etc")));
        let name = tmp.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(".hosts.sid-tmp-"));
    }
}
