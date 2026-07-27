//! Config-file editor modal (Round E §D): the Systems tab's "click a pinned/curated
//! config file -> edit it in place" flow.
//!
//! Reuses the SQL editor's gpui-component `Input`/`InputState` machinery
//! (`ui::db_tab`'s `ensure_query_widgets`) for the multi-line buffer. On open:
//! - the file is read off the render thread (`session::ssh_runtime()`) and gated
//!   through [`gate_loaded_bytes`] — at most 1 MiB, must be valid UTF-8, mirroring
//!   `session.rs`'s SFTP preview gate; a rejection shows a read-only notice instead of
//!   an editor (same shape as that preview's `PreviewContent::Notice`);
//! - access is probed once, up front, through [`PrivilegedFs::probe`] — the answer is an
//!   [`Access`], not a bool, because `/etc/fstab` (readable, not writable) and
//!   `/etc/sudoers` (not even readable) are different situations for the user.
//!
//! Save writes the buffer to a sibling temp file, copies the *original* file's
//! permissions onto it (`fs::metadata` → `set_permissions`), then renames it over the
//! original — so an `/etc` file comes back with the same mode bits it had before, not
//! whatever `fs::write` would have defaulted to.
//!
//! # Root-owned files
//!
//! A root-owned file used to end here, at a banner that said "read-only — needs root;
//! edit with sudoedit": a dead end that told the user to go and use a different program.
//! It is now an **unlock** affordance. Unlocking opens a password prompt, and an accepted
//! password gets an elevated read through [`PrivilegedFs`] and lets Save write back
//! through the same port. `/etc/sudoers` — which cannot be read at all unprivileged —
//! opens straight into the locked state, with the unlock as its only action.
//!
//! The secret's whole life is this modal. It is held in memory as a
//! [`Passphrase`](sid_core::privfs::Passphrase) for as long as the editor is open — so a
//! read and the saves that follow it cost one prompt, not one per operation — and it is
//! dropped (and zeroized) when the modal closes. It is never persisted: not to the store,
//! not to the keyring, not to the workspace config. That is the same shape as the
//! connect-time SSH prompt in `ui::password_prompt`, which likewise hands a plaintext
//! back exactly once and keeps no copy.
//!
//! Which failures re-prompt and which do not is a real decision, not a detail: only a
//! genuinely rejected password reopens the field. Anything else — not a sudoer, no `sudo`
//! installed, the file vanished — closes the prompt with a message, because a prompt the
//! user can never satisfy is worse than an error.
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
use std::sync::{Arc, OnceLock};

use gpui::{
    AnyElement, ClickEvent, Context, Entity, IntoElement, KeyDownEvent, SharedString, Subscription,
    Window, actions, anchored, deferred, div, point, prelude::*, px, rgb, rgba,
};
use gpui_component::input::{Input, InputEvent, InputState};

use sid_core::privfs::{Access, Passphrase, PrivError, PrivilegedFs};
use sid_privfs::SudoPrivilegedFs;

use crate::app::AppState;
use crate::ui::session::ssh_runtime;
use crate::ui::{TextInput, is_field_submit};
use sid_ui::theme;
use sid_ui::{Button, EmptyState, Icon, Typography as _, h_flex, v_flex};

/// Load cap for a config file opened in the editor: 1 MiB — the same value as
/// `session.rs`'s `PREVIEW_MAX_BYTES` (private to that module, so redeclared here
/// rather than importing it).
const CONFIG_MAX_BYTES: u64 = 1024 * 1024;

actions!(config_editor, [ConfigEditorCancel]);

/// The privileged-file adapter, resolved once per process.
///
/// A deliberately local composition root, the same shape as `session::ssh_runtime()`'s:
/// it keeps the config-editor track's zero-`app.rs` footprint (see the module doc) while
/// still naming `sid-privfs`'s concrete constructor in exactly one place. Everything
/// after construction goes through the `sid_core::privfs` port.
fn privileged_fs() -> &'static Arc<dyn PrivilegedFs> {
    static FS: OnceLock<Arc<dyn PrivilegedFs>> = OnceLock::new();
    FS.get_or_init(|| Arc::new(SudoPrivilegedFs::new()))
}

