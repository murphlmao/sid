//! SSH tab widget — host list + connection state + PTY pane + SFTP sub-panel
//! + per-host command history + edit-in-place state machine.
//!
//! Pure-Rust state types are factored out so they can be unit-tested without
//! constructing a real `SshClient` or `PtyProvider`. The widget is a thin
//! render layer over the state.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_core::adapters::pty::{PtyProvider, TerminalScreen};
use sid_core::adapters::ssh::{SftpEntry, SshClient};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, FooterHint, RenderTarget, Widget, WidgetId};
use sid_store::{SshHost, SshHostSource};
use sid_ui::Theme;
use sid_ui::themes::cosmos;

// ---------------------------------------------------------------------------
// SSH config entry (lite copy — widget crate never names sid-ssh)
// ---------------------------------------------------------------------------

/// A lite copy of `sid_ssh::SshConfigEntry`. The widget crate never names a
/// sid-ssh type (adapter pattern). The binary's wire layer converts.
///
/// # Examples
///
/// ```
/// use sid_widgets::ssh::SshConfigEntryLite;
/// let e = SshConfigEntryLite {
///     alias: "x".into(),
///     host: "h".into(),
///     port: 22,
///     user: "u".into(),
///     identity_file: None,
/// };
/// assert_eq!(e.port, 22);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshConfigEntryLite {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
}

// ---------------------------------------------------------------------------
// SshState — host list merge + selection
// ---------------------------------------------------------------------------

/// Host list + selection state.
pub struct SshState {
    store_hosts: Vec<SshHost>,
    config_entries: Vec<SshConfigEntryLite>,
    merged: Vec<SshHost>,
    selected_idx: usize,
}

impl SshState {
    /// Construct from the store's manual hosts plus ssh-config entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::ssh::SshState;
    /// let s = SshState::new(vec![], vec![]);
    /// assert!(s.selected_alias().is_none());
    /// ```
    pub fn new(store_hosts: Vec<SshHost>, config_entries: Vec<SshConfigEntryLite>) -> Self {
        let mut s = Self {
            store_hosts,
            config_entries,
            merged: Vec::new(),
            selected_idx: 0,
        };
        s.recompute_merged();
        s
    }

    fn recompute_merged(&mut self) {
        let mut by_alias: BTreeMap<String, SshHost> = BTreeMap::new();
        for cfg in &self.config_entries {
            by_alias.insert(
                cfg.alias.clone(),
                SshHost {
                    alias: cfg.alias.clone(),
                    host: cfg.host.clone(),
                    port: cfg.port,
                    user: cfg.user.clone(),
                    identity_file: cfg.identity_file.clone(),
                    source: SshHostSource::SshConfig,
                    last_connected: 0,
                    command_history: Vec::new(),
                    last_sftp_path: None,
                    auth_kind: sid_store::SshAuthKind::default(),
                },
            );
        }
        for h in &self.store_hosts {
            by_alias.insert(h.alias.clone(), h.clone());
        }
        self.merged = by_alias.into_values().collect();
        if self.merged.is_empty() {
            self.selected_idx = 0;
        } else if self.selected_idx >= self.merged.len() {
            self.selected_idx = self.merged.len() - 1;
        }
    }

    pub fn visible_hosts(&self) -> &[SshHost] {
        &self.merged
    }

    pub fn selected_alias(&self) -> Option<&str> {
        self.merged.get(self.selected_idx).map(|h| h.alias.as_str())
    }

    pub fn selected_host(&self) -> Option<&SshHost> {
        self.merged.get(self.selected_idx)
    }

    pub fn select_next(&mut self) {
        if self.merged.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.merged.len();
    }

    pub fn select_prev(&mut self) {
        if self.merged.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + self.merged.len() - 1) % self.merged.len();
    }

    pub fn set_store_hosts(&mut self, hosts: Vec<SshHost>) {
        self.store_hosts = hosts;
        self.recompute_merged();
    }

    pub fn set_config_entries(&mut self, entries: Vec<SshConfigEntryLite>) {
        self.config_entries = entries;
        self.recompute_merged();
    }
}

// ---------------------------------------------------------------------------
// ConnectionState — connection life cycle
// ---------------------------------------------------------------------------

/// Phase of the current connection attempt / live connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ConnectionPhase {
    #[default]
    Idle,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectionState {
    phase: ConnectionPhase,
    alias: Option<String>,
    error: Option<String>,
}

impl ConnectionState {
    pub fn phase(&self) -> ConnectionPhase {
        self.phase
    }
    pub fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }
    pub fn error_message(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn begin_connecting(&mut self, alias: String) {
        self.phase = ConnectionPhase::Connecting;
        self.alias = Some(alias);
        self.error = None;
    }
    pub fn mark_connected(&mut self) {
        self.phase = ConnectionPhase::Connected;
        self.error = None;
    }
    pub fn mark_failed(&mut self, e: String) {
        self.phase = ConnectionPhase::Failed;
        self.error = Some(e);
    }
    pub fn mark_disconnected(&mut self) {
        self.phase = ConnectionPhase::Disconnected;
    }
    pub fn reset(&mut self) {
        self.phase = ConnectionPhase::Idle;
        self.alias = None;
        self.error = None;
    }
}

