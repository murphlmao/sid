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
    App, AppContext as _, Bounds, ClickEvent, ClipboardItem, Context, Entity, EventEmitter,
    FocusHandle, Focusable, Font, FontStyle, FontWeight, Hsla, IntoElement, KeyDownEvent,
    Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render,
    ShapedLine, SharedString, TextRun, UnderlineStyle, Window, anchored, canvas, deferred, div,
    fill, font, point, prelude::*, px, rgb, rgba, uniform_list,
};
use sid_core::ssh::{SftpEntry, SftpSession, SshClient, SshError, SshShellReader, SshShellWriter};
use sid_core::term::{TermCell, TermColor, TerminalScreen};
use sid_ssh::RusshClientFactory;
use sid_store::{Host, PanelSide};
use sid_term::Vt100Screen;
use tokio::sync::Mutex as AsyncMutex;

use gpui_component::tooltip::Tooltip;

use crate::ssh_connect::connect_params;
use crate::ui::{TextInput, is_field_submit};
use sid_ui::{Row, StyledExt as _, theme, v_flex};

/// Monospace family — kitty parity (Murphy's terminal font, confirmed installed via
/// `fc-list`); gpui falls back to a proportional font if the family is missing locally. This
/// is also what fixes nerd-font ASCII-art rendering in the terminal pane.
const MONO: &str = "CaskaydiaCove Nerd Font Mono";
const TERM_FONT_SIZE: Pixels = px(14.);

/// The split's sizing rules — see [`sidebar_width`] for how they compose.
///
/// A pinned pixel width is an anti-pattern: it is right at exactly one window size and
/// wrong at every other. What replaces it is the pair every responsive split actually
/// needs — **content floors** (what each pane must have to still do its job) and a
/// **proportion** for the space above those floors.
mod sidebar_metrics {
    use gpui::{Pixels, px};

    /// The sidebar's content floor.
    ///
    /// At this width [`super::plan_entry_row`] still seats the name column at its own
    /// floor (`row_metrics::NAME_MIN`, sixteen-odd characters — enough that `.bashrc`
    /// and `.bash_logout` don't render as the same string). The orientation columns
    /// return as the sidebar widens: **size** from 318px, **modified** from 406px, both
    /// of which fall inside this band — which is the reason the band is where it is.
    pub const MIN: Pixels = px(280.);
    /// The sidebar's ceiling. Past this a file list is just wasting the terminal's
    /// space: every column [`super::plan_entry_row`] has to offer already fits.
    pub const MAX: Pixels = px(480.);
    /// The sidebar's share of the split when the user hasn't dragged it.
    pub const RATIO: f32 = 0.25;
    /// The terminal's content floor — roughly forty columns at [`TERM_FONT_SIZE`].
    ///
    /// Deliberately *not* eighty columns: the grid reflows to whatever it is given
    /// (see `render_grid`'s canvas), so this is not "what a terminal needs to be
    /// correct" but "what it needs to still be a shell you can read".
    ///
    /// [`TERM_FONT_SIZE`]: super::TERM_FONT_SIZE
    pub const TERMINAL_MIN: Pixels = px(360.);
    /// The drag divider's hit strip. Wider than the 1px border it draws over, because a
    /// hit target you have to aim at is a hit target you miss.
    pub const DIVIDER: Pixels = px(6.);
}

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

/// A divider drag in progress: where the pointer went down, and how wide the sidebar
/// was at that moment.
///
/// Both are needed because the new width is computed from the *absolute* pointer
/// position (`width_at_grab + (x - grab_x)`, signed by the dock side — see
/// [`dragged_width`]) rather than accumulated per move event. That makes the drag
/// idempotent: a dropped or coalesced move event costs nothing, and the sidebar can
/// never drift away from the pointer over a long drag.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SidebarDrag {
    /// Pointer x at mouse-down, in window coordinates.
    grab_x: Pixels,
    /// The sidebar's rendered width at mouse-down — the *clamped* one, so the drag
    /// starts from the edge the user is actually looking at.
    width_at_grab: Pixels,
}

/// A live (or connecting/failed/closed) SSH session: one adapter-backed client with a shell
/// channel feeding a [`TerminalScreen`] (the terminal pane) and an SFTP channel feeding a
/// cached directory listing (the file panel) — MobaXterm-style, over the same connection.
/// One completed terminal shape pass — see `SshSession::shaped_cache`. Colors are part
/// of the key so a live theme switch (same generation, new palette) reshapes; the
/// per-theme ANSI table always moves together with `fg`/`bg`, so those two suffice.
struct ShapedGridCache {
    generation: u64,
    cursor: (u16, u16),
    fg: Hsla,
    bg: Hsla,
    rows: Vec<ShapedLine>,
    /// Per row: the block-element cells painted procedurally instead of as font
    /// glyphs — see [`block_coverage`] (terminal-fidelity F4).
    quads: Vec<Vec<BlockQuad>>,
    cell_width: Pixels,
    line_height: Pixels,
}

/// One block-element cell to paint as cell-snapped rectangles: the terminal column,
/// the covered fractions of the cell, and the fill (fg, with alpha < 1 for the shade
/// characters). Produced by `shape_row` when a cell's glyph is in U+2580..=U+259F.
#[derive(Clone, Debug)]
struct BlockQuad {
    col: usize,
    rects: &'static [CellRect],
    color: Hsla,
}

/// `(x0, y0, x1, y1)` fractions of one terminal cell, y down.
type CellRect = (f32, f32, f32, f32);

/// The cell-fraction coverage of a Unicode block element (U+2580..=U+259F), plus the
/// fill alpha (1.0 except the ░▒▓ shades). `(x0, y0, x1, y1)` in cell space, y down.
///
/// The kitty-vs-sid A/B showed why these can't be font glyphs: glyph ink rasterized at
/// fractional advances never quite covers the cell, so solid `█▀▄▌▐` art shows hairline
/// seams of background between columns (kitty paints these procedurally for exactly
/// this reason; the A/B's bg-painted rectangle was seam-free, the glyph one wasn't).
/// Box drawing (U+2500..=U+257F) deliberately stays as glyphs — the same A/B showed
/// CaskaydiaCove's box glyphs join cleanly.
///
/// Shades approximate kitty's dither with an alpha wash over the cell background —
/// visually equivalent at cell scale.
fn block_coverage(ch: char) -> Option<(&'static [CellRect], f32)> {
    const FULL: &[CellRect] = &[(0.0, 0.0, 1.0, 1.0)];
    let rects: &'static [CellRect] = match ch {
        '\u{2580}' => &[(0.0, 0.0, 1.0, 0.5)],   // ▀ upper half
        '\u{2581}' => &[(0.0, 0.875, 1.0, 1.0)], // ▁ lower 1/8
        '\u{2582}' => &[(0.0, 0.75, 1.0, 1.0)],  // ▂ lower 1/4
        '\u{2583}' => &[(0.0, 0.625, 1.0, 1.0)], // ▃ lower 3/8
        '\u{2584}' => &[(0.0, 0.5, 1.0, 1.0)],   // ▄ lower half
        '\u{2585}' => &[(0.0, 0.375, 1.0, 1.0)], // ▅ lower 5/8
        '\u{2586}' => &[(0.0, 0.25, 1.0, 1.0)],  // ▆ lower 3/4
        '\u{2587}' => &[(0.0, 0.125, 1.0, 1.0)], // ▇ lower 7/8
        '\u{2588}' => FULL,                      // █ full
        '\u{2589}' => &[(0.0, 0.0, 0.875, 1.0)], // ▉ left 7/8
        '\u{258A}' => &[(0.0, 0.0, 0.75, 1.0)],  // ▊ left 3/4
        '\u{258B}' => &[(0.0, 0.0, 0.625, 1.0)], // ▋ left 5/8
        '\u{258C}' => &[(0.0, 0.0, 0.5, 1.0)],   // ▌ left half
        '\u{258D}' => &[(0.0, 0.0, 0.375, 1.0)], // ▍ left 3/8
        '\u{258E}' => &[(0.0, 0.0, 0.25, 1.0)],  // ▎ left 1/4
        '\u{258F}' => &[(0.0, 0.0, 0.125, 1.0)], // ▏ left 1/8
        '\u{2590}' => &[(0.5, 0.0, 1.0, 1.0)],   // ▐ right half
        '\u{2591}' => return Some((FULL, 0.25)), // ░ light shade
        '\u{2592}' => return Some((FULL, 0.5)),  // ▒ medium shade
        '\u{2593}' => return Some((FULL, 0.75)), // ▓ dark shade
        '\u{2594}' => &[(0.0, 0.0, 1.0, 0.125)], // ▔ upper 1/8
        '\u{2595}' => &[(0.875, 0.0, 1.0, 1.0)], // ▕ right 1/8
        '\u{2596}' => &[(0.0, 0.5, 0.5, 1.0)],   // ▖ lower-left
        '\u{2597}' => &[(0.5, 0.5, 1.0, 1.0)],   // ▗ lower-right
        '\u{2598}' => &[(0.0, 0.0, 0.5, 0.5)],   // ▘ upper-left
        '\u{2599}' => &[(0.0, 0.0, 0.5, 1.0), (0.5, 0.5, 1.0, 1.0)], // ▙ all but UR
        '\u{259A}' => &[(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 1.0, 1.0)], // ▚ UL + LR
        '\u{259B}' => &[(0.0, 0.0, 1.0, 0.5), (0.0, 0.5, 0.5, 1.0)], // ▛ all but LR
        '\u{259C}' => &[(0.0, 0.0, 1.0, 0.5), (0.5, 0.5, 1.0, 1.0)], // ▜ all but LL
        '\u{259D}' => &[(0.5, 0.0, 1.0, 0.5)],   // ▝ upper-right
        '\u{259E}' => &[(0.5, 0.0, 1.0, 0.5), (0.0, 0.5, 0.5, 1.0)], // ▞ UR + LL
        '\u{259F}' => &[(0.5, 0.0, 1.0, 0.5), (0.0, 0.5, 1.0, 1.0)], // ▟ all but UL
        _ => return None,
    };
    Some((rects, 1.0))
}