/// What the modal body shows, depending on how the open-time load went.
enum ConfigEditorBody {
    /// The async probe/read is still in flight.
    Loading,
    /// The load gate rejected the file (too large, not UTF-8, or a read error) — shown
    /// as a read-only notice, no editor, no save. Mirrors `session.rs`'s
    /// `PreviewContent::Notice`.
    Notice(String),
    /// Root-owned and unreadable by this user (`/etc/sudoers`): there is no content to
    /// show yet, so the body *is* the unlock affordance.
    Locked,
    /// Loaded: the multi-line editor and whether the buffer has unsaved edits. Whether
    /// it is editable is not stored here — it is derived by [`editor_mode`] from the
    /// access probe plus the held secret, so the two can never disagree.
    Editor {
        input: Entity<InputState>,
        _input_sub: Subscription,
        dirty: bool,
    },
}

/// The open unlock prompt: a masked field, the last error, and whether an attempt is in
/// flight. Holds no secret of its own beyond the field's own buffer — the accepted one
/// moves to [`ConfigEditorState::elevated`].
struct UnlockPrompt {
    password: Entity<TextInput>,
    error: Option<SharedString>,
    /// True while an elevation attempt is running: disables submit and shows progress.
    busy: bool,
}

/// One open config-file editor. Lives on `SystemsTabState::editor` (`ui::systems_tab`)
/// — `None` when nothing is open.
pub(crate) struct ConfigEditorState {
    path: PathBuf,
    body: ConfigEditorBody,
    /// How this process can reach the file, probed once at open time.
    access: Access,
    /// The accepted sudo password, held **only** while this modal is open — see the
    /// module doc. Dropping the `ConfigEditorState` zeroizes it; nothing else ever holds
    /// it, and it is never written anywhere.
    elevated: Option<Passphrase>,
    /// The open unlock prompt, if any.
    unlock: Option<UnlockPrompt>,
    /// Set only by a failed save; cleared on the next edit or successful save.
    save_error: Option<String>,
    /// True while a save is in flight — guards a second click and disables Save.
    saving: bool,
}