// ---------------------------------------------------------------------------
// PtyPane — terminal screen wrapper
// ---------------------------------------------------------------------------

/// Owns the embedded terminal screen for the SSH tab's right pane.
///
/// The pane wraps a boxed [`TerminalScreen`] (an adapter trait owned by
/// `sid-core`) so the widget crate never names `vt100` directly. The concrete
/// `Vt100Screen` is constructed by the binary (or by tests) and handed in.
pub struct PtyPane {
    screen: Box<dyn TerminalScreen>,
}

impl PtyPane {
    /// Wrap an existing `TerminalScreen` (typically a `Vt100Screen`).
    pub fn new(screen: Box<dyn TerminalScreen>) -> Self {
        Self { screen }
    }
    /// Feed bytes from the remote into the screen.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.screen.feed(bytes);
    }
    /// Resize the underlying screen. Idempotent if `(rows, cols)` already
    /// matches the current size.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.screen.resize(rows, cols);
    }
    /// Resize the screen to the inner dimensions of the given pane area.
    ///
    /// Convenience wrapper: subtracts the surrounding border (`1` cell on
    /// each side) and clamps to a minimum of `1` row/col. A no-op if the
    /// resulting `(rows, cols)` already matches the current screen size.
    ///
    /// This is called by the binary (or test harness) **before** the next
    /// `render_into_frame`, because the render pass is `&self` and must not
    /// mutate the screen — see the comment on
    /// `SshWidget::pty_pane_resize_to_area`.
    pub fn resize_to_area(&mut self, rows: u16, cols: u16) {
        let target_rows = rows.max(1);
        let target_cols = cols.max(1);
        if self.size() != (target_rows, target_cols) {
            self.resize(target_rows, target_cols);
        }
    }
    /// Current size as `(rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        self.screen.size()
    }
    /// Visible buffer, one string per row. Each row is `cols` characters wide
    /// after `Vt100Screen::lines` padding.
    pub fn lines(&self) -> Vec<String> {
        self.screen.lines()
    }
    /// Cursor position as `(row, col)`, both zero-indexed against the inner
    /// area (`row < rows`, `col < cols`).
    pub fn cursor_position(&self) -> (u16, u16) {
        self.screen.cursor_position()
    }
}

// ---------------------------------------------------------------------------
// CommandHistory — capped, dedup'd ring buffer
// ---------------------------------------------------------------------------

/// Bounded, deduplicating command history.
#[derive(Clone, Debug)]
pub struct CommandHistory {
    entries: VecDeque<String>,
    cap: usize,
}

impl CommandHistory {
    pub fn new(cap: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(cap.min(1024)),
            cap: cap.max(1),
        }
    }
    pub fn from_vec(v: Vec<String>, cap: usize) -> Self {
        let mut h = Self::new(cap);
        for s in v {
            h.push(s);
        }
        h
    }
    pub fn push(&mut self, cmd: String) {
        if cmd.trim().is_empty() {
            return;
        }
        if self.entries.back().map(|s| s == &cmd).unwrap_or(false) {
            return;
        }
        if self.entries.len() == self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(cmd);
    }
    pub fn entries(&self) -> Vec<String> {
        self.entries.iter().cloned().collect()
    }
    pub fn to_vec(&self) -> Vec<String> {
        self.entries.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// SFTP panel — directory browsing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpPanelVisibility {
    Hidden,
    Visible,
}

#[derive(Clone, Debug)]
pub struct SftpPanel {
    visibility: SftpPanelVisibility,
    cwd: String,
    entries: Vec<SftpEntry>,
    selected_idx: usize,
}

