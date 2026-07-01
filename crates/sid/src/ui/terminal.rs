//! Terminal session entity (Plan 3C, C2) — connects an SSH shell and pumps its output into
//! a [`TerminalScreen`]. Rendering (C3) and keyboard input/resize (C4) build on this.
//!
//! GPUI's own executor is single-threaded/foreground and knows nothing about Tokio, but the
//! `sid-ssh` adapter (russh) is Tokio-native end to end: connecting spawns a background
//! connection-driver task, and the shell's reader task is `tokio::spawn`ed too (see
//! `sid_ssh::shell::RusshShell::new`). So this module keeps one dedicated, process-lifetime
//! Tokio runtime (`ssh_runtime`) and only ever crosses into it for the span of a single
//! `.spawn(..).await` — the gpui-side task stays on gpui's own foreground executor throughout,
//! which is what makes the "no blocking SSH/PTY calls inline in render" rule hold structurally:
//! the only thing gpui's executor ever awaits here is a `JoinHandle`.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity};
use sid_core::ssh::{SshClient, SshError, SshShell};
use sid_core::term::TerminalScreen;
use sid_secrets::SecretStore;
use sid_ssh::RusshClientFactory;
use sid_store::Host;
use sid_term::Vt100Screen;
use tokio::sync::Mutex as AsyncMutex;

use crate::ssh_connect::{connect_params, resolve_secret};

/// Lifecycle status of a [`TerminalSession`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Connecting,
    Connected,
    Failed(String),
    Closed,
}

/// A live (or connecting/failed) SSH terminal: an adapter-backed shell feeding a
/// [`TerminalScreen`] snapshot that C3 paints and C4 drives with keyboard input.
pub struct TerminalSession {
    screen: Box<dyn TerminalScreen>,
    shell: Option<Arc<AsyncMutex<Box<dyn SshShell>>>>,
    status: SessionStatus,
    rows: u16,
    cols: u16,
}

/// How often the read-loop hops onto the Tokio runtime to drain the shell's output buffer.
// ponytail: fixed-interval poll, not event-driven — fine at ~30Hz for a terminal; revisit only
// if `SshShell` grows a readable-notify.
const POLL_INTERVAL: Duration = Duration::from_millis(33);

/// Placeholder pane size until C4's viewport-driven resize computes the real one.
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// The dedicated, process-lifetime Tokio runtime backing every `sid-ssh` call. Built once on
/// first use and driven forever on its own thread — gpui's foreground executor only ever awaits
/// the `JoinHandle`s this hands back, never polls adapter futures itself.
fn ssh_runtime() -> &'static tokio::runtime::Handle {
    static HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build sid-ssh tokio runtime");
        let handle = rt.handle().clone();
        // This thread just keeps the runtime's workers alive for the process lifetime; it never
        // returns. `pending::<()>()` never resolves, so `block_on` blocks here forever.
        std::thread::spawn(move || rt.block_on(std::future::pending::<()>()));
        handle
    })
}

impl TerminalSession {
    /// Resolve the host's secret, then spawn the connect: build params (C1) → open a client →
    /// `connect` → `open_shell` → store the shell and start the read-loop; any failure lands in
    /// `status` as `Failed`.
    pub fn connect(
        host: Host,
        secrets: &dyn SecretStore,
        known_hosts_path: PathBuf,
        cx: &mut App,
    ) -> Entity<Self> {
        // `secrets` is a borrowed trait object — not `'static`/`Send` — so it cannot cross into
        // the spawned task. Resolve it synchronously here and carry only the owned bytes over;
        // this is the only point the secret exists as plain bytes, and it is never logged.
        let secret = resolve_secret(secrets, &host);
        cx.new(|cx| {
            let mut session = TerminalSession {
                screen: Box::new(Vt100Screen::new(DEFAULT_ROWS, DEFAULT_COLS)),
                shell: None,
                status: SessionStatus::Connecting,
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
            };
            session.start_connect(host, secret, known_hosts_path, cx);
            session
        })
    }

    pub fn status(&self) -> &SessionStatus {
        &self.status
    }

    pub fn screen(&self) -> &dyn TerminalScreen {
        self.screen.as_ref()
    }

    fn start_connect(
        &mut self,
        host: Host,
        secret: Result<Option<Vec<u8>>, String>,
        known_hosts_path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let rows = self.rows;
        let cols = self.cols;
        cx.spawn(async move |this, cx| {
            let outcome: Result<Box<dyn SshShell>, String> = async {
                let secret = secret?;
                let (spec, auth) = connect_params(&host, secret)?;
                let factory = RusshClientFactory::new(known_hosts_path);
                let mut client = factory.new_client();
                let handle = ssh_runtime().spawn(async move {
                    client.connect(&spec, &auth).await?;
                    let shell = client.open_shell("xterm-256color", rows, cols).await?;
                    Ok::<_, SshError>(shell)
                });
                match handle.await {
                    Ok(Ok(shell)) => Ok(shell),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(join_err) => Err(format!("connect task panicked: {join_err}")),
                }
            }
            .await;

            let _ = this.update(cx, |session, cx| {
                match outcome {
                    Ok(shell) => {
                        session.shell = Some(Arc::new(AsyncMutex::new(shell)));
                        session.status = SessionStatus::Connected;
                        session.start_read_loop(cx);
                    }
                    Err(err) => session.status = SessionStatus::Failed(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_read_loop(&mut self, cx: &mut Context<Self>) {
        let Some(shell) = self.shell.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let shell = shell.clone();
                let read = ssh_runtime().spawn(async move { shell.lock().await.try_read().await });
                let bytes = match read.await {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(_)) | Err(_) => {
                        // Adapter error or a panicked join — the session is done either way.
                        let _ = this.update(cx, |session, cx| {
                            session.status = SessionStatus::Closed;
                            cx.notify();
                        });
                        return;
                    }
                };
                let has_output = !bytes.is_empty();
                let updated = this.update(cx, |session, cx| {
                    if has_output {
                        session.screen.feed(&bytes);
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    // Entity released (view closed/dropped) — stop polling.
                    return;
                }
            }
        })
        .detach();
    }

    /// Send raw bytes to the remote shell (C4 turns keystrokes into these). Fire-and-forget: a
    /// write failure surfaces on the next read-loop tick as a closed session.
    pub fn send_input(&self, bytes: Vec<u8>) {
        let Some(shell) = self.shell.clone() else {
            return;
        };
        ssh_runtime().spawn(async move {
            let _ = shell.lock().await.write(&bytes).await;
        });
    }

    /// Recompute the pane size (C4, on viewport change) and push it to both the PTY and the
    /// local screen model.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == (self.rows, self.cols) {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        self.screen.resize(rows, cols);
        let Some(shell) = self.shell.clone() else {
            return;
        };
        ssh_runtime().spawn(async move {
            let _ = shell.lock().await.resize(rows, cols).await;
        });
    }

    /// Gracefully close the remote shell (C5's back/disconnect control).
    pub fn disconnect(&mut self) {
        self.status = SessionStatus::Closed;
        let Some(shell) = self.shell.take() else {
            return;
        };
        ssh_runtime().spawn(async move {
            let _ = shell.lock().await.close().await;
        });
    }
}
