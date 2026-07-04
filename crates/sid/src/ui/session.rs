//! SSH split session entity (Plan 3.5) — MobaXterm-style: **one** [`SshClient`] connection
//! backing both a live shell (terminal) and a live SFTP browser, side by side. Subsumes
//! Plan 3C's `TerminalSession` and Plan 3.4's `SftpBrowser`, which this module folds
//! together — their reader-pump/render/input and list/nav/download logic is carried over
//! near-verbatim (see git history for their standalone forms), not rewritten.
//!
//! GPUI's own executor is single-threaded/foreground and knows nothing about Tokio, but the
//! `sid-ssh` adapter (russh) is Tokio-native end to end: connecting spawns a background
//! connection-driver task, and the shell's reader task is `tokio::spawn`ed too. So this
//! module keeps one dedicated, process-lifetime Tokio runtime (`ssh_runtime`) and only ever
//! crosses into it for the span of a single `.spawn(..).await` — the gpui-side task stays on
//! gpui's own foreground executor throughout, which is what makes the "no blocking SSH/SFTP
//! calls inline in render" rule hold structurally: the only thing gpui's executor ever awaits
//! here is a `JoinHandle`.
//!
//! **One connection:** [`SshSession::open`] connects the [`SshClient`] exactly once, then
//! calls `open_shell` *and* `open_sftp` on that same client — never a second `connect`/auth.
//! The client is kept alive (`client: Arc<AsyncMutex<Box<dyn SshClient>>>`) for the session's
//! whole lifetime because the shell/SFTP channels are multiplexed over its connection; if the
//! client were dropped, both channels would go with it.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui::{
    App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Font, FontStyle, FontWeight, Hsla, IntoElement, KeyDownEvent, Keystroke, Pixels,
    Render, ShapedLine, SharedString, TextRun, UnderlineStyle, Window, anchored, canvas, deferred,
    div, font, point, prelude::*, px, rgb, rgba, uniform_list,
};
use sid_core::ssh::{SftpEntry, SftpSession, SshClient, SshError, SshShellReader, SshShellWriter};
use sid_core::term::{TermCell, TermColor, TerminalScreen};
use sid_ssh::RusshClientFactory;
use sid_store::{Host, PanelSide};
use sid_term::Vt100Screen;
use tokio::sync::Mutex as AsyncMutex;

use crate::ssh_connect::connect_params;
use crate::ui::TextInput;
use crate::ui::theme;

/// Monospace family — kitty parity (Murphy's terminal font, confirmed installed via
/// `fc-list`); gpui falls back to a proportional font if the family is missing locally. This
/// is also what fixes nerd-font ASCII-art rendering in the terminal pane.
const MONO: &str = "CaskaydiaCove Nerd Font Mono";
const TERM_FONT_SIZE: Pixels = px(14.);

/// The file sidebar's fixed width (plan: "~320px").
const SIDEBAR_WIDTH: Pixels = px(320.);

/// How often the read-loop hops onto the Tokio runtime to drain the shell's output buffer.
// ponytail: fixed-interval poll, not event-driven — fine at ~30Hz for a terminal; revisit only
// if `SshShellReader` grows a readable-notify.
const POLL_INTERVAL: Duration = Duration::from_millis(33);

/// Placeholder pane size until the viewport-driven resize computes the real one.
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// `view`'s size cap: anything bigger is "download instead", never rendered inline.
const PREVIEW_MAX_BYTES: usize = 1024 * 1024;

/// The dedicated, process-lifetime Tokio runtime backing every `sid-ssh` call. Built once on
/// first use and driven forever on its own thread — gpui's foreground executor only ever awaits
/// the `JoinHandle`s this hands back, never polls adapter futures itself.
pub(crate) fn ssh_runtime() -> &'static tokio::runtime::Handle {
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

/// Lifecycle status of an [`SshSession`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Connecting,
    Connected,
    Failed(String),
    Closed,
}

/// Events an [`SshSession`] fires up to its owner (`AppState`, ssh-v3). `SshSession`
/// deliberately never touches `Store` itself — session.rs's whole surface is `sid_core`
/// SSH trait types plus this crate's constructors, no store/scope knowledge, matching
/// the plan's "keep session.rs store-free" ownership split — so persisting the flipped
/// dock side to `Settings.file_browser_side` is `AppState`'s job; this event is just the
/// notification that the header's `⇄ dock` control was clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshSessionEvent {
    ToggleDockSide,
}

impl EventEmitter<SshSessionEvent> for SshSession {}

// Only the writer is ever shared/locked — the reader is owned outright by
// `start_read_loop`'s single task (moved in by value), so it needs no `Arc`/lock at
// all. That asymmetry is the fix for the mutex-across-await freeze this module used
// to have: a write awaiting SSH flow-control window could no longer hold a lock the
// read loop needs, because there is no such shared lock anymore.
type SharedShellWriter = Arc<AsyncMutex<Box<dyn SshShellWriter>>>;
type SharedSftp = Arc<AsyncMutex<Box<dyn SftpSession>>>;
type SharedClient = Arc<AsyncMutex<Box<dyn SshClient>>>;

/// A file's preview content (`view`, P5.3) — populated by [`SshSession::view`], rendered
/// as a modal overlay, dismissed by [`SshSession::close_preview`].
#[derive(Clone)]
struct Preview {
    name: String,
    content: PreviewContent,
}

#[derive(Clone)]
enum PreviewContent {
    /// UTF-8 text small enough to show in full, read-only.
    Text(String),
    /// Why the raw bytes aren't shown (too large, binary, or the fetch failed) — the file's
    /// contents themselves are never dumped into the UI.
    Notice(String),
}

/// A live (or connecting/failed/closed) SSH session: one adapter-backed client with a shell
/// channel feeding a [`TerminalScreen`] (the terminal pane) and an SFTP channel feeding a
/// cached directory listing (the file panel) — MobaXterm-style, over the same connection.
pub struct SshSession {
    // ---- the one shared connection ------------------------------------------------------
    client: Option<SharedClient>,
    status: SessionStatus,

    // ---- shell / terminal (Plan 3C) -----------------------------------------------------
    screen: Box<dyn TerminalScreen>,
    /// The shell's write half only — `send_input`/`resize`/`disconnect` need mutual
    /// exclusion among themselves, but must never serialize against the read loop, which
    /// owns the read half outright (see [`Self::start_read_loop`]).
    shell: Option<SharedShellWriter>,
    rows: u16,
    cols: u16,
    focus_handle: FocusHandle,
    /// Set once, on the render after a successful connect, to pull keyboard focus onto the
    /// terminal without re-stealing it on every later re-render (e.g. output arriving while
    /// the user is focused elsewhere).
    needs_focus: bool,