impl SftpPanel {
    pub fn new() -> Self {
        Self {
            visibility: SftpPanelVisibility::Hidden,
            cwd: "/".into(),
            entries: Vec::new(),
            selected_idx: 0,
        }
    }
    pub fn visibility(&self) -> SftpPanelVisibility {
        self.visibility
    }
    pub fn toggle(&mut self) {
        self.visibility = match self.visibility {
            SftpPanelVisibility::Hidden => SftpPanelVisibility::Visible,
            SftpPanelVisibility::Visible => SftpPanelVisibility::Hidden,
        };
    }
    pub fn cwd(&self) -> &str {
        &self.cwd
    }
    pub fn set_cwd(&mut self, path: String) {
        self.cwd = if path.is_empty() { "/".into() } else { path };
        self.entries.clear();
        self.selected_idx = 0;
    }
    pub fn entries(&self) -> &[SftpEntry] {
        &self.entries
    }
    pub fn set_entries(&mut self, entries: Vec<SftpEntry>) {
        self.entries = entries;
        self.selected_idx = 0;
    }
    pub fn selected_entry(&self) -> Option<&SftpEntry> {
        self.entries.get(self.selected_idx)
    }
    pub fn selected_remote_path(&self) -> Option<String> {
        let e = self.selected_entry()?;
        let mut p = self.cwd.clone();
        if !p.ends_with('/') {
            p.push('/');
        }
        p.push_str(&e.name);
        Some(p)
    }
    pub fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.entries.len();
    }
    pub fn select_prev(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + self.entries.len() - 1) % self.entries.len();
    }
    pub fn ascend(&mut self) {
        if self.cwd == "/" {
            return;
        }
        let trimmed = self.cwd.trim_end_matches('/');
        if let Some(idx) = trimmed.rfind('/') {
            let parent = if idx == 0 {
                "/".into()
            } else {
                trimmed[..idx].to_string()
            };
            self.cwd = parent;
            self.entries.clear();
            self.selected_idx = 0;
        }
    }
}

impl Default for SftpPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute (remote, local) for downloading the SFTP panel's currently-
/// selected entry into `local_dir`. Returns `None` if no selection or dir.
pub fn prepare_download(panel: &SftpPanel, local_dir: &Path) -> Option<(String, PathBuf)> {
    let entry = panel.selected_entry()?;
    if entry.is_dir {
        return None;
    }
    let remote = panel.selected_remote_path()?;
    let local = local_dir.join(&entry.name);
    Some((remote, local))
}

/// Compute (local, remote) for uploading `local` into the panel's cwd.
pub fn prepare_upload(panel: &SftpPanel, local: &Path) -> Option<(PathBuf, String)> {
    if !local.is_file() {
        return None;
    }
    let basename = local.file_name()?.to_str()?;
    let mut remote = panel.cwd().to_string();
    if !remote.ends_with('/') {
        remote.push('/');
    }
    remote.push_str(basename);
    Some((local.to_path_buf(), remote))
}

// ---------------------------------------------------------------------------
// SftpEditState — edit-in-place state machine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum SftpEditPhase {
    #[default]
    Idle,
    Downloading,
    Editing,
    Uploading,
    Done,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct SftpEditState {
    phase: SftpEditPhase,
    remote_path: Option<String>,
    local_path: Option<PathBuf>,
    error: Option<String>,
}

