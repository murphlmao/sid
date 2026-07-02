//! SFTP browser entity (Plan 3.4, S1) — mirrors 3C's [`super::TerminalSession`] pattern:
//! connect a client, open an SFTP session, and marshal the async calls through the shared
//! `sid-ssh` Tokio runtime (see [`super::terminal::ssh_runtime`]), results carried back to
//! the entity via `cx.spawn`/`this.update`. Render (S3) paints purely from the cached
//! `entries`; every SFTP call runs off gpui's own executor, never inline in `render`.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, IntoElement, Render, SharedString, Window,
    div, prelude::*, px, rgb, uniform_list,
};
use sid_core::ssh::{SftpEntry, SftpSession, SshClient, SshError};
use sid_secrets::SecretStore;
use sid_ssh::RusshClientFactory;
use sid_store::Host;
use tokio::sync::Mutex as AsyncMutex;

use crate::ssh_connect::{connect_params, resolve_secret};
use crate::ui::terminal::ssh_runtime;

// ---- neutral grayscale palette, matches app.rs/terminal.rs -----------------------------
const BG: u32 = 0x161618;
const BORDER: u32 = 0x2c2c30;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_BG: u32 = 0x33343a;
const ROW_ALT: u32 = 0x1c1c20;

/// Monospace family for the breadcrumb/entry list, matching the rest of the app.
const MONO: &str = "DejaVu Sans Mono";

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

    // ---- navigation (S3) -----------------------------------------------------------------

    /// Re-list `path` over the existing session and, on success, make it current. A failed
    /// navigate leaves `path`/`entries` untouched — a bad click doesn't blank the view —
    /// and surfaces the failure in `error` instead.
    fn navigate(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        self.error = None;
        let list_path = path.clone();
        cx.spawn(async move |this, cx| {
            let handle =
                ssh_runtime().spawn(async move { session.lock().await.list(&list_path).await });
            let result = handle.await;
            let _ = this.update(cx, |browser, cx| {
                match result {
                    Ok(Ok(mut entries)) => {
                        sort_entries(&mut entries);
                        browser.path = path;
                        browser.entries = entries;
                    }
                    Ok(Err(e)) => browser.error = Some(e.to_string()),
                    Err(join_err) => {
                        browser.error = Some(format!("list task panicked: {join_err}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Navigate into a child directory of the current path (an entry-row click).
    fn enter_dir(&mut self, name: &str, cx: &mut Context<Self>) {
        let target = join_path(&self.path, name);
        self.navigate(target, cx);
    }

    /// `↑ up`: navigate to the current path's parent.
    fn go_up(&mut self, cx: &mut Context<Self>) {
        let target = parent_path(&self.path);
        self.navigate(target, cx);
    }

    /// Jump directly to `path` — a breadcrumb segment click.
    fn go_to(&mut self, path: String, cx: &mut Context<Self>) {
        self.navigate(path, cx);
    }

    /// `⟳ refresh`: re-list the current path.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self.navigate(path, cx);
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

// ---- rendering (S3): toolbar + breadcrumb + entry list ---------------------------------

impl SftpBrowser {
    /// The `Ready` view: a toolbar (up/refresh/breadcrumb) above a status line (if any)
    /// above a scrollable entry list. Painted purely from `self.entries`/`self.path` —
    /// every SFTP call that could change them already ran, off gpui's executor, before
    /// `cx.notify()` scheduled this render.
    fn render_browser(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let count = self.entries.len();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .child(self.toolbar(cx))
            .when_some(self.error.clone(), |el, msg| el.child(status_line(&msg)))
            .child(
                uniform_list(
                    "sftp-entries",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _win, cx| {
                        range.map(|ix| this.entry_row(ix, cx)).collect::<Vec<_>>()
                    }),
                )
                .flex_1(),
            )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // Small text-button factory, mirroring `app.rs`'s `host_row` action buttons.
        let button = |id: (&'static str, usize), label: SharedString, color: u32| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(color))
                .hover(|s| s.bg(rgb(ACTIVE_BG)))
                .child(label)
        };

        let up = button(("sftp-up", 0), "↑ up".into(), FG_DIM).on_click(cx.listener(
            |this, _ev: &ClickEvent, _window, cx| this.go_up(cx),
        ));
        let refresh = button(("sftp-refresh", 0), "⟳".into(), FG_DIM).on_click(cx.listener(
            |this, _ev: &ClickEvent, _window, cx| this.refresh(cx),
        ));

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(up)
            .child(refresh)
            .child(self.breadcrumb(cx))
    }

    /// Clickable breadcrumb of the current path's segments — root first, then each
    /// component built up cumulatively (`/a/b` → `/`, `a` (→`/a`), `b` (→`/a/b`)).
    fn breadcrumb(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut children = vec![self.breadcrumb_segment(0, "/".into(), "/".to_string(), cx)];
        let mut acc = String::new();
        for (ix, part) in self.path.split('/').filter(|s| !s.is_empty()).enumerate() {
            acc.push('/');
            acc.push_str(part);
            children.push(self.breadcrumb_segment(ix + 1, part.to_string().into(), acc.clone(), cx));
        }
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .flex_1()
            .text_sm()
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
        let is_current = target == self.path;
        div()
            .id(("sftp-crumb", ix))
            .px_1()
            .rounded_md()
            .cursor_pointer()
            .text_color(rgb(if is_current { FG } else { FG_DIM }))
            .hover(|s| s.bg(rgb(ACTIVE_BG)))
            .child(label)
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                this.go_to(target.clone(), cx);
            }))
    }

    /// One row of the entry list: dir/file glyph, name, right-aligned size + mtime.
    /// Directory rows are clickable (navigate in); file rows aren't yet (S4 adds
    /// per-file download).
    fn entry_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let entry = &self.entries[ix];
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

        div()
            .id(("sftp-entry", ix))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .w_full()
            .px_4()
            .py_1()
            .bg(rgb(if alt { ROW_ALT } else { BG }))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(div().w(px(18.)).text_color(rgb(FG_DIM)).child(glyph))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(rgb(FG))
                    .child(name.clone()),
            )
            .child(div().w(px(80.)).text_xs().text_color(rgb(FG_DIM)).child(size))
            .child(div().w(px(140.)).text_xs().text_color(rgb(FG_DIM)).child(mtime))
            .when(is_dir, |el| {
                el.cursor_pointer().hover(|s| s.bg(rgb(ACTIVE_BG))).on_click(cx.listener(
                    move |this, _ev: &ClickEvent, _window, cx| this.enter_dir(&name, cx),
                ))
            })
    }
}

fn message_pane(text: &str) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(BG))
        .text_color(rgb(FG_DIM))
        .font_family(MONO)
        .child(text.to_string())
}

fn status_line(text: &str) -> impl IntoElement {
    div()
        .px_4()
        .py_1()
        .text_sm()
        .text_color(rgb(FG_DIM))
        .child(text.to_string())
}

/// A human-readable byte count (`"512 B"`, `"12.3 KB"`, …). Display-only — not in the
/// plan's tested surface (path/sort logic only).
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

/// Format a Unix `mtime_secs` as `YYYY-MM-DD HH:MM` (UTC; no timezone/locale support —
/// good enough for a browse view). Display-only, same as `human_size`.
fn format_mtime(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let secs_of_day = epoch_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

/// Howard Hinnant's `civil_from_days`: days-since-epoch (1970-01-01) -> (year, month, day).
/// A well-known, publicly documented algorithm — not reimplemented from scratch here.
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

impl Render for SftpBrowser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.status.clone() {
            SftpStatus::Connecting => message_pane("Connecting…").into_any_element(),
            SftpStatus::Failed(err) => {
                message_pane(&format!("SFTP connect failed: {err}")).into_any_element()
            }
            SftpStatus::Closed => message_pane("SFTP session closed.").into_any_element(),
            SftpStatus::Ready => self.render_browser(cx).into_any_element(),
        }
    }
}