    // ---- sftp / files (Plan 3.4) ---------------------------------------------------------
    sftp: Option<SharedSftp>,
    /// The current directory's absolute path, as resolved by the server (never a bare `"."`).
    path: String,
    entries: Vec<SftpEntry>,
    /// The last file-panel operation's status or error — distinct from `status`, which is
    /// the connection's own lifecycle. A listing failure here does not fail the session:
    /// the terminal keeps working even if e.g. the home directory can't be read.
    file_error: Option<String>,
    /// Split-layout collapse toggle (P5.2): no draggable divider is cheaply available in
    /// gpui 0.2.2, so per the plan's fallback the sidebar is fixed-width with a `«`/`»`
    /// collapse control instead, letting the terminal reclaim the full pane when browsing
    /// files isn't needed.
    sidebar_collapsed: bool,
    /// Hidden-files toggle: when `false`, dotfile entries (`.config`, `.cache`, …) are
    /// filtered out of the rendered listing by [`filter_hidden`]. Session-local UI state only
    /// — not persisted, not part of the layered store. Defaults `true` (show hidden) to
    /// preserve the listing's prior behavior for anyone who hasn't touched the toggle.
    show_hidden: bool,
    /// The "go to path" toolbar field (P5.3) — navigates the whole remote filesystem, not
    /// just child directories.
    goto_input: Entity<TextInput>,
    /// `view`'s open preview, if any (P5.3).
    preview: Option<Preview>,
    /// Which side of the terminal the file sidebar renders on (ssh-v3). Initialized from
    /// `Settings.file_browser_side` by whoever calls [`Self::open`]; `AppState` pushes
    /// updates to every live session via [`Self::set_dock_side`] when the header's
    /// `⇄ dock` control flips the (global, persisted) setting — see [`SshSessionEvent`].
    dock_side: PanelSide,
}