impl SftpEditState {
    pub fn phase(&self) -> SftpEditPhase {
        self.phase
    }
    pub fn remote_path(&self) -> Option<&str> {
        self.remote_path.as_deref()
    }
    pub fn local_path(&self) -> Option<&Path> {
        self.local_path.as_deref()
    }
    pub fn error_message(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn begin_download(&mut self, remote: String, local: PathBuf) {
        self.phase = SftpEditPhase::Downloading;
        self.remote_path = Some(remote);
        self.local_path = Some(local);
        self.error = None;
    }
    pub fn mark_download_complete(&mut self) {
        self.phase = SftpEditPhase::Editing;
    }
    pub fn mark_editor_done(&mut self, ok: bool) {
        self.phase = if ok {
            SftpEditPhase::Uploading
        } else {
            SftpEditPhase::Failed
        };
    }
    pub fn mark_upload_complete(&mut self) {
        self.phase = SftpEditPhase::Done;
    }
    pub fn mark_failed(&mut self, e: String) {
        self.phase = SftpEditPhase::Failed;
        self.error = Some(e);
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// SshWidget
// ---------------------------------------------------------------------------

/// Which pane in the SSH tab currently owns keyboard input.
///
/// Tab/Shift+Tab cycle. The accent border is rendered on the focused pane;
/// the other pane uses the muted color.
///
/// # Examples
///
/// ```
/// use sid_widgets::ssh::SshFocus;
/// assert_ne!(SshFocus::Hosts, SshFocus::Detail);
/// assert_eq!(SshFocus::default(), SshFocus::Hosts);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SshFocus {
    /// The left-hand host list.
    #[default]
    Hosts,
    /// The right-hand detail/PTY/SFTP pane (read-only for now).
    Detail,
}

impl SshFocus {
    /// Cycle to the next focus (Tab).
    pub fn next(self) -> Self {
        match self {
            SshFocus::Hosts => SshFocus::Detail,
            SshFocus::Detail => SshFocus::Hosts,
        }
    }
    /// Cycle to the previous focus (Shift+Tab).
    pub fn prev(self) -> Self {
        // 2-way: prev == next.
        self.next()
    }
}

/// SSH tab widget.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::SshWidget;
/// let w = SshWidget::new();
/// assert_eq!(w.id().as_str(), "ssh.root");
/// ```
pub struct SshWidget {
    state: SshState,
    connection: ConnectionState,
    sftp_panel: SftpPanel,
    edit_state: SftpEditState,
    history: BTreeMap<String, CommandHistory>,
    id: WidgetId,
    focused_pane: SshFocus,
    // Embedded terminal screen for the right pane. `None` until the binary
    // (or a test) calls `set_pty_pane` after a successful connect.
    pty_pane: Option<PtyPane>,
    // Set by the widget when the user presses `Enter` on a host in the
    // Hosts pane. Drained by the wire layer each frame; on drain the wire
    // layer spawns an async connect task. None when no connect is pending.
    //
    // This is the outbox half of "Option A" from the SSH wiring plan: the
    // widget marks intent (alias), the binary acts on it. See
    // `take_pending_connect`.
    pending_connect: Option<String>,
    // Injected by wire.rs in production.
    _ssh_factory: Option<Arc<dyn Fn() -> Box<dyn SshClient> + Send + Sync>>,
    _pty_provider: Option<Arc<dyn PtyProvider>>,
}

impl SshWidget {
    /// Zero-arg constructor (kept for `wire::build_app` compatibility).
    pub fn new() -> Self {
        Self::with_state(SshState::new(Vec::new(), Vec::new()))
    }

    /// Construct with an explicit state value.
    pub fn with_state(state: SshState) -> Self {
        let history = state
            .visible_hosts()
            .iter()
            .map(|h| {
                (
                    h.alias.clone(),
                    CommandHistory::from_vec(h.command_history.clone(), 100),
                )
            })
            .collect();
        Self {
            state,
            connection: ConnectionState::default(),
            sftp_panel: SftpPanel::new(),
            edit_state: SftpEditState::default(),
            history,
            id: WidgetId::new("ssh.root"),
            focused_pane: SshFocus::default(),
            pty_pane: None,
            pending_connect: None,
            _ssh_factory: None,
            _pty_provider: None,
        }
    }

    /// Currently-focused pane.
    pub fn focused_pane(&self) -> SshFocus {
        self.focused_pane
    }

    /// Stable string label for the focused pane (`"Hosts"` / `"Detail"`).
    pub fn focused_pane_label(&self) -> &'static str {
        match self.focused_pane {
            SshFocus::Hosts => "Hosts",
            SshFocus::Detail => "Detail",
        }
    }

    /// Cycle focus forward (Tab).
    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    /// Cycle focus backward (Shift+Tab).
    pub fn focus_prev(&mut self) {
        self.focused_pane = self.focused_pane.prev();
    }

    /// Focus the pane that contains the given coordinate. No-op when the
    /// coordinate falls outside `area`.
    ///
    /// Layout mirrors [`Self::render_into_frame`]: a 40/60 horizontal split.
    /// Columns left of the 40% boundary focus [`SshFocus::Hosts`]; everything
    /// else focuses [`SshFocus::Detail`]. Rendering rereads `focused_pane`
    /// next frame; this method does not invoke any render path.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::Rect;
    /// use sid_widgets::SshWidget;
    /// use sid_widgets::ssh::SshFocus;
    /// let mut w = SshWidget::new();
    /// let area = Rect { x: 0, y: 0, width: 100, height: 24 };
    /// // Click in the right pane (col 80): focuses Detail.
    /// w.focus_at(area, 80, 5);
    /// assert_eq!(w.focused_pane(), SshFocus::Detail);
    /// // Click in the left pane (col 10): focuses Hosts.
    /// w.focus_at(area, 10, 5);
    /// assert_eq!(w.focused_pane(), SshFocus::Hosts);
    /// // Click outside the area is a no-op.
    /// w.focus_at(area, 200, 5);
    /// assert_eq!(w.focused_pane(), SshFocus::Hosts);
    /// ```
    pub fn focus_at(&mut self, area: Rect, col: u16, row: u16) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if col < area.x || col >= area.x.saturating_add(area.width) {
            return;
        }
        if row < area.y || row >= area.y.saturating_add(area.height) {
            return;
        }
        let split_col = area.x.saturating_add(area.width.saturating_mul(40) / 100);
        self.focused_pane = if col < split_col {
            SshFocus::Hosts
        } else {
            SshFocus::Detail
        };
    }

    /// Inject providers (called by `wire.rs`).
    pub fn with_providers(
        mut self,
        ssh_factory: Arc<dyn Fn() -> Box<dyn SshClient> + Send + Sync>,
        pty_provider: Arc<dyn PtyProvider>,
    ) -> Self {
        self._ssh_factory = Some(ssh_factory);
        self._pty_provider = Some(pty_provider);
        self
    }