/// One edge of a cell-snapped quad: `base + unit * (cell + frac)`. Factored out so the
/// seam-free property is a *tested invariant*, not a hope: for `frac == 1.0` on cell
/// `k` and `frac == 0.0` on cell `k + 1` this is the SAME float expression
/// (`k as f32 + 1.0` is exact for any realistic grid size), so adjacent block cells
/// share bit-identical edges and the rasterizer can't leave a gap.
fn quad_edge(base: Pixels, unit: Pixels, cell: usize, frac: f32) -> Pixels {
    base + unit * (cell as f32 + frac)
}

pub struct SshSession {
    // ---- the one shared connection ------------------------------------------------------
    client: Option<SharedClient>,
    status: SessionStatus,

    // ---- shell / terminal (Plan 3C) -----------------------------------------------------
    screen: Box<dyn TerminalScreen>,
    /// Bumped whenever the grid's rendered appearance may have changed: PTY bytes fed,
    /// resize. Keys [`Self::shaped_cache`], so unrelated re-renders (tab switches,
    /// overlays, sibling notifies) stop re-cloning and re-shaping the whole grid every
    /// frame — the deferred perf-audit "terminal-grid memoization" item, and the SSH
    /// tab's debug-build stutter.
    grid_generation: u64,
    /// The last shape pass, reused verbatim while `(generation, cursor, colors)` match.
    /// `ShapedLine` is `Arc`-backed, so a cache hit costs one shallow Vec clone instead
    /// of a full-grid `cells()` deep-clone + shape.
    shaped_cache: Option<ShapedGridCache>,
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
    /// Split-layout collapse toggle (P5.2): the `«`/`»` control that lets the terminal
    /// reclaim the whole pane when browsing files isn't needed. Complements the drag
    /// divider rather than substituting for it — collapse is "not now", drag is "this
    /// much".
    sidebar_collapsed: bool,
    /// What the user dragged the sidebar to, if they have. `None` means "follow the
    /// window" — [`sidebar_width`]'s proportional default.
    ///
    /// Stored **unclamped**, and clamped on every read: a window too narrow to honor it
    /// borrows the space rather than overwriting the preference, so widening the window
    /// hands it back (see [`sidebar_width`]'s rule 1).
    ///
    /// Session-local, per connection tab, and deliberately not persisted. The dock
    /// *side* lives in `Settings` because it is one enum shared by every tab; a width is
    /// a per-tab, per-window judgement — the same setting would be wrong on a laptop and
    /// on a 4K panel, which is the pixel-pinning problem again one level up. (`Settings`
    /// is also postcard-positional with a four-hop version chain, so a field there is
    /// never a small change.)
    sidebar_width_pref: Option<Pixels>,
    /// An in-flight divider drag — see [`SidebarDrag`]. `None` whenever the pointer
    /// isn't holding the divider.
    sidebar_drag: Option<SidebarDrag>,
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
                grid_generation: 0,
                shaped_cache: None,
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
                sidebar_width_pref: None,
                sidebar_drag: None,
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

    /// Make sure the SFTP file panel is on screen.
    ///
    /// The panel opens with every session and only leaves when the user collapses it, so
    /// this is almost always already true — but "browse files" on the Home row has to
    /// *guarantee* it, or the tab named "SSH / SFTP" would answer a request for files by
    /// showing a bare terminal.
    pub fn reveal_files(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_collapsed {
            self.sidebar_collapsed = false;
            cx.notify();
        }
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
                        session.grid_generation += 1;
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
        self.grid_generation += 1;
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

    // ---- the drag divider ------------------------------------------------------------------

    /// Divider mouse-down: arm a drag, or — on the second click of a double-click —
    /// drop the user's width and hand the sidebar back to [`sidebar_width`]'s
    /// proportional default. Double-click-to-reset is the escape hatch for a drag that
    /// went somewhere silly; without it the only way back to the default is to guess it.
    fn start_sidebar_drag(
        &mut self,
        event: &MouseDownEvent,
        width_at_grab: Pixels,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            self.sidebar_drag = None;
            self.sidebar_width_pref = None;
            cx.notify();
            return;
        }
        self.sidebar_drag = Some(SidebarDrag {
            grab_x: event.position.x,
            width_at_grab,
        });
        cx.notify();
    }

    /// Bound to the whole split, not to the divider: six pixels is a fine thing to
    /// *grab* and a hopeless thing to stay inside of, so once the drag is armed the
    /// pointer's position matters wherever it is (the pattern `db_diagram`'s draggable
    /// boxes use — handlers on the container, not the handle).
    ///
    /// The preference is stored raw; [`sidebar_width`] is what clamps it, on every
    /// render, against the live window.
    fn on_sidebar_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.sidebar_drag else {
            return;
        };
        let dx = event.position.x - drag.grab_x;
        self.sidebar_width_pref = Some(dragged_width(drag.width_at_grab, dx, self.dock_side));
        cx.notify();
    }

    /// Mouse-up (inside the split or out of it) ends the drag. Nothing to commit — every
    /// move already wrote the preference — so this only drops the drag state, which is
    /// what stops the sidebar following an unpressed pointer.
    fn on_sidebar_drag_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_drag.take().is_some() {
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
/// narrow end, a full timestamp doesn't fit cleanly next to the name/size columns
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

// ---- the entry row's horizontal space budget ---------------------------------------------

/// Row geometry, stated once, so [`plan_entry_row`]'s arithmetic and the row it plans for
/// cannot drift apart. Every one of these mirrors a value the renderer actually sets — if
/// you change a width below, change it in [`SshSession::entry_row`] too, and the budget
/// tests will tell you if the two stop agreeing.
mod row_metrics {
    use gpui::{Pixels, px};

    /// `sid_ui::Row`'s `row_padding()` is `px_3` — 12px on each side.
    pub const PAD_X: Pixels = px(12.);
    /// `sid_ui::Row`'s `gap_2()`, between the leading/content/meta/action slots.
    pub const SLOT_GAP: Pixels = px(8.);
    /// `gap_1()`, inside `Row`'s meta and action clusters.
    pub const CLUSTER_GAP: Pixels = px(4.);
    /// The leading type glyph.
    pub const GLYPH: Pixels = px(14.);
    /// The size column.
    pub const SIZE: Pixels = px(56.);
    /// The modified-date column (`YYYY-MM-DD`).
    pub const MTIME: Pixels = px(84.);
    /// The action cluster, sized for the *widest* row (a file: view + download + copy).
    ///
    /// Fixed rather than content-sized, and identical on directory rows — which draw only
    /// `copy path` into it — because a cluster that shrank on directory rows would slide
    /// the size column left on every second row and turn the list into a ragged edge.
    pub const ACTIONS: Pixels = px(80.);
    /// What the name column must never drop below.
    ///
    /// Roughly sixteen characters at `text_sm`. The bug this constant exists to prevent is
    /// two *different* files rendering as the same string: `.bashrc` and `.bash_logout`
    /// diverge at character six, so anything that truncates earlier than that is lying
    /// about what is in the directory.
    pub const NAME_MIN: Pixels = px(120.);
}

/// Which orientation columns an entry row can afford, at a given row width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EntryRowPlan {
    /// Draw the size column.
    show_size: bool,
    /// Draw the modified-date column.
    show_mtime: bool,
    /// What the name column is left with once every fixed-width slot has taken its share.
    /// Never negative — a row narrower than its own furniture reports zero.
    name_width: Pixels,
}