impl SshSession {
    /// Spawn the connect: build params (3C's `connect_params`) → open a client →
    /// `connect` **once** → `open_shell` + `open_sftp` on that same client → store both,
    /// start the shell's read-loop, and resolve+list the home directory. Any
    /// connect/shell/sftp-open failure lands in `status` as `Failed`; a failure to
    /// resolve or list the home directory is softer — it only sets `file_error`, since
    /// the terminal is still perfectly usable without it.
    ///
    /// `secret` is the host's secret, already resolved by the caller
    /// (`AppState::connect_host`/`finish_connect`) — round-D §A moved that resolve step
    /// up a level so a `Password`-auth host with nothing concretely resolvable can open
    /// the connect-time password prompt instead of landing here at all; this
    /// constructor no longer touches the secret store itself.
    pub fn open(
        host: Host,
        secret: Result<Option<Vec<u8>>, String>,
        known_hosts_path: PathBuf,
        dock_side: PanelSide,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let mut session = SshSession {
                client: None,
                status: SessionStatus::Connecting,
                screen: Box::new(Vt100Screen::new(DEFAULT_ROWS, DEFAULT_COLS)),
                shell: None,
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                focus_handle: cx.focus_handle(),
                needs_focus: false,
                sftp: None,
                path: "/".to_string(),
                entries: Vec::new(),
                file_error: None,
                sidebar_collapsed: false,
                show_hidden: true,
                goto_input: cx.new(|cx| TextInput::new(cx, "/path/to/go")),
                preview: None,
                dock_side,
            };
            session.start_connect(host, secret, known_hosts_path, cx);
            session
        })
    }

    pub fn status(&self) -> &SessionStatus {
        &self.status
    }

    /// The terminal grid's own [`FocusHandle`] (keyboard-driven system, 2026-07-02
    /// plan) — `app.rs`'s root key dispatcher compares this against `window.focused(cx)`
    /// to decide [`crate::keymap::FocusContext`]. Identical to [`Focusable::focus_handle`]
    /// (this session has exactly one focus handle, the terminal's), named explicitly so
    /// the call site reads as "is the terminal focused" rather than "is this session
    /// entity focused" (there being only one thing to focus here today doesn't mean
    /// there always will be).
    pub fn terminal_focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
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
            type Quad = (
                Box<dyn SshClient>,
                Box<dyn SshShellReader>,
                Box<dyn SshShellWriter>,
                Box<dyn SftpSession>,
            );
            let connect_outcome: Result<Quad, String> = async {
                let secret = secret?;
                let (spec, auth) = connect_params(&host, secret)?;
                let factory = RusshClientFactory::new(known_hosts_path);
                let mut client: Box<dyn SshClient> = Box::new(factory.new_client());
                let handle = ssh_runtime().spawn(async move {
                    client.connect(&spec, &auth).await?;
                    // Both channels open on the *same* client — one connection, one auth.
                    let (shell_reader, shell_writer) =
                        client.open_shell("xterm-256color", rows, cols).await?;
                    let sftp = client.open_sftp().await?;
                    Ok::<_, SshError>((client, shell_reader, shell_writer, sftp))
                });
                match handle.await {
                    Ok(Ok(quad)) => Ok(quad),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(join_err) => Err(format!("connect task panicked: {join_err}")),
                }
            }
            .await;

            let (client, shell_reader, shell_writer, sftp) = match connect_outcome {
                Ok(quad) => quad,
                Err(err) => {
                    let _ = this.update(cx, |session, cx| {
                        session.status = SessionStatus::Failed(err);
                        cx.notify();
                    });
                    return;
                }
            };

            // The shell/sftp channels are live; mark the session Connected before the
            // (best-effort) initial listing, so a slow or failing `list` never blocks the
            // terminal from being usable. Only the writer goes behind a shared lock — the
            // reader is handed straight to `start_read_loop`, which moves it into its own
            // task by value (see that method's doc comment).
            let _ = this.update(cx, |session, cx| {
                session.client = Some(Arc::new(AsyncMutex::new(client)));
                session.shell = Some(Arc::new(AsyncMutex::new(shell_writer)));
                session.status = SessionStatus::Connected;
                session.needs_focus = true;
                session.start_read_loop(shell_reader, cx);
                cx.notify();
            });

            // Resolve `"."` to the real home directory (SFTP servers resolve it per-user —
            // never assume a literal path), then list it. Failure here only sets
            // `file_error`; it never re-fails `status`.
            let mut sftp = sftp;
            let listing = ssh_runtime()
                .spawn(async move {
                    let home = sftp
                        .canonicalize(".")
                        .await
                        .unwrap_or_else(|_| "/".to_string());
                    let entries = sftp.list(&home).await;
                    (sftp, home, entries)
                })
                .await;

            let _ = this.update(cx, |session, cx| {
                match listing {
                    Ok((sftp, home, Ok(mut entries))) => {
                        sort_entries(&mut entries);
                        session.sftp = Some(Arc::new(AsyncMutex::new(sftp)));
                        session.path = home;
                        session.entries = entries;
                    }
                    Ok((sftp, home, Err(e))) => {
                        session.sftp = Some(Arc::new(AsyncMutex::new(sftp)));
                        session.path = home;
                        session.file_error = Some(e.to_string());
                    }
                    Err(join_err) => {
                        session.file_error = Some(format!("sftp init task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Poll the shell's read half on its own dedicated task, which owns it outright by
    /// value — no `Arc`/lock at all, since nothing else ever touches it. This is the
    /// other half of the mutex-across-await fix: before the `SshShell` trait split, this
    /// loop shared one lock with `send_input`/`resize`, so a write awaiting SSH
    /// flow-control window (e.g. mid-paste on a congested link) held that lock for its
    /// whole `.await` and starved this loop — a real terminal freeze. Now the reader has
    /// no lock to be starved behind.
    fn start_read_loop(&mut self, reader: Box<dyn SshShellReader>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut reader = reader;
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                // `SshShellReader::try_read` always returns `Ok` (it just drains a buffer —
                // it never learns the channel closed), so `disconnect()` setting `status` is
                // the only signal this loop gets; check it before every read rather than
                // relying on the read ever erroring out. Without this, disconnecting while
                // some other strong `Entity<SshSession>` handle keeps the session alive would
                // leak this task polling forever.
                let still_connected = this
                    .update(cx, |session, _cx| {
                        session.status == SessionStatus::Connected
                    })
                    .unwrap_or(false);
                if !still_connected {
                    // Dropping `reader` here runs `RusshShellReader::Drop`, which aborts its
                    // background pump task — the read loop's own shutdown is what triggers
                    // the adapter-internal one.
                    return;
                }
                // Hand `reader` to the ssh_runtime task by value and get it back alongside
                // the read's result — the same "loan it out, take it back" shape the
                // sftp-listing task below already uses — so the loop keeps sole ownership
                // across ticks with no lock in between.
                let read = ssh_runtime().spawn(async move {
                    let result = reader.try_read().await;
                    (reader, result)
                });
                let (returned_reader, bytes) = match read.await {
                    Ok((reader, Ok(bytes))) => (reader, bytes),
                    Ok((_, Err(_))) | Err(_) => {
                        // Adapter error or a panicked join — the session is done either way.
                        let _ = this.update(cx, |session, cx| {
                            session.status = SessionStatus::Closed;
                            cx.notify();
                        });
                        return;
                    }
                };
                reader = returned_reader;
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

    /// Send raw bytes to the remote shell (keystrokes turn into these). Fire-and-forget: a
    /// write failure surfaces on the next read-loop tick as a closed session.
    pub fn send_input(&self, bytes: Vec<u8>) {
        let Some(shell) = self.shell.clone() else {
            return;
        };
        ssh_runtime().spawn(async move {
            let _ = shell.lock().await.write(&bytes).await;
        });
    }

    /// Recompute the pane size (on viewport change) and push it to both the PTY and the
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

    /// Apply a (globally) flipped dock side — called by `AppState` on every live session
    /// after it persists the flip to `Settings.file_browser_side`, so all open tabs stay
    /// in sync with the one setting, not just the tab whose header was clicked.
    pub fn set_dock_side(&mut self, side: PanelSide, cx: &mut Context<Self>) {
        if self.dock_side != side {
            self.dock_side = side;
            cx.notify();
        }
    }

    /// Gracefully close everything this session opened, in one shot: the shell, the SFTP
    /// channel, then the client itself (the connection they were multiplexed over). The
    /// session's `← disconnect` control.
    pub fn disconnect(&mut self) {
        self.status = SessionStatus::Closed;
        let shell = self.shell.take();
        let sftp = self.sftp.take();
        let client = self.client.take();
        ssh_runtime().spawn(async move {
            if let Some(shell) = shell {
                let _ = shell.lock().await.close().await;
            }
            if let Some(sftp) = sftp {
                let _ = sftp.lock().await.close().await;
            }
            if let Some(client) = client {
                let _ = client.lock().await.disconnect().await;
            }
        });
    }

    // ---- file-panel navigation (reused/adapted from Plan 3.4's SftpBrowser) --------------

    /// Re-list `path` over the existing session and, on success, make it current. A failed
    /// navigate leaves `path`/`entries` untouched — a bad click doesn't blank the view — and
    /// surfaces the failure in `file_error` instead.
    fn navigate(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(sftp) = self.sftp.clone() else {
            return;
        };
        self.file_error = None;
        let list_path = path.clone();
        cx.spawn(async move |this, cx| {
            let handle =
                ssh_runtime().spawn(async move { sftp.lock().await.list(&list_path).await });
            let result = handle.await;
            let _ = this.update(cx, |session, cx| {
                match result {
                    Ok(Ok(mut entries)) => {
                        sort_entries(&mut entries);
                        session.path = path;
                        session.entries = entries;
                    }
                    Ok(Err(e)) => session.file_error = Some(e.to_string()),
                    Err(join_err) => {
                        session.file_error = Some(format!("list task panicked: {join_err}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Navigate into a child directory of the current path (an entry-row click).
    fn enter_dir(&mut self, name: &str, cx: &mut Context<Self>) {
        let target = abs_remote_path(&self.path, name);
        self.navigate(target, cx);
    }

    /// `↑ up`: navigate to the current path's parent.
    fn go_up(&mut self, cx: &mut Context<Self>) {
        let target = parent_path(&self.path);
        self.navigate(target, cx);
    }

    /// Jump directly to `path` — a breadcrumb segment click or the go-to-path field.
    fn go_to(&mut self, path: String, cx: &mut Context<Self>) {
        self.navigate(path, cx);
    }

    /// `⟳ refresh`: re-list the current path.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self.navigate(path, cx);
    }

    /// Read the "go to path" field and navigate there. A bare (non-absolute) entry is rooted
    /// (`etc` -> `/etc`) — the field navigates the filesystem, not the current directory.
    fn goto_submit(&mut self, cx: &mut Context<Self>) {
        let target = self.goto_input.read(cx).content().trim().to_string();
        if target.is_empty() {
            return;
        }
        let target = if target.starts_with('/') {
            target
        } else {
            format!("/{target}")
        };
        self.navigate(target, cx);
    }

    // ---- per-file actions (P5.3) -----------------------------------------------------------

    /// `⭳ download`: fetch `name` (a file in the current directory) and write it to
    /// `$HOME/Downloads/<name>`. `name` is untrusted (a malicious or compromised SFTP server
    /// controls `list()` results), so the local write path is derived via [`safe_local_name`]
    /// rather than from `name` directly — see that function's doc comment.
    fn download(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(sftp) = self.sftp.clone() else {
            return;
        };
        let remote_path = abs_remote_path(&self.path, &name);
        self.file_error = None;
        cx.spawn(async move |this, cx| {
            let result: Result<PathBuf, String> = async {
                let local_name = safe_local_name(&name)
                    .ok_or_else(|| format!("refusing unsafe remote filename: {name:?}"))?;
                let bytes = ssh_runtime()
                    .spawn(async move { sftp.lock().await.get(&remote_path).await })
                    .await
                    .map_err(|e| format!("download task panicked: {e}"))?
                    .map_err(|e| e.to_string())?;
                let dir = downloads_dir();
                std::fs::create_dir_all(&dir)
                    .map_err(|e| format!("create {}: {e}", dir.display()))?;
                let dest = dir.join(&local_name);
                // Defense in depth: `safe_local_name` already guarantees a bare, single
                // component, but re-check the joined path never left `dir` before writing.
                if dest.parent() != Some(dir.as_path()) {
                    return Err(format!(
                        "refusing unsafe download destination: {}",
                        dest.display()
                    ));
                }
                std::fs::write(&dest, &bytes)
                    .map_err(|e| format!("write {}: {e}", dest.display()))?;
                Ok(dest)
            }
            .await;
            let _ = this.update(cx, |session, cx| {
                session.file_error = Some(match result {
                    Ok(dest) => format!("downloaded to {}", dest.display()),
                    Err(e) => e,
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// `view`: fetch `name` and, if it's small enough (<= [`PREVIEW_MAX_BYTES`]) and valid
    /// UTF-8, show it read-only in the preview overlay. Never renders raw bytes: too-large or
    /// non-UTF-8 content gets a notice pointing at `⭳ download` instead.
    // ponytail: text preview only; no image/hex viewer yet.
    fn view(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(sftp) = self.sftp.clone() else {
            return;
        };
        let remote_path = abs_remote_path(&self.path, &name);
        cx.spawn(async move |this, cx| {
            let result = ssh_runtime()
                .spawn(async move { sftp.lock().await.get(&remote_path).await })
                .await;
            let content = match result {
                Ok(Ok(bytes)) if bytes.len() > PREVIEW_MAX_BYTES => PreviewContent::Notice(
                    format!("{name}: too large to preview (> 1 MiB) — download instead"),
                ),
                Ok(Ok(bytes)) => match String::from_utf8(bytes) {
                    Ok(text) => PreviewContent::Text(text),
                    Err(_) => {
                        PreviewContent::Notice(format!("{name}: binary file — download instead"))
                    }
                },
                Ok(Err(e)) => PreviewContent::Notice(format!("{name}: {e}")),
                Err(join_err) => {
                    PreviewContent::Notice(format!("{name}: view task panicked: {join_err}"))
                }
            };
            let _ = this.update(cx, |session, cx| {
                session.preview = Some(Preview { name, content });
                cx.notify();
            });
        })
        .detach();
    }

    /// `⧉ copy path`: put the entry's absolute remote path — never its contents — on the
    /// system clipboard. Valid for files *and* directories.
    fn copy_path(&mut self, path: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path));
    }

    /// Dismiss the preview overlay (`✕ close`).
    fn close_preview(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        cx.notify();
    }

    /// The entries actually rendered in the listing: `self.entries` filtered through
    /// [`filter_hidden`] by the hidden-files toggle. Recomputed on demand rather than cached —
    /// the source list is one directory's worth of entries, cheap to re-filter, and this way
    /// `show_hidden` needs no separate invalidation path.
    fn visible_entries(&self) -> Vec<&SftpEntry> {
        filter_hidden(&self.entries, self.show_hidden)
    }
}

// ---- pure path logic + entry ordering (reused/adapted from Plan 3.4's sftp.rs) -----------

/// Join `dir` (an absolute POSIX-style directory path) with a single path component. Renamed
/// from 3.4's `join_path` — same logic (a `dir == "/"` special case avoids a doubled slash),
/// carried over to this module's `path`/`entries` fields instead of a separate browser's.
fn abs_remote_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The parent of an absolute POSIX-style path. The root is its own parent (there is nowhere
/// further up to navigate); everything else strips its final component.
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
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
}

/// Filter a directory listing for display: when `show_hidden` is `false`, drop entries whose
/// name starts with `.` (dotfiles, e.g. `.config`/`.cache`). `.` and `..` get no special-case
/// exemption — they're just entries whose name happens to start with `.`, so the same rule
/// hides (or shows) them as any other dotfile. When `show_hidden` is `true`, every entry
/// passes through unchanged. Pure and non-owning: this borrows from the already-cached
/// `entries`, never triggers a fresh SFTP call.
fn filter_hidden(entries: &[SftpEntry], show_hidden: bool) -> Vec<&SftpEntry> {
    entries
        .iter()
        .filter(|e| show_hidden || !e.name.starts_with('.'))
        .collect()
}

/// Reduce an untrusted remote filename (an `SftpEntry.name`, as returned by whatever server
/// we're talking to) to a safe bare local filename: the final path component only, no
/// directories, no traversal. `None` if there's no usable name — the caller must refuse the
/// download rather than fall back to something guessed.
///
/// This is the one thing standing between a hostile/compromised SFTP server and writing
/// outside the local downloads directory: `list()` results are attacker-controlled data, and a
/// name like `"../../.bashrc"` must never reach `downloads_dir().join(name)` as-is.
fn safe_local_name(remote_name: &str) -> Option<String> {
    let comp = std::path::Path::new(remote_name).file_name()?.to_str()?;
    if comp.is_empty() || comp == "." || comp == ".." {
        return None;
    }
    Some(comp.to_string())
}

/// The user's `Downloads` directory: `$HOME/Downloads`. No XDG `user-dirs.dirs` parsing —
/// matches the plan's "or `$HOME/Downloads`" fallback rather than pulling in a `dirs` crate for
/// one path.
fn downloads_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Downloads")
}

/// A human-readable byte count (`"512 B"`, `"12.3 KB"`, …). Display-only — not in the tested
/// surface (path/sort/traversal-guard logic only, per the plan's pragmatic-TDD rule).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// Format a Unix `mtime_secs` as a bare `YYYY-MM-DD` (UTC; no timezone/locale support — good
/// enough for a browse view). Date-only rather than `YYYY-MM-DD HH:MM`: at the file sidebar's
/// fixed `SIDEBAR_WIDTH`, a full timestamp doesn't fit cleanly next to the name/size columns
/// and the per-row action buttons — it either clips mid-glyph or crushes the name column to
/// nothing — so the modified column trades the time-of-day for a width that always fits.
/// Display-only, same as `human_size`.
fn format_mtime(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days`: days-since-epoch (1970-01-01) -> (year, month, day). A
/// well-known, publicly documented algorithm — not reimplemented from scratch here.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

// ---- rendering: split layout (file sidebar + terminal) -----------------------------------

impl SshSession {
    /// The `Connected` view: file sidebar (fixed `SIDEBAR_WIDTH`) beside the terminal grid,
    /// filling whatever space the parent gives it — the MobaXterm-style split. Docks left
    /// or right per `self.dock_side` (ssh-v3's `⇄ dock` toggle).
    fn render_split(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.file_sidebar(cx).into_any_element();
        let terminal = div()
            .flex_1()
            .size_full()
            .child(self.render_grid(window, cx))
            .into_any_element();
        let (first, second) = match self.dock_side {
            PanelSide::Left => (sidebar, terminal),
            PanelSide::Right => (terminal, sidebar),
        };
        div()
            .flex()
            .flex_row()
            .size_full()
            .child(first)
            .child(second)
    }

    /// The file panel: the [`toolbar`](Self::toolbar) above a scrollable, read-only listing
    /// (filtered by the hidden-files toggle), with the last file-panel error (if any) between
    /// them. Painted purely from `self.entries`/`self.path`/`self.show_hidden` — every SFTP
    /// call that could change them already ran, off gpui's executor, before `cx.notify()`
    /// scheduled this render.
    fn file_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (bg, border, muted) = (t.bg, t.border, t.muted);
        if self.sidebar_collapsed {
            return div()
                .id("session-sidebar-expand")
                .w(px(20.))
                .h_full()
                .flex()
                .pt_1()
                .justify_center()
                .cursor_pointer()
                .bg(rgb(bg))
                .border_r_1()
                .border_color(rgb(border))
                .text_color(rgb(muted))
                .child("»")
                .on_click(cx.listener(|session, _ev: &ClickEvent, _window, cx| {
                    session.sidebar_collapsed = false;
                    cx.notify();
                }))
                .into_any_element();
        }

        let visible = self.visible_entries();
        let count = visible.len();
        let divider = match self.dock_side {
            PanelSide::Left => div().border_r_1(),
            PanelSide::Right => div().border_l_1(),
        };
        divider
            .w(SIDEBAR_WIDTH)
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(bg))
            .border_color(rgb(border))
            .child(self.sidebar_header(cx))
            .child(self.toolbar(cx))
            .when_some(self.file_error.clone(), |el, msg| {
                el.child(status_line(&format!("file panel: {msg}"), cx))
            })
            .child(
                uniform_list(
                    "session-sftp-entries",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _win, cx| {
                        let visible = this.visible_entries();
                        range
                            .map(|ix| this.entry_row(visible[ix], ix, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1(),
            )
            .into_any_element()
    }

    /// The sidebar's title row (ssh-v3): a "Files" label plus the `⇄ dock` control that
    /// flips which side of the terminal this panel renders on. Doesn't touch `Store`
    /// itself — clicking it just emits [`SshSessionEvent::ToggleDockSide`]; `AppState`
    /// persists the flip to `Settings.file_browser_side` and fans it out to every open
    /// session tab (see [`Self::set_dock_side`]).
    fn sidebar_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (border, muted, selection) = (t.border, t.muted, t.selection);
        let label = match self.dock_side {
            PanelSide::Left => "⇄ dock right",
            PanelSide::Right => "⇄ dock left",
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(border))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(muted))
                    .child("FILES"),
            )
            .child(
                div()
                    .id("session-dock-toggle")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(muted))
                    .hover(|s| s.bg(rgb(selection)))
                    .child(label)
                    .on_click(cx.listener(|_session, _ev: &ClickEvent, _window, cx| {
                        cx.emit(SshSessionEvent::ToggleDockSide);
                    })),
            )
    }

    /// Toolbar: three stacked rows, each of which fits [`SIDEBAR_WIDTH`] on its own with no
    /// overlap — cramming the breadcrumb, path field, nav icons, and entry count onto fewer,
    /// wider rows is what caused them to pile on top of each other at 320px.
    ///
    /// - Row 1: the breadcrumb (flexes, wraps onto multiple lines if long) plus the `«`
    ///   collapse control (fixed).
    /// - Row 2: the go-to-path field (flexes) plus `Go` (fixed).
    /// - Row 3: `↑ up` / `⟳ refresh` / the hidden-files toggle on the left, the entry count
    ///   right-aligned.
    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (border, muted, selection, accent, fg_strong) =
            (t.border, t.muted, t.selection, t.accent, t.fg_strong);
        let icon_button = |id: (&'static str, usize), label: String| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(muted))
                .hover(|s| s.bg(rgb(selection)))
                .child(label)
        };
        let up = icon_button(("session-up", 0), "↑".to_string())
            .on_click(cx.listener(|session, _ev: &ClickEvent, _window, cx| session.go_up(cx)));
        let refresh = icon_button(("session-refresh", 0), "⟳".to_string())
            .on_click(cx.listener(|session, _ev: &ClickEvent, _window, cx| session.refresh(cx)));
        let hidden_mark = if self.show_hidden { "☑" } else { "☐" };
        let hidden_toggle = icon_button(
            ("session-hidden-toggle", 0),
            format!("{hidden_mark} hidden"),
        )
        .on_click(cx.listener(|session, _ev: &ClickEvent, _window, cx| {
            session.show_hidden = !session.show_hidden;
            cx.notify();
        }));
        let go = div()
            .id("session-goto-go")
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .bg(rgb(accent))
            .text_color(rgb(fg_strong))
            .child("Go")
            .on_click(
                cx.listener(|session, _ev: &ClickEvent, _window, cx| session.goto_submit(cx)),
            );
        let collapse = div()
            .id("session-sidebar-collapse")
            .px_2()
            .cursor_pointer()
            .text_color(rgb(muted))
            .child("«")
            .on_click(cx.listener(|session, _ev: &ClickEvent, _window, cx| {
                session.sidebar_collapsed = true;
                cx.notify();
            }));
        let count = self.visible_entries().len();

        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(rgb(border))
            .child(
                // Row 1: breadcrumb + collapse.
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .pt_1()
                    .child(div().flex_1().child(self.breadcrumb(cx)))
                    .child(collapse),
            )
            .child(
                // Row 2: go-to-path field + Go.
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .py_1()
                    // `min_w(0) + overflow_hidden`: `TextInput` paints its shaped line
                    // at its own natural width regardless of the box's flex-assigned
                    // bounds (see `ui::ssh_home`'s quick-connect box for the writeup —
                    // same shared `TextInput`, same fix), so a long typed/placeholder
                    // path would otherwise bleed into the `Go` button beside it in this
                    // fixed, narrow (`SIDEBAR_WIDTH`) toolbar.
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(self.goto_input.clone()),
                    )
                    .child(go),
            )
            .child(
                // Row 3: up / refresh / hidden toggle (left) — entry count (right).
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_1()
                    .pb_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(up)
                            .child(refresh)
                            .child(hidden_toggle),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(muted))
                            .child(format!("{count} entries")),
                    ),
            )
    }

    /// Clickable breadcrumb of the current path's segments — root first, then each component
    /// built up cumulatively (`/a/b` -> `/`, `a` (-> `/a`), `b` (-> `/a/b`)). Wraps onto
    /// multiple lines if the path is longer than the sidebar is wide.
    fn breadcrumb(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut children = vec![self.breadcrumb_segment(0, "/".into(), "/".to_string(), cx)];
        let mut acc = String::new();
        for (ix, part) in self.path.split('/').filter(|s| !s.is_empty()).enumerate() {
            acc.push('/');
            acc.push_str(part);
            children.push(self.breadcrumb_segment(
                ix + 1,
                part.to_string().into(),
                acc.clone(),
                cx,
            ));
        }
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .text_xs()
            .font_family(MONO)
            .children(children)
    }

    fn breadcrumb_segment(
        &self,
        ix: usize,
        label: SharedString,
        target: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (fg, muted, selection) = (t.fg, t.muted, t.selection);
        let is_current = target == self.path;
        div()
            .id(("session-crumb", ix))
            .px_1()
            .rounded_md()
            .cursor_pointer()
            .text_color(rgb(if is_current { fg } else { muted }))
            .hover(|s| s.bg(rgb(selection)))
            .child(label)
            .on_click(cx.listener(move |session, _ev: &ClickEvent, _window, cx| {
                session.go_to(target.clone(), cx);
            }))
    }

    /// One row of the entry list: glyph, name, size, mtime, and per-row actions. `entry` comes
    /// from [`Self::visible_entries`] — already filtered by the hidden-files toggle — and `ix`
    /// is that filtered list's position, used only to keep each row's element ids unique and
    /// to alternate row shading; it is not an index into `self.entries`. Directories are
    /// entered by clicking their *name* specifically — not the whole row — so that click
    /// target sits as a sibling next to the action buttons rather than an ancestor around
    /// them; each button keeps its own independent hitbox and there is no nested-click
    /// ambiguity to resolve.
    fn entry_row(
        &self,
        entry: &SftpEntry,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (fg, muted, selection, accent, surface, bg) =
            (t.fg, t.muted, t.selection, t.accent, t.surface, t.bg);
        let name = entry.name.clone();
        let is_dir = entry.is_dir;
        let glyph = if is_dir { "▸" } else { "·" };
        let size = if is_dir {
            "—".to_string()
        } else {
            human_size(entry.size)
        };
        let mtime = format_mtime(entry.mtime_secs);
        let alt = ix % 2 == 1;
        let abs_path = abs_remote_path(&self.path, &name);

        let name_el = if is_dir {
            let enter_name = name.clone();
            div()
                .id(("session-entry-name", ix))
                .flex_1()
                .truncate()
                .cursor_pointer()
                .text_color(rgb(fg))
                .hover(|s| s.text_color(rgb(accent)))
                .child(name.clone())
                .on_click(cx.listener(move |session, _ev: &ClickEvent, _window, cx| {
                    session.enter_dir(&enter_name, cx);
                }))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .truncate()
                .text_color(rgb(fg))
                .child(name.clone())
                .into_any_element()
        };

        let action_button = |id: (&'static str, usize), label: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(accent))
                .hover(|s| s.bg(rgb(selection)))
                .child(label)
        };

        // Files get `view` + `⭳ download`; directories don't (nothing to fetch/preview).
        let file_buttons = (!is_dir).then(|| {
            let view_name = name.clone();
            let download_name = name.clone();
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    action_button(("session-view", ix), "view").on_click(cx.listener(
                        move |session, _ev: &ClickEvent, _window, cx| {
                            session.view(view_name.clone(), cx)
                        },
                    )),
                )
                .child(
                    action_button(("session-download", ix), "⭳").on_click(cx.listener(
                        move |session, _ev: &ClickEvent, _window, cx| {
                            session.download(download_name.clone(), cx)
                        },
                    )),
                )
        });

        // `⧉ copy path` applies to files *and* directories.
        let copy_path_button = action_button(("session-copy-path", ix), "⧉").on_click(cx.listener(
            move |session, _ev: &ClickEvent, _window, cx| {
                session.copy_path(abs_path.clone(), cx);
            },
        ));

        div()
            .id(("session-entry", ix))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .px_3()
            .py_2()
            .text_sm()
            .bg(rgb(if alt { surface } else { bg }))
            .child(div().w(px(14.)).text_color(rgb(muted)).child(glyph))
            .child(name_el)
            .child(
                div()
                    .w(px(60.))
                    .truncate()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(muted))
                    .child(size),
            )
            .child(
                div()
                    .w(px(84.))
                    .truncate()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(muted))
                    .child(mtime),
            )
            .children(file_buttons)
            .child(copy_path_button)
    }

    /// Paint the terminal grid: one `shape_line` call per row (gpui shapes a whole
    /// multi-run line at once, so the row — not the cell — is the unit of work), then
    /// `paint_background` + `paint` per shaped row inside a `canvas`. The canvas fills
    /// whatever space the parent layout gives it; the resize detection below reads that
    /// real size back out of the canvas's own paint bounds and reconciles
    /// `self.rows`/`self.cols` against it.
    fn render_grid(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        // The terminal viewport is a recessed editor-like surface (spec: "input/editor/
        // terminal backgrounds -> well"), not the general window/panel plane — it reads
        // as visually inset from the chrome around it.
        let default_fg: Hsla = rgb(t.fg).into();
        let default_bg: Hsla = rgb(t.well).into();
        let cells = self.screen.cells();
        let (cursor_row, cursor_col) = self.screen.cursor_position();
        let base_font = font(MONO);

        // Measure one monospace glyph — its width/the line height are the grid's cell size,
        // used both to paint rows and (in the canvas below) to turn the pane's real pixel
        // bounds back into a rows/cols count.
        let text_system = window.text_system().clone();
        let em = text_system.shape_line(
            "M".into(),
            TERM_FONT_SIZE,
            &[TextRun {
                len: 1,
                font: base_font.clone(),
                color: default_fg,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let cell_width = em.width;
        let line_height = window.line_height();

        let shaped_rows: Vec<ShapedLine> = cells
            .iter()
            .enumerate()
            .map(|(row_ix, row)| {
                let col = (row_ix as u16 == cursor_row).then_some(cursor_col as usize);
                shape_row(
                    &text_system,
                    row,
                    col,
                    &base_font,
                    TERM_FONT_SIZE,
                    default_fg,
                    default_bg,
                )
            })
            .collect();

        let current_size = (self.rows, self.cols);
        let weak = cx.weak_entity();

        div()
            .size_full()
            .bg(default_bg)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|session, event: &KeyDownEvent, window, _cx| {
                if let Some(bytes) = key_to_bytes(&event.keystroke) {
                    session.send_input(bytes);
                    window.prevent_default();
                }
            }))
            .child(
                canvas(
                    move |bounds, _window, cx| {
                        // Reconcile the pane's real pixel size against the PTY's rows/cols —
                        // deferred, since we're mid-paint of this very entity and cannot
                        // `update` it from inside its own prepaint closure.
                        let cols = ((bounds.size.width / cell_width).floor() as u16).max(1);
                        let rows = ((bounds.size.height / line_height).floor() as u16).max(1);
                        if (rows, cols) != current_size {
                            let weak = weak.clone();
                            cx.defer(move |cx| {
                                let _ = weak.update(cx, |session, cx| {
                                    session.resize(rows, cols);
                                    cx.notify();
                                });
                            });
                        }
                        shaped_rows
                    },
                    move |bounds, shaped_rows: Vec<ShapedLine>, window, cx| {
                        let mut y = bounds.top();
                        for line in &shaped_rows {
                            let origin = point(bounds.left(), y);
                            let _ = line.paint_background(origin, line_height, window, cx);
                            let _ = line.paint(origin, line_height, window, cx);
                            y += line_height;
                        }
                    },
                )
                .size_full(),
            )
    }

    /// `view`'s modal overlay — `None` when nothing is being previewed. Mirrors app.rs's
    /// host-form overlay: `anchored` pins a viewport-sized, occluding backdrop at the window
    /// origin, `deferred` paints it above everything else.
    fn preview_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let t = theme::active(cx);
        let (fg, muted, surface, border, selection) =
            (t.fg, t.muted, t.surface, t.border, t.selection);
        let preview = self.preview.clone()?;
        let viewport = window.viewport_size();

        let body = match preview.content {
            PreviewContent::Text(text) => div()
                .id("session-preview-body")
                .flex_1()
                .overflow_y_scroll()
                .p_3()
                .text_sm()
                .font_family(MONO)
                .text_color(rgb(fg))
                .child(text)
                .into_any_element(),
            PreviewContent::Notice(msg) => div()
                .flex_1()
                .p_3()
                .text_sm()
                .text_color(rgb(muted))
                .child(msg)
                .into_any_element(),
        };

        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(rgba(0x000000a8))
                        .child(
                            div()
                                .w(px(640.))
                                .h(px(480.))
                                .flex()
                                .flex_col()
                                .bg(rgb(surface))
                                .border_1()
                                .border_color(rgb(border))
                                .rounded_md()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .px_3()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(rgb(border))
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(fg))
                                                .child(preview.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .id("session-preview-close")
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .cursor_pointer()
                                                .text_color(rgb(muted))
                                                .hover(|s| s.bg(rgb(selection)))
                                                .child("✕ close")
                                                .on_click(cx.listener(
                                                    |session, _ev: &ClickEvent, _window, cx| {
                                                        session.close_preview(cx);
                                                    },
                                                )),
                                        ),
                                )
                                .child(body),
                        ),
                ),
            )
            .with_priority(1),
        )
    }
}

impl Render for SshSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus {
            self.needs_focus = false;
            window.focus(&self.focus_handle);
        }
        let content = match &self.status {
            SessionStatus::Connecting => message_pane("Connecting…", cx).into_any_element(),
            SessionStatus::Failed(err) => {
                message_pane(&format!("Connection failed: {err}"), cx).into_any_element()
            }
            SessionStatus::Closed => message_pane("Session closed.", cx).into_any_element(),
            SessionStatus::Connected => self.render_split(window, cx).into_any_element(),
        };
        let overlay = self.preview_overlay(window, cx);
        div().size_full().child(content).children(overlay)
    }
}

impl Focusable for SshSession {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Map one keystroke to the bytes sent to the remote shell. `key_char` (what the platform
/// says was actually typed, after shift/IME/etc.) covers ordinary printable input; the
/// control keys a terminal depends on are matched on the keystroke's named `key` — checked
/// first, since e.g. a ctrl-chord's `key_char` (if any) is not what a shell expects to see.
fn key_to_bytes(keystroke: &Keystroke) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let m = &keystroke.modifiers;

    if m.control && !m.alt && !m.platform {
        let mut chars = key.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c.is_ascii_alphabetic()
        {
            return Some(vec![(c.to_ascii_uppercase() as u8) & 0x1f]);
        }
    }

    match key {
        "enter" => return Some(b"\r".to_vec()),
        "backspace" => return Some(vec![0x7f]),
        "tab" => return Some(b"\t".to_vec()),
        "escape" => return Some(vec![0x1b]),
        "up" => return Some(b"\x1b[A".to_vec()),
        "down" => return Some(b"\x1b[B".to_vec()),
        "right" => return Some(b"\x1b[C".to_vec()),
        "left" => return Some(b"\x1b[D".to_vec()),
        "home" => return Some(b"\x1b[H".to_vec()),
        "end" => return Some(b"\x1b[F".to_vec()),
        "delete" => return Some(b"\x1b[3~".to_vec()),
        _ => {}
    }

    keystroke.key_char.as_ref().map(|s| s.as_bytes().to_vec())
}

fn message_pane(text: &str, cx: &App) -> impl IntoElement {
    let t = theme::active(cx);
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(t.bg))
        .text_color(rgb(t.muted))
        .font_family(MONO)
        .child(text.to_string())
}

fn status_line(text: &str, cx: &App) -> impl IntoElement {
    let muted = theme::active(cx).muted;
    div()
        .px_3()
        .py_1()
        .text_xs()
        .text_color(rgb(muted))
        .child(text.to_string())
}

/// Shape one terminal row into a single `ShapedLine`. Contiguous cells sharing the same
/// fg/bg/bold/italic/underline coalesce into one `TextRun` — the row, not the cell, is what
/// gets shaped, matching how `WindowTextSystem::shape_line` is meant to be driven.
fn shape_row(
    text_system: &gpui::WindowTextSystem,
    row: &[TermCell],
    cursor_col: Option<usize>,
    base_font: &Font,
    font_size: Pixels,
    default_fg: Hsla,
    default_bg: Hsla,
) -> ShapedLine {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();

    for (col, cell) in row.iter().enumerate() {
        // A blank cell still occupies a column — render it as a space, like `lines()` does,
        // so run byte-offsets stay aligned with terminal columns.
        let glyph: &str = if cell.text.is_empty() {
            " "
        } else {
            &cell.text
        };

        let mut fg = term_color_to_hsla(cell.fg, default_fg);
        let mut bg = term_color_to_hsla(cell.bg, default_bg);
        if cell.inverse {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cursor_col == Some(col) {
            // Block cursor: swap fg/bg on top of whatever the cell's own styling already is,
            // rather than painting a separate overlay quad.
            std::mem::swap(&mut fg, &mut bg);
        }

        let mut cell_font = base_font.clone();
        if cell.bold {
            cell_font.weight = FontWeight::BOLD;
        }
        if cell.italic {
            cell_font.style = FontStyle::Italic;
        }
        let underline = cell.underline.then(|| UnderlineStyle {
            color: Some(fg),
            thickness: px(1.0),
            wavy: false,
        });

        let byte_len = glyph.len();
        text.push_str(glyph);

        let extends_last = runs.last().is_some_and(|r: &TextRun| {
            r.font == cell_font
                && r.color == fg
                && r.background_color == Some(bg)
                && r.underline == underline
        });
        if extends_last {
            runs.last_mut().unwrap().len += byte_len;
        } else {
            runs.push(TextRun {
                len: byte_len,
                font: cell_font,
                color: fg,
                background_color: Some(bg),
                underline,
                strikethrough: None,
            });
        }
    }

    text_system.shape_line(text.into(), font_size, &runs, None)
}

/// `TermColor::Default` takes the pane's own theme color; `Indexed`/`Rgb` convert to `Hsla`
/// via a plain `0xRRGGBB` pack — gpui already gives us `Rgba: Into<Hsla>`.
fn term_color_to_hsla(color: TermColor, default: Hsla) -> Hsla {
    match color {
        TermColor::Default => default,
        TermColor::Indexed(idx) => {
            let (r, g, b) = xterm256_to_rgb(idx);
            rgb_to_hsla(r, g, b)
        }
        TermColor::Rgb(r, g, b) => rgb_to_hsla(r, g, b),
    }
}

fn rgb_to_hsla(r: u8, g: u8, b: u8) -> Hsla {
    rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into()
}

/// The standard xterm 256-color palette: 0-15 are the base 16 (xterm's own default RGBs,
/// not the VGA ones), 16-231 are a 6x6x6 color cube, and 232-255 are a 24-step grayscale ramp.
fn xterm256_to_rgb(idx: u8) -> (u8, u8, u8) {
    const BASE16: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match idx {
        0..=15 => BASE16[idx as usize],
        16..=231 => {
            let n = idx - 16;
            (
                STEPS[(n / 36) as usize],
                STEPS[((n / 6) % 6) as usize],
                STEPS[(n % 6) as usize],
            )
        }
        232..=255 => {
            let level = 8 + (idx - 232) * 10;
            (level, level, level)
        }
    }
}

#[cfg(test)]
mod key_tests {
    use gpui::Modifiers;

    use super::*;

    fn key(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: key.to_string(),
            key_char: None,
        }
    }

    fn ctrl(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            key: key.to_string(),
            key_char: None,
        }
    }

    #[test]
    fn enter_sends_cr() {
        assert_eq!(key_to_bytes(&key("enter")), Some(b"\r".to_vec()));
    }

    #[test]
    fn ctrl_c_sends_end_of_text() {
        assert_eq!(key_to_bytes(&ctrl("c")), Some(vec![0x03]));
    }

    #[test]
    fn arrows_send_csi_sequences() {
        assert_eq!(key_to_bytes(&key("up")), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_to_bytes(&key("down")), Some(b"\x1b[B".to_vec()));
        assert_eq!(key_to_bytes(&key("left")), Some(b"\x1b[D".to_vec()));
        assert_eq!(key_to_bytes(&key("right")), Some(b"\x1b[C".to_vec()));
    }

    #[test]
    fn printable_char_uses_key_char() {
        let mut k = key("a");
        k.key_char = Some("a".to_string());
        assert_eq!(key_to_bytes(&k), Some(b"a".to_vec()));
    }

    #[test]
    fn bare_modifier_with_no_key_char_is_unhandled() {
        let mut k = key("shift");
        k.modifiers.shift = true;
        assert_eq!(key_to_bytes(&k), None);
    }
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

    // filter_hidden: the hidden-files toggle backing the sidebar checkbox. `.` and `..` get no
    // special-case exemption — this pins that down, so a future edit that carves out an
    // exception for them fails this test rather than silently changing behavior.

    #[test]
    fn filter_hidden_drops_dotfiles_when_off_and_keeps_them_when_on() {
        let entries = vec![
            entry("normal.txt", false),
            entry(".hidden", false),
            entry(".", true),
            entry("..", true),
            entry("visible_dir", true),
        ];

        let shown: Vec<&str> = filter_hidden(&entries, false)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(shown, vec!["normal.txt", "visible_dir"]);

        let shown: Vec<&str> = filter_hidden(&entries, true)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            shown,
            vec!["normal.txt", ".hidden", ".", "..", "visible_dir"]
        );
    }

    #[test]
    fn abs_remote_path_appends_under_a_directory() {
        assert_eq!(abs_remote_path("/home", "a"), "/home/a");
    }

    #[test]
    fn abs_remote_path_under_root_avoids_double_slash() {
        assert_eq!(abs_remote_path("/", "a"), "/a");
    }

    #[test]
    fn parent_path_of_nested_dir_strips_last_component() {
        assert_eq!(parent_path("/a/b"), "/a");
    }

    #[test]
    fn parent_path_of_root_is_root() {
        assert_eq!(parent_path("/"), "/");
    }

    // safe_local_name: the path-traversal guard on downloads. A compromised/malicious SFTP
    // server controls `list()` results, so this is the one place TDD is required beyond the
    // pure path-join/sort logic above.

    #[test]
    fn safe_local_name_strips_relative_traversal_to_the_bare_file() {
        assert_eq!(
            safe_local_name("../../etc/passwd"),
            Some("passwd".to_string())
        );
    }

    #[test]
    fn safe_local_name_strips_absolute_paths_to_the_bare_file() {
        assert_eq!(safe_local_name("/etc/shadow"), Some("shadow".to_string()));
    }

    #[test]
    fn safe_local_name_strips_nested_relative_paths_to_the_bare_file() {
        assert_eq!(safe_local_name("a/b/c.txt"), Some("c.txt".to_string()));
    }

    #[test]
    fn safe_local_name_rejects_dot_dot_dot_and_empty() {
        assert_eq!(safe_local_name(".."), None);
        assert_eq!(safe_local_name("."), None);
        assert_eq!(safe_local_name(""), None);
    }

    #[test]
    fn safe_local_name_passes_through_a_normal_filename() {
        assert_eq!(
            safe_local_name("normal.txt"),
            Some("normal.txt".to_string())
        );
    }
}