    pub fn state(&self) -> &SshState {
        &self.state
    }
    pub fn state_mut(&mut self) -> &mut SshState {
        &mut self.state
    }
    pub fn connection(&self) -> &ConnectionState {
        &self.connection
    }
    pub fn connection_mut(&mut self) -> &mut ConnectionState {
        &mut self.connection
    }
    pub fn sftp_panel(&self) -> &SftpPanel {
        &self.sftp_panel
    }
    pub fn sftp_panel_mut(&mut self) -> &mut SftpPanel {
        &mut self.sftp_panel
    }
    pub fn edit_state(&self) -> &SftpEditState {
        &self.edit_state
    }
    pub fn edit_state_mut(&mut self) -> &mut SftpEditState {
        &mut self.edit_state
    }
    /// Borrow the embedded PTY pane, if one is attached.
    pub fn pty_pane(&self) -> Option<&PtyPane> {
        self.pty_pane.as_ref()
    }
    /// Mutably borrow the embedded PTY pane, if one is attached.
    pub fn pty_pane_mut(&mut self) -> Option<&mut PtyPane> {
        self.pty_pane.as_mut()
    }
    /// Attach a `PtyPane`. Called by the binary after a successful SSH
    /// `request_shell` returns a session bound to a fresh `Vt100Screen`. In
    /// tests we feed the pane directly via `pty_pane_mut().unwrap().feed(...)`.
    pub fn set_pty_pane(&mut self, pane: PtyPane) {
        self.pty_pane = Some(pane);
    }
    /// Detach the `PtyPane` (e.g. on disconnect).
    pub fn take_pty_pane(&mut self) -> Option<PtyPane> {
        self.pty_pane.take()
    }

    /// Borrow the alias the user just asked to connect to (set by pressing
    /// `Enter` on a host in the Hosts pane). Read-only; the wire layer drains
    /// via [`Self::take_pending_connect`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::SshWidget;
    /// let w = SshWidget::new();
    /// assert!(w.peek_pending_connect().is_none());
    /// ```
    pub fn peek_pending_connect(&self) -> Option<&str> {
        self.pending_connect.as_deref()
    }

    /// Drain the pending connect intent. The wire layer calls this each
    /// frame; when it returns `Some(alias)` the binary spawns the connect
    /// task and the widget's `ConnectionState` is already in
    /// `Connecting` (set by the Enter handler).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::SshWidget;
    /// let mut w = SshWidget::new();
    /// // No pending connect on a freshly built widget.
    /// assert!(w.take_pending_connect().is_none());
    /// ```
    pub fn take_pending_connect(&mut self) -> Option<String> {
        self.pending_connect.take()
    }

    /// Test / wire-layer hook: directly seed the pending-connect slot. The
    /// widget itself only sets this via the Enter key path; production code
    /// in the binary uses it to forge the same intent from a future
    /// "Connect" action or palette entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::SshWidget;
    /// let mut w = SshWidget::new();
    /// w.set_pending_connect(Some("acme".into()));
    /// assert_eq!(w.take_pending_connect().as_deref(), Some("acme"));
    /// ```
    pub fn set_pending_connect(&mut self, alias: Option<String>) {
        self.pending_connect = alias;
    }
    /// Resize the embedded PTY pane to match the inner dimensions of `area`
    /// (the right-hand body rect, **before** the border subtraction).
    ///
    /// `render_into_frame(&self, ...)` cannot mutate the screen (and the
    /// project rules forbid interior mutability inside render), so the binary
    /// — and the snapshot tests — must call this on a `&mut SshWidget` before
    /// every frame whose right-pane area changed. No-op when no pane is
    /// attached or when the size already matches.
    pub fn pty_pane_resize_to_area(&mut self, area: Rect) {
        if let Some(pane) = self.pty_pane.as_mut() {
            // Subtract one cell on each side for the border. Clamp to >=1.
            let rows = area.height.saturating_sub(2).max(1);
            let cols = area.width.saturating_sub(2).max(1);
            pane.resize_to_area(rows, cols);
        }
    }
    pub fn history_for(&self, alias: &str) -> Option<&CommandHistory> {
        self.history.get(alias)
    }
    pub fn record_command(&mut self, alias: &str, cmd: String) {
        self.history
            .entry(alias.to_string())
            .or_insert_with(|| CommandHistory::new(100))
            .push(cmd);
    }