impl ConfigEditorState {
    /// What the editor currently offers — derived, never stored.
    fn mode(&self) -> EditorMode {
        editor_mode(self.access, self.elevated.is_some())
    }
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
            access: Access::ReadWrite,
            elevated: None,
            unlock: None,
            save_error: None,
            saving: false,
        });
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let probe_path = path.clone();
            let handle = ssh_runtime().spawn(async move {
                let access = privileged_fs().probe(&probe_path).await;
                // Only read what this process may actually read. A `Denied` file has
                // nothing to show until an unlock succeeds, and attempting the read
                // anyway would replace the offer of an unlock with a permission error.
                let content = match access {
                    Access::ReadWrite | Access::ReadOnly => Some(read_and_gate(&probe_path)),
                    Access::Denied | Access::Missing => None,
                };
                (access, content)
            });
            let outcome = handle.await;
            let _ = this.update_in(cx, |state, window, cx| {
                let Some(editor) = state.systems.editor.as_mut() else {
                    return;
                };
                if editor.path != path {
                    return; // superseded by a newer `open_config_editor` call
                }
                match outcome {
                    Ok((access, content)) => {
                        editor.access = access;
                        editor.body = match content {
                            Some(Ok(text)) => build_editor_body(text, window, cx),
                            Some(Err(notice)) => ConfigEditorBody::Notice(notice),
                            None if access.needs_elevation() => ConfigEditorBody::Locked,
                            None => ConfigEditorBody::Notice(format!(
                                "{} does not exist",
                                path.display()
                            )),
                        };
                    }
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

    /// Esc: dismiss one layer. With the unlock prompt open that means the prompt (the
    /// editor behind it stays exactly as it was); otherwise the editor itself. Closing
    /// the outer modal first would throw away a buffer the user only wanted to stop
    /// unlocking.
    fn dismiss_config_editor_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt_is_open = self
            .systems
            .editor
            .as_ref()
            .is_some_and(|e| e.unlock.is_some());
        if prompt_is_open {
            self.close_config_editor_unlock(window, cx);
        } else {
            self.close_config_editor(window, cx);
        }
    }

    /// Open the unlock prompt, optionally carrying an error from the attempt that led
    /// here (a rejected password, or a secret that stopped working before a save).
    fn open_config_editor_unlock(
        &mut self,
        error: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let password = cx.new(|cx| TextInput::new_masked(cx, "sudo password"));
        password.read(cx).focus(window);
        let Some(editor) = self.systems.editor.as_mut() else {
            return;
        };
        editor.unlock = Some(UnlockPrompt {
            password,
            error,
            busy: false,
        });
        cx.notify();
    }

    /// Close the unlock prompt. The typed password goes with it — the `TextInput` entity
    /// is dropped, and nothing copied it anywhere.
    fn close_config_editor_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.systems.editor.as_mut() else {
            return;
        };
        editor.unlock = None;
        // Hand focus back to whatever the editor shows, so the modal stays keyboard-live
        // — the same dangling-focus care `close_config_editor` takes.
        match &editor.body {
            ConfigEditorBody::Editor { input, .. } => {
                let input = input.clone();
                input.update(cx, |state, cx| state.focus(window, cx));
            }
            _ => window.focus(&self.root_focus),
        }
        cx.notify();
    }

    /// Submit the unlock prompt: read the file with elevated privileges using the typed
    /// password. On success the secret is held for the rest of the modal session (see the
    /// module doc); on a rejected password the prompt stays open; on anything else it
    /// closes with a message.
    fn submit_config_editor_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.systems.editor.as_mut() else {
            return;
        };
        let Some(prompt) = editor.unlock.as_mut() else {
            return;
        };
        if prompt.busy {
            return;
        }
        let typed = prompt.password.read(cx).content().to_string();
        if typed.is_empty() {
            prompt.error = Some("enter your password".into());
            cx.notify();
            return;
        }
        prompt.busy = true;
        prompt.error = None;
        let secret = Passphrase::new(typed);
        let path = editor.path.clone();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let read_path = path.clone();
            let attempt = secret.clone();
            let handle = ssh_runtime().spawn(async move {
                unlock_outcome(privileged_fs().read(&read_path, &attempt).await)
            });
            let outcome = handle.await;
            let _ = this.update_in(cx, |state, window, cx| {
                let Some(editor) = state.systems.editor.as_mut() else {
                    return;
                };
                if editor.path != path {
                    return; // superseded by another file
                }
                if let Some(prompt) = editor.unlock.as_mut() {
                    prompt.busy = false;
                }
                match outcome {
                    Ok(UnlockOutcome::Unlocked(text)) => {
                        editor.elevated = Some(secret);
                        editor.unlock = None;
                        editor.save_error = None;
                        editor.body = build_editor_body(text, window, cx);
                    }
                    Ok(UnlockOutcome::Retry(msg)) => {
                        if let Some(prompt) = editor.unlock.as_mut() {
                            prompt.error = Some(msg.into());
                            // Clear the field: retyping a whole password beats editing a
                            // masked one the user cannot see.
                            let field = prompt.password.clone();
                            field.update(cx, |input, cx| input.reset(cx));
                            field.read(cx).focus(window);
                        }
                    }
                    Ok(UnlockOutcome::Fatal(msg)) => {
                        editor.unlock = None;
                        editor.body = ConfigEditorBody::Notice(msg);
                    }
                    Err(join_err) => {
                        editor.unlock = None;
                        editor.body =
                            ConfigEditorBody::Notice(format!("unlock task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The close button / the last Esc: discard and close. v1 never asks about unsaved
    /// changes even if dirty — see the module doc comment. Dropping the state drops the
    /// held secret with it.
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
        let mode = editor.mode();
        let ConfigEditorBody::Editor { input, dirty, .. } = &editor.body else {
            return;
        };
        if !can_save(mode, *dirty, editor.saving) {
            return;
        }
        let path = editor.path.clone();
        let contents = input.read(cx).value().to_string();
        // An elevated save reuses the secret the unlock already validated — one prompt
        // per modal session, not one per save.
        let secret = matches!(mode, EditorMode::Elevated)
            .then(|| editor.elevated.clone())
            .flatten();
        editor.saving = true;
        editor.save_error = None;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let save_path = path.clone();
            let handle = ssh_runtime().spawn(async move {
                match secret {
                    Some(secret) => save_outcome(
                        privileged_fs()
                            .write(&save_path, contents.as_bytes(), &secret)
                            .await,
                    ),
                    None => match save_preserving_permissions(&save_path, &contents) {
                        Ok(()) => SaveOutcome::Saved,
                        Err(e) => SaveOutcome::Failed(e.to_string()),
                    },
                }
            });
            let outcome = handle.await;
            let _ = this.update_in(cx, |state, window, cx| {
                let Some(editor) = state.systems.editor.as_mut() else {
                    return;
                };
                if editor.path != path {
                    return;
                }
                editor.saving = false;
                match outcome {
                    Ok(SaveOutcome::Saved) => {
                        if let ConfigEditorBody::Editor { dirty, .. } = &mut editor.body {
                            *dirty = false;
                        }
                    }
                    Ok(SaveOutcome::Reauth(msg)) => {
                        // The secret stopped working between the read and the save. Drop
                        // it and ask again rather than reporting an error the user has no
                        // way to act on; the buffer stays dirty and intact.
                        editor.elevated = None;
                        cx.notify();
                        state.open_config_editor_unlock(Some(msg.into()), window, cx);
                        return;
                    }
                    Ok(SaveOutcome::Failed(e)) => editor.save_error = Some(e),
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
        let prompt_path = full_path.clone();
        let saving = editor.saving;
        let save_error = editor.save_error.clone();
        let mode = editor.mode();
        let can_edit = matches!(mode, EditorMode::Direct | EditorMode::Elevated);

        let (dirty, body): (bool, AnyElement) = match &editor.body {
            ConfigEditorBody::Loading => (
                false,
                div()
                    .flex_1()
                    .p_4()
                    .text_body(&theme)
                    .text_color(rgb(theme.muted))
                    .child("loading…")
                    .into_any_element(),
            ),
            ConfigEditorBody::Notice(msg) => (
                false,
                div()
                    .flex_1()
                    .p_4()
                    .text_body(&theme)
                    .text_color(rgb(theme.muted))
                    .child(msg.clone())
                    .into_any_element(),
            ),
            // Nothing to show until an unlock succeeds, so the body *is* the offer.
            ConfigEditorBody::Locked => (
                false,
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        EmptyState::new("this file needs root to read")
                            .icon(Icon::Warning)
                            .guidance(
                                "unlock with your sudo password to view and edit it — \
                                 the password is used for this file only and is never saved",
                            )
                            .action(unlock_button("config-editor-unlock-locked", cx)),
                    )
                    .into_any_element(),
            ),
            ConfigEditorBody::Editor { input, dirty, .. } => (
                *dirty,
                div()
                    .flex_1()
                    .m_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.well))
                    .overflow_hidden()
                    // `h_full` is load-bearing: `Input` otherwise sizes to its content's
                    // *first* line and the modal shows one line of an eleven-line
                    // `/etc/fstab` inside an otherwise empty pane. The SQL editor never
                    // hit this because it sits in a fixed-height 140px box.
                    .child(Input::new(input).disabled(!can_edit).h_full())
                    .into_any_element(),
            ),
        };

        // The unsaved marker. Was a literal `•` glued onto the filename with a
        // `format!` — a Unicode bullet from whatever the text font supplied, welded into
        // the title string so it could not be coloured, sized or spaced independently of
        // it. `Icon::Asterisk` is the bundled Lucide glyph, the conventional
        // unsaved-buffer mark, and it renders beside the title rather than inside it.
        let dirty_marker = dirty.then(|| Icon::Asterisk.small().text_color(rgb(theme.accent)));

        // This banner used to be the end of the road: "read-only — needs root; edit with
        // sudoedit", i.e. go away and use another program. It carries the unlock now, and
        // the elevated state gets a banner of its own — editing a file as root is
        // something the user should be able to see at a glance, not infer.
        let banner: Option<AnyElement> = match mode {
            EditorMode::NeedsRoot { has_content: true } => Some(
                h_flex()
                    .mx_3()
                    .mt_3()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    // A 15%-alpha wash of the theme's own `warning`, not a fixed amber:
                    // `0xRRGGBB` token shifted into `rgba`'s `0xRRGGBBAA` slot.
                    .bg(rgba((theme.warning << 8) | 0x26))
                    .text_body(&theme)
                    .text_color(rgb(theme.warning))
                    .child(Icon::Warning.small())
                    .child(
                        div()
                            .flex_1()
                            .child("read-only — this file belongs to root"),
                    )
                    .child(unlock_button("config-editor-unlock", cx))
                    .into_any_element(),
            ),
            EditorMode::Elevated => Some(
                h_flex()
                    .mx_3()
                    .mt_3()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .bg(rgba((theme.accent << 8) | 0x26))
                    .text_body(&theme)
                    .text_color(rgb(theme.accent))
                    .child(Icon::Check.small())
                    .child("unlocked — edits are saved as root")
                    .into_any_element(),
            ),
            EditorMode::Direct | EditorMode::NeedsRoot { .. } | EditorMode::Sealed => None,
        };

        let unlock_layer = editor
            .unlock
            .as_ref()
            .map(|prompt| unlock_prompt_layer(prompt, &theme, viewport, prompt_path.clone(), cx));

        let save_error_line = save_error.map(|e| {
            div()
                .mx_3()
                .mt_2()
                .text_meta(&theme)
                .text_color(rgb(theme.danger))
                .child(format!("save failed: {e}"))
        });

        let can_save = can_save(mode, dirty, saving);
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
                            this.dismiss_config_editor_layer(window, cx);
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
                                                    h_flex()
                                                        .gap_1p5()
                                                        .text_title(&theme)
                                                        .child(file_name)
                                                        .children(dirty_marker),
                                                )
                                                .child(
                                                    div().text_mono_meta(&theme).child(full_path),
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
                                                            .text_body(&theme)
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
                                                        .text_body(&theme)
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
                        )
                        // Painted after the card, so it sits above it: the unlock prompt
                        // is a layer over this modal, not a replacement for it — the
                        // buffer behind stays exactly where the user left it.
                        .children(unlock_layer),
                ),
            )
            .with_priority(1),
        )
    }
}

// ---- render helpers ----------------------------------------------------------------

/// Build the editor body over `text`: the multi-line input, its change subscription, and
/// focus. Shared by the open-time load and a successful unlock, so an unlocked file gets
/// exactly the editor an ordinary one does.
fn build_editor_body(
    text: String,
    window: &mut Window,
    cx: &mut Context<AppState>,
) -> ConfigEditorBody {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .code_editor("")
            .line_number(true)
            .default_value(text)
    });
    let sub = cx.subscribe_in(&input, window, AppState::on_config_editor_input_event);
    input.update(cx, |state, cx| state.focus(window, cx));
    ConfigEditorBody::Editor {
        input,
        _input_sub: sub,
        dirty: false,
    }
}

