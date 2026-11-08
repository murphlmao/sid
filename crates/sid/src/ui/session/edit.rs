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
//!    disconnects (a last best-effort sync runs first). Ending removes the temp dir.
//!
//! The editor's own exit is deliberately *not* what ends an edit: GUI editors commonly
//! fork and return at once (`code` without `--wait`), which would otherwise stop syncing
//! while the file is still open.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    ClickEvent, Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px, rgb,
};
use sid_core::editor::{EditorError, EditorLauncher};
use sid_core::ssh::SftpSession;
use sid_ssh::remote_edit::{
    ConflictKind, RemoteEdit, RemoteEditConfig, RemoteEditError, SyncOutcome,
};
use sid_ui::{Button, Modal, Toast, ToastTone, Typography as _, modal, scaled, theme, v_flex};
use tokio::sync::Mutex as AsyncMutex;

use super::{SessionStatus, SshSession, abs_remote_path, ssh_runtime};

/// How often active edits are checked for a save.
const EDIT_POLL: Duration = Duration::from_millis(1500);
/// How long a toast stays up.
const TOAST_LIFETIME: Duration = Duration::from_secs(4);

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
}

/// One remote file open in the user's editor.
pub(super) struct ActiveEdit {
    pub(super) remote_path: String,
    pub(super) name: String,
    pub(super) phase: EditPhase,
    /// `None` until `RemoteEdit::open` returns.
    edit: Option<SharedEdit>,
}

/// A dialog the edit flow can raise.
#[derive(Clone, Debug)]
pub(super) enum EditDialog {
    Conflict {
        remote_path: String,
        name: String,
        kind: ConflictKind,
    },
    NoEditor,
}

/// The session's transient notice.
#[derive(Clone, Debug)]
pub(super) struct SessionToast {
    id: u64,
    tone: ToastTone,
    text: String,
}

/// The copy for "Saved <name> to <host>".
pub(super) fn saved_message(name: &str, host: &str) -> String {
    format!("Saved {name} to {host}")
}

/// Body text for the conflict dialog.
pub(super) fn conflict_body(name: &str, kind: ConflictKind) -> String {
    match kind {
        ConflictKind::RemoteChanged { .. } => format!(
            "{name} changed on the server after you opened it. Overwriting replaces that \
             version with yours; keeping the server copy discards your edits."
        ),
        ConflictKind::RemoteDeleted => format!(
            "{name} was deleted from the server after you opened it. Uploading puts your \
             version back; keeping the server state discards your edits."
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
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.toast = Some(SessionToast { id, tone, text });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_LIFETIME).await;
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
            // The connection is gone: nothing can be uploaded. Dropping the handles
            // removes the private directories.
            self.drop_edits();
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
            Ok(Ok(SyncOutcome::Uploaded { .. })) => {
                let msg = saved_message(name, &self.host_label);
                self.toast(ToastTone::Success, msg, cx);
            }
            Ok(Err(RemoteEditError::Conflict(kind))) => {
                if let Some(a) = self.edits.iter_mut().find(|a| a.remote_path == remote_path) {
                    a.phase = EditPhase::Conflict;
                }
                self.edit_dialog = Some(EditDialog::Conflict {
                    remote_path: remote_path.to_string(),
                    name: name.to_string(),
                    kind,
                });
                cx.notify();
            }
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

    /// The conflict dialog's "Overwrite server copy".
    pub(super) fn resolve_conflict_overwrite(&mut self, cx: &mut Context<Self>) {
        let Some(EditDialog::Conflict {
            remote_path, name, ..
        }) = self.edit_dialog.take()
        else {
            return;
        };
        cx.notify();
        let (Some(sftp), Some(edit)) = (
            self.sftp.clone(),
            self.edits
                .iter()
                .find(|a| a.remote_path == remote_path)
                .and_then(|a| a.edit.clone()),
        ) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = ssh_runtime()
                .spawn(async move {
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
            });
        })
        .detach();
    }

    /// The conflict dialog's "Keep server copy": end the edit, discarding local changes.
    pub(super) fn resolve_conflict_keep(&mut self, cx: &mut Context<Self>) {
        let Some(EditDialog::Conflict {
            remote_path, name, ..
        }) = self.edit_dialog.take()
        else {
            return;
        };
        self.stop_edit(&remote_path, cx);
        self.toast(
            ToastTone::Info,
            format!("Kept the server copy of {name}"),
            cx,
        );
    }

    /// End one edit and remove its working copy.
    pub(super) fn stop_edit(&mut self, remote_path: &str, cx: &mut Context<Self>) {
        let mut taken = Vec::new();
        self.edits.retain_mut(|a| {
            if a.remote_path == remote_path {
                taken.extend(a.edit.take());
                false
            } else {
                true
            }
        });
        close_edits(taken);
        cx.notify();
    }

    /// Forget every edit and remove the working copies (connection gone).
    pub(super) fn drop_edits(&mut self) {
        let taken: Vec<SharedEdit> = self.edits.drain(..).filter_map(|a| a.edit).collect();
        close_edits(taken);
    }

    /// For `disconnect`: the live edits, to flush before the channels close.
    pub(super) fn take_edits_for_flush(&mut self) -> Vec<SharedEdit> {
        self.edits.drain(..).filter_map(|a| a.edit).collect()
    }

    // ---- presentation -----------------------------------------------------------------

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
                .child(div().w_full().max_w(scaled(480.)).child(
                    Toast::new(toast.tone, SharedString::from(toast.text)).on_dismiss(cx.listener(
                        move |s, _ev: &ClickEvent, _w, cx| {
                            if s.toast.as_ref().is_some_and(|t| t.id == id) {
                                s.toast = None;
                                cx.notify();
                            }
                        },
                    )),
                )),
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
            EditDialog::Conflict { name, kind, .. } => {
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
                            .child(conflict_body(&name, kind)),
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
                            cx.notify();
                        })),
                )
                .into_any_element(),
        };
        Some(modal::overlay(window, modal_el))
    }
}

/// Remove working copies on the SSH runtime; never blocks the UI thread.
fn close_edits(edits: Vec<SharedEdit>) {
    if edits.is_empty() {
        return;
    }
    ssh_runtime().spawn(async move {
        for e in edits {
            if let Some(edit) = e.lock().await.take() {
                let _ = edit.close();
            }
        }
    });
}

/// Best-effort final sync then cleanup, run before the SFTP channel closes.
pub(super) async fn flush_and_close(
    edits: Vec<SharedEdit>,
    mut sftp: Option<&mut dyn SftpSession>,
) {
    for e in edits {
        if let Some(mut edit) = e.lock().await.take() {
            if let Some(s) = sftp.as_deref_mut() {
                let _ = edit.sync(s).await;
            }
            let _ = edit.close();
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
        let changed = conflict_body(
            "x",
            ConflictKind::RemoteChanged {
                now: RemoteSig {
                    mtime_secs: 1,
                    size: 2,
                },
            },
        );
        let deleted = conflict_body("x", ConflictKind::RemoteDeleted);
        assert!(changed.contains("changed"));
        assert!(deleted.contains("deleted"));
    }

    #[test]
    fn editing_lookup_is_by_remote_path() {
        let edits = vec![ActiveEdit {
            remote_path: "/etc/a".into(),
            name: "a".into(),
            phase: EditPhase::Watching,
            edit: None,
        }];
        assert!(is_editing(&edits, "/etc/a"));
        assert!(!is_editing(&edits, "/etc/b"));
    }
}
