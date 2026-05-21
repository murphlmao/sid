//! SSH tab widget — host list + connection state + PTY pane + SFTP sub-panel
//! + per-host command history + edit-in-place state machine.
//!
//! Pure-Rust state types are factored out so they can be unit-tested without
//! constructing a real `SshClient` or `PtyProvider`. The widget is a thin
//! render layer over the state.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sid_core::adapters::pty::{PtyProvider, TerminalScreen};
use sid_core::adapters::ssh::{SftpEntry, SshClient};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_store::{SshHost, SshHostSource};

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
pub struct PtyPane {
    screen: Box<dyn TerminalScreen>,
}

impl PtyPane {
    pub fn new(screen: Box<dyn TerminalScreen>) -> Self {
        Self { screen }
    }
    pub fn feed(&mut self, bytes: &[u8]) {
        self.screen.feed(bytes);
    }
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.screen.resize(rows, cols);
    }
    pub fn size(&self) -> (u16, u16) {
        self.screen.size()
    }
    pub fn lines(&self) -> Vec<String> {
        self.screen.lines()
    }
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
            _ssh_factory: None,
            _pty_provider: None,
        }
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
    pub fn history_for(&self, alias: &str) -> Option<&CommandHistory> {
        self.history.get(alias)
    }
    pub fn record_command(&mut self, alias: &str, cmd: String) {
        self.history
            .entry(alias.to_string())
            .or_insert_with(|| CommandHistory::new(100))
            .push(cmd);
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
    fn render(&self, _target: &mut dyn RenderTarget) {
        // Rendering deferred to the binary's draw() function.
    }
    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            match (chord.code, chord.mods) {
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
                        self.connection.begin_connecting(alias.to_string());
                    }
                    return EventOutcome::Consumed;
                }
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.sftp_panel.toggle();
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
        }
        EventOutcome::Bubble
    }
}