/// The "unlock with sudo" affordance. Two call sites — the read-only banner and the
/// locked body — so it is one control defined once.
fn unlock_button(id: &'static str, cx: &mut Context<AppState>) -> impl IntoElement + use<> {
    Button::new(id, "unlock with sudo")
        .small()
        .icon(Icon::User)
        .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
            this.open_config_editor_unlock(None, window, cx);
        }))
}

/// The unlock prompt, drawn as a layer over the editor modal.
///
/// Deliberately the same shape as `ui::password_prompt`'s connect-time modal — masked
/// field, an error line, Esc cancels / Enter submits, and a note saying the password is
/// not being stored — so the two places sid asks for a password look and behave alike.
/// It is a local layer rather than that shared entity because this modal owns its own
/// lifecycle (see the module doc's note about never touching `app.rs`).
fn unlock_prompt_layer(
    prompt: &UnlockPrompt,
    theme: &sid_ui::Theme,
    viewport: gpui::Size<gpui::Pixels>,
    path: SharedString,
    cx: &mut Context<AppState>,
) -> impl IntoElement + use<> {
    let busy = prompt.busy;
    let error = prompt.error.clone();
    let submit_label = if busy { "unlocking…" } else { "unlock" };

    div()
        .absolute()
        .top_0()
        .left_0()
        .w(viewport.width)
        .h(viewport.height)
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        // The canonical scrim token, not a second hand-picked black: this layer stacks
        // over the editor's own backdrop, and a scrim must darken whatever is behind it
        // rather than follow a palette.
        .bg(rgba(sid_ui::bridge::SCRIM))
        .child(
            v_flex()
                .id("config-editor-unlock-prompt")
                .w(px(440.))
                .gap_3()
                .p_4()
                .rounded_lg()
                .bg(rgb(theme.surface))
                .border_1()
                .border_color(rgb(theme.border))
                // Enter submits, the `on_key_down`-on-the-wrapper idiom `ui::mod`'s
                // `is_field_submit` documents — no new key binding, so this modal stays
                // self-contained.
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if is_field_submit(&ev.keystroke) {
                        cx.stop_propagation();
                        this.submit_config_editor_unlock(window, cx);
                    }
                }))
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(div().text_title(theme).child("unlock as root"))
                        .child(
                            div()
                                .text_meta(theme)
                                .text_color(rgb(theme.muted))
                                .child("esc cancels · enter unlocks"),
                        ),
                )
                .child(
                    div()
                        .text_mono_meta(theme)
                        .text_color(rgb(theme.muted))
                        .child(path),
                )
                .child(prompt.password.clone())
                .children(error.map(|e| {
                    div()
                        .text_meta(theme)
                        .text_color(rgb(theme.danger))
                        .child(e)
                }))
                .child(div().text_meta(theme).text_color(rgb(theme.muted)).child(
                    "your sudo password — held in memory while this file is open, \
                             never written to disk or the keyring",
                ))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("config-editor-unlock-cancel", "cancel")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                    this.close_config_editor_unlock(window, cx);
                                })),
                        )
                        .child(
                            Button::new("config-editor-unlock-submit", submit_label)
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                    this.submit_config_editor_unlock(window, cx);
                                })),
                        ),
                ),
        )
}

