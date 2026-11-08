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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    ClickEvent, ClipboardItem, Context, IntoElement, ParentElement, SharedString, Styled, Window,
    div, prelude::FluentBuilder as _, px, rgb,
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
/// A toast carrying a path the user may want to copy stays longer.
const TOAST_LIFETIME_LONG: Duration = Duration::from_secs(15);

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
        /// Where "Keep server copy" will save the user's edits.
        backup: PathBuf,
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

/// `<cache_root>/backups`: where kept-local copies live. A sibling of the per-edit private
/// directories (never inside one), so `RemoteEdit::close` and the temp-dir cleanup cannot
/// touch it.
pub(super) fn backups_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("backups")
}

/// `<name>.local-<YYYYmmdd-HHMMSS>` (UTC) for a backup of `name` taken at `epoch_secs`.
pub(super) fn backup_file_name(name: &str, epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let rem = epoch_secs.rem_euclid(86_400);
    let (y, mo, d) = super::civil_from_days(days);
    format!(
        "{name}.local-{y:04}{mo:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// The backup path for `name` at `epoch_secs`. `name` is remote-chosen, so only its final
/// component is used.
pub(super) fn backup_path(cache_root: &Path, name: &str, epoch_secs: i64) -> PathBuf {
    let bare = super::safe_local_name(name).unwrap_or_else(|| "file".to_string());
    backups_dir(cache_root).join(backup_file_name(&bare, epoch_secs))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Copy the working copy to `dest` (creating the backups directory, private to the user).
/// Never overwrites an existing backup.
pub(super) fn write_backup(src: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut from = std::fs::File::open(src)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut to = opts.open(dest)?;
    std::io::copy(&mut from, &mut to)?;
    Ok(())
}

/// The copy for "Saved <name> to <host>".
pub(super) fn saved_message(name: &str, host: &str) -> String {
    format!("Saved {name} to {host}")
}

/// Body text for the conflict dialog.
pub(super) fn conflict_body(name: &str, kind: ConflictKind, backup: &Path) -> String {
    let keep = format!(
        "Keeping the server copy leaves it as it is and saves your edits to {}.",
        backup.display()
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
                let backup = backup_path(&self.edit_cfg.cache_root, name, now_secs());
                self.edit_dialog = Some(EditDialog::Conflict {
                    remote_path: remote_path.to_string(),
                    name: name.to_string(),
                    kind,
                    backup,
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

    /// The conflict dialog's "Keep server copy": the server is left alone, the user's
    /// edits are copied to a backup that outlives the edit, then the edit ends.
    pub(super) fn resolve_conflict_keep(&mut self, cx: &mut Context<Self>) {
        let Some(EditDialog::Conflict {
            remote_path,
            name,
            backup,
            ..
        }) = self.edit_dialog.take()
        else {
            return;
        };
        let edit = self
            .edits
            .iter()
            .find(|a| a.remote_path == remote_path)
            .and_then(|a| a.edit.clone());
        self.stop_edit_keep_working_copy(&remote_path, cx);
        let Some(edit) = edit else { return };
        cx.spawn(async move |this, cx| {
            let saved: Result<PathBuf, String> = ssh_runtime()
                .spawn(async move {
                    let Some(e) = edit.lock().await.take() else {
                        return Err("the working copy was already closed".to_string());
                    };
                    let result = write_backup(e.local_path(), &backup)
                        .map(|()| backup.clone())
                        .map_err(|err| err.to_string());
                    let _ = e.close();
                    result
                })
                .await
                .unwrap_or_else(|j| Err(j.to_string()));
            let _ = this.update(cx, |s, cx| match saved {
                Ok(path) => {
                    let shown = path.display().to_string();
                    s.toast_with(
                        ToastTone::Info,
                        format!("Kept the server copy. Your edits were saved to {shown}"),
                        Some(shown),
                        cx,
                    );
                }
                Err(e) => s.toast(
                    ToastTone::Danger,
                    format!("Kept the server copy of {name}, but couldn't save your edits: {e}"),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Remove the edit from the list without closing its working copy (the caller owns
    /// that handle and still needs the file).
    fn stop_edit_keep_working_copy(&mut self, remote_path: &str, cx: &mut Context<Self>) {
        self.edits.retain(|a| a.remote_path != remote_path);
        cx.notify();
    }

    /// Esc on a dialog or the preview.
    pub(super) fn dismiss_dialog(&mut self, cx: &mut Context<Self>) {
        if matches!(self.edit_dialog, Some(EditDialog::Conflict { .. })) {
            // Keeping the server copy is non-destructive now (edits are backed up), so
            // it is the safe reading of "dismiss".
            self.resolve_conflict_keep(cx);
        }
        self.edit_dialog = None;
        self.preview = None;
        cx.notify();
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

    /// Wrap a modal so Esc dismisses it: its own key context, tracked on the dialog focus
    /// handle (focused by `render` when a dialog appears; focus returns to the terminal
    /// when it goes).
    pub(super) fn esc_wrap(
        &self,
        modal_el: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
                name, kind, backup, ..
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
                            .child(conflict_body(&name, kind, &backup)),
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
        Some(modal::overlay(window, self.esc_wrap(modal_el, cx)))
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
        let b = Path::new("/c/backups/x.local-1");
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
        assert!(changed.contains("/c/backups/x.local-1"));
        assert!(deleted.contains("/c/backups/x.local-1"));
    }

    #[test]
    fn backup_names_carry_a_utc_timestamp() {
        // 2026-11-07 22:30:05 UTC
        assert_eq!(
            backup_file_name("a.conf", 1_794_090_605),
            "a.conf.local-20261107-223005"
        );
    }

    #[test]
    fn backup_path_uses_only_the_final_component() {
        let p = backup_path(Path::new("/cache"), "../../etc/passwd", 0);
        assert_eq!(
            p.parent().unwrap(),
            Path::new("/cache/backups"),
            "backup stays inside the backups dir"
        );
        assert!(
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("passwd.local-")
        );
    }

    #[test]
    fn cleanup_of_an_edit_directory_never_removes_backups() {
        let cache = tempfile::tempdir().unwrap();
        // Mimic RemoteEdit: a private per-edit TempDir under the cache root.
        let edit_dir = tempfile::Builder::new().tempdir_in(cache.path()).unwrap();
        let work = edit_dir.path().join("work.txt");
        std::fs::write(&work, b"my edits").unwrap();
        let dest = backup_path(cache.path(), "work.txt", 1_794_090_605);
        write_backup(&work, &dest).unwrap();
        // Cleanup = dropping/closing the edit's directory.
        edit_dir.close().unwrap();
        assert!(!work.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"my edits");
        // A second backup with the same name is refused, never overwritten.
        let again = tempfile::NamedTempFile::new().unwrap();
        assert!(write_backup(again.path(), &dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"my edits");
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