    /// Render the widget into a ratatui [`Frame`]. Used by the insta snapshot
    /// tests and by the future direct-frame plumbing in the binary.
    ///
    /// Layout:
    ///
    /// ```text
    /// ┌──────────────────┬─────────────────────────────────┐
    /// │ Hosts            │ Status header                   │
    /// │  ● my-prod       ├─────────────────────────────────┤
    /// │  ○ staging (cfg) │ Body (disconnected hint / PTY / │
    /// │                  │ SFTP listing)                   │
    /// │                  ├─────────────────────────────────┤
    /// │                  │ Last command (history bar)      │
    /// └──────────────────┴─────────────────────────────────┘
    /// ```
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);
        let left = split[0];
        let right = split[1];

        // Right side: status header (3 lines, bordered), body, then a 1-line
        // history bar at the bottom.
        let right_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(right);
        let status_rect = right_split[0];
        let body_rect = right_split[1];
        let history_rect = right_split[2];

        self.render_host_list(frame, left, theme);
        self.render_status_header(frame, status_rect, theme);
        if self.sftp_panel.visibility() == SftpPanelVisibility::Visible {
            self.render_sftp_body(frame, body_rect, theme);
        } else {
            self.render_pty_body(frame, body_rect, theme);
        }
        self.render_history_bar(frame, history_rect, theme);
    }

    fn render_host_list(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let hosts = self.state.visible_hosts();
        let selected = self.state.selected_alias();
        let mut lines: Vec<Line<'_>> = Vec::with_capacity(hosts.len().max(1));
        if hosts.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no hosts configured)",
                Style::default().fg(theme.muted.into()),
            )));
        } else {
            for h in hosts {
                let is_selected = selected == Some(h.alias.as_str());
                let dot = if is_selected { '●' } else { '○' };
                let marker = if is_selected { '>' } else { ' ' };
                let suffix = if h.source == SshHostSource::SshConfig {
                    " (cfg)"
                } else {
                    ""
                };
                let label = format!("{marker} {dot} {}{suffix}", h.alias);
                let style = if is_selected {
                    Style::default()
                        .fg(theme.accent_primary.into())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.foreground.into())
                };
                lines.push(Line::from(Span::styled(label, style)));
            }
        }
        let focused = self.focused_pane == SshFocus::Hosts;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Hosts ")
            .title_style(title_style);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_status_header(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let phase = self.connection.phase();
        let alias = self.connection.alias().unwrap_or("");
        let (dot_color, label) = match phase {
            ConnectionPhase::Idle | ConnectionPhase::Disconnected => {
                (theme.muted, "Disconnected".to_string())
            }
            ConnectionPhase::Connecting => (theme.accent_warning, format!("Connecting to {alias}")),
            ConnectionPhase::Connected => (theme.accent_success, format!("Connected to {alias}")),
            ConnectionPhase::Failed => {
                let err = self.connection.error_message().unwrap_or("unknown error");
                (theme.accent_error, format!("Failed: {err}"))
            }
        };
        let line = Line::from(vec![
            Span::styled("● ", Style::default().fg(dot_color.into())),
            Span::styled(
                label,
                Style::default()
                    .fg(theme.foreground.into())
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let focused = self.focused_pane == SshFocus::Detail;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Status ")
            .title_style(title_style);
        frame.render_widget(Paragraph::new(line).block(block), rect);
    }

    fn render_pty_body(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let phase = self.connection.phase();
        let session_title = match self.state.selected_alias() {
            Some(alias) => format!(" {alias} "),
            None => " (no host selected) ".to_string(),
        };
        let focused = self.focused_pane == SshFocus::Detail;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(session_title)
            .title_style(title_style);

        // Connected + a PtyPane attached: dump the live vt100 buffer.
        if phase == ConnectionPhase::Connected
            && self.pty_pane.is_some()
            && rect.width >= 2
            && rect.height >= 2
        {
            self.render_pty_screen(frame, rect, theme, block);
            return;
        }

        let body: Vec<Line<'_>> = match phase {
            ConnectionPhase::Connected => {
                // Connected but no pane was attached yet (e.g. the binary
                // hasn't called `set_pty_pane`). Preserve the legacy
                // placeholder so the user still sees that the connection
                // is live.
                let alias = self.connection.alias().unwrap_or("?");
                vec![
                    Line::from(Span::styled(
                        format!("PTY active — connected to {alias}"),
                        Style::default().fg(theme.foreground.into()),
                    )),
                    Line::from(Span::styled(
                        "(terminal buffer rendering not yet wired)",
                        Style::default().fg(theme.muted.into()),
                    )),
                ]
            }
            ConnectionPhase::Connecting => {
                let alias = self.connection.alias().unwrap_or("?");
                vec![Line::from(Span::styled(
                    format!("Connecting to {alias}..."),
                    Style::default().fg(theme.accent_warning.into()),
                ))]
            }
            ConnectionPhase::Failed => {
                let err = self.connection.error_message().unwrap_or("unknown error");
                vec![Line::from(Span::styled(
                    format!("Connection failed: {err}"),
                    Style::default().fg(theme.accent_error.into()),
                ))]
            }
            ConnectionPhase::Idle | ConnectionPhase::Disconnected => vec![
                Line::from(Span::styled(
                    "Select a host with j/k, Enter to connect.",
                    Style::default().fg(theme.foreground.into()),
                )),
                Line::from(Span::styled(
                    "Tab toggles SFTP panel.",
                    Style::default().fg(theme.muted.into()),
                )),
            ],
        };
        frame.render_widget(Paragraph::new(body).block(block), rect);
    }

    /// Render the attached [`PtyPane`]'s live buffer into `rect`.
    ///
    /// Invariants:
    /// * Callers guarantee `self.pty_pane.is_some()`.
    /// * Callers guarantee `rect.width >= 2` and `rect.height >= 2` so the
    ///   inner area is at least `1x1` after subtracting the surrounding
    ///   border.
    ///
    /// The function:
    /// 1. Reads `lines()` from the pane.
    /// 2. If every row is whitespace, replaces them with a single dim
    ///    "(waiting for output…)" hint.
    /// 3. Truncates each row to the inner width so wide vt100 buffers do
    ///    not bleed past the border. (`vt100::Screen::lines` pads each row
    ///    to the screen's column count; we may have a stale pane that
    ///    hasn't been resized yet, so we still guard against overflow.)
    /// 4. Renders the body as a `Paragraph` inside the bordered `block`.
    /// 5. Inverts the cursor cell by flipping `fg <-> bg` directly on the
    ///    buffer. The render is `&self`, but mutating the `Buffer` via
    ///    `frame.buffer_mut()` is **not** widget interior mutability — it
    ///    is just normal rendering output.
    fn render_pty_screen(
        &self,
        frame: &mut Frame<'_>,
        rect: Rect,
        theme: &Theme,
        block: Block<'_>,
    ) {
        // Inner area inside the border. Guaranteed >= 1x1 by the caller's
        // `rect.width >= 2 && rect.height >= 2` precondition.
        let inner_width = rect.width.saturating_sub(2) as usize;
        let inner_height = rect.height.saturating_sub(2) as usize;
        let pane = self
            .pty_pane
            .as_ref()
            .expect("render_pty_screen invariant: pty_pane is Some");
        let raw_lines = pane.lines();

        let all_blank = raw_lines.iter().all(|row| row.chars().all(|c| c == ' '));
        let body: Vec<Line<'_>> = if all_blank {
            vec![Line::from(Span::styled(
                "(waiting for output…)",
                Style::default().fg(theme.muted.into()),
            ))]
        } else {
            raw_lines
                .into_iter()
                .take(inner_height.max(1))
                .map(|row| {
                    // Truncate to inner_width chars; if `row` is shorter
                    // (rare — vt100 pads, but be defensive) it renders as-is.
                    let truncated: String = row.chars().take(inner_width.max(1)).collect();
                    Line::from(Span::styled(
                        truncated,
                        Style::default().fg(theme.foreground.into()),
                    ))
                })
                .collect()
        };

        frame.render_widget(Paragraph::new(body).block(block), rect);

        // Invert the cursor cell. Skipped when we're showing the waiting hint
        // (no live cursor to draw) or when the cursor falls outside the
        // visible inner area.
        if !all_blank {
            let (cur_row, cur_col) = pane.cursor_position();
            if (cur_row as usize) < inner_height && (cur_col as usize) < inner_width {
                let x = rect.x + 1 + cur_col;
                let y = rect.y + 1 + cur_row;
                let buf = frame.buffer_mut();
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(
                        Style::default()
                            .fg(theme.background.into())
                            .bg(theme.foreground.into()),
                    );
                }
            }
        }
    }

    fn render_sftp_body(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let cwd = self.sftp_panel.cwd();
        let entries = self.sftp_panel.entries();
        let mut lines: Vec<Line<'_>> = Vec::with_capacity(entries.len() + 1);
        lines.push(Line::from(Span::styled(
            format!("cwd: {cwd}"),
            Style::default()
                .fg(theme.accent_primary.into())
                .add_modifier(Modifier::BOLD),
        )));
        if entries.is_empty() {
            lines.push(Line::from(Span::styled(
                "(no entries yet)",
                Style::default().fg(theme.muted.into()),
            )));
        } else {
            for e in entries {
                let glyph = if e.is_dir { '/' } else { ' ' };
                let label = format!("  {glyph} {}", e.name);
                lines.push(Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.foreground.into()),
                )));
            }
        }
        let sftp_title = match self.state.selected_alias() {
            Some(alias) => format!(" SFTP · {alias} "),
            None => " SFTP ".to_string(),
        };
        let focused = self.focused_pane == SshFocus::Detail;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(sftp_title)
            .title_style(title_style);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_history_bar(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let last = self
            .state
            .selected_alias()
            .and_then(|alias| self.history.get(alias))
            .and_then(|h| h.entries().into_iter().next_back());
        let text = match last {
            Some(cmd) => format!(" last: {cmd}"),
            None => " (no recent commands)".to_string(),
        };
        let para = Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(theme.muted.into()),
        )));
        frame.render_widget(para, rect);
    }
}