// ---- pure decisions (unit-tested) -------------------------------------------------

/// What the editor offers for the open file. Derived from the unprivileged access probe
/// plus whether this modal session currently holds an elevation secret — never stored,
/// so the two can never drift apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EditorMode {
    /// Ordinary file: edit it, save it directly.
    Direct,
    /// Unlocked: edit it, and save through the privileged path.
    Elevated,
    /// Root-owned and locked. `has_content` distinguishes the two shapes: a
    /// world-readable `/etc/fstab` can be *shown* while saving needs root, whereas
    /// `/etc/sudoers` cannot even be read until the unlock succeeds.
    NeedsRoot { has_content: bool },
    /// Read-only with no unlock offered, because no password would help — a file that
    /// is not there at all.
    Sealed,
}

/// Decide the mode. `elevated` is whether an accepted secret is being held for this
/// modal session.
pub(crate) fn editor_mode(access: Access, elevated: bool) -> EditorMode {
    match access {
        Access::ReadWrite => EditorMode::Direct,
        Access::Missing => EditorMode::Sealed,
        Access::ReadOnly if elevated => EditorMode::Elevated,
        Access::Denied if elevated => EditorMode::Elevated,

        Access::ReadOnly => EditorMode::NeedsRoot { has_content: true },
        Access::Denied => EditorMode::NeedsRoot { has_content: false },
    }
}

