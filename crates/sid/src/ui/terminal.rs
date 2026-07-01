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

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, Font, FontStyle, FontWeight,
    Hsla, IntoElement, KeyDownEvent, Keystroke, Pixels, Render, ShapedLine, TextRun,
    UnderlineStyle, Window, canvas, div, font, point, prelude::*, px, rgb,
};
use sid_core::ssh::{SshClient, SshError, SshShell};
use sid_core::term::{TermCell, TermColor, TerminalScreen};
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
    focus_handle: FocusHandle,
    /// Set once, the render after a successful connect, to pull keyboard focus onto the
    /// terminal without re-stealing it on every later re-render (e.g. output arriving while
    /// the user is focused elsewhere).
    needs_focus: bool,
}

/// How often the read-loop hops onto the Tokio runtime to drain the shell's output buffer.
// ponytail: fixed-interval poll, not event-driven — fine at ~30Hz for a terminal; revisit only
// if `SshShell` grows a readable-notify.
const POLL_INTERVAL: Duration = Duration::from_millis(33);

/// Placeholder pane size until C4's viewport-driven resize computes the real one.
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

// ---- rendering (C3): grid painted from `screen.cells()` ---------------------------------

/// Matches `app.rs`'s neutral palette so the terminal pane blends with the rest of the app
/// instead of announcing itself as a separate widget.
const TERM_BG: u32 = 0x161618;
const TERM_FG: u32 = 0xdcdce0;
const TERM_FONT_SIZE: Pixels = px(14.);

/// Monospace family; gpui falls back to a proportional font if it's missing locally.
const MONO: &str = "DejaVu Sans Mono";

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
                focus_handle: cx.focus_handle(),
                needs_focus: false,
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
                        session.needs_focus = true;
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

    /// Paint the grid: one `shape_line` call per row (gpui shapes a whole multi-run line at
    /// once, so the row — not the cell — is the unit of work), then `paint_background` +
    /// `paint` per shaped row inside a `canvas`. The canvas fills whatever space the parent
    /// layout gives it; C4's resize detection (below) reads that real size back out of the
    /// canvas's own paint bounds and reconciles `self.rows`/`self.cols` against it.
    fn render_grid(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cells = self.screen.cells();
        let (cursor_row, cursor_col) = self.screen.cursor_position();
        let base_font = font(MONO);
        let default_fg: Hsla = rgb(TERM_FG).into();
        let default_bg: Hsla = rgb(TERM_BG).into();

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
            .bg(rgb(TERM_BG))
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
}

impl Render for TerminalSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus {
            self.needs_focus = false;
            window.focus(&self.focus_handle);
        }
        match &self.status {
            SessionStatus::Connecting => message_pane("Connecting…").into_any_element(),
            SessionStatus::Failed(err) => {
                message_pane(&format!("Connection failed: {err}")).into_any_element()
            }
            SessionStatus::Closed => message_pane("Session closed.").into_any_element(),
            SessionStatus::Connected => self.render_grid(window, cx).into_any_element(),
        }
    }
}

impl Focusable for TerminalSession {
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

fn message_pane(text: &str) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(TERM_BG))
        .text_color(rgb(TERM_FG))
        .font_family(MONO)
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
