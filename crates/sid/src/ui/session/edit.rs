//! SFTP edit-in-place for the live session: the presentation and the polling glue around
//! `sid_ssh::remote_edit::RemoteEdit`. All the file logic (stat, vet, download, hash,
//! conflict check, upload) lives in `sid-ssh`; this module only decides *when* to call it
//! and what to tell the user. GPUI stays out of `sid-ssh`.
//!
//! Lifecycle of one edit:
//! 1. `start_edit` — `RemoteEdit::open` (stat + download into a private temp dir) on the
//!    SSH runtime, then `launch` on the user's editor via the injected
//!    [`EditorLauncher`] (policy `EditorOnly`, so a remote-named file never reaches the
//!    desktop's `xdg-open`). No editor configured -> a friendly prompt, nothing left behind.
//! 2. While the session is live, one timer task polls every active edit: `local_changed()`
//!    is an mtime+length check confirmed by a hash, so an idle edit costs a `stat`; the SFTP
//!    channel is only locked when there is something to upload.
//! 3. A save uploads and toasts "Saved <name> to <host>". A [`RemoteEditError::Conflict`]
//!    parks that edit and raises the conflict dialog; the user picks overwrite or keep.
//! 4. The edit ends on "Stop editing", on a kept server copy, or when the session
//!    disconnects. EVERY exit goes through `RemoteEdit::close_or_preserve`: edits not yet
//!    uploaded are copied to a durable backup (app data dir) and the user is told where;
//!    the working copy is deleted only when unchanged, never after a backup error. "Stop
//!    editing" and session close try a final upload first (a conflict on stop re-raises the
//!    dialog instead of losing the edit).
//!
//! Conflicts are queued FIFO: each parked edit remembers when it conflicted, and one
//! dialog at a time shows the oldest.
//!
//! LOCK ORDER (deadlock avoidance): always the edit's own mutex FIRST, then the shared
//! SFTP channel's. Every path here (poll, overwrite, stop, flush) follows it; never lock an
//! edit while holding the SFTP guard.
//!
//! The editor's own exit is deliberately *not* what ends an edit: GUI editors commonly
//! fork and return at once (`code` without `--wait`), which would otherwise stop syncing
//! while the file is still open.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    ClickEvent, ClipboardItem, Context, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use sid_core::editor::{EditorError, EditorLauncher};
use sid_ssh::remote_edit::{
    ConflictKind, Preserved, RemoteEdit, RemoteEditConfig, RemoteEditError, SyncOutcome,
};
use sid_ui::{Button, Modal, Toast, ToastTone, Typography as _, modal, scaled, theme, v_flex};
use tokio::sync::Mutex as AsyncMutex;

use super::{SessionStatus, SharedSftp, SshSession, abs_remote_path, ssh_runtime};

/// How often active edits are checked for a save.
const EDIT_POLL: Duration = Duration::from_millis(1500);
/// How long a toast stays up.
const TOAST_LIFETIME: Duration = Duration::from_secs(4);
/// A toast carrying a path the user may want to copy stays longer.
const TOAST_LIFETIME_LONG: Duration = Duration::from_secs(15);
/// A final upload on stop/close must not hang on a dead connection.
const FINAL_UPLOAD_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) type SharedEdit = Arc<AsyncMutex<Option<RemoteEdit>>>;

/// Where one edit is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditPhase {
    /// Downloading / launching.
    Opening,
    /// Open in the editor; polled for saves.
    Watching,
    /// Parked on a conflict, waiting for the user's choice.
    Conflict,
    /// "Stop editing" is making its final upload.
    Stopping,
}

/// One remote file open in the user's editor.
pub(super) struct ActiveEdit {
    pub(super) remote_path: String,
    /// The file's name as it may be *shown* ([`super::display_name`]): it is
    /// server-controlled, and only ever reaches toasts and dialogs. SFTP calls use
    /// `remote_path`.
    pub(super) name: String,
    pub(super) phase: EditPhase,
    /// `None` until `RemoteEdit::open` returns.
    edit: Option<SharedEdit>,
    /// Set while parked on a conflict: when it happened (FIFO order) and what it was.
    conflict: Option<(Instant, ConflictKind)>,
}