/// Decide an entry row's columns by arithmetic rather than by hope.
///
/// Every child of an entry row except the name is fixed-width, so what the name gets is
/// subtraction — and at the old fixed 320px sidebar the row spent the lot: glyph 14 + size 60 +
/// mtime 84 + three text buttons + five 8px gaps + 24px of padding left the name about
/// twenty pixels, which is why `.bashrc` and `.bash_logout` both rendered as `.ba`.
///
/// The name is the reason the row exists, so it gets a floor and the orientation columns
/// yield to it: **mtime first** — a date is the least load-bearing fact in a file list, and
/// [`format_mtime`] already gave up time-of-day to this same squeeze — then **size**.
///
/// One plan governs the whole list (it takes no per-row input), so every row agrees on
/// where its columns start.
fn plan_entry_row(row_width: Pixels) -> EntryRowPlan {
    use row_metrics as m;

    // Furniture that is present no matter what: the box's padding, the leading glyph, the
    // action cluster, and the two slot gaps that bracket the content.
    let base = m::PAD_X * 2. + m::GLYPH + m::ACTIONS + m::SLOT_GAP * 2.;

    for (show_size, show_mtime) in [(true, true), (true, false), (false, false)] {
        let meta = match (show_size, show_mtime) {
            (true, true) => m::SIZE + m::CLUSTER_GAP + m::MTIME + m::SLOT_GAP,
            (true, false) => m::SIZE + m::SLOT_GAP,
            _ => Pixels::ZERO,
        };
        let name_width = row_width - base - meta;
        if name_width >= m::NAME_MIN {
            return EntryRowPlan {
                show_size,
                show_mtime,
                name_width,
            };
        }
    }

    // Narrower than the furniture itself. Everything has already yielded; the name takes
    // whatever is left and the `truncate()` on it does the rest.
    EntryRowPlan {
        show_size: false,
        show_mtime: false,
        name_width: (row_width - base).max(Pixels::ZERO),
    }
}

// ---- the split's own width ----------------------------------------------------------------

/// How wide the file sidebar is, given the split's total width and whatever the user
/// dragged it to.
///
/// **The rules, in precedence order.**
///
/// 1. *The terminal's floor outranks the sidebar's preference.* A drag — or a window
///    that shrinks under an already-wide sidebar — can never push the terminal below
///    [`sidebar_metrics::TERMINAL_MIN`]. The user's dragged width is kept verbatim in
///    session state and clamped here, at read time, so a squeeze is borrowed rather than
///    destructive: widen the window again and their width comes back.
/// 2. *Within that room, the sidebar's own band governs:* the dragged width, or
///    [`sidebar_metrics::RATIO`] of the split when there isn't one, clamped to
///    `[MIN, MAX]`.
/// 3. *Below the point where both floors fit, neither one wins — they degrade together.*
///    The split is divided in proportion to the two floors (sidebar `280/640` ≈ 44%,
///    terminal ≈ 56%), so a genuinely tiny window gets a cramped-but-present file list
///    and a cramped-but-larger terminal, instead of one pane eating the other. That
///    proportion is chosen to meet rule 2 exactly at the changeover, so the width is
///    *continuous* across it — no jump as the window crosses 646px.
///
/// `viewport_px` is the whole split (both panes plus the divider);
/// [`sidebar_metrics::DIVIDER`] is reserved off the top, since the strip is a sibling of
/// both panes rather than part of either.
fn sidebar_width(viewport_px: Pixels, preferred: Option<Pixels>) -> Pixels {
    use sidebar_metrics as s;

    let usable = viewport_px - s::DIVIDER;
    if usable <= Pixels::ZERO {
        return Pixels::ZERO;
    }

    // Rule 3: not enough room for both floors — split in proportion to them. At exactly
    // `MIN + TERMINAL_MIN` this yields `MIN`, which is what makes the changeover
    // continuous with the clamp below.
    if usable < s::MIN + s::TERMINAL_MIN {
        let share = f32::from(s::MIN) / f32::from(s::MIN + s::TERMINAL_MIN);
        return usable * share;
    }

    // Rule 2, bounded by rule 1: the sidebar's own band, with the terminal's floor as a
    // hard ceiling on top of it. `usable - TERMINAL_MIN >= MIN` in this branch, so the
    // effective ceiling can never fall below the floor.
    let desired = preferred.unwrap_or(viewport_px * s::RATIO);
    let ceiling = (usable - s::TERMINAL_MIN).min(s::MAX);
    desired.max(s::MIN).min(ceiling)
}

/// Where a divider drag puts the sidebar's edge, before [`sidebar_width`] clamps it:
/// the width at the moment of grab, plus the pointer's horizontal travel — *signed by
/// which side the panel is docked to*. Dragging right widens a left-docked sidebar and
/// narrows a right-docked one, because the divider is on the panel's inner edge either
/// way.
fn dragged_width(width_at_grab: Pixels, dx: Pixels, side: PanelSide) -> Pixels {
    match side {
        PanelSide::Left => width_at_grab + dx,
        PanelSide::Right => width_at_grab - dx,
    }
}

// ---- the breadcrumb's one line -----------------------------------------------------------

/// One item on the breadcrumb line.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Crumb {
    /// What it reads.
    label: String,
    /// Where clicking it navigates — `None` for the inert `…` standing in for elided
    /// segments, which has no single path it could sensibly mean.
    target: Option<String>,
}

/// The elided-middle mark.
const ELLIPSIS: &str = "…";

/// The breadcrumb row's furniture: the sidebar's own `px_2` padding on both sides plus
/// the `«` collapse control beside the line.
const CRUMB_CHROME: Pixels = px(82.);

/// One `text_xs` mono character, near enough.
const CRUMB_CHAR_PX: f32 = 7.;

/// The narrowest breadcrumb worth computing — below this the line is `/ … x` and the
/// `overflow_hidden` backstop is doing the work anyway.
const CRUMB_BUDGET_MIN: usize = 8;

/// Characters the breadcrumb line can hold at a given sidebar width.
///
/// Deliberately an estimate — it only decides *when* the middle elides, and the
/// `overflow_hidden` on the line is what makes being a little wrong about it harmless
/// rather than a second overlap bug. Calibrated to the 34 characters the old fixed
/// 320px sidebar used, so a sidebar left at its old size elides exactly where it did.
fn crumb_budget(sidebar_width: Pixels) -> usize {
    let room = f32::from(sidebar_width) - f32::from(CRUMB_CHROME);
    if room <= 0. {
        return CRUMB_BUDGET_MIN;
    }
    ((room / CRUMB_CHAR_PX).floor() as usize).max(CRUMB_BUDGET_MIN)
}

/// Split an absolute remote path into its clickable crumbs, root first, each targeting the
/// path built up to and including it (`/a/b` -> `/`, `a`->`/a`, `b`->`/a/b`).
fn path_crumbs(path: &str) -> Vec<Crumb> {
    let mut crumbs = vec![Crumb {
        label: "/".to_string(),
        target: Some("/".to_string()),
    }];
    let mut acc = String::new();
    for part in path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(part);
        crumbs.push(Crumb {
            label: part.to_string(),
            target: Some(acc.clone()),
        });
    }
    crumbs
}

/// What one crumb costs on the line: its text, plus the gap and padding around it.
fn crumb_cost(label: &str) -> usize {
    label.chars().count() + 2
}

/// The line's total cost.
fn line_cost(crumbs: &[Crumb]) -> usize {
    crumbs.iter().map(|c| crumb_cost(&c.label)).sum()
}