impl Default for SshWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for SshWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }
    fn title(&self) -> &str {
        "SSH"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("N", "new host"),
            FooterHint::new("G", "gen key"),
            FooterHint::new("S", "setup remote"),
            FooterHint::new("K", "keys"),
            FooterHint::new("X", "debug"),
            FooterHint::new("?", "help"),
        ]
    }
    fn render(&self, _target: &mut dyn RenderTarget) {
        // Rendering deferred to the binary's draw() function.
    }
    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            // Tab / Shift+Tab cycle the focused pane FIRST, before any
            // pane-local key routing.
            match (chord.code, chord.mods) {
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.focus_next();
                    return EventOutcome::Consumed;
                }
                (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => {
                    self.focus_prev();
                    return EventOutcome::Consumed;
                }
                (KeyCode::BackTab, _) => {
                    self.focus_prev();
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
            // Alt+<key> is reserved for future cross-pane actions. For now
            // we explicitly do nothing so nothing leaks through to pane
            // handlers under an Alt modifier.
            if chord.mods.contains(KeyModifiers::ALT) {
                // TODO: cross-pane actions on Alt+<key>
                return EventOutcome::Bubble;
            }
            // Pane-gated routing: keys only reach the focused pane.
            match self.focused_pane {
                SshFocus::Hosts => match (chord.code, chord.mods) {
                    (KeyCode::Char('j') | KeyCode::Down, _) => {
                        self.state.select_next();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Char('k') | KeyCode::Up, _) => {
                        self.state.select_prev();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Enter, KeyModifiers::NONE) => {
                        if let Some(alias) = self.state.selected_alias() {
                            let alias = alias.to_string();
                            self.connection.begin_connecting(alias.clone());
                            // Mark intent for the wire layer to pick up on the
                            // next event-loop iteration. The wire layer drains
                            // and spawns the real russh connect; on completion
                            // it flips the connection state to Connected or
                            // Failed via a separate outcome channel.
                            self.pending_connect = Some(alias);
                        }
                        return EventOutcome::Consumed;
                    }
                    _ => {}
                },
                SshFocus::Detail => {
                    // Read-only until PTY/SFTP wiring lands. Intentionally
                    // do not move the host list when j/k are pressed here.
                }
            }
        }
        EventOutcome::Bubble
    }
}