/// Whether the Save affordance is live. Elevation does not bypass the other two
/// conditions: an unlocked file with no edits still has nothing to save.
pub(crate) fn can_save(mode: EditorMode, dirty: bool, saving: bool) -> bool {
    matches!(mode, EditorMode::Direct | EditorMode::Elevated) && dirty && !saving
}

/// What an unlock attempt's result means for the prompt.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum UnlockOutcome {
    /// Elevated read succeeded and the content passed the load gate — hold the secret,
    /// show the editor.
    Unlocked(String),
    /// Keep the prompt open with this error and let the user type again. **Only** for a
    /// genuinely mistyped password: any other cause would be an unsatisfiable loop.
    Retry(String),
    /// Close the prompt and surface this — no password will change the outcome.
    Fatal(String),
}

/// Classify an elevated read. The content gate runs here too, so a file that unlocks but
/// is 4 MiB of binary is a `Fatal` (correct password, unusable file) rather than a
/// retry.
pub(crate) fn unlock_outcome(result: Result<Vec<u8>, PrivError>) -> UnlockOutcome {
    match result {
        Ok(bytes) => match gate_loaded_bytes(bytes) {
            Ok(text) => UnlockOutcome::Unlocked(text),
            // The password was accepted; the file is simply not editable as text.
            Err(notice) => UnlockOutcome::Fatal(notice),
        },
        Err(err) if err.is_retryable() => UnlockOutcome::Retry(err.to_string()),
        Err(err) => UnlockOutcome::Fatal(err.to_string()),
    }
}