/// Middle-truncate a single label to `budget` characters (`verylongname` -> `very…name`).
fn elide_label(label: &str, budget: usize) -> String {
    let chars: Vec<char> = label.chars().collect();
    if chars.len() <= budget {
        return label.to_string();
    }
    if budget <= 1 {
        return ELLIPSIS.to_string();
    }
    let keep = budget - 1;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push_str(ELLIPSIS);
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Fit `crumbs` onto **one** line of `budget` characters by eliding the middle.
///
/// The breadcrumb used to `flex_wrap` instead, and its parent row keeps a single line's
/// height, so from depth two onward the wrapped remainder painted straight over the
/// go-to-path field below it.
///
/// What survives the squeeze is chosen by what a reader needs: the **root** (so "where does
/// this tree start" never disappears) and the **current directory** (the one segment naming
/// where you actually are), with as many of the intervening segments as fit, taken from the
/// current end backwards — near ancestors orient you, distant ones don't. A current
/// directory longer than the whole line is itself middle-truncated rather than dropped.
fn truncate_crumbs(crumbs: &[Crumb], budget: usize) -> Vec<Crumb> {
    if crumbs.len() <= 1 || line_cost(crumbs) <= budget {
        return crumbs.to_vec();
    }

    let root = crumbs[0].clone();
    let last = crumbs[crumbs.len() - 1].clone();
    let gap = Crumb {
        label: ELLIPSIS.to_string(),
        target: None,
    };
    let frame = crumb_cost(&root.label) + crumb_cost(&gap.label);

    // Not even `/ … <current>` fits: keep the frame and shorten the current segment, which
    // is still more informative than showing an ancestor and hiding where you are.
    if frame + crumb_cost(&last.label) > budget {
        let room = budget.saturating_sub(frame + 2);
        return vec![
            root,
            gap,
            Crumb {
                label: elide_label(&last.label, room.max(1)),
                ..last
            },
        ];
    }

    // Walk back from the current directory, taking whole segments while they fit.
    let mut kept = vec![last];
    let mut used = frame + crumb_cost(&kept[0].label);
    for crumb in crumbs[1..crumbs.len() - 1].iter().rev() {
        let cost = crumb_cost(&crumb.label);
        if used + cost > budget {
            break;
        }
        used += cost;
        kept.push(crumb.clone());
    }
    kept.push(gap);
    kept.push(root);
    kept.reverse();
    kept
}

#[cfg(test)]
mod entry_row_space_tests {
    use super::*;

    /// A 1600px window's default sidebar — 25% of the split, well inside the clamp.
    const DEFAULT_AT_1600: Pixels = px(400.);

    /// The acceptance case from the bug report, as arithmetic. Checked at the *floor*,
    /// not at some comfortable width: the floor is the only width the sidebar is
    /// guaranteed to be at least, so it is the one the name column has to survive.
    #[test]
    fn at_the_sidebar_floor_the_name_column_stays_readable() {
        let plan = plan_entry_row(sidebar_metrics::MIN);
        assert!(
            plan.name_width >= row_metrics::NAME_MIN,
            "name got {:?}, floor is {:?}",
            plan.name_width,
            row_metrics::NAME_MIN
        );
    }

    #[test]
    fn mtime_is_the_first_column_to_yield() {
        // At a typical default width something has to give, and the date goes first.
        let plan = plan_entry_row(DEFAULT_AT_1600);
        assert!(!plan.show_mtime, "the date should have yielded");
        assert!(
            plan.show_size,
            "size should still fit at {DEFAULT_AT_1600:?}"
        );
    }

    /// The two thresholds `sidebar_metrics::MIN`'s doc comment quotes, pinned, so the
    /// prose and the arithmetic can't drift: size returns at 318px, modified at 406px.
    /// Both sit inside `[MIN, MAX]`, which is what makes dragging the sidebar wider a
    /// *visible* trade rather than a cosmetic one.
    #[test]
    fn the_orientation_columns_return_inside_the_clamp_band() {
        assert!(!plan_entry_row(px(317.)).show_size);
        assert!(plan_entry_row(px(318.)).show_size);
        assert!(!plan_entry_row(px(405.)).show_mtime);
        assert!(plan_entry_row(px(406.)).show_mtime);
        for w in [px(318.), px(406.)] {
            assert!(
                w > sidebar_metrics::MIN && w < sidebar_metrics::MAX,
                "{w:?} should fall inside the clamp band"
            );
        }
    }

    #[test]
    fn size_yields_next_rather_than_the_name_dropping_below_its_floor() {
        // 280px: wide enough to seat a floor-width name *or* a name plus the size column,
        // but not both. The name is what must survive that choice.
        let plan = plan_entry_row(px(280.));
        assert!(!plan.show_size, "size should have yielded too");
        assert!(!plan.show_mtime);
        assert!(
            plan.name_width >= row_metrics::NAME_MIN,
            "name got {:?}",
            plan.name_width
        );
    }

    #[test]
    fn a_wide_row_affords_every_column() {
        let plan = plan_entry_row(px(600.));
        assert!(plan.show_size);
        assert!(plan.show_mtime);
        assert!(plan.name_width >= row_metrics::NAME_MIN);
    }

    #[test]
    fn a_row_narrower_than_its_own_furniture_never_reports_a_negative_name() {
        let plan = plan_entry_row(px(40.));
        assert_eq!(plan.name_width, Pixels::ZERO);
        assert!(!plan.show_size);
        assert!(!plan.show_mtime);
    }

    #[test]
    fn columns_only_ever_appear_as_the_row_gets_wider() {
        // Monotonicity: widening a row must never take a column away. Without it the
        // yield order could oscillate and the list would flicker on a resize.
        let mut seen_size = false;
        let mut seen_mtime = false;
        for w in (100..800).step_by(10) {
            let plan = plan_entry_row(px(w as f32));
            if plan.show_size {
                seen_size = true;
            } else {
                assert!(!seen_size, "size came back off at {w}px");
            }
            if plan.show_mtime {
                seen_mtime = true;
            } else {
                assert!(!seen_mtime, "mtime came back off at {w}px");
            }
            // And the name is never starved at any width.
            assert!(
                plan.name_width >= row_metrics::NAME_MIN || (!plan.show_size && !plan.show_mtime),
                "at {w}px the name got {:?} while still paying for a column",
                plan.name_width
            );
        }
    }
}

#[cfg(test)]
mod sidebar_width_tests {
    use super::sidebar_metrics as s;
    use super::*;

    /// What the terminal is left with, which is the half of the split the sidebar's
    /// arithmetic is really about. Floored at zero: a viewport too small to seat even
    /// the divider has no terminal, not a negative one.
    fn terminal_width(viewport: f32, preferred: Option<Pixels>) -> Pixels {
        (px(viewport) - sidebar_width(px(viewport), preferred) - s::DIVIDER).max(Pixels::ZERO)
    }

    #[test]
    fn the_default_is_a_proportion_of_the_split() {
        // The middle of the band: 25% of 1600 is 400, which is neither floor nor ceiling.
        assert_eq!(sidebar_width(px(1600.), None), px(400.));
    }

    #[test]
    fn a_narrow_window_clamps_up_to_the_content_floor() {
        // 25% of 900 is 225 — narrower than a file list can say anything useful in.
        assert_eq!(sidebar_width(px(900.), None), s::MIN);
    }

    #[test]
    fn a_wide_window_clamps_down_to_the_ceiling() {
        // 25% of a 4K-ish window is 640px of file list, which is just stolen terminal.
        assert_eq!(sidebar_width(px(2560.), None), s::MAX);
    }

    #[test]
    fn the_terminal_floor_outranks_a_dragged_width() {
        // Dragged to the ceiling, then the window shrinks to 800: honoring 480 would
        // leave the terminal 314px. It gets its floor and the sidebar yields.
        let w = sidebar_width(px(800.), Some(s::MAX));
        assert!(w < s::MAX, "the sidebar should have yielded, got {w:?}");
        assert_eq!(terminal_width(800., Some(s::MAX)), s::TERMINAL_MIN);
    }

    #[test]
    fn a_squeeze_is_borrowed_not_destructive() {
        // The same stored preference, read at two window sizes: squeezed at 800,
        // returned whole at 1600. Nothing clamps the *stored* value.
        let pref = Some(px(440.));
        assert!(sidebar_width(px(800.), pref) < px(440.));
        assert_eq!(sidebar_width(px(1600.), pref), px(440.));
    }

    #[test]
    fn below_both_floors_the_panes_shrink_together_rather_than_one_starving() {
        let sidebar = sidebar_width(px(500.), None);
        let terminal = terminal_width(500., None);
        assert!(
            sidebar < s::MIN,
            "the sidebar is under its floor, as expected"
        );
        assert!(terminal < s::TERMINAL_MIN, "so is the terminal");
        assert!(
            sidebar > px(0.),
            "but the sidebar is still present: {sidebar:?}"
        );
        assert!(
            terminal > sidebar,
            "and the terminal keeps the larger share: {terminal:?} vs {sidebar:?}"
        );
    }

    #[test]
    fn the_two_regimes_meet_without_a_jump() {
        // Both floors fit exactly at MIN + TERMINAL_MIN + DIVIDER = 646px. The
        // proportional rule below it and the clamped rule above it must agree there,
        // or the sidebar would snap as the window crossed that width.
        let boundary = f32::from(s::MIN + s::TERMINAL_MIN + s::DIVIDER);
        assert_eq!(sidebar_width(px(boundary), None), s::MIN);
        let just_below = sidebar_width(px(boundary - 1.), None);
        assert!(
            f32::from(s::MIN - just_below) < 1.,
            "a 1px narrower window moved the sidebar {:?}",
            s::MIN - just_below
        );
    }

    #[test]
    fn a_degenerate_viewport_never_returns_a_negative_width() {
        for viewport in [0., 1., 6., 6.5] {
            let w = sidebar_width(px(viewport), None);
            assert!(
                w >= Pixels::ZERO && w <= px(viewport),
                "viewport {viewport} gave {w:?}"
            );
        }
    }

    #[test]
    fn widening_the_window_never_narrows_the_sidebar() {
        // Monotonicity, with and without a dragged preference: a resize that only adds
        // space must never take space away from either pane, or the split would jitter
        // as the window is dragged.
        for pref in [None, Some(px(440.)), Some(px(100.)), Some(px(9000.))] {
            let mut last_sidebar = Pixels::ZERO;
            let mut last_terminal = Pixels::ZERO;
            for v in (0..3000).step_by(10) {
                let sidebar = sidebar_width(px(v as f32), pref);
                let terminal = terminal_width(v as f32, pref);
                assert!(
                    sidebar >= last_sidebar - px(0.01),
                    "sidebar shrank at {v}px ({sidebar:?} < {last_sidebar:?}), pref {pref:?}"
                );
                assert!(
                    terminal >= last_terminal - px(0.01),
                    "terminal shrank at {v}px ({terminal:?} < {last_terminal:?}), pref {pref:?}"
                );
                last_sidebar = sidebar;
                last_terminal = terminal.max(Pixels::ZERO);
            }
        }
    }

    #[test]
    fn the_sidebar_never_takes_the_whole_split() {
        for v in (0..3000).step_by(7) {
            let sidebar = sidebar_width(px(v as f32), Some(px(9000.)));
            assert!(
                sidebar <= px(v as f32),
                "at {v}px the sidebar claimed {sidebar:?}"
            );
        }
    }

    #[test]
    fn dragging_moves_the_edge_in_the_direction_the_panel_faces() {
        // The divider is on the panel's *inner* edge, so the same rightward pointer
        // travel widens a left-docked sidebar and narrows a right-docked one.
        assert_eq!(dragged_width(px(320.), px(40.), PanelSide::Left), px(360.));
        assert_eq!(dragged_width(px(320.), px(40.), PanelSide::Right), px(280.));
        assert_eq!(dragged_width(px(320.), px(-40.), PanelSide::Left), px(280.));
        assert_eq!(
            dragged_width(px(320.), px(-40.), PanelSide::Right),
            px(360.)
        );
    }

    #[test]
    fn a_drag_can_never_leave_the_clamp_band() {
        // A wide window, so the terminal floor isn't the binding constraint: whatever
        // the pointer does, the width it produces is inside [MIN, MAX].
        for dx in (-2000..2000).step_by(37) {
            let raw = dragged_width(px(320.), px(dx as f32), PanelSide::Left);
            let w = sidebar_width(px(1920.), Some(raw));
            assert!(
                (s::MIN..=s::MAX).contains(&w),
                "dx {dx} produced {raw:?} -> {w:?}"
            );
        }
    }

    #[test]
    fn the_breadcrumb_budget_follows_the_live_width() {
        // Calibration: the old fixed 320px sidebar's budget was 34 characters.
        assert_eq!(crumb_budget(px(320.)), 34);
        assert!(crumb_budget(s::MAX) > crumb_budget(s::MIN));
        assert!(crumb_budget(px(0.)) >= CRUMB_BUDGET_MIN);
        let mut last = 0;
        for w in (0..600).step_by(5) {
            let b = crumb_budget(px(w as f32));
            assert!(b >= last, "budget shrank as the sidebar widened at {w}px");
            last = b;
        }
    }
}

#[cfg(test)]
mod crumb_tests {
    use super::*;

    /// The budget the old fixed-width sidebar had — the width these cases were written
    /// against, kept so they keep testing elision rather than arithmetic.
    fn budget() -> usize {
        crumb_budget(px(320.))
    }

    fn labels(crumbs: &[Crumb]) -> Vec<String> {
        crumbs.iter().map(|c| c.label.clone()).collect()
    }

    #[test]
    fn a_path_becomes_root_first_cumulative_targets() {
        let crumbs = path_crumbs("/a/b");
        assert_eq!(labels(&crumbs), vec!["/", "a", "b"]);
        let targets: Vec<Option<&str>> = crumbs.iter().map(|c| c.target.as_deref()).collect();
        assert_eq!(targets, vec![Some("/"), Some("/a"), Some("/a/b")]);
    }

    #[test]
    fn a_shallow_path_keeps_every_segment() {
        // Depth 1 and depth 2 — the depths the wrapping bug appeared at — fit whole.
        for path in ["/home", "/home/sid_test"] {
            let crumbs = path_crumbs(path);
            assert_eq!(
                truncate_crumbs(&crumbs, budget()),
                crumbs,
                "{path} should not have been elided"
            );
        }
    }

    #[test]
    fn a_deep_path_keeps_the_root_and_where_you_are() {
        let crumbs = path_crumbs("/usr/local/share/applications/vendor");
        let out = truncate_crumbs(&crumbs, budget());
        assert_eq!(out.first().unwrap().label, "/", "root survives");
        assert_eq!(
            out.last().unwrap().label,
            "vendor",
            "the current directory survives"
        );
        assert!(out.len() < crumbs.len(), "something should have been cut");
        assert!(
            out.iter().any(|c| c.label == ELLIPSIS),
            "the cut is marked: {:?}",
            labels(&out)
        );
    }

    #[test]
    fn the_elided_line_actually_fits() {
        // The whole point: one line, inside the budget, at every depth.
        let mut path = String::new();
        for seg in ["usr", "local", "share", "applications", "vendor", "themes"] {
            path.push('/');
            path.push_str(seg);
            let out = truncate_crumbs(&path_crumbs(&path), budget());
            assert!(
                line_cost(&out) <= budget(),
                "{path} -> {:?} costs {} > {}",
                labels(&out),
                line_cost(&out),
                budget()
            );
        }
    }

    #[test]
    fn one_pathologically_long_segment_is_middle_truncated_not_dropped() {
        let long = "a".repeat(200);
        let out = truncate_crumbs(&path_crumbs(&format!("/{long}")), budget());
        assert!(line_cost(&out) <= budget(), "{:?}", labels(&out));
        let current = &out.last().unwrap().label;
        assert!(
            current.contains(ELLIPSIS),
            "the long segment should be elided in place, got {current:?}"
        );
        assert!(current.starts_with('a'), "keeps its head");
        assert!(current.ends_with('a'), "and its tail");
    }

    #[test]
    fn the_elision_mark_navigates_nowhere() {
        let out = truncate_crumbs(&path_crumbs("/usr/local/share/applications/vendor"), 20);
        let gap = out.iter().find(|c| c.label == ELLIPSIS).unwrap();
        assert_eq!(
            gap.target, None,
            "the ellipsis stands for several paths, so it cannot click through to one"
        );
    }

    #[test]
    fn nearer_ancestors_are_kept_before_distant_ones() {
        // Given room for exactly one intervening segment, it should be the parent — the
        // segment that says what you are inside of, not one four levels up.
        let crumbs = path_crumbs("/aaaa/bbbb/cccc/parent/here");
        let out = truncate_crumbs(&crumbs, 24);
        let kept = labels(&out);
        assert!(kept.contains(&"parent".to_string()), "{kept:?}");
        assert!(!kept.contains(&"aaaa".to_string()), "{kept:?}");
    }

    #[test]
    fn the_root_alone_is_never_elided() {
        let crumbs = path_crumbs("/");
        assert_eq!(truncate_crumbs(&crumbs, 1), crumbs);
    }
}

// ---- rendering: split layout (file sidebar + terminal) -----------------------------------

impl SshSession {
    /// The `Connected` view: file sidebar beside the terminal grid, with a drag divider
    /// between them — the MobaXterm-style split. Docks left or right per
    /// `self.dock_side` (ssh-v3's `⇄ dock` toggle).
    ///
    /// The sidebar's width is [`sidebar_width`] of the live window rather than a pinned
    /// constant, so it is a share of the split at every window size instead of being
    /// right at one. The split is the tab's full width (the SSH tab stacks its chrome
    /// *above* this, never beside it), so `viewport_size().width` is the split's width;
    /// reading it here — not from a measured canvas — is what keeps the sidebar, the
    /// entry-row plan and the breadcrumb budget all agreeing within a single frame.
    fn render_split(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = sidebar_width(window.viewport_size().width, self.sidebar_width_pref);
        let collapsed = self.sidebar_collapsed;
        let dragging = self.sidebar_drag.is_some();
        let sidebar = self.file_sidebar(width, cx).into_any_element();
        let divider = (!collapsed).then(|| self.split_divider(width, cx).into_any_element());
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
            .id("session-split")
            .flex()
            .flex_row()
            .size_full()
            // Only while a drag is live: an always-on mouse-move listener over the whole
            // split would re-enter this entity on every pointer motion across the
            // terminal for nothing.
            .when(dragging, |el| {
                el.on_mouse_move(cx.listener(Self::on_sidebar_drag_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_sidebar_drag_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_sidebar_drag_up))
            })
            .child(first)
            .children(divider)
            .child(second)
    }

    /// The drag handle between the two panes: a [`sidebar_metrics::DIVIDER`]-wide strip
    /// on the sidebar's inner edge, which lights up under the pointer and takes the
    /// `col-resize` cursor so it reads as grabbable before it is grabbed.
    ///
    /// It is a sibling of both panes rather than a child of the sidebar, so grabbing it
    /// never lands on a file row, and the sidebar's width stays exactly the file panel's
    /// width — which is the number [`plan_entry_row`] and [`crumb_budget`] are given.
    fn split_divider(&self, width: Pixels, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (accent, bg) = (t.accent, t.bg);
        let active = self.sidebar_drag.is_some();
        div()
            .id("session-split-divider")
            .w(sidebar_metrics::DIVIDER)
            .h_full()
            .flex_none()
            .cursor_col_resize()
            // Idle it is just the seam between the panes — the sidebar's own 1px border
            // is the visible line. It fills with the accent under the pointer and stays
            // filled for the length of the drag, so the handle is *findable* without
            // being a permanent 6px stripe down the middle of the tab.
            .bg(rgb(if active { accent } else { bg }))
            .hover(|s| s.bg(rgb(accent)))
            .tooltip(|window, cx| {
                Tooltip::new("drag to resize · double-click to reset").build(window, cx)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |session, ev: &MouseDownEvent, _window, cx| {
                    session.start_sidebar_drag(ev, width, cx);
                }),
            )
    }

    /// The file panel: the [`toolbar`](Self::toolbar) above a scrollable, read-only listing
    /// (filtered by the hidden-files toggle), with the last file-panel error (if any) between
    /// them. Painted purely from `self.entries`/`self.path`/`self.show_hidden` — every SFTP
    /// call that could change them already ran, off gpui's executor, before `cx.notify()`
    /// scheduled this render.
    ///
    /// `width` is the live one from [`sidebar_width`], and it is what the entry-row plan
    /// and the breadcrumb budget are computed from — both are pure functions of a width,
    /// so a resize re-plans the list on the same frame it re-sizes the panel.
    fn file_sidebar(&self, width: Pixels, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
        let plan = plan_entry_row(width);
        let edge = match self.dock_side {
            PanelSide::Left => div().border_r_1(),
            PanelSide::Right => div().border_l_1(),
        };
        edge.w(width)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(bg))
            .border_color(rgb(border))
            .child(self.sidebar_header(cx))
            .child(self.toolbar(crumb_budget(width), cx))
            .when_some(self.file_error.clone(), |el, msg| {
                el.child(status_line(&format!("file panel: {msg}"), cx))
            })
            .child(
                uniform_list(
                    "session-sftp-entries",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _win, cx| {
                        let visible = this.visible_entries();
                        range
                            .map(|ix| this.entry_row(visible[ix], ix, plan, cx))
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

    /// Toolbar: three stacked rows, each of which fits the sidebar's *floor* width on its
    /// own with no overlap — cramming the breadcrumb, path field, nav icons, and entry
    /// count onto fewer, wider rows is what caused them to pile on top of each other at
    /// 320px. `crumb_budget` is [`crumb_budget`] of the live width, handed down to the
    /// breadcrumb so its elision follows the panel instead of a constant.
    ///
    /// - Row 1: the breadcrumb (flexes, wraps onto multiple lines if long) plus the `«`
    ///   collapse control (fixed).
    /// - Row 2: the go-to-path field (flexes) plus `Go` (fixed).
    /// - Row 3: `↑ up` / `⟳ refresh` / the hidden-files toggle on the left, the entry count
    ///   right-aligned.
    fn toolbar(&self, crumb_budget: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                    // `min_w(0)` on the wrapper as well as on the breadcrumb inside it:
                    // the breadcrumb's own clip only bounds *its* children, and this
                    // flex item would still refuse to shrink below its content width and
                    // shove `collapse` out of the fixed-width sidebar.
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.breadcrumb(crumb_budget, cx)),
                    )
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
                    // `v_flex`, not a plain `div`: a `TextInput` sizes itself entirely in
                    // percentages, and a `display: block` parent doesn't resolve them —
                    // the field collapsed to its own padding and border, a ~20px stub
                    // that swallowed clicks aimed at the field you could see. A flex
                    // column stretches it to a real width on the cross axis, which is
                    // exactly why the stacked form fields never had this bug.
                    //
                    // Enter submits, same as clicking `Go`. `TextInput` claims neither
                    // Enter nor Escape, so the wrapper can take it — the technique
                    // `db_tab`'s inline rename rows use for the same shape (one field,
                    // one button beside it).
                    .child(
                        v_flex()
                            .id("session-goto-field")
                            .flex_1()
                            .min_w(px(0.))
                            .on_key_down(cx.listener(|session, ev: &KeyDownEvent, _window, cx| {
                                if is_field_submit(&ev.keystroke) {
                                    cx.stop_propagation();
                                    session.goto_submit(cx);
                                }
                            }))
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
    /// built up cumulatively (`/a/b` -> `/`, `a` (-> `/a`), `b` (-> `/a/b`)).
    ///
    /// **One line, always.** This used to `flex_wrap()`, and because the toolbar row that
    /// holds it keeps a single line's height, every wrapped line from depth two onward
    /// spilled downward and painted over the go-to-path field. [`truncate_crumbs`] decides
    /// what to drop; `overflow_hidden` is the backstop that makes a wrong guess about the
    /// character budget a clipped crumb rather than a second overlap.
    fn breadcrumb(&self, budget: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let crumbs = truncate_crumbs(&path_crumbs(&self.path), budget);
        let children: Vec<_> = crumbs
            .into_iter()
            .enumerate()
            .map(|(ix, crumb)| self.breadcrumb_segment(ix, crumb, cx))
            .collect();
        div()
            .flex()
            .flex_row()
            .items_center()
            .min_w(px(0.))
            .overflow_hidden()
            .gap_1()
            .text_xs()
            .font_family(MONO)
            .children(children)
    }

    /// One crumb. A crumb with no target is the elided-middle mark: it reads as part of the
    /// path but is deliberately inert, because `…` stands for several directories and there
    /// is no single one it could navigate to.
    fn breadcrumb_segment(
        &self,
        ix: usize,
        crumb: Crumb,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (fg, muted, faint, selection) = (t.fg, t.muted, t.faint, t.selection);
        let label: SharedString = crumb.label.into();
        let Some(target) = crumb.target else {
            return div()
                .px_1()
                .flex_none()
                .text_color(rgb(faint))
                .child(label)
                .into_any_element();
        };
        let is_current = target == self.path;
        div()
            .id(("session-crumb", ix))
            .px_1()
            .flex_none()
            .rounded_md()
            .cursor_pointer()
            .text_color(rgb(if is_current { fg } else { muted }))
            .hover(|s| s.bg(rgb(selection)))
            .child(label)
            .on_click(cx.listener(move |session, _ev: &ClickEvent, _window, cx| {
                session.go_to(target.clone(), cx);
            }))
            .into_any_element()
    }

    /// One row of the entry list: type glyph, name, whichever orientation columns
    /// [`plan_entry_row`] says fit, and the per-row actions.
    ///
    /// `entry` comes from [`Self::visible_entries`] — already filtered by the hidden-files
    /// toggle — and `ix` is that filtered list's position, used only to keep each row's
    /// element ids unique; it is not an index into `self.entries`.
    ///
    /// The name is the one child that grows, and [`plan_entry_row`] guarantees what it
    /// grows to. Everything else is fixed-width, including the action cluster — which keeps
    /// its full width on directory rows that draw only one button into it, so the size
    /// column starts at the same x on every row instead of stepping in and out.
    ///
    /// A directory is entered by clicking anywhere on its row; the action buttons stop the
    /// click travelling so they never also trigger it. File rows stay inert, which is why
    /// only directory rows take a hover fill — a row that lights up under the pointer is
    /// promising it does something.
    fn entry_row(
        &self,
        entry: &SftpEntry,
        ix: usize,
        plan: EntryRowPlan,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (fg, muted, selection, accent) = (t.fg, t.muted, t.selection, t.accent);
        let name = entry.name.clone();
        let is_dir = entry.is_dir;
        let glyph = if is_dir { "▸" } else { "·" };
        let size = if is_dir {
            "—".to_string()
        } else {
            human_size(entry.size)
        };
        let mtime = format_mtime(entry.mtime_secs);
        let abs_path = abs_remote_path(&self.path, &name);

        // The full name plus the facts the row had to drop, so a truncated row is never
        // the only place a file's identity lives.
        let tooltip: SharedString = if is_dir {
            format!("{name}\nmodified {mtime}").into()
        } else {
            format!("{name}\n{size} · modified {mtime}").into()
        };

        let action_button = |id: (&'static str, usize), label: &'static str, width: Pixels| {
            div()
                .id(id)
                .w(width)
                .flex()
                .justify_center()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(accent))
                .hover(|s| s.bg(rgb(selection)))
                .child(label)
        };

        // Files get `view` + `⭳ download`; directories don't (nothing to fetch or preview).
        // The cluster is `row_metrics::ACTIONS` wide either way — see the doc comment.
        let file_buttons = (!is_dir).then(|| {
            let view_name = name.clone();
            let download_name = name.clone();
            div()
                .flex()
                .flex_row()
                .gap_1()
                .child(
                    action_button(("session-view", ix), "view", px(32.)).on_click(cx.listener(
                        move |session, _ev: &ClickEvent, _window, cx| {
                            cx.stop_propagation();
                            session.view(view_name.clone(), cx)
                        },
                    )),
                )
                .child(
                    action_button(("session-download", ix), "⭳", px(20.)).on_click(cx.listener(
                        move |session, _ev: &ClickEvent, _window, cx| {
                            cx.stop_propagation();
                            session.download(download_name.clone(), cx)
                        },
                    )),
                )
        });

        // `⧉ copy path` applies to files *and* directories.
        let copy_path_button = action_button(("session-copy-path", ix), "⧉", px(20.)).on_click(
            cx.listener(move |session, _ev: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                session.copy_path(abs_path.clone(), cx);
            }),
        );

        let meta_column = |text: String, width: Pixels| {
            div()
                .w(width)
                .clamp_one_line()
                .font_family(MONO)
                .text_xs()
                .text_color(rgb(muted))
                .child(text)
        };

        let enter_name = name.clone();
        Row::new(("session-entry", ix))
            .text_sm()
            .leading(
                div()
                    .w(row_metrics::GLYPH)
                    .text_color(rgb(muted))
                    .child(glyph),
            )
            .child(
                // `clamp_one_line`, never gpui's `truncate()` — see
                // [`sid_ui::StyledExt::clamp_one_line`]. Under `truncate()` the ellipsis
                // never landed and the row's own clip cut the name mid-glyph, hard against
                // the size column: `a-very-long-remot—`, `this-is-an-extreme0 B`.
                div()
                    .id(("session-entry-name", ix))
                    .min_w(px(0.))
                    .clamp_one_line()
                    .text_color(rgb(fg))
                    .child(name)
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx)),
            )
            .when(plan.show_size, |row| {
                row.meta(meta_column(size, row_metrics::SIZE))
            })
            .when(plan.show_mtime, |row| {
                row.meta(meta_column(mtime, row_metrics::MTIME))
            })
            .action(
                div()
                    .w(row_metrics::ACTIONS)
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_1()
                    .children(file_buttons)
                    .child(copy_path_button),
            )
            .when(is_dir, |row| {
                row.on_click(cx.listener(move |session, _ev: &ClickEvent, _window, cx| {
                    session.enter_dir(&enter_name, cx);
                }))
            })
    }

    /// Paint the terminal grid: one `shape_line` call per row (gpui shapes a whole
    /// multi-run line at once, so the row — not the cell — is the unit of work), then
    /// `paint_background` + `paint` per shaped row inside a `canvas`. The canvas fills
    /// whatever space the parent layout gives it; the resize detection below reads that
    /// real size back out of the canvas's own paint bounds and reconciles
    /// `self.rows`/`self.cols` against it.
    ///
    /// Shaping is memoized on `(grid_generation, cursor, default colors)` — see
    /// [`ShapedGridCache`]. A re-render with no new PTY bytes (tab switches, overlay
    /// opens, sibling entity notifies) reuses the previous pass instead of deep-cloning
    /// and re-shaping the whole grid.
    fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        // The terminal viewport is a recessed editor-like surface (spec: "input/editor/
        // terminal backgrounds -> well"), not the general window/panel plane — it reads
        // as visually inset from the chrome around it.
        let default_fg: Hsla = rgb(t.fg).into();
        let default_bg: Hsla = rgb(t.well).into();
        let ansi = t.ansi;
        let cursor = self.screen.cursor_position();
        let (cursor_row, cursor_col) = cursor;

        let cache_valid = self.shaped_cache.as_ref().is_some_and(|c| {
            c.generation == self.grid_generation
                && c.cursor == cursor
                && c.fg == default_fg
                && c.bg == default_bg
        });
        if !cache_valid {
            let base_font = font(MONO);
            let cells = self.screen.cells();
            // Measure one monospace glyph — its width/the line height are the grid's
            // cell size, used both to paint rows and (in the canvas below) to turn the
            // pane's real pixel bounds back into a rows/cols count.
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
            // Terminal cell height is the FONT's ascent+descent (kitty's geometry), not
            // the UI text style's ~1.5× `window.line_height()`: rows must stack so
            // glyph-drawn block art (█ ▀ ▄) tiles with no default-background bands
            // between rows, and so the grid's proportions match a real terminal's.
            // `paint_background` already fills whatever height it's given, so
            // backgrounds were never the problem — the glyph ink box was
            // (terminal-fidelity F1).
            let (rows, quads): (Vec<ShapedLine>, Vec<Vec<BlockQuad>>) = cells
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
                        &ansi,
                    )
                })
                .unzip();
            self.shaped_cache = Some(ShapedGridCache {
                generation: self.grid_generation,
                cursor,
                fg: default_fg,
                bg: default_bg,
                rows,
                quads,
                cell_width: em.width,
                line_height: em.ascent + em.descent,
            });
        }
        let cache = self.shaped_cache.as_ref().expect("just populated above");
        let shaped_rows: Vec<ShapedLine> = cache.rows.clone();
        let row_quads: Vec<Vec<BlockQuad>> = cache.quads.clone();
        let cell_width = cache.cell_width;
        let line_height = cache.line_height;

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
                        (shaped_rows, row_quads)
                    },
                    move |bounds,
                          (shaped_rows, row_quads): (Vec<ShapedLine>, Vec<Vec<BlockQuad>>),
                          window,
                          cx| {
                        for (row_ix, line) in shaped_rows.iter().enumerate() {
                            // Row tops computed multiplicatively (not accumulated) so
                            // every row/quad edge is the same float expression — see
                            // `quad_edge`'s doc comment for why that makes seams
                            // impossible rather than merely unlikely.
                            let row_top = quad_edge(bounds.top(), line_height, row_ix, 0.0);
                            let origin = point(bounds.left(), row_top);
                            let _ = line.paint_background(origin, line_height, window, cx);
                            for q in row_quads.get(row_ix).into_iter().flatten() {
                                for &(x0, y0, x1, y1) in q.rects {
                                    let quad_bounds = Bounds::from_corners(
                                        point(
                                            quad_edge(bounds.left(), cell_width, q.col, x0),
                                            quad_edge(bounds.top(), line_height, row_ix, y0),
                                        ),
                                        point(
                                            quad_edge(bounds.left(), cell_width, q.col, x1),
                                            quad_edge(bounds.top(), line_height, row_ix, y1),
                                        ),
                                    );
                                    window.paint_quad(fill(quad_bounds, q.color));
                                }
                            }
                            let _ = line.paint(origin, line_height, window, cx);
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
                                        // A remote file name is arbitrary length and the
                                        // modal is a fixed 640px, so the name has to be
                                        // the part that gives: `flex_1 + min_w(0)` lets
                                        // it shrink (a flex item's default min-width is
                                        // its content, which would push both the name
                                        // and the close button out through the modal's
                                        // border), and `clamp_one_line` then cuts it
                                        // with a real ellipsis instead of a hard clip.
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.))
                                                .text_sm()
                                                .text_color(rgb(fg))
                                                .clamp_one_line()
                                                .child(preview.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .id("session-preview-close")
                                                .flex_none()
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

/// The whole-tab message a session paints instead of a terminal: connecting, closed, or
/// the connect error.
///
/// **The text block is a separate, shrinkable child, and it has to be.** This used to
/// hand the string straight to a `justify_center` row, which put the text in as a flex
/// item whose min-width is `auto` — i.e. its *content* width. A flex item that cannot
/// shrink below its content does not wrap; it overflows, and because the row centres it
/// the overflow goes out **both** edges at once. `Connection failed: connect failed:
/// failed to lookup address information: Name or service not known` on a 640px-wide
/// window rendered as `…ction failed: connect failed: failed to lookup address
/// information: Name or service not…`, sentence-ends amputated on the left and the right
/// with no ellipsis anywhere — the exact "text jumping out of bounds" report, reached by
/// double-clicking a host card (double-click connects) on a host that won't answer.
///
/// `min_w(0)` restores the ability to shrink, which is what gives the text a real wrap
/// width and lets it wrap to as many lines as it needs; `max_w` keeps it to a reading
/// measure on a wide window instead of one 1900px line; `px_6` keeps it off the frame.
fn message_pane(text: &str, cx: &App) -> impl IntoElement {
    let t = theme::active(cx);
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px_6()
        // The backstop: whatever the text does, it stops at the pane's edge.
        .overflow_hidden()
        .bg(rgb(t.bg))
        .text_color(rgb(t.muted))
        .font_family(MONO)
        .child(
            // gpui reports a text element's MIN-content width as its FULL single-line
            // width (`elements/text.rs` only derives a wrap width from an
            // `AvailableSpace::Definite`), so a bare string is a flex item with an
            // automatic minimum it can never shrink below. Centred, that means a long
            // message overflows *both* sides: "Connection failed: connect failed: failed
            // to lookup address information: No address associated with hostname" ran off
            // the left and right edges of an 800px window with its first letters painted
            // outside the app. `min_w(0)` drops that floor, which is what finally hands
            // the text a definite width — and therefore a wrap width — to lay out inside.
            div()
                .min_w(px(0.))
                .max_w(px(720.))
                .text_center()
                .child(text.to_string()),
        )
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
// ponytail: 8 args; a GridStyle struct when the terminal-fidelity work adds more.
#[allow(clippy::too_many_arguments)]
fn shape_row(
    text_system: &gpui::WindowTextSystem,
    row: &[TermCell],
    cursor_col: Option<usize>,
    base_font: &Font,
    font_size: Pixels,
    default_fg: Hsla,
    default_bg: Hsla,
    ansi: &[u32; 16],
) -> (ShapedLine, Vec<BlockQuad>) {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut quads: Vec<BlockQuad> = Vec::new();

    for (col, cell) in row.iter().enumerate() {
        // A blank cell still occupies a column — render it as a space, like `lines()` does,
        // so run byte-offsets stay aligned with terminal columns.
        let mut glyph: &str = if cell.text.is_empty() {
            " "
        } else {
            &cell.text
        };

        let mut fg = term_color_to_hsla(cell.fg, default_fg, ansi);
        let mut bg = term_color_to_hsla(cell.bg, default_bg, ansi);
        if cell.inverse {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cursor_col == Some(col) {
            // Block cursor: swap fg/bg on top of whatever the cell's own styling already is,
            // rather than painting a separate overlay quad.
            std::mem::swap(&mut fg, &mut bg);
        }

        // Block elements (U+2580..=U+259F) never go through the font: they become
        // cell-snapped quads painted by the grid's canvas (see `block_coverage` for the
        // A/B evidence). The cell still contributes a SPACE to the shaped line so run
        // byte-offsets stay column-aligned and `paint_background` still fills its bg.
        let mut chars = glyph.chars();
        if let (Some(ch), None) = (chars.next(), chars.next())
            && let Some((rects, alpha)) = block_coverage(ch)
        {
            let mut color = fg;
            color.a *= alpha;
            quads.push(BlockQuad { col, rects, color });
            glyph = " ";
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

    (
        text_system.shape_line(text.into(), font_size, &runs, None),
        quads,
    )
}

/// `TermColor::Default` takes the pane's own theme color; `Indexed(0..=15)` goes through
/// the active theme's ANSI palette ([`crate::ui::theme::Theme::ansi`] — the same way
/// kitty renders the base 16 through the user's scheme, terminal-fidelity F2);
/// `Indexed(16..)` uses the universal xterm cube/ramp; `Rgb` converts directly.
fn term_color_to_hsla(color: TermColor, default: Hsla, ansi: &[u32; 16]) -> Hsla {
    match color {
        TermColor::Default => default,
        TermColor::Indexed(idx) if idx < 16 => rgb(ansi[idx as usize]).into(),
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
mod block_quad_tests {
    use super::*;

    #[test]
    fn every_block_element_has_coverage_and_nothing_else_does() {
        for cp in 0x2580..=0x259F_u32 {
            let ch = char::from_u32(cp).unwrap();
            assert!(block_coverage(ch).is_some(), "U+{cp:04X} must be covered");
        }
        // Box drawing deliberately stays a font glyph (A/B: Caskaydia joins cleanly).
        assert!(block_coverage('─').is_none());
        assert!(block_coverage('│').is_none());
        assert!(block_coverage('┼').is_none());
        assert!(block_coverage('a').is_none());
        assert!(block_coverage('█').is_some());
    }

    #[test]
    fn coverage_areas_match_the_glyph_semantics() {
        let area = |ch: char| -> f32 {
            let (rects, _) = block_coverage(ch).unwrap();
            rects
                .iter()
                .map(|(x0, y0, x1, y1)| (x1 - x0) * (y1 - y0))
                .sum()
        };
        assert_eq!(area('\u{2588}'), 1.0, "full block");
        assert_eq!(area('\u{2580}'), 0.5, "upper half");
        assert_eq!(area('\u{2584}'), 0.5, "lower half");
        assert_eq!(area('\u{258C}'), 0.5, "left half");
        assert_eq!(area('\u{2590}'), 0.5, "right half");
        assert_eq!(area('\u{2581}'), 0.125, "lower eighth");
        assert_eq!(area('\u{2596}'), 0.25, "one quadrant");
        assert_eq!(area('\u{2599}'), 0.75, "three quadrants");
        assert_eq!(area('\u{259A}'), 0.5, "two diagonal quadrants");
    }

    #[test]
    fn shades_are_full_cover_with_partial_alpha() {
        for (ch, want) in [('\u{2591}', 0.25), ('\u{2592}', 0.5), ('\u{2593}', 0.75)] {
            let (rects, alpha) = block_coverage(ch).unwrap();
            assert_eq!(rects, &[(0.0, 0.0, 1.0, 1.0)]);
            assert_eq!(alpha, want);
        }
    }

    #[test]
    fn quad_edges_of_adjacent_cells_are_bit_identical() {
        // The seam-free invariant: cell k's right edge and cell k+1's left edge must be
        // the SAME float, not merely close — `k as f32 + 1.0` is exact well past any
        // realistic column count, so `quad_edge` collapses both to one expression.
        let (base, unit) = (px(3.7), px(8.437_5));
        for k in [0usize, 1, 7, 79, 210, 511] {
            assert_eq!(
                quad_edge(base, unit, k, 1.0),
                quad_edge(base, unit, k + 1, 0.0),
                "cell {k}"
            );
        }
    }
}

#[cfg(test)]
mod term_color_tests {
    use super::*;

    const TEST_ANSI: [u32; 16] = [
        0x000001, 0x000002, 0x000003, 0x000004, 0x000005, 0x000006, 0x000007, 0x000008, 0x000009,
        0x00000a, 0x00000b, 0x00000c, 0x00000d, 0x00000e, 0x00000f, 0x000010,
    ];

    #[test]
    fn indexed_base16_reads_the_theme_palette_not_xterm() {
        // Slot 1 (red) must come from the provided palette — the whole point of F2.
        let got = term_color_to_hsla(TermColor::Indexed(1), Hsla::default(), &TEST_ANSI);
        assert_eq!(got, rgb(0x000002).into());
        let got = term_color_to_hsla(TermColor::Indexed(15), Hsla::default(), &TEST_ANSI);
        assert_eq!(got, rgb(0x000010).into());
    }

    #[test]
    fn indexed_cube_and_ramp_stay_universal() {
        // 196 is pure red in the xterm cube regardless of theme palette.
        let got = term_color_to_hsla(TermColor::Indexed(196), Hsla::default(), &TEST_ANSI);
        assert_eq!(got, rgb_to_hsla(255, 0, 0));
        // Grayscale ramp end.
        let got = term_color_to_hsla(TermColor::Indexed(255), Hsla::default(), &TEST_ANSI);
        assert_eq!(got, rgb_to_hsla(238, 238, 238));
    }

    #[test]
    fn default_color_passes_through() {
        let default: Hsla = rgb(0x123456).into();
        let got = term_color_to_hsla(TermColor::Default, default, &TEST_ANSI);
        assert_eq!(got, default);
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