/// A dialog the edit flow can raise.
#[derive(Clone, Debug)]
pub(super) enum EditDialog {
    Conflict {
        remote_path: String,
        name: String,
        kind: ConflictKind,
        /// The folder "Keep server copy" will save the user's edits into.
        backups: PathBuf,
    },
    NoEditor,
}

/// The session's transient notice.
#[derive(Clone, Debug)]
pub(super) struct SessionToast {
    id: u64,
    tone: ToastTone,
    text: String,
    /// A path the toast offers to copy (a saved backup).
    copy_path: Option<String>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The copy for "Saved <name> to <host>".
pub(super) fn saved_message(name: &str, host: &str) -> String {
    format!("Saved {name} to {host}")
}

/// Body text for the conflict dialog.
pub(super) fn conflict_body(name: &str, kind: ConflictKind, backups: &Path) -> String {
    let keep = format!(
        "Keeping the server copy leaves it as it is and saves your edits under {}.",
        backups.display()
    );
    match kind {
        ConflictKind::RemoteChanged { .. } => format!(
            "{name} changed on the server after you opened it. Overwriting replaces that \
             version with yours. {keep}"
        ),
        ConflictKind::RemoteDeleted => format!(
            "{name} was deleted from the server after you opened it. Uploading puts your \
             version back. {keep}"
        ),
    }
}

/// Is there already a live edit for this remote path?
pub(super) fn is_editing(edits: &[ActiveEdit], remote_path: &str) -> bool {
    edits.iter().any(|e| e.remote_path == remote_path)
}

/// Build the shared launcher: the only place the concrete Linux impl is named.
pub(super) fn default_launcher() -> Arc<dyn EditorLauncher> {
    Arc::new(sid_os::LinuxEditorLauncher::new())
}

impl SshSession {
    /// Whether `name` (a file in the current directory) has a live edit.
    pub(super) fn is_editing_name(&self, name: &str) -> bool {
        is_editing(&self.edits, &abs_remote_path(&self.path, name))
    }

    /// Show a toast for a few seconds.
    pub(super) fn toast(&mut self, tone: ToastTone, text: String, cx: &mut Context<Self>) {
        self.toast_with(tone, text, None, cx);
    }

