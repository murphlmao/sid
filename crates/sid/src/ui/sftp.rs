//! SFTP browser entity (Plan 3.4, S1) — mirrors 3C's [`super::TerminalSession`] pattern:
//! connect a client, open an SFTP session, and marshal the async calls through the shared
//! `sid-ssh` Tokio runtime (see [`super::terminal::ssh_runtime`]), results carried back to
//! the entity via `cx.spawn`/`this.update`. Render (S3) paints purely from the cached
//! `entries`; every SFTP call runs off gpui's own executor, never inline in `render`.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use sid_core::ssh::{SftpEntry, SftpSession, SshClient, SshError};
use sid_secrets::SecretStore;
use sid_ssh::RusshClientFactory;
use sid_store::Host;
use tokio::sync::Mutex as AsyncMutex;

use crate::ssh_connect::{connect_params, resolve_secret};
use crate::ui::terminal::ssh_runtime;

/// Lifecycle status of an [`SftpBrowser`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SftpStatus {
    Connecting,
    Ready,
    Failed(String),
    Closed,
}

type SharedSftp = Arc<AsyncMutex<Box<dyn SftpSession>>>;

/// A live (or connecting/failed) SFTP browser: an adapter-backed session plus the current
/// directory's cached listing. S3 adds navigation/render; S4 adds download/upload.
pub struct SftpBrowser {
    session: Option<SharedSftp>,
    status: SftpStatus,
    path: String,
    entries: Vec<SftpEntry>,
    /// The last list/download/upload operation's status or error message. Distinct from a
    /// connect failure, which lives in `status` instead.
    error: Option<String>,
}

impl SftpBrowser {
    /// Resolve the host's secret, then spawn the connect: build params (3C's `connect_params`)
    /// → open a client → `connect` → `open_sftp()` → `list` the root → cache. Mirrors
    /// `TerminalSession::connect`; any failure lands in `status` as `Failed`.
    pub fn open(
        host: Host,
        secrets: &dyn SecretStore,
        known_hosts_path: PathBuf,
        cx: &mut App,
    ) -> Entity<Self> {
        // Resolved synchronously, same reasoning as `TerminalSession::connect`: `secrets` is a
        // borrowed trait object that cannot cross into the spawned task.
        let secret = resolve_secret(secrets, &host);
        cx.new(|cx| {
            let mut browser = SftpBrowser {
                session: None,
                status: SftpStatus::Connecting,
                path: "/".to_string(),
                entries: Vec::new(),
                error: None,
            };
            browser.start_connect(host, secret, known_hosts_path, cx);
            browser
        })
    }

    pub fn status(&self) -> &SftpStatus {
        &self.status
    }

    fn start_connect(
        &mut self,
        host: Host,
        secret: Result<Option<Vec<u8>>, String>,
        known_hosts_path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let root = self.path.clone();
        cx.spawn(async move |this, cx| {
            let outcome: Result<(Box<dyn SftpSession>, Vec<SftpEntry>), String> = async {
                let secret = secret?;
                let (spec, auth) = connect_params(&host, secret)?;
                let factory = RusshClientFactory::new(known_hosts_path);
                let mut client = factory.new_client();
                let handle = ssh_runtime().spawn(async move {
                    client.connect(&spec, &auth).await?;
                    let mut sftp = client.open_sftp().await?;
                    let entries = sftp.list(&root).await?;
                    Ok::<_, SshError>((sftp, entries))
                });
                match handle.await {
                    Ok(Ok((sftp, entries))) => Ok((sftp, entries)),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(join_err) => Err(format!("connect task panicked: {join_err}")),
                }
            }
            .await;

            let _ = this.update(cx, |browser, cx| {
                match outcome {
                    Ok((sftp, mut entries)) => {
                        sort_entries(&mut entries);
                        browser.session = Some(Arc::new(AsyncMutex::new(sftp)));
                        browser.entries = entries;
                        browser.status = SftpStatus::Ready;
                    }
                    Err(err) => browser.status = SftpStatus::Failed(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Gracefully close the SFTP session (the SSH tab's back control, S5).
    pub fn close(&mut self) {
        self.status = SftpStatus::Closed;
        let Some(session) = self.session.take() else {
            return;
        };
        ssh_runtime().spawn(async move {
            let _ = session.lock().await.close().await;
        });
    }
}

// ---- pure path logic + entry ordering (S2) ---------------------------------------------
//
// The only unit-tested surface in this module — everything else here is I/O or gpui
// wiring, observation-gated per the plan's pragmatic-TDD rule.

/// Join `base` (an absolute POSIX-style directory path) with a single path component.
/// `base == "/"` is the one case that needs special handling — everywhere else a plain
/// `/`-join is correct — because appending `/name` after an already-trailing `/` would
/// double it.
fn join_path(base: &str, name: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// The parent of an absolute POSIX-style path. The root is its own parent (there is
/// nowhere further up to navigate); everything else strips its final component.
fn parent_path(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => trimmed[..idx].to_string(),
        None => "/".to_string(),
    }
}

/// Sort directory listings dirs-first, then alphabetically (case-insensitive) within
/// each group — called after every `list`.
fn sort_entries(entries: &mut [SftpEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
}

#[cfg(test)]
mod path_tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> SftpEntry {
        SftpEntry {
            name: name.to_string(),
            is_dir,
            size: 0,
            mtime_secs: 0,
            mode: 0,
        }
    }

    #[test]
    fn join_path_appends_under_a_directory() {
        assert_eq!(join_path("/home", "a"), "/home/a");
    }

    #[test]
    fn join_path_under_root_avoids_double_slash() {
        assert_eq!(join_path("/", "a"), "/a");
    }

    #[test]
    fn parent_path_of_nested_dir_strips_last_component() {
        assert_eq!(parent_path("/a/b"), "/a");
    }

    #[test]
    fn parent_path_of_root_is_root() {
        assert_eq!(parent_path("/"), "/");
    }

    #[test]
    fn sort_entries_puts_dirs_before_files_then_alphabetical() {
        let mut entries = vec![
            entry("zeta.txt", false),
            entry("Banana", true),
            entry("apple.txt", false),
            entry("alpha", true),
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Banana", "apple.txt", "zeta.txt"]);
    }
}

impl Render for SftpBrowser {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Placeholder rendering; S3 replaces `Ready` with the breadcrumb + entry list.
        let text = match &self.status {
            SftpStatus::Connecting => "Connecting…".to_string(),
            SftpStatus::Failed(err) => format!("SFTP connect failed: {err}"),
            SftpStatus::Closed => "SFTP session closed.".to_string(),
            SftpStatus::Ready => format!(
                "Ready — {} entries at {}",
                self.entries.len(),
                self.path
            ),
        };
        div().size_full().child(text)
    }
}