// ---------------------------------------------------------------------------
// Convenience: render the widget into a fresh ratatui `Buffer` for tests.
// ---------------------------------------------------------------------------

/// Render the widget into a fresh test buffer of `(width, height)` using the
/// cosmos theme. Mirrors `sid_widgets::network::render_to_string`.
///
/// # Examples
///
/// ```
/// use sid_widgets::ssh::render_to_string;
/// use sid_widgets::SshWidget;
/// let w = SshWidget::new();
/// let s = render_to_string(&w, 80, 24);
/// assert!(s.contains("Hosts"));
/// ```
pub fn render_to_string(widget: &SshWidget, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    term.draw(|f| widget.render_into_frame(f, f.area(), &theme))
        .unwrap();
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        s.push('\n');
    }
    s
}

/// Compute the right-pane body rect for a widget rendered into `(width, height)`.
///
/// Mirrors the layout used by [`SshWidget::render_into_frame`] — horizontal
/// 40/60 split, then a vertical (3, Min(1), 1) split inside the right half.
/// Exposed so tests (and the binary) can resize an attached
/// [`PtyPane`] to match the body rect before the next frame.
///
/// # Examples
///
/// ```
/// use ratatui::layout::Rect;
/// use sid_widgets::ssh::body_rect_for;
/// let outer = Rect::new(0, 0, 80, 16);
/// let body = body_rect_for(outer);
/// // The body is somewhere inside the right 60%, narrower than the outer.
/// assert!(body.x >= outer.width * 40 / 100 - 1);
/// assert!(body.width < outer.width);
/// ```
pub fn body_rect_for(outer: Rect) -> Rect {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(outer);
    let right = split[1];
    let right_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(right);
    right_split[1]
}

/// Mutably resize an attached [`PtyPane`] to fit `(width, height)` then
/// render the widget to a string. Convenience helper for tests that want a
/// snapshot of the live PTY buffer: they can feed bytes into the pane, call
/// this, and the screen is sized to the body rect before render. No-op if
/// the widget has no `PtyPane` attached.
///
/// # Examples
///
/// ```
/// use sid_widgets::SshWidget;
/// use sid_widgets::ssh::render_to_string_with_resize;
/// let mut w = SshWidget::new();
/// let s = render_to_string_with_resize(&mut w, 80, 16);
/// assert!(s.contains("Hosts"));
/// ```
pub fn render_to_string_with_resize(widget: &mut SshWidget, width: u16, height: u16) -> String {
    let outer = Rect::new(0, 0, width, height);
    widget.pty_pane_resize_to_area(body_rect_for(outer));
    render_to_string(widget, width, height)
}