    /// A toast that also offers "Copy path" for `copy_path`.
    pub(super) fn toast_with(
        &mut self,
        tone: ToastTone,
        text: String,
        copy_path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.toast = Some(SessionToast {
            id,
            tone,
            text,
            copy_path,
        });
        cx.notify();
        let lifetime = if self.toast.as_ref().is_some_and(|t| t.copy_path.is_some()) {
            TOAST_LIFETIME_LONG
        } else {
            TOAST_LIFETIME
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(lifetime).await;
            let _ = this.update(cx, |s, cx| {
                if s.toast.as_ref().is_some_and(|t| t.id == id) {
                    s.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// "Edit": download, open in the user's editor, and start watching for saves.
    pub(super) fn start_edit(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(sftp) = self.sftp.clone() else {
            return;
        };
        let remote_path = abs_remote_path(&self.path, &name);
        // From here on `name` is only ever shown (toasts, dialogs): display-safe it, as
        // every other remote name in the session is. The raw name lives in `remote_path`.
        let name = super::display_name(&name);
        if is_editing(&self.edits, &remote_path) {
            self.toast(
                ToastTone::Info,
                format!("{name} is already open in your editor"),
                cx,
            );
            return;
        }
        self.edits.push(ActiveEdit {
            remote_path: remote_path.clone(),
            name: name.clone(),
            phase: EditPhase::Opening,
            edit: None,
            conflict: None,
        });
        cx.notify();
        let cfg: RemoteEditConfig = self.edit_cfg.clone();
        let launcher = self.editor_launcher.clone();
        let path_for_open = remote_path.clone();
        cx.spawn(async move |this, cx| {
            let opened = ssh_runtime()
                .spawn(async move {
                    let mut guard = sftp.lock().await;
                    RemoteEdit::open(&mut **guard, &path_for_open, &cfg).await
                })
                .await;
            let outcome: Result<RemoteEdit, RemoteEditError> = match opened {
                Ok(Ok(mut edit)) => match edit.launch(launcher.as_ref()) {
                    Ok(()) => Ok(edit),
                    Err(e) => {
                        let _ = edit.close();
                        Err(e)
                    }
                },
                Ok(Err(e)) => Err(e),
                Err(join) => Err(RemoteEditError::Io(std::io::Error::other(format!(
                    "edit task panicked: {join}"
                )))),
            };
            let _ = this.update(cx, |s, cx| {
                match outcome {
                    Ok(edit) => {
                        if let Some(a) = s.edits.iter_mut().find(|a| a.remote_path == remote_path) {
                            a.edit = Some(Arc::new(AsyncMutex::new(Some(edit))));
                            a.phase = EditPhase::Watching;
                            s.ensure_edit_poll(cx);
                        } else {
                            // Stopped while opening: clean up.
                            let _ = edit.close();
                        }
                    }
                    Err(e) => {
                        s.edits.retain(|a| a.remote_path != remote_path);
                        match e {
                            RemoteEditError::Editor(EditorError::NoEditorConfigured) => {
                                s.edit_dialog = Some(EditDialog::NoEditor);
                            }
                            other => s.toast(
                                ToastTone::Danger,
                                format!("Couldn't edit {name}: {other}"),
                                cx,
                            ),
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Start the poll task if it isn't already running.
    fn ensure_edit_poll(&mut self, cx: &mut Context<Self>) {
        if self.edit_poll_running {
            return;
        }
        self.edit_poll_running = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(EDIT_POLL).await;
                let keep_going = this.update(cx, |s, cx| s.poll_edits(cx)).unwrap_or(false);
                if !keep_going {
                    let _ = this.update(cx, |s, _| s.edit_poll_running = false);
                    break;
                }
            }
        })
        .detach();
    }

    /// One poll pass. Returns whether polling should continue.
    fn poll_edits(&mut self, cx: &mut Context<Self>) -> bool {
        if self.status != SessionStatus::Connected {
            // The connection is gone: nothing can be uploaded, but nothing may be lost
            // either - unsaved edits are backed up and the user told where.
            self.drop_edits(cx);
            return false;
        }
        let Some(sftp) = self.sftp.clone() else {
            return !self.edits.is_empty();
        };
        for a in &self.edits {
            if a.phase != EditPhase::Watching {
                continue;
            }
            let Some(edit) = a.edit.clone() else { continue };
            let sftp = sftp.clone();
            let remote_path = a.remote_path.clone();
            let name = a.name.clone();
            cx.spawn(async move |this, cx| {
                let result = ssh_runtime()
                    .spawn(async move {
                        // Lock order: the edit, then SFTP.
                        let mut slot = edit.lock().await;
                        let Some(e) = slot.as_mut() else {
                            return Ok(SyncOutcome::Unchanged);
                        };
                        // Cheap check first so an idle edit never contends for SFTP.
                        if !e.local_changed()? {
                            return Ok(SyncOutcome::Unchanged);
                        }
                        let mut guard = sftp.lock().await;
                        e.sync(&mut **guard).await
                    })
                    .await;
                let _ = this.update(cx, |s, cx| {
                    s.handle_sync_result(&remote_path, &name, result, cx)
                });
            })
            .detach();
        }
        !self.edits.is_empty()
    }

    fn handle_sync_result(
        &mut self,
        remote_path: &str,
        name: &str,
        result: Result<Result<SyncOutcome, RemoteEditError>, tokio::task::JoinError>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(Ok(SyncOutcome::Unchanged)) => {}
            Ok(Ok(SyncOutcome::Uploaded { atomic })) => {
                let mut msg = saved_message(name, &self.host_label);
                if !atomic {
                    msg.push_str(
                        " (rewritten in place: the server can't replace files atomically)",
                    );
                }
                self.toast(ToastTone::Success, msg, cx);
            }
            Ok(Err(RemoteEditError::Conflict(kind))) => self.park_conflict(remote_path, kind, cx),
            Ok(Err(e)) => {
                // Keep the edit alive (the next save may succeed) but say what happened.
                let msg = format!("Couldn't save {name} to {}: {e}", self.host_label);
                self.toast(ToastTone::Danger, msg, cx);
            }
            Err(join) => {
                self.toast(
                    ToastTone::Danger,
                    format!("Saving {name} failed unexpectedly: {join}"),
                    cx,
                );
            }
        }
    }

    /// Park an edit on a conflict (queued behind any earlier ones) and show its dialog if
    /// none is up.
    fn park_conflict(&mut self, remote_path: &str, kind: ConflictKind, cx: &mut Context<Self>) {
        if let Some(a) = self.edits.iter_mut().find(|a| a.remote_path == remote_path) {
            a.phase = EditPhase::Conflict;
            a.conflict.get_or_insert((Instant::now(), kind));
        }
        self.show_next_conflict(cx);
    }

    /// If no dialog is up, raise the oldest parked conflict's.
    fn show_next_conflict(&mut self, cx: &mut Context<Self>) {
        if self.edit_dialog.is_some() {
            return;
        }
        let backups = self.edit_cfg.backups_root.clone();
        let next = self
            .edits
            .iter()
            .filter_map(|a| a.conflict.map(|(at, kind)| (at, kind, a)))
            .min_by_key(|(at, _, _)| *at);
        if let Some((_, kind, a)) = next {
            self.edit_dialog = Some(EditDialog::Conflict {
                remote_path: a.remote_path.clone(),
                name: a.name.clone(),
                kind,
                backups,
            });
        }
        cx.notify();
    }

    /// The conflict dialog's "Overwrite server copy".
    pub(super) fn resolve_conflict_overwrite(&mut self, cx: &mut Context<Self>) {
        let Some(EditDialog::Conflict {
            remote_path, name, ..
        }) = self.edit_dialog.take()
        else {
            return;
        };
        let (Some(sftp), Some(edit)) = (
            self.sftp.clone(),
            self.edits
                .iter()
                .find(|a| a.remote_path == remote_path)
                .and_then(|a| a.edit.clone()),
        ) else {
            self.toast(
                ToastTone::Danger,
                format!("Couldn't overwrite {name}: the connection is gone. Your edits are kept."),
                cx,
            );
            self.show_next_conflict(cx);
            return;
        };
        if let Some(a) = self.edits.iter_mut().find(|a| a.remote_path == remote_path) {
            a.conflict = None;
            a.phase = EditPhase::Stopping; // not polled while the upload runs
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = ssh_runtime()
                .spawn(async move {
                    // Lock order: the edit, then SFTP.
                    let mut slot = edit.lock().await;
                    let Some(e) = slot.as_mut() else {
                        return Ok(SyncOutcome::Unchanged);
                    };
                    let mut guard = sftp.lock().await;
                    e.force_upload(&mut **guard).await
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                if let Some(a) = s.edits.iter_mut().find(|a| a.remote_path == remote_path) {
                    a.phase = EditPhase::Watching;
                }
                s.handle_sync_result(&remote_path, &name, result, cx);
                s.show_next_conflict(cx);
            });
        })
        .detach();
    }

    /// The conflict dialog's "Keep server copy": the server is left alone, the user's
    /// edits are preserved in the backups folder, then the edit ends.
    pub(super) fn resolve_conflict_keep(&mut self, cx: &mut Context<Self>) {
        let Some(EditDialog::Conflict {
            remote_path, name, ..
        }) = self.edit_dialog.take()
        else {
            return;
        };
        let edit = self.take_active(&remote_path);
        cx.notify();
        let host = self.host_label.clone();
        let Some(edit) = edit else {
            self.toast(
                ToastTone::Warning,
                format!("Kept the server copy of {name}. (It was already closed here.)"),
                cx,
            );
            self.show_next_conflict(cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            let done = ssh_runtime()
                .spawn(async move { finish_edit(edit, &host, None).await })
                .await;
            let _ = this.update(cx, |s, cx| {
                match done {
                    Ok(Some(p)) => s.report_preserved(&name, p, true, cx),
                    Ok(None) | Err(_) => s.toast(
                        ToastTone::Warning,
                        format!("Kept the server copy of {name}. (It was already closed here.)"),
                        cx,
                    ),
                }
                s.show_next_conflict(cx);
            });
        })
        .detach();
    }

    /// Remove an edit from the list and return its shared handle.
    fn take_active(&mut self, remote_path: &str) -> Option<SharedEdit> {
        let mut taken = None;
        self.edits.retain_mut(|a| {
            if a.remote_path == remote_path {
                taken = a.edit.take();
                false
            } else {
                true
            }
        });
        taken
    }

    /// Tell the user what `close_or_preserve` did.
    fn report_preserved(
        &mut self,
        name: &str,
        p: Preserved,
        kept_server: bool,
        cx: &mut Context<Self>,
    ) {
        let lead = if kept_server {
            "Kept the server copy. "
        } else {
            ""
        };
        match p {
            Preserved::Removed => {
                if kept_server {
                    self.toast(
                        ToastTone::Info,
                        format!("Kept the server copy of {name}"),
                        cx,
                    );
                }
            }
            Preserved::BackedUp(path) => {
                let shown = path.display().to_string();
                self.toast_with(
                    ToastTone::Warning,
                    format!("{lead}Your unsaved edits to {name} were saved to {shown}"),
                    Some(shown),
                    cx,
                );
            }
            Preserved::Failed { error, kept } => {
                let shown = kept.display().to_string();
                self.toast_with(
                    ToastTone::Danger,
                    format!(
                        "{lead}Couldn't back up your edits to {name} ({error}). \
                         They are still at {shown}"
                    ),
                    Some(shown),
                    cx,
                );
            }
        }
    }

    /// Esc on a dialog or the preview.
    pub(super) fn dismiss_dialog(&mut self, cx: &mut Context<Self>) {
        if matches!(self.edit_dialog, Some(EditDialog::Conflict { .. })) {
            // Keeping the server copy is non-destructive (edits are backed up), so it is
            // the safe reading of "dismiss". It also raises the next queued dialog.
            self.resolve_conflict_keep(cx);
            self.preview = None;
            cx.notify();
            return;
        }
        self.edit_dialog = None;
        self.preview = None;
        self.show_next_conflict(cx);
        cx.notify();
    }

    /// "Stop editing": one last upload of any unsaved change, then the working copy goes
    /// through `close_or_preserve`. A conflict on that upload re-raises the conflict
    /// dialog instead of ending the edit; any other failure backs the edit up.
    pub(super) fn stop_edit(&mut self, remote_path: &str, cx: &mut Context<Self>) {
        let Some(a) = self.edits.iter_mut().find(|a| a.remote_path == remote_path) else {
            return;
        };
        let Some(edit) = a.edit.clone() else {
            // Still opening: drop the entry; the open task cleans up its own download.
            self.edits.retain(|a| a.remote_path != remote_path);
            cx.notify();
            return;
        };
        if a.phase == EditPhase::Stopping {
            return;
        }
        a.phase = EditPhase::Stopping;
        let name = a.name.clone();
        let remote_path = remote_path.to_string();
        let sftp = self.sftp.clone();
        let host = self.host_label.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let done = ssh_runtime()
                .spawn(async move { finish_edit_stop(edit, &host, sftp).await })
                .await;
            let _ = this.update(cx, |s, cx| match done {
                Ok(StopResult::Conflict(kind)) => s.park_conflict(&remote_path, kind, cx),
                Ok(StopResult::Done(p, err)) => {
                    s.take_active(&remote_path);
                    if let Some(e) = err {
                        s.toast(
                            ToastTone::Danger,
                            format!("Couldn't save {name} to {}: {e}", s.host_label),
                            cx,
                        );
                    }
                    s.report_preserved(&name, p, false, cx);
                    cx.notify();
                }
                Err(j) => {
                    if let Some(a) = s.edits.iter_mut().find(|a| a.remote_path == remote_path) {
                        a.phase = EditPhase::Watching;
                    }
                    s.toast(
                        ToastTone::Danger,
                        format!("Stopping {name} failed unexpectedly: {j}"),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// Connection gone: back up every unsaved edit (no upload is possible) and tell the
    /// user where they went.
    pub(super) fn drop_edits(&mut self, cx: &mut Context<Self>) {
        let taken: Vec<(String, SharedEdit)> = self
            .edits
            .drain(..)
            .filter_map(|a| a.edit.map(|e| (a.name, e)))
            .collect();
        self.edit_dialog = None;
        cx.notify();
        if taken.is_empty() {
            return;
        }
        let host = self.host_label.clone();
        cx.spawn(async move |this, cx| {
            let done = ssh_runtime()
                .spawn(async move {
                    let mut out = Vec::new();
                    for (name, e) in taken {
                        if let Some(p) = finish_edit(e, &host, None).await {
                            out.push((name, p));
                        }
                    }
                    out
                })
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |s, cx| {
                for (name, p) in done {
                    s.report_preserved(&name, p, false, cx);
                }
            });
        })
        .detach();
    }

    /// For `disconnect`: the live edits, to flush before the channels close.
    pub(super) fn take_edits_for_flush(&mut self) -> Vec<SharedEdit> {
        self.edits.drain(..).filter_map(|a| a.edit).collect()
    }

    // ---- presentation -----------------------------------------------------------------

    /// Wrap a modal so Esc dismisses it: its own key context, tracked on the dialog focus
    /// handle (focused by `render` when a dialog appears; focus returns to the terminal
    /// when it goes).
    pub(super) fn esc_wrap<M: IntoElement>(
        &self,
        modal_el: M,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<M> {
        div()
            .key_context(super::SESSION_DIALOG_CONTEXT)
            .track_focus(&self.dialog_focus)
            .on_action(
                cx.listener(|s, _: &super::SessionDialogCancel, _w, cx| s.dismiss_dialog(cx)),
            )
            .child(modal_el)
    }

    /// The session toast, floating bottom-centre of the pane.
    pub(super) fn toast_layer(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let toast = self.toast.clone()?;
        let id = toast.id;
        Some(
            div()
                .absolute()
                .bottom(px(16.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .px_4()
                .child(
                    div().w_full().max_w(scaled(480.)).child(
                        Toast::new(toast.tone, SharedString::from(toast.text))
                            .when_some(toast.copy_path, |t, path| {
                                t.action(
                                    Button::new("session-toast-copy-path", "Copy path")
                                        .small()
                                        .on_click(move |_ev, _w, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                path.clone(),
                                            ));
                                        }),
                                )
                            })
                            .on_dismiss(cx.listener(move |s, _ev: &ClickEvent, _w, cx| {
                                if s.toast.as_ref().is_some_and(|t| t.id == id) {
                                    s.toast = None;
                                    cx.notify();
                                }
                            })),
                    ),
                ),
        )
    }

    /// The conflict / no-editor dialog, if one is up.
    pub(super) fn edit_dialog_layer(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let dialog = self.edit_dialog.clone()?;
        let t = theme::active(cx).clone();
        let modal_el = match dialog {
            EditDialog::Conflict {
                name,
                kind,
                backups,
                ..
            } => {
                let overwrite = match kind {
                    ConflictKind::RemoteChanged { .. } => "Overwrite server copy",
                    ConflictKind::RemoteDeleted => "Upload anyway",
                };
                Modal::new("session-edit-conflict", "The file changed on the server")
                    .width(px(480.))
                    .child(
                        div()
                            .text_body(&t)
                            .text_color(rgb(t.fg))
                            .child(conflict_body(&name, kind, &backups)),
                    )
                    .footer(
                        Button::new("session-edit-keep", "Keep server copy").on_click(
                            cx.listener(|s, _ev: &ClickEvent, _w, cx| s.resolve_conflict_keep(cx)),
                        ),
                    )
                    .footer(
                        Button::new("session-edit-overwrite", overwrite)
                            .primary()
                            .on_click(cx.listener(|s, _ev: &ClickEvent, _w, cx| {
                                s.resolve_conflict_overwrite(cx)
                            })),
                    )
                    .into_any_element()
            }
            EditDialog::NoEditor => Modal::new("session-edit-noeditor", "Pick an editor first")
                .width(px(480.))
                .on_dismiss(cx.listener(|s, _ev: &ClickEvent, _w, cx| {
                    s.edit_dialog = None;
                    s.show_next_conflict(cx);
                    cx.notify();
                }))
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_body(&t).text_color(rgb(t.fg)).child(
                            "sid opens remote files in your own editor, but none is set up. \
                             Set the VISUAL or EDITOR environment variable (for example \
                             EDITOR=nvim) and restart sid.",
                        ))
                        .child(div().text_meta(&t).child(
                            "sid never hands remote files to the desktop's default opener: \
                             that could run them instead of editing them.",
                        )),
                )
                .footer(
                    Button::new("session-edit-noeditor-ok", "Got it")
                        .primary()
                        .on_click(cx.listener(|s, _ev: &ClickEvent, _w, cx| {
                            s.edit_dialog = None;
                            s.show_next_conflict(cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        };
        Some(modal::overlay(window, self.esc_wrap(modal_el, cx)))
    }
}

/// What "Stop editing" ended up doing.
enum StopResult {
    /// The edit is finished. The error, if any, is why the final upload failed (the edit
    /// was then preserved).
    Done(Preserved, Option<RemoteEditError>),
    /// The final upload conflicted: nothing was closed.
    Conflict(ConflictKind),
}

fn epoch_now() -> i64 {
    now_secs()
}

/// Close one edit through `close_or_preserve`, optionally after a bounded final upload
/// (lock order: edit, then SFTP). `None` if the edit was already closed.
async fn finish_edit(edit: SharedEdit, host: &str, sftp: Option<SharedSftp>) -> Option<Preserved> {
    let mut slot = edit.lock().await;
    let mut e = slot.take()?;
    if let Some(sftp) = sftp
        && e.has_unsaved_changes()
    {
        let mut guard = sftp.lock().await;
        let _ = tokio::time::timeout(FINAL_UPLOAD_TIMEOUT, e.sync(&mut **guard)).await;
    }
    Some(e.close_or_preserve(host, epoch_now()))
}

/// "Stop editing": final upload, but a conflict leaves the edit open for the dialog.
async fn finish_edit_stop(edit: SharedEdit, host: &str, sftp: Option<SharedSftp>) -> StopResult {
    let mut slot = edit.lock().await;
    let Some(e) = slot.as_mut() else {
        return StopResult::Done(Preserved::Removed, None);
    };
    let mut upload_err = None;
    if e.has_unsaved_changes()
        && let Some(sftp) = sftp
    {
        let mut guard = sftp.lock().await;
        match tokio::time::timeout(FINAL_UPLOAD_TIMEOUT, e.sync(&mut **guard)).await {
            Ok(Ok(_)) => {}
            Ok(Err(RemoteEditError::Conflict(kind))) => return StopResult::Conflict(kind),
            Ok(Err(other)) => upload_err = Some(other),
            Err(_) => {
                upload_err = Some(RemoteEditError::Io(std::io::Error::other(
                    "the upload timed out",
                )));
            }
        }
    }
    let Some(e) = slot.take() else {
        return StopResult::Done(Preserved::Removed, upload_err);
    };
    StopResult::Done(e.close_or_preserve(host, epoch_now()), upload_err)
}

/// Session close: best-effort bounded final upload for each edit, then
/// `close_or_preserve` (a conflicted or failed edit is backed up, never dropped). Edits
/// are locked before the SFTP channel, matching every other path.
pub(super) async fn flush_and_close(edits: Vec<SharedEdit>, sftp: Option<SharedSftp>, host: &str) {
    for e in edits {
        if let Some(Preserved::BackedUp(path)) = finish_edit(e, host, sftp.clone()).await {
            // The session UI is going away, so there is no toast; leave a trail.
            eprintln!("sid: unsaved edits were preserved at {}", path.display());
        } else {
            // nothing to report
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_ssh::remote_edit::RemoteSig;

    #[test]
    fn saved_toast_names_file_and_host() {
        assert_eq!(saved_message("a.conf", "web1"), "Saved a.conf to web1");
    }

    #[test]
    fn conflict_copy_differs_for_deleted() {
        let b = Path::new("/c/backups");
        let changed = conflict_body(
            "x",
            ConflictKind::RemoteChanged {
                now: RemoteSig {
                    mtime_secs: 1,
                    size: 2,
                },
            },
            b,
        );
        let deleted = conflict_body("x", ConflictKind::RemoteDeleted, b);
        assert!(changed.contains("changed"));
        assert!(deleted.contains("deleted"));
        // Both say where the user's edits go.
        assert!(changed.contains("/c/backups"));
        assert!(deleted.contains("/c/backups"));
    }

    #[test]
    fn editing_lookup_is_by_remote_path() {
        let edits = vec![ActiveEdit {
            remote_path: "/etc/a".into(),
            name: "a".into(),
            phase: EditPhase::Watching,
            edit: None,
            conflict: None,
        }];
        assert!(is_editing(&edits, "/etc/a"));
        assert!(!is_editing(&edits, "/etc/b"));
    }
}