/// What a privileged save's result means.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SaveOutcome {
    /// Written; the buffer is clean again.
    Saved,
    /// The held secret was rejected — it was accepted for the read, so something changed
    /// underneath (a password change, a revoked timestamp). Ask again rather than
    /// reporting a save error the user cannot act on.
    Reauth(String),
    /// Surface this and keep the buffer dirty.
    Failed(String),
}

/// Classify a privileged save.
pub(crate) fn save_outcome(result: Result<(), PrivError>) -> SaveOutcome {
    match result {
        Ok(()) => SaveOutcome::Saved,
        Err(err) if err.is_retryable() => SaveOutcome::Reauth(err.to_string()),
        Err(err) => SaveOutcome::Failed(err.to_string()),
    }
}

// ---- pure helpers (unit-tested) ---------------------------------------------------

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

    // ---- editor_mode: what the banner and the Save button are derived from --------

    #[test]
    fn an_ordinary_file_is_edited_directly() {
        assert_eq!(editor_mode(Access::ReadWrite, false), EditorMode::Direct);
    }

    #[test]
    fn a_world_readable_root_file_shows_its_content_and_offers_an_unlock() {
        // /etc/fstab: readable by anyone, writable only by root.
        assert_eq!(
            editor_mode(Access::ReadOnly, false),
            EditorMode::NeedsRoot { has_content: true }
        );
    }

    #[test]
    fn an_unreadable_root_file_offers_an_unlock_with_nothing_to_show() {
        // /etc/sudoers: mode 0440 root:root. There is no content to display yet.
        assert_eq!(
            editor_mode(Access::Denied, false),
            EditorMode::NeedsRoot { has_content: false }
        );
    }

    #[test]
    fn holding_a_secret_turns_either_locked_shape_into_an_elevated_editor() {
        assert_eq!(editor_mode(Access::ReadOnly, true), EditorMode::Elevated);
        assert_eq!(editor_mode(Access::Denied, true), EditorMode::Elevated);
    }

    #[test]
    fn a_missing_file_is_sealed_because_no_password_would_conjure_it() {
        assert_eq!(editor_mode(Access::Missing, false), EditorMode::Sealed);
        assert_eq!(editor_mode(Access::Missing, true), EditorMode::Sealed);
    }

    #[test]
    fn an_ordinary_file_stays_direct_even_if_a_secret_is_held() {
        // Nothing needed elevation, so nothing routes through sudo — holding a secret
        // from an earlier file must not silently promote an ordinary save.
        assert_eq!(editor_mode(Access::ReadWrite, true), EditorMode::Direct);
    }

    // ---- can_save ----------------------------------------------------------------

    #[test]
    fn a_dirty_editable_buffer_can_be_saved() {
        assert!(can_save(EditorMode::Direct, true, false));
        assert!(can_save(EditorMode::Elevated, true, false));
    }

    #[test]
    fn a_clean_buffer_can_never_be_saved() {
        assert!(!can_save(EditorMode::Direct, false, false));
        assert!(!can_save(EditorMode::Elevated, false, false));
    }

    #[test]
    fn a_save_already_in_flight_blocks_a_second_one() {
        assert!(!can_save(EditorMode::Direct, true, true));
        assert!(!can_save(EditorMode::Elevated, true, true));
    }

    #[test]
    fn a_locked_file_can_never_be_saved_however_dirty() {
        // The bug this guards: an unlock affordance that enabled Save before the
        // elevation actually succeeded would write through the unprivileged path and
        // fail — or, worse, half-write.
        assert!(!can_save(
            EditorMode::NeedsRoot { has_content: true },
            true,
            false
        ));
        assert!(!can_save(
            EditorMode::NeedsRoot { has_content: false },
            true,
            false
        ));
        assert!(!can_save(EditorMode::Sealed, true, false));
    }

    // ---- unlock_outcome ----------------------------------------------------------

    #[test]
    fn an_elevated_read_that_passes_the_gate_unlocks_the_editor() {
        let got = unlock_outcome(Ok(b"PermitRootLogin no\n".to_vec()));
        assert_eq!(got, UnlockOutcome::Unlocked("PermitRootLogin no\n".into()));
    }

    #[test]
    fn a_mistyped_password_keeps_the_prompt_open() {
        match unlock_outcome(Err(PrivError::AuthFailed)) {
            UnlockOutcome::Retry(msg) => assert!(
                msg.to_lowercase().contains("password"),
                "the retry message must say what to fix: {msg}"
            ),
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn a_non_sudoer_is_told_once_and_the_prompt_closes() {
        // The unsatisfiable-loop guard: re-prompting someone who is not in sudoers can
        // never succeed, so the prompt must not survive it.
        match unlock_outcome(Err(PrivError::NotPermitted(
            "not in the sudoers file".into(),
        ))) {
            UnlockOutcome::Fatal(msg) => assert!(msg.contains("sudoers"), "got {msg}"),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn every_non_auth_failure_closes_the_prompt() {
        for err in [
            PrivError::Unavailable("no sudo".into()),
            PrivError::Timeout,
            PrivError::UnsafePath("relative".into()),
            PrivError::Io("No such file".into()),
        ] {
            assert!(
                matches!(unlock_outcome(Err(err.clone())), UnlockOutcome::Fatal(_)),
                "{err:?} must not re-prompt"
            );
        }
    }

    #[test]
    fn a_correct_password_on_an_unusable_file_is_fatal_not_a_retry() {
        // The password worked; the file is simply not editable as text. Asking for it
        // again would blame the user for the wrong thing.
        let got = unlock_outcome(Ok(vec![0xff, 0xfe, 0xfd]));
        match got {
            UnlockOutcome::Fatal(msg) => assert!(msg.contains("binary"), "got {msg}"),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn an_unlocked_file_over_the_cap_is_fatal() {
        let got = unlock_outcome(Ok(vec![b'a'; CONFIG_MAX_BYTES as usize + 1]));
        assert!(matches!(got, UnlockOutcome::Fatal(msg) if msg.contains("too large")));
    }

    // ---- save_outcome ------------------------------------------------------------

    #[test]
    fn a_privileged_save_that_succeeds_cleans_the_buffer() {
        assert_eq!(save_outcome(Ok(())), SaveOutcome::Saved);
    }

    #[test]
    fn a_secret_rejected_at_save_time_asks_again_rather_than_reporting_an_error() {
        // It was accepted for the read, so something changed underneath. A save error
        // the user cannot act on is worse than a second prompt.
        match save_outcome(Err(PrivError::AuthFailed)) {
            SaveOutcome::Reauth(msg) => assert!(!msg.is_empty()),
            other => panic!("expected Reauth, got {other:?}"),
        }
    }

    #[test]
    fn any_other_privileged_save_failure_is_reported_not_re_prompted() {
        for err in [
            PrivError::NotPermitted("nope".into()),
            PrivError::Timeout,
            PrivError::Io("Read-only file system".into()),
        ] {
            assert!(
                matches!(save_outcome(Err(err.clone())), SaveOutcome::Failed(_)),
                "{err:?} must be reported as-is"
            );
        }
    }

    #[test]
    fn a_reported_save_failure_carries_the_underlying_reason() {
        match save_outcome(Err(PrivError::Io("Read-only file system".into()))) {
            SaveOutcome::Failed(msg) => assert!(msg.contains("Read-only file system")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

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

    #[test]
    fn sibling_temp_path_is_hidden_and_same_directory() {
        let tmp = sibling_temp_path(Path::new("/etc/hosts"));
        assert_eq!(tmp.parent(), Some(Path::new("/etc")));
        let name = tmp.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(".hosts.sid-tmp-"));
    }
}
