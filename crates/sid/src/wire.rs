//! Wires together concrete implementations — RedbStore, all six widgets, the
//! keybind map and action registry — into a running [`App`], and contains the
//! Ratatui render loop.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use directories::{ProjectDirs, UserDirs};
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};
use sid_core::Result as SidResult;
use sid_core::action::{Action, ActionRegistry};
use sid_core::adapters::sys::SysProvider;
use sid_core::adapters::systemctl::{
    JournalEntry, SystemUnit, SystemctlClient, SystemctlError, UnitBus, UnitFilter,
};
use sid_core::adapters::terminal_spawner::{SpawnRequest, SpawnerError, TerminalSpawner};
use sid_core::animation::AnimationConfig;
use sid_core::app::{App, Dispatch};
use sid_core::event::Event as SidEvent;
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::sys_probe::{SysProbe, SysSnapshot};
use sid_core::tab::{Tab, TabId, TabManager};
use sid_core::widget::Widget;
use sid_core::workspace_discovery::{
    WorkspaceUpserter, merge_discoveries_into, scan_workspace_root,
};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_fx::FxState;
use sid_git::Git2ProviderFactory;
use sid_store::{RedbStore, SessionRecord, Store, Workspace, now_epoch};
use sid_ui::helpers::styled_block;
use sid_ui::theme::{Color as UiColor, GlyphSet, Theme};
use sid_ui::theme_registry::ThemeRegistry;
use sid_ui::themes::cosmos;
use sid_widgets::{
    DatabaseWidget, NetworkWidget, SettingsWidget, SshWidget, SystemWidget, WorkspacesWidget,
};
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::mpsc::Receiver;

use crate::toast::{Toast, ToastQueue};

/// Outcome of a background job spawned via [`SidApp::jobs`]. Each variant
/// carries a short human-readable label (used in the toast prefix) and the
/// final captured text.
///
/// Constructed inside Tokio tasks that wrap `Command::output()` — the task
/// converts the subprocess result into a [`JobOutcome`] and pushes it into
/// the [`sid_job::JobQueue`]. The event loop drains the queue each iteration
/// and converts every outcome into a [`Toast`].
#[derive(Clone, Debug)]
pub enum JobOutcome {
    /// The job completed successfully; the body is whatever short summary
    /// the spawning handler wants surfaced (typically a one-line success).
    Success {
        /// Context shown ahead of the message ("ssh-copy-id", "ssh -vv", ...).
        label: String,
        /// One-line success message ("copied key to <alias>").
        message: String,
    },
    /// The job ran but the subprocess returned a non-zero exit code, or the
    /// subprocess could not be launched at all (binary missing).
    Failure {
        /// Context shown ahead of the message.
        label: String,
        /// Captured stderr trailer / launch error.
        message: String,
    },
}

/// Top-level wired application state owned by the binary.
///
/// Bundles together the [`App`] (tab manager + keybinds + action registry),
/// the backing [`RedbStore`], and the current session identifier.  Owned
/// exclusively by the main task for the duration of a sid invocation.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use std::sync::Arc;
/// use sid::wire::{build_app, JobOutcome, NoopSystemctlClient, NoopTerminalSpawner, SidApp};
/// use sid::toast::ToastQueue;
/// use sid_job::JobQueue;
/// use sid_store::{OpenStore, RedbStore, Store};
///
/// let store = Arc::new(RedbStore::open(Path::new("/tmp/test.redb")).unwrap());
/// let app = build_app(None, vec![]);
/// let secrets: Arc<dyn sid_core::adapters::secrets::SecretStore> =
///     Arc::new(sid_secrets::PlainStore::new(Arc::clone(&store) as Arc<dyn Store>));
/// let sid_app = SidApp {
///     app,
///     store,
///     session_id: "sess-1".to_string(),
///     sys_probe: None,
///     sys_rx: None,
///     systemctl: Arc::new(NoopSystemctlClient),
///     spawner: Arc::new(NoopTerminalSpawner),
///     postgres: sid_db_clients::PostgresClient::factory(),
///     sqlite: sid_db_clients::SqliteClient::factory(),
///     secrets,
///     animation: sid_core::animation::AnimationConfig::default(),
///     fx_state: None,
///     modal_stack: Vec::new(),
///     pending_submits: Vec::new(),
///     toasts: ToastQueue::new(4),
///     jobs: Arc::new(JobQueue::<JobOutcome>::new()),
/// };
/// ```
pub struct SidApp {
    pub app: App,
    pub store: Arc<RedbStore>,
    pub session_id: String,
    /// Periodic system / network probe. `None` in tests that don't want a
    /// background polling task; constructed by [`build_sys_probe`] in the
    /// production binary path.
    #[allow(dead_code)]
    pub sys_probe: Option<Arc<SysProbe>>,
    /// Live receiver of [`SysSnapshot`]s broadcast by `sys_probe`. Drained
    /// non-blockingly each event-loop pass; every snapshot is forwarded into
    /// the active Network widget via [`refresh_network_widget`].
    ///
    /// `None` matches `sys_probe == None` (tests / no-probe runs).
    pub sys_rx: Option<tokio::sync::broadcast::Receiver<SysSnapshot>>,
    /// Adapter for `systemctl` operations (Plan 6 / System tab).
    #[allow(dead_code)]
    pub systemctl: Arc<dyn SystemctlClient>,
    /// Adapter for spawning external terminal windows (Plan 6 / pinned
    /// configs).
    #[allow(dead_code)]
    pub spawner: Arc<dyn TerminalSpawner>,
    /// Postgres `DbClient` factory (Plan 4).
    #[allow(dead_code)]
    pub postgres: Arc<dyn sid_core::adapters::db_client::DbClient>,
    /// SQLite `DbClient` factory (Plan 4).
    #[allow(dead_code)]
    pub sqlite: Arc<dyn sid_core::adapters::db_client::DbClient>,
    /// Plaintext-backed secret store (Plan 4).
    #[allow(dead_code)]
    pub secrets: Arc<dyn sid_core::adapters::secrets::SecretStore>,
    /// Background-animation configuration (Phase 6.1). Persisted via the
    /// `setting:animation` key.
    pub animation: AnimationConfig,
    /// Live starfield state. `None` disables the background layer (tests +
    /// `animation.enabled == false`).
    pub fx_state: Option<FxState>,
    /// Stack of open modals. The topmost entry intercepts key events; widgets
    /// see them only when the stack is empty. New modals push on top.
    pub modal_stack: Vec<sid_widgets::ModalSpec>,
    /// Modals submitted on the previous frame whose handler hasn't run yet.
    /// Drained at the top of [`run_event_loop`] each iteration.
    pub pending_submits: Vec<(sid_widgets::ModalId, Vec<(String, sid_widgets::FieldValue)>)>,
    /// Lower-right corner toast queue. Pushed by modal submit handlers
    /// (success / error) and by completed background jobs.
    pub toasts: ToastQueue,
    /// Job queue used for asynchronous subprocess work (ssh-copy-id, ssh-keygen,
    /// ssh -vv, ssh-add, etc.). Each spawned task pushes a [`JobOutcome`];
    /// the event loop drains completed outcomes once per iteration and
    /// converts them into toasts.
    pub jobs: Arc<sid_job::JobQueue<JobOutcome>>,
}

/// Fallback [`SystemctlClient`] used when `systemctl` / `journalctl` are not
/// reachable on PATH. Every method returns
/// [`SystemctlError::SystemctlMissing`] so the widget can surface a single
/// consistent error → toast mapping.
#[derive(Debug, Default)]
pub struct NoopSystemctlClient;

impl SystemctlClient for NoopSystemctlClient {
    fn list_units(&self, _f: UnitFilter) -> Result<Vec<SystemUnit>, SystemctlError> {
        Err(SystemctlError::SystemctlMissing)
    }
    fn status(&self, _b: UnitBus, _u: &str) -> Result<SystemUnit, SystemctlError> {
        Err(SystemctlError::SystemctlMissing)
    }
    fn start(&self, _b: UnitBus, _u: &str) -> Result<(), SystemctlError> {
        Err(SystemctlError::SystemctlMissing)
    }
    fn stop(&self, _b: UnitBus, _u: &str) -> Result<(), SystemctlError> {
        Err(SystemctlError::SystemctlMissing)
    }
    fn restart(&self, _b: UnitBus, _u: &str) -> Result<(), SystemctlError> {
        Err(SystemctlError::SystemctlMissing)
    }
    fn journal_tail(
        &self,
        _b: UnitBus,
        _u: &str,
        _n: usize,
    ) -> Result<Vec<JournalEntry>, SystemctlError> {
        Err(SystemctlError::JournalctlMissing)
    }
}

/// Fallback [`TerminalSpawner`] used when `kitty` is missing. `spawn` returns
/// [`SpawnerError::TerminalMissing`].
#[derive(Debug, Default)]
pub struct NoopTerminalSpawner;

impl TerminalSpawner for NoopTerminalSpawner {
    fn spawn(&self, _req: SpawnRequest) -> Result<(), SpawnerError> {
        Err(SpawnerError::TerminalMissing("kitty".into()))
    }
    fn name(&self) -> &'static str {
        "noop"
    }
}

/// Resolve the systemctl adapter. Logs and falls back to
/// [`NoopSystemctlClient`] if the system lacks systemd.
pub fn build_systemctl_client() -> Arc<dyn SystemctlClient> {
    match sid_system::SystemctlCmdClient::new() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            tracing::warn!("systemctl unavailable: {e}; System tab services pane will show empty");
            Arc::new(NoopSystemctlClient)
        }
    }
}

/// Resolve the terminal spawner. Logs and falls back to
/// [`NoopTerminalSpawner`] if `kitty` is missing.
pub fn build_terminal_spawner() -> Arc<dyn TerminalSpawner> {
    match sid_system::KittyTerminalSpawner::new() {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::warn!(
                "kitty unavailable: {e}; pinned configs will surface 'kitty missing' toasts"
            );
            Arc::new(NoopTerminalSpawner)
        }
    }
}

impl SidApp {
    /// Subscribe to fresh [`sid_core::sys_probe::SysSnapshot`]s if a probe
    /// is attached. Returns `None` when the probe is absent (e.g., in tests
    /// that opt out of the background polling task).
    ///
    /// The returned receiver lives independently of the probe; dropping it
    /// is fine. Snapshots only flow while the probe's `run()` future is
    /// being polled on a Tokio task.
    #[allow(dead_code)]
    pub fn subscribe_to_sys(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<sid_core::sys_probe::SysSnapshot>> {
        self.sys_probe.as_ref().map(|p| p.subscribe())
    }
}

/// Return the path to the redb database file.
///
/// Uses `override_path` when provided; otherwise derives from the XDG/platform
/// data directory via [`directories::ProjectDirs`]. Falls back to `./sid.redb`
/// if the platform dirs cannot be determined.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid::wire::db_path;
///
/// // With an override, the override is returned unchanged.
/// let p = db_path(Some(PathBuf::from("/tmp/test.redb")));
/// assert_eq!(p, PathBuf::from("/tmp/test.redb"));
///
/// // Without an override, an XDG-derived path is returned.
/// let p = db_path(None);
/// assert!(p.to_string_lossy().contains("sid"));
/// ```
pub fn db_path(override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    // Check `~/.config/sid/sid.toml` for an override before falling back to XDG.
    if let Some(dirs) = ProjectDirs::from("dev", "sid", "sid") {
        let toml_path = dirs.config_dir().join("sid.toml");
        if let Ok(cfg) = sid_store::sid_toml::read_sid_toml(&toml_path)
            && let Some(override_from_toml) = cfg.db_path_override
        {
            return expand_tilde_path(&override_from_toml);
        }
        let data = dirs.data_local_dir().to_path_buf();
        std::fs::create_dir_all(&data).ok();
        return data.join("sid.redb");
    }
    PathBuf::from("./sid.redb")
}

fn expand_tilde_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    p.to_path_buf()
}

/// Path to the `sid.toml` config file (XDG-rooted by default). Exposed for
/// integration tests and the [`crate::wire::load_active_theme`] /
/// [`crate::wire::load_active_keybinds`] startup helpers; the binary uses
/// the same resolution implicitly via [`db_path`].
///
/// # Examples
///
/// ```
/// use sid::wire::sid_toml_path;
/// let p = sid_toml_path();
/// assert!(p.to_string_lossy().ends_with("sid.toml"));
/// ```
#[allow(dead_code)]
pub fn sid_toml_path() -> PathBuf {
    if let Some(dirs) = ProjectDirs::from("dev", "sid", "sid") {
        return dirs.config_dir().join("sid.toml");
    }
    PathBuf::from("./sid.toml")
}

/// Convert a persisted [`sid_store::ThemeSpec`] back into a [`Theme`] so it can
/// be merged into a [`ThemeRegistry`].
fn theme_spec_to_theme(spec: sid_store::ThemeSpec) -> Theme {
    let rgb = |v: u32| {
        UiColor::rgb(
            ((v >> 16) & 0xFF) as u8,
            ((v >> 8) & 0xFF) as u8,
            (v & 0xFF) as u8,
        )
    };
    Theme {
        name: spec.name,
        background: rgb(spec.palette.background),
        surface: rgb(spec.palette.surface),
        foreground: rgb(spec.palette.foreground),
        muted: rgb(spec.palette.muted),
        accent_primary: rgb(spec.palette.accent_primary),
        accent_success: rgb(spec.palette.accent_success),
        accent_warning: rgb(spec.palette.accent_warning),
        accent_error: rgb(spec.palette.accent_error),
        border: rgb(spec.palette.border),
        glyphs: GlyphSet {
            star: spec.glyphs.star,
            small_star: spec.glyphs.small_star,
            dot: spec.glyphs.dot,
        },
    }
}

/// Load the active theme + the merged [`ThemeRegistry`] (built-ins plus user
/// themes from the store). Falls back to `cosmos` with a warning if the
/// configured theme name is missing.
///
/// # Examples
///
/// ```
/// use sid::wire::load_active_theme;
/// use sid_store::{OpenStore, RedbStore};
/// use tempfile::tempdir;
///
/// let d = tempdir().unwrap();
/// let store = RedbStore::open(&d.path().join("s.redb")).unwrap();
/// let (theme, _registry) = load_active_theme(&store);
/// assert_eq!(theme.name, "cosmos");
/// ```
pub fn load_active_theme(store: &dyn Store) -> (Theme, ThemeRegistry) {
    use sid_store::TypedSettings;
    let mut registry = ThemeRegistry::with_builtins();
    if let Ok(user_themes) = store.list_themes() {
        for spec in user_themes {
            registry.register(theme_spec_to_theme(spec));
        }
    }
    let name = store
        .get_string(sid_store::settings_keys::THEME_NAME)
        .ok()
        .flatten()
        .unwrap_or_else(|| "cosmos".to_string());
    let theme = registry.get(&name).cloned().unwrap_or_else(|| {
        tracing::warn!(theme = %name, "theme not found, falling back to cosmos");
        cosmos()
    });
    (theme, registry)
}

/// Load the active keybind profile from the store. On first run (empty store)
/// seeds and returns the cosmos default.
///
/// # Examples
///
/// ```
/// use sid::wire::load_active_keybinds;
/// use sid_store::{OpenStore, RedbStore};
/// use tempfile::tempdir;
///
/// let d = tempdir().unwrap();
/// let store = RedbStore::open(&d.path().join("s.redb")).unwrap();
/// let map = load_active_keybinds(&store);
/// assert!(map.iter().count() > 0);
/// ```
pub fn load_active_keybinds(store: &dyn Store) -> KeybindMap {
    use sid_store::TypedSettings;
    let name = store
        .get_string(sid_store::settings_keys::KEYBIND_PROFILE_NAME)
        .ok()
        .flatten()
        .unwrap_or_else(|| "cosmos".to_string());
    match sid_store::keybind_load::load_keybind_profile(store, &name) {
        Ok(Some(map)) => map,
        _ => {
            let m = KeybindMap::cosmos_default();
            // Best-effort seed; ignore errors so a read-only store still boots.
            let _ = sid_store::keybind_load::save_keybind_profile(store, "cosmos", &m);
            m
        }
    }
}

/// Load the persisted [`AnimationConfig`] from `store`, falling back to the
/// default. The value is a JSON-encoded `AnimationConfig` blob written by the
/// Settings tab under the `animation` setting key. JSON is used over postcard
/// so the binary can read values written by hand-edited `sid.toml` later.
pub fn load_animation_config(store: &dyn Store) -> AnimationConfig {
    let key = sid_core::animation::SETTING_ANIMATION_KEY;
    if let Ok(Some(val)) = store.get_setting(key) {
        if let Ok(cfg) = serde_json::from_slice::<AnimationConfig>(&val.0) {
            return cfg;
        }
    }
    AnimationConfig::default()
}

/// Build an [`App`] with the six Plan-1 tabs pre-wired.
///
/// Injects the real [`Git2ProviderFactory`] into the [`WorkspacesWidget`] and
/// pre-populates it with `workspaces` loaded from the store.
///
/// Optionally switches to `start_tab` if a matching tab id is found.
///
/// # Examples
///
/// ```
/// use sid::wire::build_app;
///
/// let app = build_app(None, vec![]);
/// assert_eq!(app.tabs().tabs().len(), 6);
/// assert_eq!(app.tabs().active().id.as_str(), "workspaces");
/// ```
/// Build a [`SysProbe`] backed by the production [`sid_sysinfo::SysinfoProvider`]
/// with the given poll interval.
///
/// The probe is returned but not yet spawned; the caller is responsible for
/// calling `tokio::spawn(async move { probe.run().await })` and keeping the
/// `Arc<SysProbe>` alive for the lifetime of the run.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
/// use sid::wire::build_sys_probe;
///
/// let probe = build_sys_probe(Duration::from_secs(2));
/// assert_eq!(probe.interval(), Duration::from_secs(2));
/// ```
pub fn build_sys_probe(interval: Duration) -> Arc<SysProbe> {
    let provider: Arc<Mutex<dyn SysProvider>> =
        Arc::new(Mutex::new(sid_sysinfo::SysinfoProvider::new()));
    Arc::new(SysProbe::new(provider, interval))
}

#[allow(dead_code)]
pub fn build_app(start_tab: Option<&str>, workspaces: Vec<Workspace>) -> App {
    build_app_full(start_tab, workspaces, vec![], vec![], None)
}

/// Construct an SSH client factory (russh-backed). Cheap; no I/O.
#[allow(dead_code)]
pub fn build_ssh_client_factory() -> Arc<sid_ssh::RusshClientFactory> {
    Arc::new(sid_ssh::RusshClientFactory::new())
}

/// Construct a PTY provider (portable-pty-backed). Cheap; no I/O.
#[allow(dead_code)]
pub fn build_pty_provider() -> Arc<sid_pty::PortablePtyProvider> {
    Arc::new(sid_pty::PortablePtyProvider::new())
}

/// Build the App with optional SSH host hydration. The SSH tab is initialized
/// with a merged view of `ssh_hosts` (from the store) + `ssh_config_entries`
/// (from `~/.ssh/config`). If `start_ssh_alias` is `Some`, the SSH tab is
/// pre-selected and the connection state is set to Connecting on that alias.
pub fn build_app_full(
    start_tab: Option<&str>,
    workspaces: Vec<Workspace>,
    ssh_hosts: Vec<sid_store::SshHost>,
    ssh_config_entries: Vec<sid_widgets::ssh::SshConfigEntryLite>,
    start_ssh_alias: Option<String>,
) -> App {
    build_app_hydrated(
        start_tab,
        BuildAppData::just_workspaces(workspaces, ssh_hosts, ssh_config_entries, start_ssh_alias),
    )
}

/// Pre-loaded state for `build_app_hydrated`.
///
/// Holds everything the binary's startup code has read from the store, so
/// each widget can be constructed with real data instead of empty defaults.
/// Used by `main.rs`; tests typically use `BuildAppData::just_workspaces`
/// which keeps every other field empty.
#[derive(Default)]
pub struct BuildAppData {
    pub workspaces: Vec<Workspace>,
    pub ssh_hosts: Vec<sid_store::SshHost>,
    pub ssh_config_entries: Vec<sid_widgets::ssh::SshConfigEntryLite>,
    pub start_ssh_alias: Option<String>,
    pub db_connections: Vec<sid_store::DbConnection>,
    pub pinned_configs: Vec<sid_store::PinnedConfig>,
    pub quick_actions: Vec<sid_store::QuickAction>,
    pub settings_categories: Vec<sid_widgets::SettingsCategory>,
}

impl BuildAppData {
    pub fn just_workspaces(
        workspaces: Vec<Workspace>,
        ssh_hosts: Vec<sid_store::SshHost>,
        ssh_config_entries: Vec<sid_widgets::ssh::SshConfigEntryLite>,
        start_ssh_alias: Option<String>,
    ) -> Self {
        Self {
            workspaces,
            ssh_hosts,
            ssh_config_entries,
            start_ssh_alias,
            ..Default::default()
        }
    }
}

pub fn build_app_hydrated(start_tab: Option<&str>, data: BuildAppData) -> App {
    let git_factory = Arc::new(Git2ProviderFactory::new());

    // Build the SSH widget with pre-loaded state.
    let ssh_state = sid_widgets::ssh::SshState::new(data.ssh_hosts, data.ssh_config_entries);
    let mut ssh_widget = SshWidget::with_state(ssh_state);
    if let Some(ref alias) = data.start_ssh_alias {
        let aliases: Vec<_> = ssh_widget
            .state()
            .visible_hosts()
            .iter()
            .map(|h| h.alias.clone())
            .collect();
        if let Some(idx) = aliases.iter().position(|a| a == alias) {
            for _ in 0..idx {
                ssh_widget.state_mut().select_next();
            }
            ssh_widget.connection_mut().begin_connecting(alias.clone());
        }
    }

    // System widget: load pinned configs + quick actions from store.
    let mut system_widget = SystemWidget::new();
    *system_widget.pinned_configs_mut() =
        sid_widgets::system::PinnedConfigsState::new(data.pinned_configs);
    *system_widget.quick_actions_mut() =
        sid_widgets::system::QuickActionsState::new(data.quick_actions);

    // Settings widget: build with pre-loaded categories, falling back to the
    // legacy empty constructor only when callers haven't filled them in.
    let settings_widget = if data.settings_categories.is_empty() {
        SettingsWidget::new()
    } else {
        SettingsWidget::with_categories(data.settings_categories)
    };

    let tabs = TabManager::new(vec![
        tab(
            "workspaces",
            "Workspaces",
            Box::new(WorkspacesWidget::new(data.workspaces, Some(git_factory))),
            Some('1'),
        ),
        tab("ssh", "SSH", Box::new(ssh_widget), Some('2')),
        tab(
            "database",
            "Database",
            Box::new(DatabaseWidget::new(data.db_connections)),
            Some('3'),
        ),
        tab(
            "network",
            "Network",
            Box::new(NetworkWidget::new()),
            Some('4'),
        ),
        tab("system", "System", Box::new(system_widget), Some('5')),
        tab("settings", "Settings", Box::new(settings_widget), Some('6')),
    ]);
    let kb = KeybindMap::cosmos_default();
    let mut reg = ActionRegistry::new();
    for a in [
        "app.quit",
        "palette.open",
        "tabs.next",
        "tabs.prev",
        "app.open_settings",
        "tab.detach",
        "tab.attach",
        "tab.reload",
    ] {
        reg.register(Action::new(a, pretty_label(a)));
    }
    for i in 1..=6 {
        reg.register(Action::new(
            format!("tabs.jump.{i}"),
            format!("Jump to tab {i}"),
        ));
    }
    let mut app = App::new(tabs, kb, reg);
    let effective_start_tab = start_tab
        .map(|s| s.to_string())
        .or_else(|| data.start_ssh_alias.as_ref().map(|_| "ssh".to_string()));
    if let Some(id) = effective_start_tab {
        let _ = app.tabs_mut().switch_to(&TabId::new(&id));
    }
    app
}

fn tab(id: &str, title: &str, widget: Box<dyn Widget>, hotkey: Option<char>) -> Tab {
    Tab {
        id: TabId::new(id),
        title: title.to_string(),
        layout: Layout::Single(widget),
        hotkey,
    }
}

/// Convert a known action id to a human-readable label.
///
/// Unknown action ids are returned unchanged — this function never panics.
///
/// # Examples
///
/// ```
/// use sid::wire::pretty_label;
///
/// assert_eq!(pretty_label("app.quit"), "Quit");
/// assert_eq!(pretty_label("tabs.next"), "Next tab");
/// assert_eq!(pretty_label("unknown.action"), "unknown.action");
/// ```
pub fn pretty_label(action_id: &str) -> String {
    match action_id {
        "app.quit" => "Quit".into(),
        "palette.open" => "Open command palette".into(),
        "tabs.next" => "Next tab".into(),
        "tabs.prev" => "Previous tab".into(),
        "app.open_settings" => "Open Settings".into(),
        "tab.detach" => "Detach tab (Plan 8)".into(),
        "tab.attach" => "Attach widget (Plan 8)".into(),
        "tab.reload" => "Reload tab data".into(),
        other => other.to_string(),
    }
}

/// Persist the current active tab into the session record.
///
/// Creates or updates the session identified by `session_id`.  The session
/// record stores the active tab id and the full ordered list of open tab ids.
///
/// # Errors
///
/// Returns a [`sid_core::Error`] if the underlying store write fails (e.g.,
/// redb I/O error).
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid::wire::{build_app, save_active_tab};
/// use sid_store::{OpenStore, RedbStore};
///
/// let store = RedbStore::open(Path::new("/tmp/test.redb")).unwrap();
/// let app = build_app(Some("ssh"), vec![]);
/// save_active_tab(&store, "sess-1", &app).unwrap();
/// // The session record is now persisted; open the store again to verify.
/// ```
pub fn save_active_tab(store: &dyn Store, session_id: &str, app: &App) -> SidResult<()> {
    let sess = SessionRecord {
        id: session_id.to_string(),
        started_at: now_epoch(),
        last_active: now_epoch(),
        ended_at: None,
        active_tab: Some(app.tabs().active().id.clone()),
        open_tabs: app.tabs().tabs().iter().map(|t| t.id.clone()).collect(),
    };
    store.upsert_session(&sess)
}

/// Draw one frame: tab strip on top, active panel body, help bar on bottom,
/// optional command-palette overlay centred over everything.
///
/// Uses the cosmos theme throughout. Pure layout — does not mutate any state.
/// Receives `&SidApp` (not just `&App`) so the active panel can read live data
/// out of the store (workspaces list, etc.) instead of relying on widget state.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use std::sync::Arc;
/// use ratatui::Terminal;
/// use ratatui::backend::TestBackend;
/// use sid::wire::{NoopSystemctlClient, NoopTerminalSpawner, SidApp, build_app, draw};
/// use sid_store::{OpenStore, RedbStore, Store};
///
/// let store = Arc::new(RedbStore::open(Path::new("/tmp/draw_test.redb")).unwrap());
/// let secrets: Arc<dyn sid_core::adapters::secrets::SecretStore> =
///     Arc::new(sid_secrets::PlainStore::new(Arc::clone(&store) as Arc<dyn Store>));
/// let sid_app = SidApp {
///     app: build_app(None, vec![]),
///     store,
///     session_id: "sess-1".to_string(),
///     sys_probe: None,
///     sys_rx: None,
///     systemctl: Arc::new(NoopSystemctlClient),
///     spawner: Arc::new(NoopTerminalSpawner),
///     postgres: sid_db_clients::PostgresClient::factory(),
///     sqlite: sid_db_clients::SqliteClient::factory(),
///     secrets,
///     animation: sid_core::animation::AnimationConfig::default(),
///     fx_state: None,
///     modal_stack: Vec::new(),
///     pending_submits: Vec::new(),
///     toasts: sid::toast::ToastQueue::new(4),
///     jobs: std::sync::Arc::new(sid_job::JobQueue::<sid::wire::JobOutcome>::new()),
/// };
/// let backend = TestBackend::new(120, 40);
/// let mut terminal = Terminal::new(backend).unwrap();
/// terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
/// ```
pub fn draw(frame: &mut Frame<'_>, sid_app: &SidApp) {
    use ratatui::style::Modifier as TextMod;
    use ratatui::style::Style as TextStyle;
    use ratatui::widgets::{Block as RBlock, BorderType, Borders as RBorders};

    let theme = cosmos();
    let app = &sid_app.app;
    let size = frame.area();
    if size.width == 0 || size.height == 0 {
        return;
    }

    // ─── Starfield + supernovae background ────────────────────────────────
    // Stars first, supernovae second. Both are background layers — widget
    // body draws on top, but supernova glyphs show through any empty cell.
    // Net effect: a celebration bloom is visible in "empty space" of the
    // active tab without overdrawing real content.
    if let Some(fx) = &sid_app.fx_state {
        sid_fx::render_starfield(frame.buffer_mut(), size, fx, &sid_app.animation, &theme);
        sid_fx::render_supernovae(frame.buffer_mut(), size, fx, &sid_app.animation, &theme);
    }

    // ─── Outer "✦ sid — <active>" bordered window ─────────────────────────
    let active_title = app.tabs().active().title.clone();
    let outer_title = format!(" ✦ sid — {} ", active_title);
    let outer = RBlock::default()
        .title(outer_title)
        .borders(RBorders::ALL)
        .border_type(BorderType::Double)
        .border_style(TextStyle::default().fg(ui_to_ratatui(theme.border)));
    let inner = outer.inner(size);
    frame.render_widget(outer, size);

    // Within the outer border we want a tab strip, body, status line, and
    // a two-line footer (per-tab + global). Heights are conservative so
    // small terminals still draw something usable.
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let tabs_h: u16 = 2;
    let status_h: u16 = if inner.height >= 12 { 1 } else { 0 };
    let footer_h: u16 = if inner.height >= 10 { 2 } else { 1 };
    let body_h = inner.height.saturating_sub(tabs_h + status_h + footer_h);

    let mut y = inner.y;
    let tabs_rect = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: tabs_h.min(inner.height),
    };
    y = y.saturating_add(tabs_rect.height);
    let body_rect = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: body_h,
    };
    y = y.saturating_add(body_h);
    let status_rect = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: status_h,
    };
    y = y.saturating_add(status_h);
    let footer_rect = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: footer_h,
    };

    // ─── Tab strip ────────────────────────────────────────────────────────
    let active_idx = app.tabs().active_index();
    let mut spans: Vec<Span> = Vec::new();
    for (i, t) in app.tabs().tabs().iter().enumerate() {
        let (marker, marker_style) = if i == active_idx {
            (
                '●',
                TextStyle::default()
                    .fg(ui_to_ratatui(theme.accent_primary))
                    .add_modifier(TextMod::BOLD),
            )
        } else {
            ('·', TextStyle::default().fg(ui_to_ratatui(theme.muted)))
        };
        if i > 0 {
            spans.push(Span::styled("  ", TextStyle::default()));
        }
        spans.push(Span::styled(format!("{marker} "), marker_style));
        let title_style = if i == active_idx {
            TextStyle::default()
                .fg(ui_to_ratatui(theme.foreground))
                .add_modifier(TextMod::BOLD)
        } else {
            TextStyle::default().fg(ui_to_ratatui(theme.muted))
        };
        spans.push(Span::styled(t.title.clone(), title_style));
    }
    let tab_line = Line::from(spans);
    let tab_para = Paragraph::new(tab_line);
    frame.render_widget(tab_para, tabs_rect);

    // ─── Body (panel for active tab) ──────────────────────────────────────
    let active_id = app.tabs().active().id.as_str().to_string();
    let active_layout = &app.tabs().active().layout;
    let widget = active_layout.iter_widgets().next();

    // Each concrete widget exposes a ratatui-aware `render_into_frame` that the
    // Widget trait cannot — sid-core must not depend on ratatui. Downcast
    // through `Widget::as_any` to call the right one, falling back to a text
    // panel only for tabs whose widget isn't recognised.
    let rendered_via_widget = match (active_id.as_str(), widget) {
        ("workspaces", Some(w)) => {
            if let Some(ws) = w.as_any().downcast_ref::<WorkspacesWidget>() {
                ws.render_into_frame(frame, body_rect, &theme);
                true
            } else {
                // Fallback: legacy string-based body for the rare case where
                // the widget downcast unexpectedly fails. Keeps the body
                // usable even when the WorkspacesWidget isn't registered.
                let block = styled_block(&theme, &active_title);
                let body = Paragraph::new(render_workspaces_body(&*sid_app.store)).block(block);
                frame.render_widget(body, body_rect);
                true
            }
        }
        ("workspaces", None) => {
            let block = styled_block(&theme, &active_title);
            let body = Paragraph::new(render_workspaces_body(&*sid_app.store)).block(block);
            frame.render_widget(body, body_rect);
            true
        }
        ("ssh", Some(w)) => {
            if let Some(s) = w.as_any().downcast_ref::<SshWidget>() {
                s.render_into_frame(frame, body_rect, &theme);
                true
            } else {
                false
            }
        }
        ("database", Some(w)) => {
            if let Some(d) = w.as_any().downcast_ref::<DatabaseWidget>() {
                d.render_into_frame(frame, body_rect, &theme);
                true
            } else {
                false
            }
        }
        ("network", Some(w)) => {
            if let Some(n) = w.as_any().downcast_ref::<NetworkWidget>() {
                n.render_into_frame(frame, body_rect, &theme);
                true
            } else {
                false
            }
        }
        ("system", Some(w)) => {
            if let Some(s) = w.as_any().downcast_ref::<SystemWidget>() {
                s.render_into_frame(frame, body_rect, &theme);
                true
            } else {
                false
            }
        }
        ("settings", Some(w)) => {
            if let Some(s) = w.as_any().downcast_ref::<SettingsWidget>() {
                s.render_into_frame(frame, body_rect, &theme);
                true
            } else {
                false
            }
        }
        _ => false,
    };

    if !rendered_via_widget {
        let block = styled_block(&theme, &active_title);
        let body = Paragraph::new(stub_panel(&active_title, "(unknown tab)")).block(block);
        frame.render_widget(body, body_rect);
    }

    // ─── Status line (above footer) ───────────────────────────────────────
    if status_h > 0 {
        let status_text = build_status_line(sid_app);
        let status =
            Paragraph::new(status_text).style(TextStyle::default().fg(ui_to_ratatui(theme.muted)));
        frame.render_widget(status, status_rect);
    }

    // ─── Footer hint strip ────────────────────────────────────────────────
    // Upper line: per-tab capital-letter actions from the active widget.
    // Lower line: global hints (Ctrl+Q, Ctrl+F, ...).
    let footer_split = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints(if footer_h >= 2 {
            vec![
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(1),
            ]
        } else {
            vec![ratatui::layout::Constraint::Length(1)]
        })
        .split(footer_rect);
    if footer_h >= 2 {
        if let Some(w) = widget {
            let hints = w.footer_hint();
            let spans: Vec<Span> = hints
                .iter()
                .flat_map(|h| {
                    [
                        Span::styled("  [ ", TextStyle::default().fg(ui_to_ratatui(theme.muted))),
                        Span::styled(
                            h.chord.clone(),
                            TextStyle::default()
                                .fg(ui_to_ratatui(theme.accent_primary))
                                .add_modifier(TextMod::BOLD),
                        ),
                        Span::styled(
                            format!(": {}", h.label),
                            TextStyle::default().fg(ui_to_ratatui(theme.foreground)),
                        ),
                        Span::styled(" ]", TextStyle::default().fg(ui_to_ratatui(theme.muted))),
                    ]
                })
                .collect();
            let p = Paragraph::new(Line::from(spans));
            frame.render_widget(p, footer_split[0]);
        }
        let global =
            Paragraph::new(help_line()).style(TextStyle::default().fg(ui_to_ratatui(theme.muted)));
        frame.render_widget(global, footer_split[1]);
    } else {
        let global =
            Paragraph::new(help_line()).style(TextStyle::default().fg(ui_to_ratatui(theme.muted)));
        frame.render_widget(global, footer_split[0]);
    }

    // ─── Toasts (bottom-right corner) ─────────────────────────────────────
    // Drawn after the body / footer but BEFORE modal/palette so modals
    // visually cover the toast region. Toasts continue to age while a modal
    // is open; once dismissed they appear if still alive.
    render_toasts(frame, inner, &theme, &sid_app.toasts);

    // ─── Modal overlay (Phase 3) ──────────────────────────────────────────
    // The topmost modal renders on top of body+footer+status+toasts.
    // Animation is already painted but we don't tick stars while a modal is
    // open — see `run_event_loop`. Render after the body so the modal covers
    // it cleanly.
    if let Some(modal) = sid_app.modal_stack.last() {
        sid_widgets::render_modal(frame, inner, &theme, modal);
    }

    // ─── Palette overlay if open ──────────────────────────────────────────
    if app.palette().is_open() {
        let overlay_rect = centered(size, 60, 40);
        let mut lines = vec![format!("> {}", app.palette().query())];
        for (i, a) in app.palette().matches(app.actions()).into_iter().enumerate() {
            let prefix = if i == app.palette().selected_index() {
                ">"
            } else {
                " "
            };
            lines.push(format!("{prefix} {} ({})", a.label, a.id));
        }
        let p = Paragraph::new(lines.join("\n")).block(styled_block(&theme, "command palette"));
        frame.render_widget(p, overlay_rect);
    }
}

/// Render a stub panel body for tabs whose plan hasn't shipped yet.
fn stub_panel(title: &str, hint: &str) -> String {
    format!(
        "{title}\n\n{hint}\n\n(this tab's implementation lands in a future plan — \
the foundation is in place, just not the panel body)"
    )
}

/// Render the Workspaces tab body. Reads the registered workspaces from the
/// store each frame and lists them as a tree (umbrella + sub-repos).
fn render_workspaces_body(store: &dyn Store) -> String {
    let workspaces = match store.list_workspaces() {
        Ok(v) => v,
        Err(e) => return format!("error reading workspaces: {e}"),
    };
    if workspaces.is_empty() {
        return String::from(
            "no workspaces registered yet\n\n\
             try one of:\n  \
             - sid workspace add /path/to/repo   (register a single repo)\n  \
             - put repos under ~/vcs/ and relaunch (auto-discovered)\n\n\
             once registered, j/k to navigate, Enter to expand umbrellas, Tab to cycle sub-views",
        );
    }
    let mut lines: Vec<String> = Vec::with_capacity(workspaces.len() + 2);
    lines.push(format!("{} workspace(s) registered:", workspaces.len()));
    lines.push(String::new());
    // Group by parent for tree-ish display.
    let parents: Vec<&Workspace> = workspaces.iter().filter(|w| w.parent.is_none()).collect();
    for w in &parents {
        let glyph = match w.kind {
            WorkspaceKind::Umbrella => '▾',
            WorkspaceKind::Repo => '·',
        };
        lines.push(format!("  {glyph} {:<28}  {}", w.name, w.path.display()));
        // Children
        for child in workspaces
            .iter()
            .filter(|c| c.parent.as_deref() == Some(&w.path))
        {
            lines.push(format!(
                "      · {:<24}  {}",
                child.name,
                child.path.display()
            ));
        }
    }
    // Loose children (parent set but parent not registered): show under "orphans"
    let orphans: Vec<&Workspace> = workspaces
        .iter()
        .filter(|w| {
            w.parent
                .as_ref()
                .is_some_and(|p| !workspaces.iter().any(|q| &q.path == p))
        })
        .collect();
    if !orphans.is_empty() {
        lines.push(String::new());
        lines.push(String::from("  (orphan children — parent not registered):"));
        for w in orphans {
            lines.push(format!("      · {:<24}  {}", w.name, w.path.display()));
        }
    }
    lines.join("\n")
}

/// One-line help bar shown at the bottom of every frame.
fn help_line() -> &'static str {
    " Ctrl+Q quit  ·  Ctrl+F palette  ·  Ctrl+←/→ tabs  ·  Ctrl+1..6 jump  ·  Ctrl+, settings"
}

/// Render a one-line status string for the bar between body and footer.
fn build_status_line(sid_app: &SidApp) -> String {
    let workspaces = sid_app
        .store
        .list_workspaces()
        .map(|v| v.len())
        .unwrap_or(0);
    let hosts = sid_app.store.list_ssh_hosts().map(|v| v.len()).unwrap_or(0);
    let dbs = sid_app
        .store
        .list_db_connections()
        .map(|v| v.len())
        .unwrap_or(0);
    let pins = sid_app
        .store
        .list_pinned_configs()
        .map(|v| v.len())
        .unwrap_or(0);
    let anim = if sid_app.animation.enabled {
        format!("animation on @ {}fps", sid_app.animation.fps)
    } else {
        "animation off".to_string()
    };
    format!(
        " workspaces: {workspaces}  ·  hosts: {hosts}  ·  databases: {dbs}  ·  pins: {pins}  ·  {anim}"
    )
}

/// Convert a `sid_ui::theme::Color` to a `ratatui::style::Color`.
fn ui_to_ratatui(c: UiColor) -> ratatui::style::Color {
    ratatui::style::Color::Rgb(c.r, c.g, c.b)
}

/// Return a [`Rect`] centred within `area` that is `pct_w`% wide and `pct_h`%
/// tall.
///
/// When the computed dimensions exceed `area`, the original `area` is returned
/// unchanged (e.g., when `pct_w >= 100` or `pct_h >= 100`).  Zero-percent
/// dimensions produce a zero-size rect pinned to the centre.
///
/// # Examples
///
/// ```
/// use ratatui::layout::Rect;
/// use sid::wire::centered;
///
/// let area = Rect { x: 0, y: 0, width: 100, height: 50 };
///
/// // 100% = original area returned.
/// assert_eq!(centered(area, 100, 100), area);
///
/// // 0% = zero-size rect at the centre.
/// let z = centered(area, 0, 0);
/// assert_eq!(z.width, 0);
/// assert_eq!(z.height, 0);
///
/// // Normal usage.
/// let c = centered(area, 60, 40);
/// assert!(c.width < area.width);
/// assert!(c.height < area.height);
/// ```
pub fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = area.width.saturating_mul(pct_w.min(100)) / 100;
    let h = area.height.saturating_mul(pct_h.min(100)) / 100;
    // Guard: if computed size is larger than or equal to area, return area.
    if w >= area.width && h >= area.height {
        return area;
    }
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Return the default workspace discovery roots.
///
/// Scans `~/vcs/` if `$HOME` resolves via [`directories::UserDirs`], otherwise
/// returns an empty list (discovery is a best-effort operation).
///
/// # Examples
///
/// ```
/// use sid::wire::default_discovery_roots;
///
/// let roots = default_discovery_roots();
/// // May be empty when $HOME is unset; never panics.
/// assert!(roots.len() <= 1);
/// ```
pub fn default_discovery_roots() -> Vec<PathBuf> {
    UserDirs::new()
        .map(|u| u.home_dir().join("vcs"))
        .into_iter()
        .collect()
}

/// Scan each root for workspaces and merge discoveries into the store.
///
/// Uses [`scan_workspace_root`] with a max depth of 2, then calls
/// [`merge_discoveries_into`] with a [`WorkspaceUpserter`] adapter that
/// delegates to the store.  Discovery is best-effort: errors from scanning an
/// individual root are propagated to the caller; errors from a single upsert
/// are surfaced as `Err(String)` from [`merge_discoveries_into`].
///
/// Returns the total number of workspaces upserted across all roots.
///
/// # Errors
///
/// Returns `anyhow::Error` if any scan or upsert fails.
///
/// # Examples
///
/// ```no_run
/// use std::path::PathBuf;
/// use sid::wire::startup_discover;
/// use sid_store::{OpenStore, RedbStore};
///
/// let store = RedbStore::open(std::path::Path::new("/tmp/discover_test.redb")).unwrap();
/// let roots = vec![PathBuf::from("/tmp/vcs-roots")];
/// let count = startup_discover(&store, &roots).unwrap_or(0);
/// // count is how many workspaces were upserted.
/// let _ = count;
/// ```
pub fn startup_discover(store: &dyn Store, roots: &[PathBuf]) -> anyhow::Result<usize> {
    struct Bridge<'a> {
        store: &'a dyn Store,
    }

    impl<'a> WorkspaceUpserter for Bridge<'a> {
        fn upsert(&self, path: &Path, kind: WorkspaceKind, name: &str) -> Result<(), String> {
            let w = Workspace {
                path: path.to_path_buf(),
                name: name.to_string(),
                kind,
                manifest_hash: 0,
                last_seen: now_epoch(),
                parent: None,
            };
            self.store.upsert_workspace(&w).map_err(|e| format!("{e}"))
        }
    }

    let mut total = 0usize;
    for root in roots {
        if !root.exists() {
            continue;
        }
        let discovered =
            scan_workspace_root(root, 2).map_err(|e| anyhow::anyhow!("scan {:?}: {e}", root))?;
        let n = merge_discoveries_into(&Bridge { store }, &discovered)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        total += n;
    }
    Ok(total)
}

/// Run the main render + event loop until the app requests to quit.
///
/// Draws each frame, waits for the next event, dispatches it through the
/// [`App`], and persists the session after each event.  The loop exits when
/// [`App::handle_event`] returns [`Dispatch::Quit`] or the event channel is
/// closed.
///
/// # Errors
///
/// Returns an error if any terminal draw call fails.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use std::sync::Arc;
/// use ratatui::Terminal;
/// use ratatui::backend::TestBackend;
/// use sid::wire::{SidApp, build_app, run_event_loop};
/// use sid_store::{OpenStore, RedbStore, Store};
///
/// #[tokio::main]
/// async fn main() {
///     let backend = TestBackend::new(120, 40);
///     let mut terminal = Terminal::new(backend).unwrap();
///     let store = Arc::new(RedbStore::open(Path::new("/tmp/test.redb")).unwrap());
///     let app = build_app(None, vec![]);
///     let secrets: Arc<dyn sid_core::adapters::secrets::SecretStore> =
///         Arc::new(sid_secrets::PlainStore::new(Arc::clone(&store) as Arc<dyn Store>));
///     let mut sid_app = SidApp {
///         app,
///         store,
///         session_id: "sess-1".to_string(),
///         sys_probe: None,
///         sys_rx: None,
///         systemctl: Arc::new(sid::wire::NoopSystemctlClient),
///         spawner: Arc::new(sid::wire::NoopTerminalSpawner),
///         postgres: sid_db_clients::PostgresClient::factory(),
///         sqlite: sid_db_clients::SqliteClient::factory(),
///         secrets,
///         animation: sid_core::animation::AnimationConfig::default(),
///         fx_state: None,
///         modal_stack: Vec::new(),
///         pending_submits: Vec::new(),
///         toasts: sid::toast::ToastQueue::new(4),
///         jobs: std::sync::Arc::new(sid_job::JobQueue::<sid::wire::JobOutcome>::new()),
///     };
///     let (tx, mut rx) = tokio::sync::mpsc::channel(1);
///     // Drop the sender to close the channel so the loop exits immediately.
///     drop(tx);
///     run_event_loop(&mut terminal, &mut sid_app, &mut rx).await.unwrap();
/// }
/// ```
pub async fn run_event_loop<B>(
    terminal: &mut Terminal<B>,
    sid_app: &mut SidApp,
    rx: &mut Receiver<SidEvent>,
) -> Result<()>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
{
    let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
    loop {
        // Drain any modal submits queued in the previous iteration.
        drain_pending_submits(sid_app);

        // Pull every fresh SysSnapshot the probe has broadcast since the
        // previous frame; forward each into the Network widget. This is what
        // populates the Interfaces / Ports / Processes panes — without it the
        // widget shows three empty tables forever.
        drain_sys_snapshots(sid_app);

        // Drain completed background jobs (ssh-copy-id, ssh-keygen, ssh -vv,
        // ...) and convert each outcome into a toast.
        drain_job_outcomes(sid_app);

        // Sweep expired toasts so they fade out on the next render.
        sid_app.toasts.drain_expired();

        // Advance starfield phase on each frame before drawing.
        if let Some(fx) = sid_app.fx_state.as_mut() {
            let area = terminal
                .size()
                .map(|s| Rect {
                    x: 0,
                    y: 0,
                    width: s.width,
                    height: s.height,
                })
                .unwrap_or(Rect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 24,
                });
            fx.tick(area, &sid_app.animation);
        }
        terminal.draw(|f| draw(f, sid_app))?;
        let ev = match rx.recv().await {
            Some(e) => e,
            None => break,
        };

        // Translate mouse events into synthetic key events (scroll → j/k)
        // or direct tab switches (click on the tab strip). Other mouse kinds
        // are dropped for v1. See `route_mouse_event` for the policy.
        // We rewrite `ev` in place so the rest of the loop treats the result
        // as the originating event.
        // TODO: route LeftDown to widget for focus-on-click once focus model
        // is committed (parallel agent's PR).
        let ev = if let SidEvent::Mouse(m) = ev {
            match route_mouse_event(sid_app, terminal_size_rect(terminal), m) {
                MouseRouting::Synthesize(chord) => SidEvent::Key(chord),
                MouseRouting::SwitchToTab(idx) => {
                    if let Some(tab) = sid_app.app.tabs().tabs().get(idx) {
                        let id = tab.id.clone();
                        let _ = sid_app.app.tabs_mut().switch_to(&id);
                    }
                    // Switching the tab is the whole action; tick the loop.
                    continue;
                }
                MouseRouting::Drop => continue,
            }
        } else {
            ev
        };

        // Route key events. If a modal is open it intercepts everything except
        // global-quit. Otherwise check per-tab modal triggers; if one fires we
        // open the modal and swallow the event. Otherwise dispatch normally.
        let mut handled = false;
        if let SidEvent::Key(chord) = ev {
            // Global quit always wins, even with a modal open.
            let is_global_quit = chord.code == crossterm::event::KeyCode::Char('q')
                && chord.mods.contains(crossterm::event::KeyModifiers::CONTROL);
            if !is_global_quit && !sid_app.modal_stack.is_empty() {
                handled = true;
                let outcome = {
                    let modal = sid_app
                        .modal_stack
                        .last_mut()
                        .expect("modal_stack non-empty");
                    sid_widgets::route_key_to_modal(modal, chord)
                };
                match outcome {
                    sid_widgets::ModalKeyOutcome::Consumed => {}
                    sid_widgets::ModalKeyOutcome::Cancel => {
                        sid_app.modal_stack.pop();
                    }
                    sid_widgets::ModalKeyOutcome::Submit => {
                        let popped = sid_app.modal_stack.pop().expect("modal popped");
                        let values = popped.collect_values();
                        sid_app.pending_submits.push((popped.id, values));
                    }
                }
            } else if !is_global_quit && let Some(modal) = modal_for_active_tab_key(sid_app, chord)
            {
                sid_app.modal_stack.push(modal);
                handled = true;
            }
        }

        if !handled {
            let dispatch = sid_app.app.handle_event(&ev);
            let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
            if matches!(dispatch, Dispatch::Quit) {
                break;
            }
        }
    }
    Ok(())
}

/// What the mouse-event router decided to do with a raw [`crossterm::event::MouseEvent`].
///
/// The three cases match the policy in [`route_mouse_event`]: scrolls become
/// synthetic key events (so widget lists scroll through their existing j/k
/// handlers), clicks on the tab strip switch tabs, anything else is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseRouting {
    /// Translate the mouse event into a key chord that the rest of the loop
    /// dispatches via the existing key path.
    Synthesize(sid_core::event::KeyChord),
    /// Switch directly to the tab at the given zero-based index.
    SwitchToTab(usize),
    /// Drop the event silently. v1 routes only scrolls and tab clicks; any
    /// other mouse kind ends up here.
    Drop,
}

/// Compute the [`Rect`] occupied by the terminal viewport. Used by
/// [`route_mouse_event`] to figure out where the tab strip lives. Returns a
/// default 80x24 if the terminal size cannot be read.
fn terminal_size_rect<B>(terminal: &Terminal<B>) -> Rect
where
    B: Backend,
{
    terminal
        .size()
        .map(|s| Rect {
            x: 0,
            y: 0,
            width: s.width,
            height: s.height,
        })
        .unwrap_or(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        })
}

/// Decide what to do with a raw mouse event.
///
/// Policy (v1):
///
/// - `MouseEventKind::ScrollUp`   → `KeyChord(Char('k'), NONE)` (focus prev row).
/// - `MouseEventKind::ScrollDown` → `KeyChord(Char('j'), NONE)` (focus next row).
/// - `MouseEventKind::Down(Left)` on the tab strip → switch to that tab.
/// - Anything else → [`MouseRouting::Drop`].
///
/// The tab strip is the second row of the rendered frame (y = 1, just below
/// the outer block's top border) — see `draw` for the layout.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
/// use ratatui::layout::Rect;
/// use sid::wire::{MouseRouting, build_app, route_mouse_event, SidApp};
/// // ScrollUp anywhere → Char('k')
/// // (constructing a full SidApp for a doctest is too much chrome — see
/// // wire's unit tests for the integration shape.)
/// ```
pub fn route_mouse_event(
    sid_app: &SidApp,
    full_area: Rect,
    m: crossterm::event::MouseEvent,
) -> MouseRouting {
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
    match m.kind {
        MouseEventKind::ScrollUp => MouseRouting::Synthesize(sid_core::event::KeyChord::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        )),
        MouseEventKind::ScrollDown => MouseRouting::Synthesize(sid_core::event::KeyChord::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )),
        MouseEventKind::Down(MouseButton::Left) => {
            // Match against the tab strip's row. The outer block adds a
            // one-row top border, so the tab strip sits at y = 1.
            // `draw` lays out the tab strip on row inner.y, where inner.y
            // == full_area.y + 1 (because the outer Block::ALL borders eat
            // a row on each side).
            let tab_row = full_area.y.saturating_add(1);
            if m.row != tab_row {
                return MouseRouting::Drop;
            }
            // Compute per-tab horizontal extents using the same layout the
            // tab strip painter uses: [marker(1)][space(1)][title(N)][gap(2 if not last)].
            // The first tab starts at inner.x == full_area.x + 1.
            let mut x = full_area.x.saturating_add(1);
            let tabs = sid_app.app.tabs().tabs();
            for (i, t) in tabs.iter().enumerate() {
                let title_width = t.title.chars().count() as u16;
                // Marker glyph + space + title.
                let span = 2u16.saturating_add(title_width);
                let end = x.saturating_add(span);
                if m.column >= x && m.column < end {
                    return MouseRouting::SwitchToTab(i);
                }
                x = end.saturating_add(2); // 2-char gap between tabs
                if i + 1 == tabs.len() {
                    break;
                }
            }
            MouseRouting::Drop
        }
        _ => MouseRouting::Drop,
    }
}

// ---------------------------------------------------------------------------
// Phase 3 / 4 / 5 — Modal routing + per-tab CRUD
// ---------------------------------------------------------------------------

/// Dispatch a key chord to the per-tab modal-opener for whichever tab is
/// currently active. Returns `None` if the key has no modal binding on the
/// active tab (or if a global modifier is held).
///
/// Per-tab branches live in their own helpers
/// ([`workspaces_modal_for_key`], [`ssh_modal_for_key`],
/// [`database_modal_for_key`], [`system_modal_for_key`]).
fn modal_for_active_tab_key(
    sid_app: &SidApp,
    chord: sid_core::event::KeyChord,
) -> Option<sid_widgets::ModalSpec> {
    use crossterm::event::{KeyCode, KeyModifiers};
    // Only plain (unmodified) keys open modals; ctrl/alt combos are reserved
    // for global actions.
    if chord
        .mods
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    // Global help modal: `?` on any tab.
    if chord.code == KeyCode::Char('?') {
        return Some(help_modal_for_active_tab(sid_app));
    }
    match sid_app.app.tabs().active().id.as_str() {
        "workspaces" => workspaces_modal_for_key(sid_app, chord),
        "ssh" => ssh_modal_for_key(sid_app, chord),
        "database" => database_modal_for_key(sid_app, chord),
        "system" => system_modal_for_key(sid_app, chord),
        _ => None,
    }
}

/// Workspaces-tab modal opener. `N` creates, `A` adds a sub-repo to the
/// selected umbrella, `R` confirms removal of the selected workspace.
fn workspaces_modal_for_key(
    sid_app: &SidApp,
    chord: sid_core::event::KeyChord,
) -> Option<sid_widgets::ModalSpec> {
    use crossterm::event::KeyCode;
    use sid_widgets::{Field, ModalSpec};
    match chord.code {
        KeyCode::Char('N') | KeyCode::Char('n') => Some(
            ModalSpec::new(
                "workspaces.new",
                "New Workspace",
                vec![
                    Field::Text {
                        label: "name".into(),
                        value: String::new(),
                        placeholder: Some("e.g. my-monorepo".into()),
                    },
                    Field::Picker {
                        label: "path".into(),
                        value: String::new(),
                        hint: "absolute path".into(),
                    },
                    Field::Choice {
                        label: "kind".into(),
                        options: vec!["Umbrella".into(), "Repo".into()],
                        selected: 0,
                    },
                ],
            )
            .with_help("Tab moves between fields · Enter saves · Esc cancels"),
        ),
        KeyCode::Char('A') | KeyCode::Char('a') => {
            // Only meaningful when an umbrella is selected; if not, drop the
            // open and let the event flow through.
            let parent = workspaces_selected_path(sid_app)?;
            Some(
                ModalSpec::new(
                    format!("workspaces.add_repo:{}", parent.display()),
                    format!("Add repo to {}", parent.display()),
                    vec![Field::Picker {
                        label: "repo path".into(),
                        value: String::new(),
                        hint: "absolute path".into(),
                    }],
                )
                .with_help("Tab moves between fields · Enter saves · Esc cancels"),
            )
        }
        KeyCode::Char('R') | KeyCode::Char('r') => {
            let target = workspaces_selected_path(sid_app)?;
            Some(
                ModalSpec::new(
                    format!("workspaces.remove:{}", target.display()),
                    format!("Remove workspace {}?", target.display()),
                    vec![Field::Choice {
                        label: "confirm".into(),
                        options: vec!["No, cancel".into(), "Yes, remove".into()],
                        selected: 0,
                    }],
                )
                .with_help("Removes the workspace registration. Files are NOT deleted."),
            )
        }
        _ => None,
    }
}

/// SSH-tab modal opener.
///
/// Key bindings:
/// - `N` / `n` — add new host
/// - `E` / `e` — edit selected manual host (ssh-config entries are read-only)
/// - `G` / `g` — generate-key wizard (step 1: choose algorithm)
/// - `S` / `s` — setup-remote-auth wizard (step 1: pick identity)
/// - `K`       — key manager drawer (uppercase only — `k` is "select prev")
/// - `X`       — debug drawer (uppercase only — `x` is reserved for selection)
/// - `F` / `f` — persist last-SFTP-path on selected host
/// - `Del` / `D` / `d` — remove selected manual host
fn ssh_modal_for_key(
    sid_app: &SidApp,
    chord: sid_core::event::KeyChord,
) -> Option<sid_widgets::ModalSpec> {
    use crossterm::event::KeyCode;
    use sid_store::SshHostSource;
    use sid_widgets::{Field, ModalSpec};
    match chord.code {
        KeyCode::Char('N') | KeyCode::Char('n') => Some(ssh_new_modal()),
        KeyCode::Char('E') | KeyCode::Char('e') => {
            let host = ssh_selected_host(sid_app)?;
            // ssh-config entries are read-only; no Edit modal for them.
            if host.source == SshHostSource::SshConfig {
                return None;
            }
            Some(ssh_edit_modal(&host))
        }
        KeyCode::Char('G') | KeyCode::Char('g') => Some(ssh_gen_key_step1_modal()),
        KeyCode::Char('S') | KeyCode::Char('s') => {
            let host = ssh_selected_host(sid_app)?;
            Some(ssh_setup_remote_step1_modal(&host.alias))
        }
        // Key manager: uppercase only so it doesn't collide with widget `k`
        // (which means "select prev").
        KeyCode::Char('K') => Some(ssh_key_manager_modal()),
        // Debug drawer: uppercase only for the same reason as `K`.
        KeyCode::Char('X') => {
            let host = ssh_selected_host(sid_app)?;
            Some(ssh_debug_modal(&host.alias))
        }
        KeyCode::Char('F') | KeyCode::Char('f') => {
            let host = ssh_selected_host(sid_app)?;
            // Persisting SFTP path on an ssh-config-only entry makes no sense
            // because there is no store record to update.
            if host.source == SshHostSource::SshConfig {
                return None;
            }
            Some(ssh_sftp_persist_modal(&host))
        }
        KeyCode::Delete | KeyCode::Char('D') | KeyCode::Char('d') => {
            let host = ssh_selected_host(sid_app)?;
            if host.source == SshHostSource::SshConfig {
                return None;
            }
            let alias = host.alias.clone();
            Some(
                ModalSpec::new(
                    format!("ssh.remove:{alias}"),
                    format!("Remove host {alias}?"),
                    vec![Field::Choice {
                        label: "confirm".into(),
                        options: vec!["No, cancel".into(), "Yes, remove".into()],
                        selected: 0,
                    }],
                )
                .with_help("Removes the host from the store. ssh-config entries are unaffected."),
            )
        }
        _ => None,
    }
}

/// Build the "Add Host" modal — extracted from [`ssh_modal_for_key`] so the
/// edit modal can share field shapes.
fn ssh_new_modal() -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    let default_user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    ModalSpec::new(
        "ssh.new",
        "Add Host",
        vec![
            Field::Text {
                label: "alias".into(),
                value: String::new(),
                placeholder: Some("e.g. my-prod".into()),
            },
            Field::Text {
                label: "host".into(),
                value: String::new(),
                placeholder: Some("e.g. host.example.com".into()),
            },
            Field::Text {
                label: "user".into(),
                value: default_user,
                placeholder: None,
            },
            Field::Text {
                label: "port".into(),
                value: "22".into(),
                placeholder: None,
            },
            Field::Picker {
                label: "identity_file".into(),
                value: String::new(),
                hint: "optional".into(),
            },
            Field::Choice {
                label: "auth".into(),
                options: vec!["Key".into(), "Password".into(), "Agent".into()],
                selected: 0,
            },
        ],
    )
    .with_help("Tab moves between fields · Enter saves · Esc cancels")
}

/// Build the "Edit Host" modal pre-filled with the host's current values.
fn ssh_edit_modal(host: &sid_store::SshHost) -> sid_widgets::ModalSpec {
    use sid_store::SshAuthKind;
    use sid_widgets::{Field, ModalSpec};
    let auth_idx = match host.auth_kind {
        SshAuthKind::Key => 0,
        SshAuthKind::Password => 1,
        SshAuthKind::Agent => 2,
    };
    ModalSpec::new(
        format!("ssh.edit:{}", host.alias),
        format!("Edit Host: {}", host.alias),
        vec![
            Field::Text {
                label: "alias".into(),
                value: host.alias.clone(),
                placeholder: None,
            },
            Field::Text {
                label: "host".into(),
                value: host.host.clone(),
                placeholder: None,
            },
            Field::Text {
                label: "user".into(),
                value: host.user.clone(),
                placeholder: None,
            },
            Field::Text {
                label: "port".into(),
                value: host.port.to_string(),
                placeholder: None,
            },
            Field::Picker {
                label: "identity_file".into(),
                value: host.identity_file.clone().unwrap_or_default(),
                hint: "optional".into(),
            },
            Field::Choice {
                label: "auth".into(),
                options: vec!["Key".into(), "Password".into(), "Agent".into()],
                selected: auth_idx,
            },
        ],
    )
    .with_help("Tab moves between fields · Enter saves · Esc cancels")
}

/// Build the gen-key wizard step 1 modal — algorithm choice. Step 2 is
/// pushed by the submit handler after Save.
fn ssh_gen_key_step1_modal() -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    ModalSpec::new(
        "ssh.gen_key.step1",
        "Generate SSH Key — 1/3 algorithm",
        vec![Field::Choice {
            label: "algorithm".into(),
            options: vec!["Ed25519".into(), "RSA-4096".into(), "ECDSA-256".into()],
            selected: 0,
        }],
    )
    .with_help("Ed25519 is recommended. Enter to continue, Esc to cancel.")
}

/// Build the gen-key wizard step 2 — output path + passphrase + comment.
fn ssh_gen_key_step2_modal(algorithm: &str) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    let default_user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    let host = hostname_or_local();
    let default_comment = format!("{default_user}@{host}");
    let default_path = home_join(&format!(".ssh/id_{}", algo_filename_suffix(algorithm)));
    ModalSpec::new(
        format!("ssh.gen_key.step2:{algorithm}"),
        format!("Generate SSH Key — 2/3 path + passphrase ({algorithm})"),
        vec![
            Field::Picker {
                label: "output_path".into(),
                value: default_path,
                hint: "path".into(),
            },
            Field::Password {
                label: "passphrase".into(),
                value: String::new(),
            },
            Field::Password {
                label: "confirm_passphrase".into(),
                value: String::new(),
            },
            Field::Text {
                label: "comment".into(),
                value: default_comment,
                placeholder: None,
            },
        ],
    )
    .with_help("Passphrase fields must match. Enter to run ssh-keygen, Esc to cancel.")
}

/// Build the gen-key wizard step 3 — optionally copy the new key to a remote
/// host via `ssh-copy-id`.
fn ssh_gen_key_step3_modal(
    algorithm: &str,
    output_path: &str,
    aliases: &[String],
) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    let mut options: Vec<String> = vec!["<None — copy manually later>".into()];
    options.extend(aliases.iter().cloned());
    ModalSpec::new(
        format!("ssh.gen_key.step3:{algorithm}:{output_path}"),
        "Generate SSH Key — 3/3 copy to remote".to_string(),
        vec![Field::Choice {
            label: "target_host".into(),
            options,
            selected: 0,
        }],
    )
    .with_help("Selecting a host runs ssh-copy-id <alias>. Choose None to skip.")
}

/// Suffix used for the default ssh-keygen output filename per algorithm.
fn algo_filename_suffix(algorithm: &str) -> &'static str {
    match algorithm {
        "Ed25519" => "ed25519",
        "RSA-4096" => "rsa",
        "ECDSA-256" => "ecdsa",
        _ => "ed25519",
    }
}

/// Build the "Setup remote auth" step 1 modal — pick an existing identity.
fn ssh_setup_remote_step1_modal(alias: &str) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    let keys = discover_ssh_keys();
    let mut options: Vec<String> = keys.iter().map(|k| k.path.clone()).collect();
    if options.is_empty() {
        options.push("(no existing key found in ~/.ssh/)".into());
    }
    ModalSpec::new(
        format!("ssh.setup_remote.identity:{alias}"),
        format!("Setup remote auth — 1/3 pick identity ({alias})"),
        vec![Field::Choice {
            label: "identity_path".into(),
            options,
            selected: 0,
        }],
    )
    .with_help("Pick the local private key to install on the remote.")
}

/// Build the "Setup remote auth" step 2 modal — confirmation summary.
fn ssh_setup_remote_step2_modal(alias: &str, identity: &str) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    let preview = format!(
        "Will install {identity} on {alias} (ssh-copy-id). The host record's \
         identity_file will be updated on success."
    );
    ModalSpec::new(
        format!("ssh.setup_remote.confirm:{alias}:{identity}"),
        format!("Setup remote auth — 2/3 confirm ({alias})"),
        vec![
            Field::Text {
                label: "summary".into(),
                value: preview,
                placeholder: None,
            },
            Field::Choice {
                label: "proceed".into(),
                options: vec!["Yes, proceed".into(), "No, cancel".into()],
                selected: 0,
            },
        ],
    )
    .with_help("Step 3 runs ssh-copy-id and reports the captured output.")
}

/// Build the "Setup remote auth" step 3 modal — show the captured output.
///
/// Currently unused: the async `ssh-copy-id` flow surfaces its outcome via a
/// toast (`drain_job_outcomes`) instead of pushing a synchronous result
/// modal. Kept around for future flows that want to display long-form
/// captured output inline.
#[allow(dead_code)]
fn ssh_setup_remote_step3_modal(alias: &str, summary: &str) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    ModalSpec::new(
        format!("ssh.setup_remote.result:{alias}"),
        format!("Setup remote auth — 3/3 result ({alias})"),
        vec![Field::Text {
            label: "output".into(),
            value: truncate_lines(summary, 10),
            placeholder: None,
        }],
    )
    .with_help("Esc closes. The host's identity_file was updated on success.")
}

/// Build the SSH key manager modal.
fn ssh_key_manager_modal() -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    let keys = discover_ssh_keys();
    let mut fields: Vec<Field> = Vec::new();
    if keys.is_empty() {
        fields.push(Field::Text {
            label: "keys".into(),
            value: "(no keys found under ~/.ssh/)".into(),
            placeholder: None,
        });
    } else {
        for k in &keys {
            fields.push(Field::Text {
                label: k.path.clone(),
                value: format!(
                    "{} · {}",
                    k.fingerprint.as_deref().unwrap_or("(no fingerprint)"),
                    k.comment.as_deref().unwrap_or("")
                ),
                placeholder: None,
            });
        }
    }
    let target_options: Vec<String> = if keys.is_empty() {
        vec!["(none)".into()]
    } else {
        keys.iter().map(|k| k.path.clone()).collect()
    };
    fields.push(Field::Choice {
        label: "target".into(),
        options: target_options,
        selected: 0,
    });
    fields.push(Field::Choice {
        label: "action".into(),
        options: vec![
            "Show public key".into(),
            "Regenerate".into(),
            "Delete".into(),
            "Cancel".into(),
        ],
        selected: 0,
    });
    ModalSpec::new("ssh.key_manager", "SSH Key Manager", fields)
        .with_help("Pick target + action. Delete/Regenerate require a confirm step.")
}

/// Build the SSH debug modal.
fn ssh_debug_modal(alias: &str) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    ModalSpec::new(
        format!("ssh.debug:{alias}"),
        format!("SSH Debug — {alias}"),
        vec![Field::Choice {
            label: "action".into(),
            options: vec![
                "Show known_hosts entry".into(),
                "Remove known_hosts entry".into(),
                "Show identity diagnostics".into(),
                "Test connection (ssh -vv)".into(),
                "Clear cached agent identities (ssh-add -D)".into(),
            ],
            selected: 0,
        }],
    )
    .with_help("Output is captured to the tracing log; Esc closes.")
}

/// Build the SFTP-persist modal — store a last-browsed remote path on the
/// host record.
fn ssh_sftp_persist_modal(host: &sid_store::SshHost) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    ModalSpec::new(
        format!("ssh.sftp_persist:{}", host.alias),
        format!("SFTP last path for {}", host.alias),
        vec![Field::Text {
            label: "last_path".into(),
            value: host.last_sftp_path.clone().unwrap_or_default(),
            placeholder: Some("e.g. /var/log".into()),
        }],
    )
    .with_help("Saved on the host record; restored on the next SFTP open.")
}

/// Build the help modal showing per-tab footer hints + global hints.
fn help_modal_for_active_tab(sid_app: &SidApp) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    let tab_id = sid_app.app.tabs().active().id.as_str().to_string();
    let tab_title = sid_app.app.tabs().active().title.clone();
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("{tab_title}:"));
    if let Some(w) = sid_app.app.tabs().active().layout.iter_widgets().next() {
        let hints = w.footer_hint();
        if hints.is_empty() {
            lines.push("  (no tab-local actions)".into());
        } else {
            for h in hints {
                lines.push(format!("  {}: {}", h.chord, h.label));
            }
        }
    } else {
        lines.push("  (no widget)".into());
    }
    lines.push(String::new());
    lines.push("Global:".into());
    lines.push("  Ctrl+Q: quit".into());
    lines.push("  Ctrl+F: palette".into());
    lines.push("  Ctrl+\u{2190}/\u{2192}: tabs".into());
    lines.push("  Ctrl+1..6: jump to tab".into());
    lines.push("  Ctrl+,: settings".into());
    lines.push("  ?: this help".into());
    ModalSpec::new(
        format!("help:{tab_id}"),
        format!("Help — {tab_title}"),
        // `Field::Display` paints each `\n`-separated line on its own row.
        // Previously this used `Field::Text` whose single-row renderer
        // showed literal `\n` characters or clipped the body to one line.
        vec![Field::Display {
            label: "keys".into(),
            body: lines.join("\n"),
        }],
    )
    .with_help("Esc closes.")
}

/// Truncate `s` to at most `max_lines` lines (joined by `\n`).
fn truncate_lines(s: &str, max_lines: usize) -> String {
    let mut out: Vec<&str> = s.lines().take(max_lines).collect();
    if s.lines().count() > max_lines {
        out.push("…");
    }
    out.join("\n")
}

/// Information about a private key discovered under `~/.ssh/`.
#[derive(Debug, Clone)]
pub struct SshKeyInfo {
    /// Absolute path to the private key file.
    pub path: String,
    /// `ssh-keygen -lf <path>` fingerprint, if the tool is available and the
    /// key is readable. `None` on any error.
    pub fingerprint: Option<String>,
    /// Trailing comment from the fingerprint line (best-effort).
    pub comment: Option<String>,
}

/// Discover candidate SSH private-key files under `~/.ssh/`.
///
/// Selects regular files whose name starts with `id_` and does not end with
/// `.pub`. Best-effort: errors are swallowed and produce an empty list.
/// Fingerprints are pulled from `ssh-keygen -lf` when available.
pub fn discover_ssh_keys() -> Vec<SshKeyInfo> {
    discover_ssh_keys_in(None)
}

/// Same as [`discover_ssh_keys`] but accepts a custom `~/.ssh/` directory —
/// used by tests to avoid touching the user's real keys.
pub fn discover_ssh_keys_in(override_dir: Option<&Path>) -> Vec<SshKeyInfo> {
    let dir = match override_dir {
        Some(d) => d.to_path_buf(),
        None => match UserDirs::new() {
            Some(u) => u.home_dir().join(".ssh"),
            None => return Vec::new(),
        },
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.starts_with("id_") || name.ends_with(".pub") {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let path_str = path.to_string_lossy().into_owned();
        let (fingerprint, comment) = read_key_fingerprint(&path_str);
        out.push(SshKeyInfo {
            path: path_str,
            fingerprint,
            comment,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Best-effort `(fingerprint, comment)` extracted from
/// `ssh-keygen -lf <path>`. Returns `(None, None)` on any failure.
fn read_key_fingerprint(path: &str) -> (Option<String>, Option<String>) {
    use std::process::Command;
    let out = Command::new("ssh-keygen").arg("-lf").arg(path).output();
    let Ok(out) = out else {
        return (None, None);
    };
    if !out.status.success() {
        return (None, None);
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Line shape: "<bits> <fingerprint> <comment...> (<type>)"
    let mut parts = line.splitn(3, ' ');
    let _bits = parts.next();
    let fp = parts.next().map(|s| s.to_string());
    let rest = parts.next().map(|s| s.to_string());
    (fp, rest)
}

/// Database-tab modal opener. `N` adds a new connection; `Del` / `D` removes
/// the selected one.
fn database_modal_for_key(
    sid_app: &SidApp,
    chord: sid_core::event::KeyChord,
) -> Option<sid_widgets::ModalSpec> {
    use crossterm::event::KeyCode;
    use sid_widgets::{Field, ModalSpec};
    match chord.code {
        KeyCode::Char('N') | KeyCode::Char('n') => Some(
            ModalSpec::new(
                "database.new",
                "Add Connection",
                vec![
                    Field::Text {
                        label: "id".into(),
                        value: String::new(),
                        placeholder: Some("stable id, e.g. prod-pg".into()),
                    },
                    Field::Text {
                        label: "name".into(),
                        value: String::new(),
                        placeholder: Some("display label".into()),
                    },
                    Field::Choice {
                        label: "kind".into(),
                        options: vec!["Postgres".into(), "SQLite".into()],
                        selected: 0,
                    },
                    Field::Text {
                        label: "dsn".into(),
                        value: String::new(),
                        placeholder: Some("postgres://user@host/db or /path/to/file.sqlite".into()),
                    },
                    Field::Password {
                        label: "password".into(),
                        value: String::new(),
                    },
                ],
            )
            .with_help("Password is stored in the secrets table (Postgres only)."),
        ),
        KeyCode::Delete | KeyCode::Char('D') | KeyCode::Char('d') => {
            let conn = database_selected_connection(sid_app)?;
            let id = conn.id.clone();
            Some(
                ModalSpec::new(
                    format!("database.remove:{id}"),
                    format!("Remove connection {id}?"),
                    vec![Field::Choice {
                        label: "confirm".into(),
                        options: vec!["No, cancel".into(), "Yes, remove".into()],
                        selected: 0,
                    }],
                )
                .with_help("Removes the connection record and forgets the stored password."),
            )
        }
        _ => None,
    }
}

/// System-tab modal opener. The shape of the modal depends on which sub-pane
/// is focused — `PinnedConfigs` or `QuickActions`. Pressing `N` / `D` while
/// the `Services` pane is focused is a no-op (services are read from
/// systemd, not stored).
fn system_modal_for_key(
    sid_app: &SidApp,
    chord: sid_core::event::KeyChord,
) -> Option<sid_widgets::ModalSpec> {
    use crossterm::event::KeyCode;
    use sid_widgets::system::SystemPane;
    use sid_widgets::{Field, ModalSpec};
    let pane = system_focused_pane(sid_app)?;
    match (chord.code, pane) {
        (KeyCode::Char('N') | KeyCode::Char('n'), SystemPane::PinnedConfigs) => Some(
            ModalSpec::new(
                "system.pin_config",
                "Pin a Config",
                vec![
                    Field::Picker {
                        label: "path".into(),
                        value: String::new(),
                        hint: "absolute path".into(),
                    },
                    Field::Text {
                        label: "label".into(),
                        value: String::new(),
                        placeholder: Some("defaults to basename".into()),
                    },
                ],
            )
            .with_help("Pins the file; opens later via the System tab pinned-configs pane."),
        ),
        (KeyCode::Char('N') | KeyCode::Char('n'), SystemPane::QuickActions) => Some(
            ModalSpec::new(
                "system.quick_action.new",
                "Add Quick Action",
                vec![
                    Field::Text {
                        label: "id".into(),
                        value: String::new(),
                        placeholder: Some("e.g. qa-restart-pg".into()),
                    },
                    Field::Text {
                        label: "label".into(),
                        value: String::new(),
                        placeholder: Some("display label".into()),
                    },
                    Field::Text {
                        label: "command".into(),
                        value: String::new(),
                        placeholder: Some("shell command to run".into()),
                    },
                    Field::Choice {
                        label: "scope".into(),
                        options: vec!["Global".into(), "Workspace".into()],
                        selected: 0,
                    },
                    Field::Text {
                        label: "keybind".into(),
                        value: String::new(),
                        placeholder: Some("optional chord, e.g. Char('r')|2".into()),
                    },
                ],
            )
            .with_help("Global actions appear in the command palette after save."),
        ),
        (KeyCode::Delete | KeyCode::Char('D') | KeyCode::Char('d'), SystemPane::PinnedConfigs) => {
            let pin = system_selected_pin(sid_app)?;
            let path = pin.path.clone();
            Some(
                ModalSpec::new(
                    format!("system.remove_pin:{}", path.display()),
                    format!("Remove pin {}?", path.display()),
                    vec![Field::Choice {
                        label: "confirm".into(),
                        options: vec!["No, cancel".into(), "Yes, remove".into()],
                        selected: 0,
                    }],
                )
                .with_help("Removes the pin. The file on disk is untouched."),
            )
        }
        (KeyCode::Delete | KeyCode::Char('D') | KeyCode::Char('d'), SystemPane::QuickActions) => {
            let qa = system_selected_quick_action(sid_app)?;
            let id = qa.id.clone();
            Some(
                ModalSpec::new(
                    format!("system.remove_quick_action:{id}"),
                    format!("Remove quick action {id}?"),
                    vec![Field::Choice {
                        label: "confirm".into(),
                        options: vec!["No, cancel".into(), "Yes, remove".into()],
                        selected: 0,
                    }],
                )
                .with_help("Removes the action from the store and the command palette."),
            )
        }
        // Services pane has no add/remove modal; everything comes from systemd.
        _ => None,
    }
}

/// Inspect the active Workspaces widget for the selected workspace's path.
fn workspaces_selected_path(sid_app: &SidApp) -> Option<PathBuf> {
    use sid_widgets::WorkspacesWidget;
    let layout = &sid_app.app.tabs().active().layout;
    let widget = layout.iter_widgets().next()?;
    let ws = widget.as_any().downcast_ref::<WorkspacesWidget>()?;
    ws.state().selected_workspace().map(|w| w.path.clone())
}

/// Inspect the active SSH widget for the currently-selected host.
fn ssh_selected_host(sid_app: &SidApp) -> Option<sid_store::SshHost> {
    let layout = &sid_app.app.tabs().active().layout;
    let widget = layout.iter_widgets().next()?;
    let ssh = widget.as_any().downcast_ref::<SshWidget>()?;
    ssh.state().selected_host().cloned()
}

/// Inspect the active Database widget for the currently-selected connection.
fn database_selected_connection(sid_app: &SidApp) -> Option<sid_store::DbConnection> {
    let layout = &sid_app.app.tabs().active().layout;
    let widget = layout.iter_widgets().next()?;
    let db = widget.as_any().downcast_ref::<DatabaseWidget>()?;
    db.state().selected_connection().cloned()
}

/// Which sub-pane is focused on the System tab, if any.
fn system_focused_pane(sid_app: &SidApp) -> Option<sid_widgets::system::SystemPane> {
    let layout = &sid_app.app.tabs().active().layout;
    let widget = layout.iter_widgets().next()?;
    let sys = widget.as_any().downcast_ref::<SystemWidget>()?;
    Some(sys.state().focused_pane())
}

/// Inspect the System widget for the selected pinned config.
fn system_selected_pin(sid_app: &SidApp) -> Option<sid_store::PinnedConfig> {
    let layout = &sid_app.app.tabs().active().layout;
    let widget = layout.iter_widgets().next()?;
    let sys = widget.as_any().downcast_ref::<SystemWidget>()?;
    sys.pinned_configs().selected().cloned()
}

/// Inspect the System widget for the selected quick action.
fn system_selected_quick_action(sid_app: &SidApp) -> Option<sid_store::QuickAction> {
    let layout = &sid_app.app.tabs().active().layout;
    let widget = layout.iter_widgets().next()?;
    let sys = widget.as_any().downcast_ref::<SystemWidget>()?;
    sys.quick_actions().selected().cloned()
}

/// Best-effort lookup of a default `$HOME`-relative path for the SSH keygen
/// modal. Falls back to a bare relative path if `$HOME` is unset.
fn home_join(rel: &str) -> String {
    match std::env::var_os("HOME") {
        Some(h) => PathBuf::from(h).join(rel).to_string_lossy().into_owned(),
        None => rel.to_string(),
    }
}

/// Best-effort hostname for use in default ssh-keygen comments. Returns
/// `"localhost"` if the lookup fails.
fn hostname_or_local() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| {
        // Try /etc/hostname as a fallback; some shells don't export HOSTNAME.
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "localhost".to_string())
    })
}

/// Drain any queued modal submits and call the corresponding handler.
fn drain_pending_submits(sid_app: &mut SidApp) {
    let submits = std::mem::take(&mut sid_app.pending_submits);
    for (id, values) in submits {
        if let Err(e) = dispatch_modal_submit(sid_app, &id, &values) {
            tracing::warn!("modal submit {id:?} failed: {e}");
            sid_app
                .toasts
                .push(Toast::error(format!("{}: {}", id.0, e)));
        }
    }
}

/// Drain every snapshot the probe has produced since the previous frame.
///
/// Returns once the channel reports `Empty`. Lag (slow consumer relative to
/// the broadcast channel capacity) is logged and treated as a missed frame —
/// the loop continues so the next snapshot is still applied.
pub fn drain_sys_snapshots(sid_app: &mut SidApp) {
    if sid_app.sys_rx.is_none() {
        return;
    }
    // Pull snapshots into a local buffer first to release the &mut borrow on
    // `sid_app.sys_rx` before we hand `sid_app` to `refresh_network_widget`.
    let mut snapshots: Vec<SysSnapshot> = Vec::new();
    let mut closed = false;
    {
        let rx = sid_app.sys_rx.as_mut().expect("checked is_none above");
        loop {
            match rx.try_recv() {
                Ok(snap) => snapshots.push(snap),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        "sys_probe broadcast lagged; {skipped} snapshots skipped — continuing"
                    );
                }
                Err(TryRecvError::Closed) => {
                    tracing::warn!("sys_probe broadcast closed; sys_rx detached");
                    closed = true;
                    break;
                }
            }
        }
    }
    if closed {
        sid_app.sys_rx = None;
    }
    for snap in snapshots {
        refresh_network_widget(sid_app, snap);
    }
}

/// Forward a fresh [`SysSnapshot`] into the Network widget.
///
/// Mirrors the shape of [`refresh_workspaces_widget`]: look up the tab by id,
/// downcast, and call the widget's existing `apply_snapshot`. Silently no-ops
/// when the Network tab isn't installed (e.g., a custom `TabManager` in tests).
pub fn refresh_network_widget(sid_app: &mut SidApp, snap: SysSnapshot) {
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "network" {
            if let Some(w) = t.layout.iter_widgets_mut().next() {
                let any_ref = w as &mut dyn std::any::Any;
                if let Some(n) = any_ref.downcast_mut::<NetworkWidget>() {
                    n.apply_snapshot(snap);
                }
            }
            return;
        }
    }
}

/// Drain every completed [`JobOutcome`] from the queue and convert each into
/// a toast. Pure transformation — never blocks.
pub fn drain_job_outcomes(sid_app: &mut SidApp) {
    let completed = sid_app.jobs.drain_completed();
    for r in completed {
        match r {
            Ok(JobOutcome::Success { label, message }) => {
                sid_app
                    .toasts
                    .push(Toast::success(format!("{label}: {message}")));
            }
            Ok(JobOutcome::Failure { label, message }) => {
                sid_app
                    .toasts
                    .push(Toast::error(format!("{label}: {message}")));
            }
            Err(e) => {
                sid_app.toasts.push(Toast::error(format!("job: {e}")));
            }
        }
    }
}

/// Render the toast queue anchored to the bottom-right of `area`.
///
/// Toasts stack vertically newest-at-the-bottom, with a maximum of 3 visible
/// at once. Each toast is a single-line `Paragraph` consisting of a coloured
/// glyph prefix (a check, an x, or a dot) + space + message body. The
/// rendered region is right-padded by 1 cell from `area.right()`.
///
/// Called from [`draw`] AFTER the body + footer but BEFORE the modal / palette
/// overlay so modals visually cover the toast region.
pub fn render_toasts(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: &Theme,
    queue: &ToastQueue,
) {
    use crate::toast::ToastKind;
    use ratatui::style::Modifier as TextMod;
    use ratatui::style::Style as TextStyle;
    use ratatui::widgets::Paragraph;

    if queue.is_empty() || area.width < 6 || area.height == 0 {
        return;
    }
    let cap_visible = 3usize;
    let total = queue.len();
    let take = total.min(cap_visible);
    if take == 0 {
        return;
    }
    let visible: Vec<&Toast> = queue.iter().skip(total - take).collect();
    let right_pad: u16 = 1;
    let max_width: u16 = visible
        .iter()
        .map(|t| (t.message.chars().count() as u16).saturating_add(4))
        .max()
        .unwrap_or(20);
    let strip_w = max_width.min(area.width.saturating_sub(right_pad + 1));
    let strip_h: u16 = (take as u16).min(area.height);
    if strip_w == 0 || strip_h == 0 {
        return;
    }
    let x = area.x + area.width.saturating_sub(strip_w + right_pad);
    let y = area.y + area.height.saturating_sub(strip_h);

    for (i, t) in visible.iter().enumerate() {
        let line_y = y + i as u16;
        let (glyph, color) = match t.kind {
            ToastKind::Success => ('\u{2713}', theme.accent_success),
            ToastKind::Error => ('\u{2717}', theme.accent_error),
            ToastKind::Info => ('\u{00B7}', theme.muted),
        };
        let glyph_style = TextStyle::default()
            .fg(ui_to_ratatui(color))
            .add_modifier(TextMod::BOLD);
        let body_style = TextStyle::default().fg(ui_to_ratatui(theme.foreground));
        let line = Line::from(vec![
            Span::styled(format!("{glyph} "), glyph_style),
            Span::styled(t.message.clone(), body_style),
        ]);
        let p = Paragraph::new(line);
        let rect = Rect {
            x,
            y: line_y,
            width: strip_w,
            height: 1,
        };
        frame.render_widget(p, rect);
    }
}

/// Trigger a celebration supernova bloom on the configured FX state.
///
/// No-op when:
/// - `fx_state` is None (animation disabled or in tests),
/// - `animation.enabled == false`, or
/// - `animation.supernova_on_event == false`.
///
/// Called after every successful "add"-flavoured mutation (new workspace,
/// new SSH host, new DB connection, new pinned config, new quick action,
/// new SSH key). Removals don't celebrate.
fn celebrate(sid_app: &mut SidApp, palette: sid_fx::SupernovaPalette) {
    if !sid_app.animation.enabled || !sid_app.animation.supernova_on_event {
        return;
    }
    let Some(fx) = sid_app.fx_state.as_mut() else {
        return;
    };
    // Use a representative area; the FxState clamps internally to the last
    // tick area, so passing 80x24 here is safe for tests. The real binary
    // re-ticks with the actual terminal size every frame.
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    fx.trigger_supernova(area, palette);
}

/// Look up the submit handler for a modal id and run it. Refreshes any
/// affected widget after a successful mutation.
fn dispatch_modal_submit(
    sid_app: &mut SidApp,
    id: &sid_widgets::ModalId,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    use sid_widgets::FieldValue;
    let key = id.0.as_str();
    if key == "workspaces.new" {
        let name = string_value(values, "name").unwrap_or_default();
        let path_str = string_value(values, "path").unwrap_or_default();
        let kind_str = choice_value(values, "kind").unwrap_or_else(|| "Repo".into());
        if name.is_empty() || path_str.is_empty() {
            return Err(anyhow::anyhow!("name and path are required"));
        }
        let path = PathBuf::from(&path_str);
        let abs = std::fs::canonicalize(&path).unwrap_or(path);
        let kind = match kind_str.as_str() {
            "Umbrella" => WorkspaceKind::Umbrella,
            _ => WorkspaceKind::Repo,
        };
        let w = Workspace {
            path: abs,
            name: name.clone(),
            kind,
            manifest_hash: 0,
            last_seen: now_epoch(),
            parent: None,
        };
        sid_app
            .store
            .upsert_workspace(&w)
            .map_err(|e| anyhow::anyhow!("upsert workspace: {e}"))?;
        refresh_workspaces_widget(sid_app);
        celebrate(sid_app, sid_fx::SupernovaPalette::Celebrate);
        sid_app
            .toasts
            .push(Toast::success(format!("workspace '{name}' added")));
    } else if let Some(parent_str) = key.strip_prefix("workspaces.add_repo:") {
        let parent = PathBuf::from(parent_str);
        let _ = values; // path comes from the picker field
        let raw_path = match values
            .iter()
            .find(|(k, _)| k == "repo path")
            .map(|(_, v)| v)
        {
            Some(FieldValue::Picker(s) | FieldValue::Text(s)) => s.clone(),
            _ => String::new(),
        };
        if raw_path.is_empty() {
            return Err(anyhow::anyhow!("repo path is required"));
        }
        let path = PathBuf::from(&raw_path);
        let abs = std::fs::canonicalize(&path).unwrap_or(path);
        let name = abs
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("repo")
            .to_string();
        let w = Workspace {
            path: abs,
            name: name.clone(),
            kind: WorkspaceKind::Repo,
            manifest_hash: 0,
            last_seen: now_epoch(),
            parent: Some(parent),
        };
        sid_app
            .store
            .upsert_workspace(&w)
            .map_err(|e| anyhow::anyhow!("upsert workspace: {e}"))?;
        refresh_workspaces_widget(sid_app);
        celebrate(sid_app, sid_fx::SupernovaPalette::Celebrate);
        sid_app
            .toasts
            .push(Toast::success(format!("repo '{name}' added")));
    } else if let Some(target_str) = key.strip_prefix("workspaces.remove:") {
        let target = PathBuf::from(target_str);
        let confirm = choice_value(values, "confirm").unwrap_or_default();
        if confirm == "Yes, remove" {
            sid_app
                .store
                .remove_workspace(&target)
                .map_err(|e| anyhow::anyhow!("remove workspace: {e}"))?;
            refresh_workspaces_widget(sid_app);
            let name = target
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(target_str)
                .to_string();
            sid_app
                .toasts
                .push(Toast::success(format!("workspace '{name}' removed")));
        }
    } else if key == "ssh.new" {
        let alias = submit_ssh_new(sid_app, values)?;
        celebrate(sid_app, sid_fx::SupernovaPalette::Celebrate);
        sid_app
            .toasts
            .push(Toast::success(format!("host '{alias}' saved")));
    } else if let Some(alias) = key.strip_prefix("ssh.remove:") {
        submit_ssh_remove(sid_app, alias, values)?;
    } else if let Some(alias) = key.strip_prefix("ssh.edit:") {
        submit_ssh_edit(sid_app, alias, values)?;
    } else if let Some(alias) = key.strip_prefix("ssh.sftp_persist:") {
        submit_ssh_sftp_persist(sid_app, alias, values)?;
    } else if let Some(alias) = key.strip_prefix("ssh.setup_remote.identity:") {
        submit_ssh_setup_remote_step1(sid_app, alias, values)?;
    } else if let Some(rest) = key.strip_prefix("ssh.setup_remote.confirm:") {
        // rest is "<alias>:<identity_path>"
        if let Some((alias, identity)) = rest.split_once(':') {
            submit_ssh_setup_remote_step2(sid_app, alias, identity, values)?;
        }
    } else if key.starts_with("ssh.setup_remote.result:") {
        // Display-only; submit is a no-op (Esc closes it).
    } else if key == "ssh.gen_key.step1" {
        submit_ssh_gen_key_step1(sid_app, values)?;
    } else if let Some(algorithm) = key.strip_prefix("ssh.gen_key.step2:") {
        submit_ssh_gen_key_step2(sid_app, algorithm, values)?;
        celebrate(sid_app, sid_fx::SupernovaPalette::Celebrate);
    } else if let Some(rest) = key.strip_prefix("ssh.gen_key.step3:") {
        // rest is "<algorithm>:<output_path>"
        if let Some((algorithm, output_path)) = rest.split_once(':') {
            submit_ssh_gen_key_step3(sid_app, algorithm, output_path, values)?;
        }
    } else if key == "ssh.key_manager" {
        submit_ssh_key_manager(sid_app, values)?;
    } else if let Some(target) = key.strip_prefix("ssh.key_manager.confirm_delete:") {
        submit_ssh_key_manager_confirm_delete(target, values)?;
    } else if let Some(target) = key.strip_prefix("ssh.key_manager.confirm_regen:") {
        submit_ssh_key_manager_confirm_regen(target, values)?;
    } else if let Some(alias) = key.strip_prefix("ssh.debug:") {
        submit_ssh_debug(sid_app, alias, values)?;
    } else if key.starts_with("help:") {
        // Read-only help modal; submit is a no-op (Esc closes it).
    } else if key == "database.new" {
        let conn_id = submit_database_new(sid_app, values)?;
        celebrate(sid_app, sid_fx::SupernovaPalette::Celebrate);
        sid_app
            .toasts
            .push(Toast::success(format!("connection '{conn_id}' saved")));
    } else if let Some(conn_id) = key.strip_prefix("database.remove:") {
        submit_database_remove(sid_app, conn_id, values)?;
    } else if key == "system.pin_config" {
        let label = submit_system_pin_config(sid_app, values)?;
        celebrate(sid_app, sid_fx::SupernovaPalette::Celebrate);
        sid_app
            .toasts
            .push(Toast::success(format!("pinned '{label}'")));
    } else if let Some(path_str) = key.strip_prefix("system.remove_pin:") {
        submit_system_remove_pin(sid_app, path_str, values)?;
    } else if key == "system.quick_action.new" {
        let label = submit_system_quick_action_new(sid_app, values)?;
        celebrate(sid_app, sid_fx::SupernovaPalette::Celebrate);
        sid_app
            .toasts
            .push(Toast::success(format!("quick action '{label}' added")));
    } else if let Some(qa_id) = key.strip_prefix("system.remove_quick_action:") {
        submit_system_remove_quick_action(sid_app, qa_id, values)?;
    } else {
        tracing::debug!("unhandled modal submit id={key}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-tab submit handlers
// ---------------------------------------------------------------------------

/// Handle a successful submit of the `ssh.new` modal: validate inputs,
/// upsert the host into the store, refresh the SSH widget. Returns the alias
/// of the newly-added host so the caller can populate a context-rich toast.
fn submit_ssh_new(
    sid_app: &mut SidApp,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<String> {
    use sid_store::{SshHost, SshHostSource};
    let alias = string_value(values, "alias").unwrap_or_default();
    let host = string_value(values, "host").unwrap_or_default();
    let user = string_value(values, "user").unwrap_or_default();
    let port_str = string_value(values, "port").unwrap_or_default();
    let identity_file = string_value(values, "identity_file").filter(|s| !s.is_empty());
    let auth_kind = parse_auth_choice(choice_value(values, "auth").as_deref());
    if alias.is_empty() || host.is_empty() || user.is_empty() {
        return Err(anyhow::anyhow!("alias, host, and user are required"));
    }
    let port: u16 = port_str
        .parse()
        .map_err(|e| anyhow::anyhow!("port must be a u16 (got {port_str:?}): {e}"))?;
    let record = SshHost {
        alias: alias.clone(),
        host,
        port,
        user,
        identity_file,
        source: SshHostSource::Manual,
        last_connected: 0,
        command_history: Vec::new(),
        last_sftp_path: None,
        auth_kind,
    };
    sid_app
        .store
        .upsert_ssh_host(&record)
        .map_err(|e| anyhow::anyhow!("upsert ssh host: {e}"))?;
    refresh_ssh_widget(sid_app);
    Ok(alias)
}

/// Translate the modal's `auth` Choice value to a typed [`SshAuthKind`].
/// Unknown / missing values fall back to [`SshAuthKind::Agent`] — the
/// most permissive default that works on standard setups without
/// further user configuration.
fn parse_auth_choice(choice: Option<&str>) -> sid_store::SshAuthKind {
    use sid_store::SshAuthKind;
    match choice {
        Some("Key") => SshAuthKind::Key,
        Some("Password") => SshAuthKind::Password,
        _ => SshAuthKind::Agent,
    }
}

/// Handle a `ssh.remove:<alias>` submit. Confirms via the Choice field
/// before deleting.
fn submit_ssh_remove(
    sid_app: &mut SidApp,
    alias: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    if choice_value(values, "confirm").as_deref() != Some("Yes, remove") {
        return Ok(());
    }
    sid_app
        .store
        .remove_ssh_host(alias)
        .map_err(|e| anyhow::anyhow!("remove ssh host: {e}"))?;
    refresh_ssh_widget(sid_app);
    sid_app
        .toasts
        .push(Toast::success(format!("host '{alias}' removed")));
    Ok(())
}

/// Handle a successful submit of `ssh.edit:<alias>`: validate, update the
/// host record (preserves `last_sftp_path` and `command_history`), and
/// refresh the widget.
fn submit_ssh_edit(
    sid_app: &mut SidApp,
    alias_in_id: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    use sid_store::{SshHost, SshHostSource};
    let new_alias = string_value(values, "alias").unwrap_or_default();
    let host = string_value(values, "host").unwrap_or_default();
    let user = string_value(values, "user").unwrap_or_default();
    let port_str = string_value(values, "port").unwrap_or_default();
    let identity_file = string_value(values, "identity_file").filter(|s| !s.is_empty());
    let auth_kind = parse_auth_choice(choice_value(values, "auth").as_deref());
    if new_alias.is_empty() || host.is_empty() || user.is_empty() {
        return Err(anyhow::anyhow!("alias, host, and user are required"));
    }
    let port: u16 = port_str
        .parse()
        .map_err(|e| anyhow::anyhow!("port must be a u16 (got {port_str:?}): {e}"))?;
    // Preserve last_sftp_path / command_history / last_connected from the
    // existing record so the edit doesn't blow them away.
    let existing = sid_app
        .store
        .get_ssh_host(alias_in_id)
        .map_err(|e| anyhow::anyhow!("get ssh host: {e}"))?;
    let (last_connected, command_history, last_sftp_path) = match existing.as_ref() {
        Some(h) => (
            h.last_connected,
            h.command_history.clone(),
            h.last_sftp_path.clone(),
        ),
        None => (0, Vec::new(), None),
    };
    let record = SshHost {
        alias: new_alias.clone(),
        host,
        port,
        user,
        identity_file,
        source: SshHostSource::Manual,
        last_connected,
        command_history,
        last_sftp_path,
        auth_kind,
    };
    // If alias changed, remove the old key first so we don't leak a dupe.
    if new_alias != alias_in_id {
        sid_app
            .store
            .remove_ssh_host(alias_in_id)
            .map_err(|e| anyhow::anyhow!("remove old ssh host: {e}"))?;
    }
    sid_app
        .store
        .upsert_ssh_host(&record)
        .map_err(|e| anyhow::anyhow!("upsert ssh host: {e}"))?;
    refresh_ssh_widget(sid_app);
    sid_app
        .toasts
        .push(Toast::success(format!("host '{new_alias}' updated")));
    Ok(())
}

/// Handle `ssh.sftp_persist:<alias>`: write `last_sftp_path` onto the host.
fn submit_ssh_sftp_persist(
    sid_app: &mut SidApp,
    alias: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    let last_path = string_value(values, "last_path").unwrap_or_default();
    let existing = sid_app
        .store
        .get_ssh_host(alias)
        .map_err(|e| anyhow::anyhow!("get ssh host: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("no host with alias {alias} in store"))?;
    let mut record = existing;
    record.last_sftp_path = if last_path.is_empty() {
        None
    } else {
        Some(last_path)
    };
    sid_app
        .store
        .upsert_ssh_host(&record)
        .map_err(|e| anyhow::anyhow!("upsert ssh host: {e}"))?;
    refresh_ssh_widget(sid_app);
    sid_app
        .toasts
        .push(Toast::success(format!("SFTP path saved for '{alias}'")));
    Ok(())
}

/// Handle setup-remote-auth step 1: identity picked → push step 2.
fn submit_ssh_setup_remote_step1(
    sid_app: &mut SidApp,
    alias: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    let identity = choice_value(values, "identity_path").unwrap_or_default();
    if identity.starts_with('(') {
        return Err(anyhow::anyhow!("no identity selected"));
    }
    sid_app
        .modal_stack
        .push(ssh_setup_remote_step2_modal(alias, &identity));
    Ok(())
}

/// Handle setup-remote-auth step 2: confirm → spawn `ssh-copy-id` on the job
/// queue. The modal closes immediately; a toast reports the outcome later.
/// On success, the host's `identity_file` is persisted from the background
/// task itself (the store handle is `Arc<RedbStore>`, cheap to clone).
fn submit_ssh_setup_remote_step2(
    sid_app: &mut SidApp,
    alias: &str,
    identity: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    let proceed = choice_value(values, "proceed").unwrap_or_default();
    if proceed != "Yes, proceed" {
        return Ok(());
    }
    let alias_owned = alias.to_string();
    let identity_owned = identity.to_string();
    let store = Arc::clone(&sid_app.store);
    sid_app.toasts.push(Toast::info(format!(
        "ssh-copy-id: connecting to {alias}..."
    )));
    sid_app.jobs.spawn(async move {
        let outcome = tokio::task::spawn_blocking({
            let alias = alias_owned.clone();
            let identity = identity_owned.clone();
            move || run_ssh_copy_id(&alias, Some(&identity))
        })
        .await
        .unwrap_or_else(|e| format!("err: task join failed: {e}"));
        let label = "ssh-copy-id".to_string();
        if let Some(rest) = outcome.strip_prefix("ok:") {
            if let Ok(Some(mut existing)) = store.get_ssh_host(&alias_owned) {
                existing.identity_file = Some(identity_owned.clone());
                if let Err(e) = store.upsert_ssh_host(&existing) {
                    tracing::warn!(alias = %alias_owned, error = %e, "persist identity_file failed");
                }
            }
            tracing::info!(alias = %alias_owned, identity = %identity_owned, "ssh-copy-id ok");
            JobOutcome::Success {
                label,
                message: format!("copied key to {alias_owned}{}", trail_one(rest)),
            }
        } else {
            let trimmed = outcome
                .strip_prefix("err:")
                .unwrap_or(&outcome)
                .trim()
                .to_string();
            tracing::warn!(alias = %alias_owned, error = %trimmed, "ssh-copy-id failed");
            JobOutcome::Failure {
                label,
                message: format!("{alias_owned}: {}", first_nonempty_line(&trimmed)),
            }
        }
    });
    Ok(())
}

/// Capture `ssh-copy-id` output (best-effort; the binary may be missing).
/// Returns either `"ok: <stdout>"` or `"err: <stderr/stdout>"` so callers can
/// branch on the prefix. Runs synchronously and is meant to be invoked from
/// `tokio::task::spawn_blocking`.
fn run_ssh_copy_id(alias: &str, identity: Option<&str>) -> String {
    use std::process::Command;
    let mut cmd = Command::new("ssh-copy-id");
    if let Some(i) = identity {
        let pub_path = if i.ends_with(".pub") {
            i.to_string()
        } else {
            format!("{i}.pub")
        };
        cmd.arg("-i").arg(&pub_path);
    }
    cmd.arg(alias);
    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if out.status.success() {
                format!("ok: {stdout}")
            } else {
                format!("err: {stderr}\n{stdout}")
            }
        }
        Err(e) => format!("err: ssh-copy-id not on PATH: {e}"),
    }
}

/// Return the first non-empty trimmed line of `s`, or "(no output)".
fn first_nonempty_line(s: &str) -> String {
    s.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("(no output)")
        .to_string()
}

/// Best-effort trailing context for a successful command — picks the last
/// non-empty line of `s` and prefixes it with " — " if found, otherwise "".
fn trail_one(s: &str) -> String {
    let last = s.lines().map(|l| l.trim()).rfind(|l| !l.is_empty());
    match last {
        Some(line) if !line.is_empty() => format!(" — {line}"),
        _ => String::new(),
    }
}

/// Step 1 of the gen-key wizard. Validates and pushes step 2.
fn submit_ssh_gen_key_step1(
    sid_app: &mut SidApp,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    let algorithm = choice_value(values, "algorithm").unwrap_or_default();
    match algorithm.as_str() {
        "Ed25519" | "RSA-4096" | "ECDSA-256" => {}
        other => return Err(anyhow::anyhow!("unknown algorithm: {other}")),
    }
    sid_app
        .modal_stack
        .push(ssh_gen_key_step2_modal(&algorithm));
    Ok(())
}

/// Step 2 of the gen-key wizard. Runs `ssh-keygen`, then pushes step 3.
fn submit_ssh_gen_key_step2(
    sid_app: &mut SidApp,
    algorithm: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    use std::process::Command;
    let output_path = string_value(values, "output_path").unwrap_or_default();
    let passphrase = string_value(values, "passphrase").unwrap_or_default();
    let confirm = string_value(values, "confirm_passphrase").unwrap_or_default();
    let comment = string_value(values, "comment").unwrap_or_default();

    if output_path.is_empty() {
        return Err(anyhow::anyhow!("output_path is required"));
    }
    if passphrase != confirm {
        return Err(anyhow::anyhow!(
            "passphrase and confirm_passphrase do not match"
        ));
    }
    let out_path = PathBuf::from(&output_path);
    if out_path.exists() {
        return Err(anyhow::anyhow!(
            "output_path already exists: {output_path} (ssh-keygen would overwrite — refusing)"
        ));
    }
    let algo_flag = match algorithm {
        "Ed25519" => "ed25519",
        "RSA-4096" => "rsa",
        "ECDSA-256" => "ecdsa",
        other => return Err(anyhow::anyhow!("unknown algorithm: {other}")),
    };
    let mut cmd = Command::new("ssh-keygen");
    cmd.arg("-t").arg(algo_flag);
    if algorithm == "RSA-4096" {
        cmd.arg("-b").arg("4096");
    }
    if algorithm == "ECDSA-256" {
        cmd.arg("-b").arg("256");
    }
    cmd.arg("-f").arg(&output_path);
    cmd.arg("-N").arg(&passphrase);
    if !comment.is_empty() {
        cmd.arg("-C").arg(&comment);
    }
    let status = cmd
        .status()
        .map_err(|e| anyhow::anyhow!("ssh-keygen not on PATH: {e}"))?;
    if !status.success() {
        tracing::warn!(?status, "ssh-keygen exited with non-zero status");
        return Err(anyhow::anyhow!(
            "ssh-keygen exited with non-zero status: {status}"
        ));
    }
    tracing::info!(path = %output_path, "ssh-keygen completed successfully");
    let aliases: Vec<String> = sid_app
        .store
        .list_ssh_hosts()
        .map(|hs| hs.into_iter().map(|h| h.alias).collect())
        .unwrap_or_default();
    sid_app
        .modal_stack
        .push(ssh_gen_key_step3_modal(algorithm, &output_path, &aliases));
    Ok(())
}

/// Step 3 of the gen-key wizard: optionally copy the new key to a remote via
/// `ssh-copy-id`. Runs asynchronously through the job queue; the modal closes
/// immediately and a toast reports the outcome. On success the host's
/// `identity_file` is persisted from the background task.
fn submit_ssh_gen_key_step3(
    sid_app: &mut SidApp,
    _algorithm: &str,
    output_path: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    let target = choice_value(values, "target_host").unwrap_or_default();
    if target.is_empty() || target.starts_with('<') {
        return Ok(());
    }
    let target_owned = target.clone();
    let output_path_owned = output_path.to_string();
    let store = Arc::clone(&sid_app.store);
    sid_app.toasts.push(Toast::info(format!(
        "ssh-copy-id: connecting to {target}..."
    )));
    sid_app.jobs.spawn(async move {
        let result = tokio::task::spawn_blocking({
            let target = target_owned.clone();
            let key = output_path_owned.clone();
            move || run_ssh_copy_id(&target, Some(&key))
        })
        .await
        .unwrap_or_else(|e| format!("err: task join failed: {e}"));
        let label = "ssh-copy-id".to_string();
        if let Some(rest) = result.strip_prefix("ok:") {
            if let Ok(Some(mut existing)) = store.get_ssh_host(&target_owned) {
                existing.identity_file = Some(output_path_owned.clone());
                if let Err(e) = store.upsert_ssh_host(&existing) {
                    tracing::warn!(target = %target_owned, error = %e, "persist identity_file failed");
                }
            }
            tracing::info!(target = %target_owned, output_path = %output_path_owned, "gen_key step3 ok");
            JobOutcome::Success {
                label,
                message: format!("copied key to {target_owned}{}", trail_one(rest)),
            }
        } else {
            let trimmed = result
                .strip_prefix("err:")
                .unwrap_or(&result)
                .trim()
                .to_string();
            tracing::warn!(target = %target_owned, error = %trimmed, "gen_key step3 failed");
            JobOutcome::Failure {
                label,
                message: format!("{target_owned}: {}", first_nonempty_line(&trimmed)),
            }
        }
    });
    Ok(())
}

/// Handle the key manager modal's primary submit. Dispatches on the
/// `action` choice; Delete/Regenerate push a confirm modal.
fn submit_ssh_key_manager(
    sid_app: &mut SidApp,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    use sid_widgets::{Field, ModalSpec};
    let action = choice_value(values, "action").unwrap_or_default();
    let target = choice_value(values, "target").unwrap_or_default();
    if target.is_empty() || target == "(none)" {
        return Ok(());
    }
    match action.as_str() {
        "Show public key" => {
            let pub_path = format!("{target}.pub");
            match std::fs::read_to_string(&pub_path) {
                Ok(contents) => tracing::info!(path = %pub_path, %contents, "public key"),
                Err(e) => tracing::warn!(path = %pub_path, error = %e, "read pub key failed"),
            }
        }
        "Regenerate" => {
            sid_app.modal_stack.push(
                ModalSpec::new(
                    format!("ssh.key_manager.confirm_regen:{target}"),
                    format!("Regenerate {target}?"),
                    vec![Field::Choice {
                        label: "confirm".into(),
                        options: vec!["No, cancel".into(), "Yes, regenerate".into()],
                        selected: 0,
                    }],
                )
                .with_help(
                    "Deletes the existing key + .pub then runs ssh-keygen with the same algorithm.",
                ),
            );
        }
        "Delete" => {
            sid_app.modal_stack.push(
                ModalSpec::new(
                    format!("ssh.key_manager.confirm_delete:{target}"),
                    format!("Delete {target}?"),
                    vec![Field::Choice {
                        label: "confirm".into(),
                        options: vec!["No, cancel".into(), "Yes, delete".into()],
                        selected: 0,
                    }],
                )
                .with_help("Deletes the private key and its .pub sibling. This cannot be undone."),
            );
        }
        _ => {}
    }
    Ok(())
}

/// Handle the key manager Delete confirm.
fn submit_ssh_key_manager_confirm_delete(
    target: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    if choice_value(values, "confirm").as_deref() != Some("Yes, delete") {
        return Ok(());
    }
    let priv_path = PathBuf::from(target);
    let pub_path = PathBuf::from(format!("{target}.pub"));
    if let Err(e) = std::fs::remove_file(&priv_path) {
        tracing::warn!(path = %priv_path.display(), error = %e, "delete private key failed");
    }
    if let Err(e) = std::fs::remove_file(&pub_path) {
        tracing::warn!(path = %pub_path.display(), error = %e, "delete public key failed");
    }
    Ok(())
}

/// Handle the key manager Regenerate confirm — best effort: delete + run
/// `ssh-keygen` with `-t ed25519` (the user picks via the gen-key wizard for
/// finer control).
fn submit_ssh_key_manager_confirm_regen(
    target: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    use std::process::Command;
    if choice_value(values, "confirm").as_deref() != Some("Yes, regenerate") {
        return Ok(());
    }
    let _ = std::fs::remove_file(target);
    let _ = std::fs::remove_file(format!("{target}.pub"));
    let out = Command::new("ssh-keygen")
        .arg("-t")
        .arg("ed25519")
        .arg("-f")
        .arg(target)
        .arg("-N")
        .arg("")
        .output();
    match out {
        Ok(o) if o.status.success() => tracing::info!(target, "regenerate ok"),
        Ok(o) => tracing::warn!(target, status = ?o.status, "regenerate failed"),
        Err(e) => tracing::warn!(target, error = %e, "ssh-keygen not on PATH"),
    }
    Ok(())
}

/// Handle the SSH debug modal.
///
/// Each subprocess (ssh-keygen, ssh-add, ssh -vv) is dispatched to the job
/// queue via `tokio::task::spawn_blocking` and a `Toast::info("running ...")`
/// is pushed immediately. The final `JobOutcome` is converted to a Toast on
/// completion. Tracing log lines remain — useful for post-mortem analysis.
fn submit_ssh_debug(
    sid_app: &mut SidApp,
    alias: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    let action = choice_value(values, "action").unwrap_or_default();
    match action.as_str() {
        "Show known_hosts entry" => {
            sid_app
                .toasts
                .push(Toast::info(format!("ssh-keygen -F {alias}...")));
            let alias_for_label = alias.to_string();
            let alias_for_cmd = alias.to_string();
            sid_app.jobs.spawn(async move {
                run_ssh_debug_cmd("ssh-keygen -F", alias_for_label, move || {
                    std::process::Command::new("ssh-keygen")
                        .arg("-F")
                        .arg(&alias_for_cmd)
                        .output()
                })
                .await
            });
        }
        "Remove known_hosts entry" => {
            sid_app
                .toasts
                .push(Toast::info(format!("ssh-keygen -R {alias}...")));
            let alias_for_label = alias.to_string();
            let alias_for_cmd = alias.to_string();
            sid_app.jobs.spawn(async move {
                run_ssh_debug_cmd("ssh-keygen -R", alias_for_label, move || {
                    std::process::Command::new("ssh-keygen")
                        .arg("-R")
                        .arg(&alias_for_cmd)
                        .output()
                })
                .await
            });
        }
        "Show identity diagnostics" => {
            sid_app
                .toasts
                .push(Toast::info("ssh-add -l...".to_string()));
            let alias_for_label = alias.to_string();
            sid_app.jobs.spawn(async move {
                run_ssh_debug_cmd("ssh-add -l", alias_for_label, || {
                    std::process::Command::new("ssh-add").arg("-l").output()
                })
                .await
            });
        }
        "Test connection (ssh -vv)" => {
            sid_app
                .toasts
                .push(Toast::info(format!("ssh -vv {alias}...")));
            let alias_for_label = alias.to_string();
            let alias_for_cmd = alias.to_string();
            sid_app.jobs.spawn(async move {
                run_ssh_debug_cmd("ssh -vv", alias_for_label, move || {
                    std::process::Command::new("ssh")
                        .arg("-vv")
                        .arg("-o")
                        .arg("BatchMode=yes")
                        .arg(&alias_for_cmd)
                        .arg("exit")
                        .output()
                })
                .await
            });
        }
        "Clear cached agent identities (ssh-add -D)" => {
            sid_app
                .toasts
                .push(Toast::info("ssh-add -D...".to_string()));
            let alias_for_label = alias.to_string();
            sid_app.jobs.spawn(async move {
                run_ssh_debug_cmd("ssh-add -D", alias_for_label, || {
                    std::process::Command::new("ssh-add").arg("-D").output()
                })
                .await
            });
        }
        other => {
            tracing::debug!(action = other, "unhandled ssh debug action");
        }
    }
    Ok(())
}

/// Run an ssh-debug subprocess on a blocking pool, log the captured
/// stdout/stderr, and return a `JobOutcome` describing the result.
async fn run_ssh_debug_cmd<F>(label: &str, alias: String, run: F) -> JobOutcome
where
    F: FnOnce() -> std::io::Result<std::process::Output> + Send + 'static,
{
    let label = label.to_string();
    let outcome = tokio::task::spawn_blocking(run).await;
    match outcome {
        Ok(Ok(o)) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            tracing::info!(
                action = %label,
                alias = %alias,
                status = ?o.status,
                stdout = %truncate_lines(&stdout, 50),
                stderr = %truncate_lines(&stderr, 50),
                "ssh debug action"
            );
            if o.status.success() {
                JobOutcome::Success {
                    label,
                    message: first_nonempty_line(&stdout),
                }
            } else {
                JobOutcome::Failure {
                    label,
                    message: first_nonempty_line(&stderr),
                }
            }
        }
        Ok(Err(e)) => {
            tracing::warn!(action = %label, alias = %alias, error = %e, "subprocess launch failed");
            JobOutcome::Failure {
                label,
                message: format!("launch failed: {e}"),
            }
        }
        Err(e) => {
            tracing::warn!(action = %label, alias = %alias, error = %e, "task join failed");
            JobOutcome::Failure {
                label,
                message: format!("task join failed: {e}"),
            }
        }
    }
}

/// Handle a `database.new` submit: validate, persist the connection, and
/// (for Postgres) write the password to the secret store. Returns the
/// new connection id so the caller can populate a toast.
fn submit_database_new(
    sid_app: &mut SidApp,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<String> {
    use sid_core::adapters::db_client::DbKind;
    use sid_core::adapters::secrets::SecretId;
    use sid_store::{DbConnection, now_epoch};
    let id = string_value(values, "id").unwrap_or_default();
    let name = string_value(values, "name").unwrap_or_default();
    let kind_str = choice_value(values, "kind").unwrap_or_default();
    let dsn = string_value(values, "dsn").unwrap_or_default();
    let password = string_value(values, "password").unwrap_or_default();
    if id.is_empty() || name.is_empty() || dsn.is_empty() {
        return Err(anyhow::anyhow!("id, name, and dsn are required"));
    }
    let kind = match kind_str.as_str() {
        "Postgres" => DbKind::Postgres,
        "SQLite" => DbKind::Sqlite,
        other => return Err(anyhow::anyhow!("unknown db kind: {other}")),
    };
    let secret_ref = if kind == DbKind::Postgres && !password.is_empty() {
        let sid = SecretId::new(format!("db.connection.{id}.password"));
        sid_app
            .secrets
            .put(&sid, password.as_bytes())
            .map_err(|e| anyhow::anyhow!("write db password: {e}"))?;
        Some(sid)
    } else {
        None
    };
    let conn = DbConnection {
        id: id.clone(),
        kind,
        name,
        dsn,
        secret_ref,
        created_at: now_epoch(),
    };
    sid_app
        .store
        .upsert_db_connection(&conn)
        .map_err(|e| anyhow::anyhow!("upsert db connection: {e}"))?;
    refresh_database_widget(sid_app);
    Ok(id)
}

/// Handle a `database.remove:<id>` submit. On confirm, removes the
/// connection record AND best-effort deletes any stored password.
fn submit_database_remove(
    sid_app: &mut SidApp,
    conn_id: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    use sid_core::adapters::secrets::SecretId;
    if choice_value(values, "confirm").as_deref() != Some("Yes, remove") {
        return Ok(());
    }
    sid_app
        .store
        .remove_db_connection(conn_id)
        .map_err(|e| anyhow::anyhow!("remove db connection: {e}"))?;
    let secret = SecretId::new(format!("db.connection.{conn_id}.password"));
    if let Err(e) = sid_app.secrets.delete(&secret) {
        tracing::warn!("failed to delete db connection secret: {e}");
    }
    refresh_database_widget(sid_app);
    sid_app
        .toasts
        .push(Toast::success(format!("connection '{conn_id}' removed")));
    Ok(())
}

/// Handle a `system.pin_config` submit: validate the path, default the label
/// to the basename, persist. Returns the resolved label so the caller can
/// surface it in a toast.
fn submit_system_pin_config(
    sid_app: &mut SidApp,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<String> {
    use sid_store::{PinnedConfig, now_epoch};
    let path_str = string_value(values, "path").unwrap_or_default();
    let label = string_value(values, "label").unwrap_or_default();
    if path_str.is_empty() {
        return Err(anyhow::anyhow!("path is required"));
    }
    let path = PathBuf::from(&path_str);
    let abs = std::fs::canonicalize(&path).unwrap_or(path);
    let label = if label.is_empty() {
        abs.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("pin")
            .to_string()
    } else {
        label
    };
    let pin = PinnedConfig {
        path: abs,
        label: label.clone(),
        opener_cmd: None,
        created_at: now_epoch(),
    };
    sid_app
        .store
        .upsert_pinned_config(&pin)
        .map_err(|e| anyhow::anyhow!("upsert pinned config: {e}"))?;
    refresh_system_widget(sid_app);
    Ok(label)
}

/// Handle a `system.remove_pin:<path>` submit.
fn submit_system_remove_pin(
    sid_app: &mut SidApp,
    path_str: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    if choice_value(values, "confirm").as_deref() != Some("Yes, remove") {
        return Ok(());
    }
    let path = PathBuf::from(path_str);
    sid_app
        .store
        .remove_pinned_config(&path)
        .map_err(|e| anyhow::anyhow!("remove pinned config: {e}"))?;
    refresh_system_widget(sid_app);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path_str)
        .to_string();
    sid_app
        .toasts
        .push(Toast::success(format!("unpinned '{name}'")));
    Ok(())
}

/// Handle a `system.quick_action.new` submit. Stores the action and, if it
/// is globally scoped, refreshes the palette via
/// [`rehydrate_global_quick_actions`]. Returns the label so the caller can
/// reference it in a toast.
fn submit_system_quick_action_new(
    sid_app: &mut SidApp,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<String> {
    use sid_store::{QuickAction, QuickActionScope};
    let id = string_value(values, "id").unwrap_or_default();
    let label = string_value(values, "label").unwrap_or_default();
    let cmd = string_value(values, "command").unwrap_or_default();
    let scope_str = choice_value(values, "scope").unwrap_or_default();
    let keybind = string_value(values, "keybind").filter(|s| !s.is_empty());
    if id.is_empty() || label.is_empty() || cmd.is_empty() {
        return Err(anyhow::anyhow!("id, label, and command are required"));
    }
    let scope = match scope_str.as_str() {
        "Workspace" => QuickActionScope::Workspace,
        _ => QuickActionScope::Global,
    };
    let qa = QuickAction {
        id,
        label: label.clone(),
        cmd,
        keybind,
        scope,
    };
    sid_app
        .store
        .upsert_quick_action(&qa)
        .map_err(|e| anyhow::anyhow!("upsert quick action: {e}"))?;
    refresh_system_widget(sid_app);
    rehydrate_palette_quick_actions(sid_app);
    Ok(label)
}

/// Handle a `system.remove_quick_action:<id>` submit.
fn submit_system_remove_quick_action(
    sid_app: &mut SidApp,
    qa_id: &str,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<()> {
    if choice_value(values, "confirm").as_deref() != Some("Yes, remove") {
        return Ok(());
    }
    sid_app
        .store
        .remove_quick_action(qa_id)
        .map_err(|e| anyhow::anyhow!("remove quick action: {e}"))?;
    refresh_system_widget(sid_app);
    rehydrate_palette_quick_actions(sid_app);
    sid_app
        .toasts
        .push(Toast::success(format!("quick action '{qa_id}' removed")));
    Ok(())
}

/// Replace the globally-scoped quick-action entries in the App's action
/// registry with the current store contents. Errors are logged but do not
/// propagate — the palette is best-effort, not authoritative.
fn rehydrate_palette_quick_actions(sid_app: &mut SidApp) {
    if let Err(e) = rehydrate_global_quick_actions(&*sid_app.store, sid_app.app.actions_mut()) {
        tracing::warn!("rehydrate quick actions: {e}");
    }
}

fn string_value(values: &[(String, sid_widgets::FieldValue)], label: &str) -> Option<String> {
    use sid_widgets::FieldValue;
    values
        .iter()
        .find(|(k, _)| k == label)
        .and_then(|(_, v)| match v {
            FieldValue::Text(s) | FieldValue::Picker(s) | FieldValue::Password(s) => {
                Some(s.clone())
            }
            _ => None,
        })
}

fn choice_value(values: &[(String, sid_widgets::FieldValue)], label: &str) -> Option<String> {
    use sid_widgets::FieldValue;
    values
        .iter()
        .find(|(k, _)| k == label)
        .and_then(|(_, v)| match v {
            FieldValue::Choice(s) => Some(s.clone()),
            _ => None,
        })
}

/// Reload the WorkspacesWidget's state from `store.list_workspaces()`.
///
/// The widget already exposes `state_mut()`; we replace the inner
/// `WorkspacesState` wholesale. This loses transient sub-view state (focused
/// commit, etc.) but that's acceptable after a CRUD — the user just changed
/// the list.
fn refresh_workspaces_widget(sid_app: &mut SidApp) {
    let ws = match sid_app.store.list_workspaces() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("list_workspaces after modal submit failed: {e}");
            return;
        }
    };
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "workspaces" {
            if let Some(w) = t.layout.iter_widgets_mut().next() {
                let any_ref = w as &mut dyn std::any::Any;
                if let Some(ww) = any_ref.downcast_mut::<WorkspacesWidget>() {
                    *ww.state_mut() = sid_widgets::workspaces::WorkspacesState::new(ws);
                }
            }
            break;
        }
    }
}

/// Reload the SshWidget's host list from `store.list_ssh_hosts()`. Preserves
/// the rest of the widget's transient state (connection phase, SFTP panel,
/// per-host history).
fn refresh_ssh_widget(sid_app: &mut SidApp) {
    let hosts = match sid_app.store.list_ssh_hosts() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("list_ssh_hosts after modal submit failed: {e}");
            return;
        }
    };
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "ssh" {
            if let Some(w) = t.layout.iter_widgets_mut().next() {
                let any_ref = w as &mut dyn std::any::Any;
                if let Some(ww) = any_ref.downcast_mut::<SshWidget>() {
                    ww.state_mut().set_store_hosts(hosts);
                }
            }
            break;
        }
    }
}

/// Reload the DatabaseWidget's connection list from
/// `store.list_db_connections()`. Other state (active client, results,
/// history) is left intact.
fn refresh_database_widget(sid_app: &mut SidApp) {
    let conns = match sid_app.store.list_db_connections() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("list_db_connections after modal submit failed: {e}");
            return;
        }
    };
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "database" {
            if let Some(w) = t.layout.iter_widgets_mut().next() {
                let any_ref = w as &mut dyn std::any::Any;
                if let Some(ww) = any_ref.downcast_mut::<DatabaseWidget>() {
                    ww.state_mut().set_connections(conns);
                }
            }
            break;
        }
    }
}

/// Reload the SystemWidget's pinned configs and quick actions from the
/// store. Services pane is read live from systemd; nothing to reload.
fn refresh_system_widget(sid_app: &mut SidApp) {
    let pins = match sid_app.store.list_pinned_configs() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("list_pinned_configs after modal submit failed: {e}");
            return;
        }
    };
    let qas = match sid_app.store.list_quick_actions() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("list_quick_actions after modal submit failed: {e}");
            return;
        }
    };
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "system" {
            if let Some(w) = t.layout.iter_widgets_mut().next() {
                let any_ref = w as &mut dyn std::any::Any;
                if let Some(ww) = any_ref.downcast_mut::<SystemWidget>() {
                    ww.pinned_configs_mut().replace_items(pins);
                    ww.quick_actions_mut().replace_items(qas);
                }
            }
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Plan 6 — System tab integration
// ---------------------------------------------------------------------------

/// Register every global [`sid_store::QuickAction`] from `store` into
/// `registry`. Workspace-scoped actions are ignored (the Workspaces tab owns
/// that surface).
///
/// Returns the number of actions added.
pub fn hydrate_quick_actions_into_registry(
    store: &dyn Store,
    registry: &mut ActionRegistry,
) -> anyhow::Result<usize> {
    use sid_store::QuickActionScope;
    let actions = store.list_quick_actions()?;
    let mut n = 0;
    for qa in actions {
        if !matches!(qa.scope, QuickActionScope::Global) {
            continue;
        }
        let mut action = Action::new(qa.id.clone(), qa.label.clone());
        if let Some(kb) = qa.keybind.clone() {
            action.keybind_hint = Some(kb);
        }
        registry.register(action);
        n += 1;
    }
    Ok(n)
}

/// Clear all globally-scoped quick-actions from `registry` (identified by the
/// `qa-` id prefix) and re-add from `store`. Called after any QuickAction CRUD
/// in the System / Settings widgets.
///
/// The widget-side event wiring that calls this is added in Plan 6 Task 24
/// alongside the System widget render harness; until then this helper is
/// exercised only by unit tests in this module and CLI subcommand handlers.
#[allow(dead_code)]
pub fn rehydrate_global_quick_actions(
    store: &dyn Store,
    registry: &mut ActionRegistry,
) -> anyhow::Result<usize> {
    registry.unregister_with_prefix("qa-");
    hydrate_quick_actions_into_registry(store, registry)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use sid_core::tab::TabId;
    use sid_store::{OpenStore, RedbStore, Store};
    use tempfile::tempdir;

    use super::*;

    // ---- build_app ----

    /// `build_app` creates a TabManager with exactly 6 tabs in the correct order.
    #[test]
    fn build_app_has_six_tabs_in_order() {
        let app = build_app(None, vec![]);
        let ids: Vec<&str> = app.tabs().tabs().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            &[
                "workspaces",
                "ssh",
                "database",
                "network",
                "system",
                "settings"
            ]
        );
    }

    /// `build_app` defaults to the first tab (workspaces).
    #[test]
    fn build_app_defaults_to_workspaces() {
        let app = build_app(None, vec![]);
        assert_eq!(app.tabs().active().id.as_str(), "workspaces");
    }

    /// `build_app` with a valid start_tab switches to that tab.
    #[test]
    fn build_app_start_tab_switches() {
        let app = build_app(Some("settings"), vec![]);
        assert_eq!(app.tabs().active().id.as_str(), "settings");
    }

    /// `build_app` with an unknown start_tab falls back to the first tab.
    #[test]
    fn build_app_unknown_start_tab_falls_back() {
        let app = build_app(Some("does-not-exist"), vec![]);
        // switch_to returns false but doesn't panic; active stays at 0.
        assert_eq!(app.tabs().active_index(), 0);
    }

    // ---- pretty_label ----

    /// Known action ids map to friendly labels.
    #[test]
    fn pretty_label_known_actions() {
        assert_eq!(pretty_label("app.quit"), "Quit");
        assert_eq!(pretty_label("palette.open"), "Open command palette");
        assert_eq!(pretty_label("tabs.next"), "Next tab");
        assert_eq!(pretty_label("tabs.prev"), "Previous tab");
        assert_eq!(pretty_label("app.open_settings"), "Open Settings");
        assert_eq!(pretty_label("tab.detach"), "Detach tab (Plan 8)");
        assert_eq!(pretty_label("tab.attach"), "Attach widget (Plan 8)");
        assert_eq!(pretty_label("tab.reload"), "Reload tab data");
    }

    /// Unknown action ids are returned as-is.
    #[test]
    fn pretty_label_unknown_returns_action_id() {
        assert_eq!(pretty_label("unknown.action"), "unknown.action");
        assert_eq!(pretty_label(""), "");
        assert_eq!(
            pretty_label("some.deeply.nested.action.id"),
            "some.deeply.nested.action.id"
        );
    }

    // ---- centered ----

    /// `centered(area, 100, 100)` returns the original area.
    #[test]
    fn centered_100pct_returns_original() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(centered(area, 100, 100), area);
    }

    /// `centered(area, 0, 0)` returns a zero-size rect.
    #[test]
    fn centered_0pct_returns_zero_size() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let z = centered(area, 0, 0);
        assert_eq!(z.width, 0);
        assert_eq!(z.height, 0);
    }

    /// `centered` on a normal area returns something smaller than the area.
    #[test]
    fn centered_normal_is_smaller() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let c = centered(area, 60, 40);
        assert!(c.width < area.width);
        assert!(c.height < area.height);
        // The rect must fit inside area.
        assert!(c.x >= area.x);
        assert!(c.y >= area.y);
        assert!(c.x + c.width <= area.x + area.width);
        assert!(c.y + c.height <= area.y + area.height);
    }

    /// A small area with large pct still returns the area (not a zero-size rect).
    #[test]
    fn centered_small_area_large_pct_returns_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 3,
        };
        let c = centered(area, 100, 100);
        assert_eq!(c, area);
    }

    /// `centered` with a 1×1 area and 50% returns a zero-size rect.
    #[test]
    fn centered_1x1_50pct() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let c = centered(area, 50, 50);
        // 1 * 50 / 100 = 0; so width and height are 0
        assert_eq!(c.width, 0);
        assert_eq!(c.height, 0);
    }

    // ---- db_path ----

    /// `db_path(Some(p))` returns `p` unchanged.
    #[test]
    fn db_path_override_returned_unchanged() {
        let p = PathBuf::from("/tmp/explicit.redb");
        assert_eq!(db_path(Some(p.clone())), p);
    }

    /// `db_path(None)` returns an XDG-derived path containing "sid".
    #[test]
    fn db_path_none_contains_sid() {
        let p = db_path(None);
        assert!(
            p.to_string_lossy().contains("sid"),
            "XDG path should contain 'sid': {p:?}"
        );
    }

    /// `db_path(None)` returns a `.redb` path.
    #[test]
    fn db_path_none_ends_with_redb() {
        let p = db_path(None);
        assert!(
            p.to_string_lossy().ends_with(".redb"),
            "path should end with .redb: {p:?}"
        );
    }

    // ---- save_active_tab ----

    /// `save_active_tab` persists the active tab and it round-trips through the store.
    #[test]
    fn save_active_tab_round_trips() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app = build_app(Some("ssh"), vec![]);

        save_active_tab(&store, "sess-1", &app).unwrap();

        let loaded = store.current_session().unwrap().unwrap();
        assert_eq!(loaded.id, "sess-1");
        assert_eq!(loaded.active_tab.unwrap().as_str(), "ssh");
    }

    /// `save_active_tab` records all 6 open tabs.
    #[test]
    fn save_active_tab_records_open_tabs() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app = build_app(None, vec![]);

        save_active_tab(&store, "sess-2", &app).unwrap();

        let loaded = store.current_session().unwrap().unwrap();
        assert_eq!(loaded.open_tabs.len(), 6);
    }

    /// `save_active_tab` called twice overwrites the first record (upsert semantics).
    #[test]
    fn save_active_tab_upsert_semantics() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app1 = build_app(Some("ssh"), vec![]);
        let app2 = build_app(Some("database"), vec![]);

        save_active_tab(&store, "sess-3", &app1).unwrap();
        save_active_tab(&store, "sess-3", &app2).unwrap();

        let loaded = store.current_session().unwrap().unwrap();
        assert_eq!(loaded.active_tab.unwrap().as_str(), "database");
    }

    /// Adversarial: `save_active_tab` with different session IDs creates distinct records.
    #[test]
    fn save_active_tab_distinct_sessions() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app = build_app(None, vec![]);

        save_active_tab(&store, "sess-A", &app).unwrap();
        save_active_tab(&store, "sess-B", &app).unwrap();

        // current_session should point to the last upserted one.
        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"sess-A"), "should contain sess-A");
        assert!(ids.contains(&"sess-B"), "should contain sess-B");
    }

    // ---- build_app additional: all 6 tabs have titles ----

    #[test]
    fn build_app_all_tabs_have_titles() {
        let app = build_app(None, vec![]);
        let expected_titles = [
            "Workspaces",
            "SSH",
            "Database",
            "Network",
            "System",
            "Settings",
        ];
        for (tab, expected) in app.tabs().tabs().iter().zip(expected_titles.iter()) {
            assert_eq!(tab.title, *expected);
        }
    }

    /// `build_app` registers 14 actions (8 named + 6 jump).
    #[test]
    fn build_app_registers_expected_actions() {
        let app = build_app(None, vec![]);
        // 8 named + 6 jump actions
        let all: Vec<_> = app.actions().all().collect();
        assert_eq!(all.len(), 14, "expected 14 actions, got {}", all.len());
    }

    /// start_tab with "workspaces" ID stays at index 0.
    #[test]
    fn build_app_start_tab_workspaces_is_index_0() {
        let app = build_app(Some("workspaces"), vec![]);
        assert_eq!(app.tabs().active_index(), 0);
    }

    /// switch_to with each valid tab id works.
    #[test]
    fn build_app_can_switch_to_all_tabs() {
        let expected = [
            ("workspaces", 0usize),
            ("ssh", 1),
            ("database", 2),
            ("network", 3),
            ("system", 4),
            ("settings", 5),
        ];
        for (id, idx) in expected {
            let app = build_app(Some(id), vec![]);
            assert_eq!(app.tabs().active_index(), idx, "for tab id={id}");
        }
    }

    // ---- db_path: empty override doesn't fail ----

    #[test]
    fn db_path_empty_pathbuf_is_returned_as_is() {
        let p = PathBuf::from("");
        let result = db_path(Some(p.clone()));
        assert_eq!(result, p);
    }

    // ---- centered: non-zero origin ----

    #[test]
    fn centered_handles_non_zero_origin() {
        let area = Rect {
            x: 10,
            y: 5,
            width: 80,
            height: 40,
        };
        let c = centered(area, 50, 50);
        // Result must be within the area bounds.
        assert!(c.x >= area.x);
        assert!(c.y >= area.y);
        assert!(c.x + c.width <= area.x + area.width);
        assert!(c.y + c.height <= area.y + area.height);
    }

    // ---- centered: TabId round-trip ----

    #[test]
    fn tab_id_round_trips_through_save() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app = build_app(Some("network"), vec![]);

        save_active_tab(&store, "sess-net", &app).unwrap();
        let loaded = store.current_session().unwrap().unwrap();
        assert_eq!(
            loaded.active_tab.as_ref().map(TabId::as_str),
            Some("network")
        );
    }

    // ---- build_app adversarial: whitespace / edge-case start_tab values ----

    /// `build_app` with a start_tab containing only whitespace falls back to
    /// the default tab (no match for " " as a tab id).
    #[test]
    fn build_app_start_tab_whitespace_falls_back_to_default() {
        let app = build_app(Some("   "), vec![]);
        assert_eq!(
            app.tabs().active_index(),
            0,
            "whitespace should not match any tab"
        );
    }

    /// `build_app` with a start_tab containing a newline character falls back
    /// to the default tab — tab ids never contain newlines.
    #[test]
    fn build_app_start_tab_with_newline_falls_back_to_default() {
        let app = build_app(Some("settings\n"), vec![]);
        // "settings\n" is not a valid tab id; active index stays at 0.
        assert_eq!(
            app.tabs().active_index(),
            0,
            "newline-suffixed id should not match"
        );
    }

    /// `build_app` with a very long start_tab string does not panic.
    #[test]
    fn build_app_start_tab_very_long_does_not_panic() {
        let long = "x".repeat(100_000);
        let app = build_app(Some(&long), vec![]);
        // No known tab id matches; falls back to default.
        assert_eq!(app.tabs().active_index(), 0);
    }

    /// `build_app` with a start_tab that contains a dot does not panic and
    /// falls back to the default (dots are not part of any tab id).
    #[test]
    fn build_app_start_tab_with_dot_falls_back() {
        let app = build_app(Some("settings.extra"), vec![]);
        assert_eq!(app.tabs().active_index(), 0);
    }

    // ---- pretty_label edge cases ----

    /// `pretty_label` on an empty string returns an empty string (no panic).
    #[test]
    fn pretty_label_empty_string_returns_empty() {
        assert_eq!(pretty_label(""), "");
    }

    /// `pretty_label` on an action id that contains a dot in an unexpected
    /// position is returned unchanged.
    #[test]
    fn pretty_label_dot_in_id_returned_unchanged() {
        assert_eq!(pretty_label("a.b.c.d.e"), "a.b.c.d.e");
        assert_eq!(pretty_label(".leading.dot"), ".leading.dot");
        assert_eq!(pretty_label("trailing.dot."), "trailing.dot.");
    }

    /// `pretty_label` on a string containing a newline returns it unchanged.
    #[test]
    fn pretty_label_newline_returned_unchanged() {
        let s = "app\nquit";
        assert_eq!(
            pretty_label(s),
            s,
            "newline in action_id returned unchanged"
        );
    }

    /// `pretty_label` on a string containing unicode is returned unchanged
    /// (no known mapping).
    #[test]
    fn pretty_label_unicode_returned_unchanged() {
        let s = "app.日本語";
        assert_eq!(pretty_label(s), s);
        let s2 = "😀.action";
        assert_eq!(pretty_label(s2), s2);
    }

    // ---- centered adversarial: zero-dimension areas ----

    /// `centered` with 0-width area returns a zero-width rect (no overflow).
    #[test]
    fn centered_zero_width_area_does_not_panic() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 24,
        };
        let c = centered(area, 60, 60);
        // 0 * 60 / 100 = 0 width; must not overflow or panic.
        assert_eq!(c.width, 0);
    }

    /// `centered` with 0-height area returns a zero-height rect.
    #[test]
    fn centered_zero_height_area_does_not_panic() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 0,
        };
        let c = centered(area, 60, 60);
        assert_eq!(c.height, 0);
    }

    /// `centered` with both dimensions 0 returns a zero-size rect at the origin.
    #[test]
    fn centered_zero_both_dimensions_does_not_panic() {
        let area = Rect {
            x: 5,
            y: 3,
            width: 0,
            height: 0,
        };
        let c = centered(area, 50, 50);
        assert_eq!(c.width, 0);
        assert_eq!(c.height, 0);
    }

    /// `centered` with pct_w = 0 produces a zero-width result (even on large area).
    #[test]
    fn centered_zero_pct_w_produces_zero_width() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        };
        let c = centered(area, 0, 50);
        assert_eq!(c.width, 0, "0% width should yield width=0");
        // Height should be non-zero.
        assert!(c.height > 0, "height should be > 0 for 50%");
    }

    /// `centered` with pct_h = 0 produces a zero-height result.
    #[test]
    fn centered_zero_pct_h_produces_zero_height() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        };
        let c = centered(area, 50, 0);
        assert_eq!(c.height, 0, "0% height should yield height=0");
        assert!(c.width > 0, "width should be > 0 for 50%");
    }

    /// `centered` with pct_w = 200 (> 100) is clamped to 100; the guard
    /// returns the original area when the computed rect equals or exceeds it.
    #[test]
    fn centered_oversized_pct_returns_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        // 200% is clamped to 100% internally via `pct_w.min(100)`.
        let c = centered(area, 200, 200);
        assert_eq!(
            c, area,
            "200% should be clamped to 100% and return the area"
        );
    }

    /// `centered` where only one dimension is > 100% still uses the clamped
    /// value and may or may not return the full area depending on the other
    /// dimension.
    #[test]
    fn centered_partial_oversized_pct_clamped() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        // 200% width → clamped to 100 → w = 100 = area.width.
        // 50% height → h = 25 < area.height.
        // Guard: w >= area.width AND h >= area.height → false (h=25 < 50).
        // So we get a partially-centred rect.
        let c = centered(area, 200, 50);
        assert_eq!(
            c.width, area.width,
            "width should equal area.width at 200% clamped"
        );
        assert!(
            c.height < area.height,
            "height should be < area.height at 50%"
        );
        // Must still fit inside area.
        assert!(c.x + c.width <= area.x + area.width);
        assert!(c.y + c.height <= area.y + area.height);
    }

    // ---- draw: TestBackend smoke ----

    /// `draw` renders without panicking on a normal-sized terminal.
    #[test]
    fn draw_does_not_panic_on_normal_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let sid_app = build_test_sid_app(None);
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
    }

    /// `draw` renders without panicking on a very small (1×1) terminal.
    #[test]
    fn draw_does_not_panic_on_tiny_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let sid_app = build_test_sid_app(None);
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
    }

    /// `draw` renders without panicking when the terminal is smaller than the
    /// tab bar (height = 2, which is less than the 3-row bar height).
    #[test]
    fn draw_does_not_panic_when_shorter_than_bar() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let sid_app = build_test_sid_app(None);
        // Height 2 < bar height 3; body_rect will have saturating_sub(3) = 0 height.
        let backend = TestBackend::new(80, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
    }

    /// `draw` renders all six tabs without panicking.
    #[test]
    fn draw_all_tabs_render_without_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for tab_id in [
            "workspaces",
            "ssh",
            "database",
            "network",
            "system",
            "settings",
        ] {
            let sid_app = build_test_sid_app(Some(tab_id));
            let backend = TestBackend::new(120, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw(frame, &sid_app))
                .unwrap_or_else(|e| panic!("draw panicked for tab '{tab_id}': {e}"));
        }
    }

    /// Build a `SidApp` with a fresh tempdir-backed store for draw tests.
    /// Each call uses a unique temp file so tests can run in parallel.
    fn build_test_sid_app(start_tab: Option<&str>) -> SidApp {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("draw_test.redb");
        let store = Arc::new(RedbStore::open(&db_file).unwrap());
        // Leak tempdir so it isn't deleted before draw runs — only used in tests.
        std::mem::forget(dir);
        let secrets: Arc<dyn sid_core::adapters::secrets::SecretStore> = Arc::new(
            sid_secrets::PlainStore::new(Arc::clone(&store) as Arc<dyn Store>),
        );
        SidApp {
            app: build_app(start_tab, vec![]),
            store,
            session_id: "draw-test-sess".into(),
            sys_probe: None,
            sys_rx: None,
            systemctl: Arc::new(NoopSystemctlClient),
            spawner: Arc::new(NoopTerminalSpawner),
            postgres: sid_db_clients::PostgresClient::factory(),
            sqlite: sid_db_clients::SqliteClient::factory(),
            secrets,
            animation: AnimationConfig::default(),
            fx_state: None,
            modal_stack: Vec::new(),
            pending_submits: Vec::new(),
            toasts: ToastQueue::new(4),
            jobs: Arc::new(sid_job::JobQueue::<JobOutcome>::new()),
        }
    }

    // ---- load_active_theme / load_active_keybinds ----

    fn fresh_store() -> (tempfile::TempDir, RedbStore) {
        let d = tempdir().unwrap();
        let s = RedbStore::open(&d.path().join("s.redb")).unwrap();
        (d, s)
    }

    #[test]
    fn load_active_theme_first_run_returns_cosmos() {
        let (_d, store) = fresh_store();
        let (theme, registry) = load_active_theme(&store);
        assert_eq!(theme.name, "cosmos");
        assert!(registry.get("void").is_some());
    }

    #[test]
    fn load_active_theme_honours_setting() {
        use sid_store::TypedSettings;
        let (_d, store) = fresh_store();
        store
            .put_string(sid_store::settings_keys::THEME_NAME, "void")
            .unwrap();
        let (theme, _) = load_active_theme(&store);
        assert_eq!(theme.name, "void");
    }

    #[test]
    fn load_active_theme_falls_back_when_setting_unknown() {
        use sid_store::TypedSettings;
        let (_d, store) = fresh_store();
        store
            .put_string(sid_store::settings_keys::THEME_NAME, "nope")
            .unwrap();
        let (theme, _) = load_active_theme(&store);
        assert_eq!(theme.name, "cosmos");
    }

    #[test]
    fn load_active_theme_merges_user_themes() {
        use sid_store::{ThemeGlyphs, ThemePalette, ThemeSpec};
        let (_d, store) = fresh_store();
        store
            .upsert_theme(&ThemeSpec {
                name: "mine".into(),
                palette: ThemePalette {
                    background: 0x010203,
                    surface: 0,
                    foreground: 0,
                    muted: 0,
                    accent_primary: 0,
                    accent_success: 0,
                    accent_warning: 0,
                    accent_error: 0,
                    border: 0,
                },
                glyphs: ThemeGlyphs {
                    star: '*',
                    small_star: '.',
                    dot: '.',
                },
            })
            .unwrap();
        let (_theme, registry) = load_active_theme(&store);
        assert!(registry.get("mine").is_some());
        assert_eq!(registry.get("mine").unwrap().background.r, 0x01);
    }

    #[test]
    fn load_active_keybinds_first_run_seeds_cosmos_default() {
        let (_d, store) = fresh_store();
        let map = load_active_keybinds(&store);
        assert!(map.iter().count() > 0);
        // Cosmos profile should now be persisted.
        assert!(store.get_keybind_profile("cosmos").unwrap().is_some());
    }

    #[test]
    fn load_active_keybinds_unknown_name_falls_back() {
        use sid_store::TypedSettings;
        let (_d, store) = fresh_store();
        store
            .put_string(sid_store::settings_keys::KEYBIND_PROFILE_NAME, "missing")
            .unwrap();
        // Should not panic; returns cosmos default (and seeds 'cosmos').
        let map = load_active_keybinds(&store);
        assert!(map.iter().count() > 0);
    }

    // ─── Plan 6 — palette hydration ──────────────────────────────────────────

    #[test]
    fn hydrate_quick_actions_into_registry_adds_globals() {
        use sid_store::{QuickAction, QuickActionScope};

        let (_d, store) = fresh_store();
        let a = QuickAction {
            id: QuickAction::new_id(),
            label: "kill 5432".into(),
            scope: QuickActionScope::Global,
            cmd: "fuser -k 5432/tcp".into(),
            keybind: None,
        };
        store.upsert_quick_action(&a).unwrap();

        let mut reg = sid_core::action::ActionRegistry::new();
        let n = super::hydrate_quick_actions_into_registry(&store, &mut reg).unwrap();
        assert_eq!(n, 1);
        assert!(!reg.fuzzy("kill").is_empty());
    }

    #[test]
    fn hydrate_skips_workspace_scoped_actions() {
        use sid_store::{QuickAction, QuickActionScope};

        let (_d, store) = fresh_store();
        store
            .upsert_quick_action(&QuickAction {
                id: QuickAction::new_id(),
                label: "ws-only".into(),
                scope: QuickActionScope::Workspace,
                cmd: "echo".into(),
                keybind: None,
            })
            .unwrap();
        let mut reg = sid_core::action::ActionRegistry::new();
        let n = super::hydrate_quick_actions_into_registry(&store, &mut reg).unwrap();
        assert_eq!(n, 0);
        assert!(reg.fuzzy("ws-only").is_empty());
    }

    #[test]
    fn rehydrate_drops_old_qa_entries_and_adds_new() {
        use sid_store::{QuickAction, QuickActionScope};

        let (_d, store) = fresh_store();
        let a = QuickAction {
            id: QuickAction::new_id(),
            label: "before".into(),
            scope: QuickActionScope::Global,
            cmd: "echo".into(),
            keybind: None,
        };
        store.upsert_quick_action(&a).unwrap();
        let mut reg = sid_core::action::ActionRegistry::new();
        super::hydrate_quick_actions_into_registry(&store, &mut reg).unwrap();
        assert!(!reg.fuzzy("before").is_empty());

        // Replace with a different action and rehydrate.
        store.remove_quick_action(&a.id).unwrap();
        let b = QuickAction {
            id: QuickAction::new_id(),
            label: "after".into(),
            scope: QuickActionScope::Global,
            cmd: "echo".into(),
            keybind: None,
        };
        store.upsert_quick_action(&b).unwrap();
        super::rehydrate_global_quick_actions(&store, &mut reg).unwrap();
        assert!(reg.fuzzy("before").is_empty());
        assert!(!reg.fuzzy("after").is_empty());
    }

    #[test]
    fn rehydrate_preserves_non_qa_actions() {
        use sid_core::action::Action;

        let (_d, store) = fresh_store();
        let mut reg = sid_core::action::ActionRegistry::new();
        reg.register(Action::new("app.quit", "Quit"));
        super::rehydrate_global_quick_actions(&store, &mut reg).unwrap();
        assert!(reg.get(&"app.quit".into()).is_some());
    }

    #[test]
    fn noop_systemctl_client_returns_missing_for_every_method() {
        use sid_core::adapters::systemctl::{SystemctlClient, SystemctlError, UnitBus, UnitFilter};
        let c = super::NoopSystemctlClient;
        assert!(matches!(
            c.list_units(UnitFilter::default()).unwrap_err(),
            SystemctlError::SystemctlMissing
        ));
        assert!(matches!(
            c.status(UnitBus::User, "x").unwrap_err(),
            SystemctlError::SystemctlMissing
        ));
        assert!(matches!(
            c.start(UnitBus::User, "x").unwrap_err(),
            SystemctlError::SystemctlMissing
        ));
        assert!(matches!(
            c.stop(UnitBus::User, "x").unwrap_err(),
            SystemctlError::SystemctlMissing
        ));
        assert!(matches!(
            c.restart(UnitBus::User, "x").unwrap_err(),
            SystemctlError::SystemctlMissing
        ));
        assert!(matches!(
            c.journal_tail(UnitBus::User, "x", 10).unwrap_err(),
            SystemctlError::JournalctlMissing
        ));
    }

    #[test]
    fn noop_terminal_spawner_reports_terminal_missing() {
        use sid_core::adapters::terminal_spawner::{SpawnRequest, SpawnerError, TerminalSpawner};
        let s = super::NoopTerminalSpawner;
        assert_eq!(s.name(), "noop");
        let err = s
            .spawn(SpawnRequest {
                cwd: std::path::PathBuf::from("/"),
                cmd: "echo".into(),
            })
            .unwrap_err();
        assert!(matches!(err, SpawnerError::TerminalMissing(_)));
    }

    #[test]
    fn build_systemctl_client_does_not_panic() {
        // On a systemd host this returns SystemctlCmdClient; on others, NoopSystemctlClient.
        let _ = super::build_systemctl_client();
    }

    #[test]
    fn build_terminal_spawner_does_not_panic() {
        let _ = super::build_terminal_spawner();
    }

    // ---- Phase 3 modal routing ----

    /// `modal_for_active_tab_key` opens a "New Workspace" modal when on the
    /// Workspaces tab and `N` is pressed.
    #[test]
    fn modal_for_key_n_on_workspaces_opens_new_modal() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let sid_app = build_test_sid_app(Some("workspaces"));
        let chord = KeyChord {
            code: KeyCode::Char('N'),
            mods: KeyModifiers::NONE,
        };
        let modal = modal_for_active_tab_key(&sid_app, chord);
        assert!(modal.is_some(), "N on workspaces should open a modal");
        let m = modal.unwrap();
        assert_eq!(m.id.0, "workspaces.new");
        // Three fields: name, path, kind.
        assert_eq!(m.fields.len(), 3);
    }

    /// Pressing `N` on a non-workspaces tab does not open a Workspaces modal.
    #[test]
    fn modal_for_key_n_on_other_tab_returns_none() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        // Settings has no modal trigger for `N`.
        let sid_app = build_test_sid_app(Some("settings"));
        let chord = KeyChord {
            code: KeyCode::Char('N'),
            mods: KeyModifiers::NONE,
        };
        assert!(modal_for_active_tab_key(&sid_app, chord).is_none());
    }

    /// Modifier-combined keys (Ctrl+N) do NOT trigger the modal — those are
    /// reserved for global actions.
    #[test]
    fn modal_for_key_ctrl_n_does_not_open() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let sid_app = build_test_sid_app(Some("workspaces"));
        let chord = KeyChord {
            code: KeyCode::Char('N'),
            mods: KeyModifiers::CONTROL,
        };
        assert!(modal_for_active_tab_key(&sid_app, chord).is_none());
    }

    /// Submitting a "New Workspace" modal upserts the workspace into the
    /// store and the WorkspacesWidget then sees it on next refresh.
    #[test]
    fn modal_submit_new_workspace_persists_and_refreshes() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        // Create a real directory so canonicalize succeeds and the workspace
        // path is stable.
        let dir = tempdir().unwrap();
        let target = dir.path().to_path_buf();

        // Before: no workspaces in the store.
        assert!(sid_app.store.list_workspaces().unwrap().is_empty());

        // Simulate the modal's collected values after Enter.
        let id = ModalId("workspaces.new".to_string());
        let values = vec![
            ("name".to_string(), FieldValue::Text("test-ws".into())),
            (
                "path".to_string(),
                FieldValue::Picker(target.to_string_lossy().into_owned()),
            ),
            ("kind".to_string(), FieldValue::Choice("Umbrella".into())),
        ];

        dispatch_modal_submit(&mut sid_app, &id, &values).expect("submit ok");

        let ws = sid_app.store.list_workspaces().unwrap();
        assert_eq!(ws.len(), 1, "exactly one workspace persisted");
        assert_eq!(ws[0].name, "test-ws");
        assert_eq!(ws[0].kind, WorkspaceKind::Umbrella);
    }

    /// Submitting "Remove" with the "No, cancel" choice is a no-op.
    #[test]
    fn modal_submit_remove_cancel_does_not_delete() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let dir = tempdir().unwrap();
        let target = dir.path().to_path_buf();
        sid_app
            .store
            .upsert_workspace(&Workspace {
                path: target.clone(),
                name: "victim".into(),
                kind: WorkspaceKind::Repo,
                manifest_hash: 0,
                last_seen: now_epoch(),
                parent: None,
            })
            .unwrap();
        assert_eq!(sid_app.store.list_workspaces().unwrap().len(), 1);

        let id = ModalId(format!("workspaces.remove:{}", target.display()));
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("No, cancel".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).expect("submit ok");
        assert_eq!(
            sid_app.store.list_workspaces().unwrap().len(),
            1,
            "no-cancel must not delete"
        );
    }

    /// Submitting "Remove" with "Yes, remove" actually removes the workspace.
    #[test]
    fn modal_submit_remove_yes_deletes() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let dir = tempdir().unwrap();
        let target = dir.path().to_path_buf();
        sid_app
            .store
            .upsert_workspace(&Workspace {
                path: target.clone(),
                name: "victim".into(),
                kind: WorkspaceKind::Repo,
                manifest_hash: 0,
                last_seen: now_epoch(),
                parent: None,
            })
            .unwrap();

        let id = ModalId(format!("workspaces.remove:{}", target.display()));
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("Yes, remove".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).expect("submit ok");
        assert!(
            sid_app.store.list_workspaces().unwrap().is_empty(),
            "yes-remove must delete"
        );
    }

    // ─── Phase 4 — SSH tab modals ───────────────────────────────────────────

    fn plain_chord(c: char) -> sid_core::event::KeyChord {
        use crossterm::event::{KeyCode, KeyModifiers};
        sid_core::event::KeyChord {
            code: KeyCode::Char(c),
            mods: KeyModifiers::NONE,
        }
    }

    fn delete_chord() -> sid_core::event::KeyChord {
        use crossterm::event::{KeyCode, KeyModifiers};
        sid_core::event::KeyChord {
            code: KeyCode::Delete,
            mods: KeyModifiers::NONE,
        }
    }

    /// Pressing `N` on the SSH tab opens the `ssh.new` modal with the six
    /// expected fields.
    #[test]
    fn ssh_new_modal_for_key_opens_on_ssh() {
        let sid_app = build_test_sid_app(Some("ssh"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'))
            .expect("N on ssh should open a modal");
        assert_eq!(modal.id.0, "ssh.new");
        assert_eq!(modal.fields.len(), 6);
    }

    /// `N` on a non-SSH tab does NOT open `ssh.new`. (Confirmed by checking
    /// the modal id, since other tabs may have their own `N` modals.)
    #[test]
    fn ssh_new_modal_for_key_does_not_open_on_other_tabs() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert_ne!(id, "ssh.new", "workspaces N must not produce ssh.new");
    }

    /// Submitting an `ssh.new` modal upserts the host into the store AND the
    /// SSH widget sees the new host on the next render.
    #[test]
    fn ssh_new_submit_persists_and_refreshes() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        assert!(sid_app.store.list_ssh_hosts().unwrap().is_empty());

        let id = ModalId("ssh.new".to_string());
        let values = vec![
            ("alias".to_string(), FieldValue::Text("my-prod".into())),
            ("host".to_string(), FieldValue::Text("10.0.0.1".into())),
            ("user".to_string(), FieldValue::Text("root".into())),
            ("port".to_string(), FieldValue::Text("22".into())),
            (
                "identity_file".to_string(),
                FieldValue::Picker(String::new()),
            ),
            ("auth".to_string(), FieldValue::Choice("Key".into())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).expect("submit ok");

        let hosts = sid_app.store.list_ssh_hosts().unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "my-prod");
        assert_eq!(hosts[0].port, 22);
        assert!(hosts[0].identity_file.is_none());
        // The Auth choice persists — the user picked "Key" above.
        assert_eq!(hosts[0].auth_kind, sid_store::SshAuthKind::Key);

        // The widget should see it too.
        let widget_aliases: Vec<String> = sid_app
            .app
            .tabs()
            .active()
            .layout
            .iter_widgets()
            .next()
            .and_then(|w| w.as_any().downcast_ref::<SshWidget>())
            .map(|s| {
                s.state()
                    .visible_hosts()
                    .iter()
                    .map(|h| h.alias.clone())
                    .collect()
            })
            .unwrap_or_default();
        assert!(widget_aliases.contains(&"my-prod".to_string()));
    }

    /// The `auth` Choice value persists through `ssh.new` for every variant.
    #[test]
    fn ssh_new_submit_persists_each_auth_kind() {
        use sid_store::SshAuthKind;
        use sid_widgets::{FieldValue, ModalId};

        let cases = [
            ("Key", SshAuthKind::Key),
            ("Password", SshAuthKind::Password),
            ("Agent", SshAuthKind::Agent),
            // Unknown / missing value falls back to Agent (most permissive).
            ("WeirdNotAnOption", SshAuthKind::Agent),
        ];
        for (label, expected) in cases {
            let mut sid_app = build_test_sid_app(Some("ssh"));
            let id = ModalId("ssh.new".to_string());
            let values = vec![
                ("alias".to_string(), FieldValue::Text(format!("h-{label}"))),
                ("host".to_string(), FieldValue::Text("h".into())),
                ("user".to_string(), FieldValue::Text("u".into())),
                ("port".to_string(), FieldValue::Text("22".into())),
                (
                    "identity_file".to_string(),
                    FieldValue::Picker(String::new()),
                ),
                ("auth".to_string(), FieldValue::Choice(label.into())),
            ];
            dispatch_modal_submit(&mut sid_app, &id, &values).expect("submit ok");
            let hosts = sid_app.store.list_ssh_hosts().unwrap();
            assert_eq!(
                hosts[0].auth_kind, expected,
                "{label} choice should persist as {expected:?}"
            );
        }
    }

    /// `ssh.new` requires alias, host, user. Empty alias → Err.
    #[test]
    fn ssh_new_submit_rejects_missing_required_fields() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let id = ModalId("ssh.new".to_string());
        let values = vec![
            ("alias".to_string(), FieldValue::Text(String::new())),
            ("host".to_string(), FieldValue::Text("h".into())),
            ("user".to_string(), FieldValue::Text("u".into())),
            ("port".to_string(), FieldValue::Text("22".into())),
            (
                "identity_file".to_string(),
                FieldValue::Picker(String::new()),
            ),
            ("auth".to_string(), FieldValue::Choice("Key".into())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().contains("alias"));
    }

    /// `ssh.new` rejects a port that is not a u16.
    #[test]
    fn ssh_new_submit_rejects_non_u16_port() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let id = ModalId("ssh.new".to_string());
        let values = vec![
            ("alias".to_string(), FieldValue::Text("a".into())),
            ("host".to_string(), FieldValue::Text("h".into())),
            ("user".to_string(), FieldValue::Text("u".into())),
            ("port".to_string(), FieldValue::Text("not-a-number".into())),
            (
                "identity_file".to_string(),
                FieldValue::Picker(String::new()),
            ),
            ("auth".to_string(), FieldValue::Choice("Key".into())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("port"));
    }

    /// `Del` on the SSH tab with a selected manual host opens the remove
    /// modal. With no hosts present, it returns None.
    #[test]
    fn ssh_remove_modal_for_key_opens_on_ssh_with_manual_host() {
        use sid_store::{SshHost, SshHostSource};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        sid_app
            .store
            .upsert_ssh_host(&SshHost {
                alias: "to-kill".into(),
                host: "h".into(),
                port: 22,
                user: "u".into(),
                identity_file: None,
                source: SshHostSource::Manual,
                last_connected: 0,
                command_history: Vec::new(),
                last_sftp_path: None,
                auth_kind: sid_store::SshAuthKind::Agent,
            })
            .unwrap();
        // Refresh the widget to pick up the new host.
        refresh_ssh_widget(&mut sid_app);

        let modal = modal_for_active_tab_key(&sid_app, delete_chord())
            .expect("Del on ssh with selected host opens remove modal");
        assert_eq!(modal.id.0, "ssh.remove:to-kill");
    }

    /// `Del` on the SSH tab does not open a modal on a different tab (e.g.
    /// Database). Database has its own Del handler with its own id prefix.
    #[test]
    fn ssh_remove_modal_for_key_does_not_open_on_other_tabs() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal = modal_for_active_tab_key(&sid_app, delete_chord());
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert!(!id.starts_with("ssh.remove"));
    }

    /// "No, cancel" on `ssh.remove:<alias>` leaves the host present.
    #[test]
    fn ssh_remove_cancel_does_not_delete() {
        use sid_store::{SshHost, SshHostSource};
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        sid_app
            .store
            .upsert_ssh_host(&SshHost {
                alias: "keep".into(),
                host: "h".into(),
                port: 22,
                user: "u".into(),
                identity_file: None,
                source: SshHostSource::Manual,
                last_connected: 0,
                command_history: Vec::new(),
                last_sftp_path: None,
                auth_kind: sid_store::SshAuthKind::Agent,
            })
            .unwrap();
        let id = ModalId("ssh.remove:keep".to_string());
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("No, cancel".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert_eq!(sid_app.store.list_ssh_hosts().unwrap().len(), 1);
    }

    /// "Yes, remove" on `ssh.remove:<alias>` deletes the host.
    #[test]
    fn ssh_remove_yes_deletes() {
        use sid_store::{SshHost, SshHostSource};
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        sid_app
            .store
            .upsert_ssh_host(&SshHost {
                alias: "doomed".into(),
                host: "h".into(),
                port: 22,
                user: "u".into(),
                identity_file: None,
                source: SshHostSource::Manual,
                last_connected: 0,
                command_history: Vec::new(),
                last_sftp_path: None,
                auth_kind: sid_store::SshAuthKind::Agent,
            })
            .unwrap();
        let id = ModalId("ssh.remove:doomed".to_string());
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("Yes, remove".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert!(sid_app.store.list_ssh_hosts().unwrap().is_empty());
    }

    /// `G` on the SSH tab opens the gen-key wizard step 1 modal.
    #[test]
    fn ssh_gen_key_modal_for_key_opens_on_ssh() {
        let sid_app = build_test_sid_app(Some("ssh"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('G'))
            .expect("G on ssh opens keygen wizard");
        assert_eq!(modal.id.0, "ssh.gen_key.step1");
        // Step 1 has a single Choice field — algorithm.
        assert_eq!(modal.fields.len(), 1);
    }

    /// `G` on a non-SSH tab does not open the keygen wizard.
    #[test]
    fn ssh_gen_key_modal_for_key_does_not_open_on_other_tabs() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('G'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert!(!id.starts_with("ssh.gen_key"));
    }

    /// Step 2 of the wizard rejects mismatched passphrase before invoking
    /// `ssh-keygen`.
    #[test]
    fn ssh_gen_key_step2_rejects_mismatched_passphrase() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let dir = tempdir().unwrap();
        let out = dir.path().join("id_ed25519");
        let id = ModalId("ssh.gen_key.step2:Ed25519".to_string());
        let values = vec![
            (
                "output_path".to_string(),
                FieldValue::Picker(out.to_string_lossy().into_owned()),
            ),
            (
                "passphrase".to_string(),
                FieldValue::Password("alpha".into()),
            ),
            (
                "confirm_passphrase".to_string(),
                FieldValue::Password("BETA".into()),
            ),
            ("comment".to_string(), FieldValue::Text("c".into())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().contains("do not match"));
        assert!(!out.exists());
    }

    /// Step 2 rejects an existing output path so the test never shells out.
    #[test]
    fn ssh_gen_key_step2_rejects_existing_path() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let dir = tempdir().unwrap();
        let out = dir.path().join("id_existing");
        std::fs::write(&out, "preexisting").unwrap();
        let id = ModalId("ssh.gen_key.step2:Ed25519".to_string());
        let values = vec![
            (
                "output_path".to_string(),
                FieldValue::Picker(out.to_string_lossy().into_owned()),
            ),
            (
                "passphrase".to_string(),
                FieldValue::Password(String::new()),
            ),
            (
                "confirm_passphrase".to_string(),
                FieldValue::Password(String::new()),
            ),
            ("comment".to_string(), FieldValue::Text("c".into())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "preexisting");
    }

    /// Step 2 rejects an empty output_path.
    #[test]
    fn ssh_gen_key_step2_rejects_empty_output_path() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let id = ModalId("ssh.gen_key.step2:Ed25519".to_string());
        let values = vec![
            ("output_path".to_string(), FieldValue::Picker(String::new())),
            (
                "passphrase".to_string(),
                FieldValue::Password(String::new()),
            ),
            (
                "confirm_passphrase".to_string(),
                FieldValue::Password(String::new()),
            ),
            ("comment".to_string(), FieldValue::Text(String::new())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().contains("output_path"));
    }

    /// Step 1 → step 2 chain: a valid step-1 submit pushes step 2 onto the
    /// modal stack. Step 1 does NOT shell out, so no ssh-keygen dependency.
    #[test]
    fn ssh_gen_key_step1_to_step2_to_step3() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        // Step 1 — algorithm choice.
        let id1 = ModalId("ssh.gen_key.step1".to_string());
        let v1 = vec![(
            "algorithm".to_string(),
            FieldValue::Choice("Ed25519".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id1, &v1).unwrap();
        assert_eq!(sid_app.modal_stack.len(), 1);
        assert_eq!(sid_app.modal_stack[0].id.0, "ssh.gen_key.step2:Ed25519");
        // Step 2 submit with valid inputs and a path we can write to — but
        // since ssh-keygen may not be present in this test env, we just
        // assert that submitting an "exists" path stops the chain with an
        // error (which exercises step 2's validation rather than the shell-out).
        let dir = tempdir().unwrap();
        let out = dir.path().join("must_not_exist");
        std::fs::write(&out, "x").unwrap();
        let id2 = ModalId("ssh.gen_key.step2:Ed25519".to_string());
        let v2 = vec![
            (
                "output_path".to_string(),
                FieldValue::Picker(out.to_string_lossy().into_owned()),
            ),
            (
                "passphrase".to_string(),
                FieldValue::Password(String::new()),
            ),
            (
                "confirm_passphrase".to_string(),
                FieldValue::Password(String::new()),
            ),
            ("comment".to_string(), FieldValue::Text(String::new())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id2, &v2).unwrap_err();
        assert!(err.to_string().contains("already exists"));
        // Step 3 chain not pushed (step 2 errored).
        // The pre-pushed step 2 from step 1 is still in the stack.
        assert_eq!(sid_app.modal_stack.len(), 1);

        // Now exercise step 3 submit with target == "<None>" so it doesn't shell out.
        let id3 = ModalId("ssh.gen_key.step3:Ed25519:/tmp/none".to_string());
        let v3 = vec![(
            "target_host".to_string(),
            FieldValue::Choice("<None — copy manually later>".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id3, &v3).unwrap();
    }

    /// Real ssh-keygen end-to-end test. Skipped by default — ssh-keygen may
    /// not be on PATH inside CI.
    #[test]
    #[ignore = "needs ssh-keygen"]
    fn ssh_gen_key_step2_invokes_ssh_keygen_when_inputs_valid() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let dir = tempdir().unwrap();
        let out = dir.path().join("id_ed25519_gen_test");
        let id = ModalId("ssh.gen_key.step2:Ed25519".to_string());
        let values = vec![
            (
                "output_path".to_string(),
                FieldValue::Picker(out.to_string_lossy().into_owned()),
            ),
            (
                "passphrase".to_string(),
                FieldValue::Password(String::new()),
            ),
            (
                "confirm_passphrase".to_string(),
                FieldValue::Password(String::new()),
            ),
            ("comment".to_string(), FieldValue::Text("sid-test".into())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert!(out.exists());
        let pub_path = dir.path().join("id_ed25519_gen_test.pub");
        assert!(pub_path.exists());
    }

    // ─── New SSH actions: Edit / Setup remote / Key manager / Debug / SFTP / Help

    fn upsert_host_for(sid_app: &mut SidApp, alias: &str) {
        use sid_store::{SshHost, SshHostSource};
        sid_app
            .store
            .upsert_ssh_host(&SshHost {
                alias: alias.into(),
                host: "h".into(),
                port: 22,
                user: "u".into(),
                identity_file: None,
                source: SshHostSource::Manual,
                last_connected: 0,
                command_history: Vec::new(),
                last_sftp_path: None,
                auth_kind: sid_store::SshAuthKind::Agent,
            })
            .unwrap();
        refresh_ssh_widget(sid_app);
    }

    /// `E` on the SSH tab with a selected manual host opens the edit modal.
    #[test]
    fn ssh_edit_modal_for_key_opens_on_ssh() {
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "edit-me");
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('E'))
            .expect("E on ssh opens edit modal");
        assert_eq!(modal.id.0, "ssh.edit:edit-me");
        assert_eq!(modal.fields.len(), 6);
    }

    /// `E` on a non-SSH tab does not open an SSH modal.
    #[test]
    fn ssh_edit_modal_does_not_open_on_other_tabs() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('E'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert!(!id.starts_with("ssh.edit"));
    }

    /// Submitting `ssh.edit:<alias>` updates the host record fields.
    #[test]
    fn ssh_edit_submit_updates_host() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "alpha");
        let id = ModalId("ssh.edit:alpha".to_string());
        let values = vec![
            ("alias".to_string(), FieldValue::Text("alpha".into())),
            ("host".to_string(), FieldValue::Text("10.99.99.99".into())),
            ("user".to_string(), FieldValue::Text("admin".into())),
            ("port".to_string(), FieldValue::Text("2222".into())),
            (
                "identity_file".to_string(),
                FieldValue::Picker("/tmp/id_test".into()),
            ),
            ("auth".to_string(), FieldValue::Choice("Key".into())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let h = sid_app.store.get_ssh_host("alpha").unwrap().unwrap();
        assert_eq!(h.host, "10.99.99.99");
        assert_eq!(h.user, "admin");
        assert_eq!(h.port, 2222);
        assert_eq!(h.identity_file.as_deref(), Some("/tmp/id_test"));
    }

    /// Editing with an empty alias is rejected.
    #[test]
    fn ssh_edit_submit_rejects_empty_alias() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "alpha");
        let id = ModalId("ssh.edit:alpha".to_string());
        let values = vec![
            ("alias".to_string(), FieldValue::Text(String::new())),
            ("host".to_string(), FieldValue::Text("h".into())),
            ("user".to_string(), FieldValue::Text("u".into())),
            ("port".to_string(), FieldValue::Text("22".into())),
            (
                "identity_file".to_string(),
                FieldValue::Picker(String::new()),
            ),
            ("auth".to_string(), FieldValue::Choice("Key".into())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().contains("alias"));
    }

    /// `S` on the SSH tab opens the setup-remote step 1 modal.
    #[test]
    fn ssh_setup_remote_modal_opens_on_ssh() {
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "with-host");
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('S'))
            .expect("S on ssh opens setup-remote modal");
        assert_eq!(modal.id.0, "ssh.setup_remote.identity:with-host");
    }

    /// Step 1 of setup-remote pushes step 2 onto the modal stack on Save.
    #[test]
    fn ssh_setup_remote_step1_pushes_step2_on_submit() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let id = ModalId("ssh.setup_remote.identity:alpha".to_string());
        let values = vec![(
            "identity_path".to_string(),
            FieldValue::Choice("/home/u/.ssh/id_ed25519".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert_eq!(sid_app.modal_stack.len(), 1);
        let pushed_id = &sid_app.modal_stack[0].id.0;
        assert!(pushed_id.starts_with("ssh.setup_remote.confirm:alpha:"));
    }

    /// Step 1 rejects a placeholder "(no existing key…)" identity.
    #[test]
    fn ssh_setup_remote_step1_rejects_placeholder() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let id = ModalId("ssh.setup_remote.identity:alpha".to_string());
        let values = vec![(
            "identity_path".to_string(),
            FieldValue::Choice("(no existing key found in ~/.ssh/)".into()),
        )];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().contains("no identity"));
    }

    /// Setup-remote "No, cancel" on step 2 is a no-op (no step 3 pushed).
    #[test]
    fn ssh_setup_remote_step2_cancel_no_op() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let id = ModalId("ssh.setup_remote.confirm:alpha:/tmp/id".to_string());
        let values = vec![
            ("summary".to_string(), FieldValue::Text("...".into())),
            (
                "proceed".to_string(),
                FieldValue::Choice("No, cancel".into()),
            ),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert!(sid_app.modal_stack.is_empty());
    }

    /// `K` (uppercase only) on the SSH tab opens the key manager.
    #[test]
    fn ssh_key_manager_modal_opens_on_ssh() {
        let sid_app = build_test_sid_app(Some("ssh"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('K'))
            .expect("K on ssh opens key manager");
        assert_eq!(modal.id.0, "ssh.key_manager");
    }

    /// Lowercase `k` is the widget's "select prev" — it must NOT open the
    /// key manager modal.
    #[test]
    fn ssh_key_manager_modal_does_not_open_on_lowercase_k() {
        let sid_app = build_test_sid_app(Some("ssh"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('k'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert_ne!(id, "ssh.key_manager");
    }

    /// `ssh.key_manager` listing uses [`discover_ssh_keys_in`] when pointed at
    /// a tempdir, picking up `id_*` files and skipping `.pub`.
    #[test]
    fn ssh_key_manager_lists_keys() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("id_test_ed25519"), "stub").unwrap();
        std::fs::write(dir.path().join("id_test_ed25519.pub"), "stub-pub").unwrap();
        std::fs::write(dir.path().join("known_hosts"), "unrelated").unwrap();
        let keys = discover_ssh_keys_in(Some(dir.path()));
        let names: Vec<&str> = keys
            .iter()
            .filter_map(|k| k.path.rsplit('/').next())
            .collect();
        assert!(names.contains(&"id_test_ed25519"));
        assert!(!names.contains(&"id_test_ed25519.pub"));
        assert!(!names.contains(&"known_hosts"));
    }

    /// `X` (uppercase only) on SSH opens the debug modal for the selected host.
    #[test]
    fn ssh_debug_modal_opens() {
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "diag-target");
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('X'))
            .expect("X on ssh opens debug modal");
        assert_eq!(modal.id.0, "ssh.debug:diag-target");
    }

    /// Lowercase `x` does not open the debug modal.
    #[test]
    fn ssh_debug_modal_does_not_open_on_lowercase_x() {
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "diag-target");
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('x'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert!(!id.starts_with("ssh.debug"));
    }

    /// `F` on the SSH tab with a manual host opens the SFTP-persist modal.
    #[test]
    fn ssh_sftp_persist_modal_opens() {
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "sftp-host");
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('F'))
            .expect("F on ssh opens sftp persist modal");
        assert_eq!(modal.id.0, "ssh.sftp_persist:sftp-host");
    }

    /// Submitting `ssh.sftp_persist:<alias>` updates the host's last_sftp_path.
    #[test]
    fn ssh_sftp_persist_writes_last_path() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "sftp-host");
        let id = ModalId("ssh.sftp_persist:sftp-host".to_string());
        let values = vec![(
            "last_path".to_string(),
            FieldValue::Text("/srv/data".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let h = sid_app.store.get_ssh_host("sftp-host").unwrap().unwrap();
        assert_eq!(h.last_sftp_path.as_deref(), Some("/srv/data"));
    }

    /// Empty `last_path` clears the field back to None.
    #[test]
    fn ssh_sftp_persist_empty_clears_field() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "sftp-host");
        // Pre-populate.
        {
            let mut existing = sid_app.store.get_ssh_host("sftp-host").unwrap().unwrap();
            existing.last_sftp_path = Some("/old".into());
            sid_app.store.upsert_ssh_host(&existing).unwrap();
        }
        let id = ModalId("ssh.sftp_persist:sftp-host".to_string());
        let values = vec![("last_path".to_string(), FieldValue::Text(String::new()))];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let h = sid_app.store.get_ssh_host("sftp-host").unwrap().unwrap();
        assert!(h.last_sftp_path.is_none());
    }

    /// `?` on any tab opens the help modal for that tab.
    #[test]
    fn ssh_help_modal_lists_footer_hints() {
        let sid_app = build_test_sid_app(Some("ssh"));
        let modal =
            modal_for_active_tab_key(&sid_app, plain_chord('?')).expect("? always opens help");
        assert_eq!(modal.id.0, "help:ssh");
        // The keys field should contain the SshWidget's footer hints (N/G/S/K/X/?).
        // Help modals use `Field::Display` so multi-line bodies render one
        // row per `\n`-separated line.
        let keys_val = modal
            .fields
            .iter()
            .find_map(|f| match f {
                sid_widgets::Field::Display { label, body } if label == "keys" => {
                    Some(body.clone())
                }
                _ => None,
            })
            .unwrap_or_default();
        for ch in ["N:", "G:", "S:", "K:", "X:", "?:"] {
            assert!(
                keys_val.contains(ch),
                "expected help to mention {ch}; got: {keys_val}"
            );
        }
        // Global hints too.
        assert!(keys_val.contains("Ctrl+Q"));
    }

    /// `?` on the Workspaces tab also opens a help modal (id keyed by tab).
    #[test]
    fn ssh_help_modal_opens_on_other_tabs_too() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal =
            modal_for_active_tab_key(&sid_app, plain_chord('?')).expect("? always opens help");
        assert_eq!(modal.id.0, "help:workspaces");
    }

    /// The help modal uses `Field::Display` so multi-line bodies render
    /// one row per `\n`-separated line. The body must contain newline
    /// characters (proving the modal will paint multi-row) and the field
    /// variant must be `Display` (so the renderer takes the multi-row path
    /// instead of clipping to a single value row).
    #[test]
    fn help_modal_uses_display_field_with_multiline_body() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal =
            modal_for_active_tab_key(&sid_app, plain_chord('?')).expect("? always opens help");
        let first_field = modal.fields.first().expect("help modal has a field");
        match first_field {
            sid_widgets::Field::Display { label, body } => {
                assert_eq!(label, "keys");
                assert!(
                    body.contains('\n'),
                    "help body must contain newlines so the Display renderer paints multi-row"
                );
                assert!(
                    body.contains("Global:"),
                    "help body must contain Global section"
                );
            }
            other => panic!("help modal first field must be Display; got {other:?}"),
        }
    }

    // ─── Phase 6 — Mouse routing ────────────────────────────────────────────

    fn mouse_event(
        kind: crossterm::event::MouseEventKind,
        col: u16,
        row: u16,
    ) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind,
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn full_area() -> ratatui::layout::Rect {
        ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        }
    }

    /// A scroll-up wheel event maps to `Char('k')` so list widgets advance
    /// their selection upward via their existing key handler.
    #[test]
    fn mouse_scroll_up_translates_to_k() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let m = mouse_event(crossterm::event::MouseEventKind::ScrollUp, 10, 10);
        let outcome = route_mouse_event(&sid_app, full_area(), m);
        match outcome {
            MouseRouting::Synthesize(c) => {
                assert_eq!(c.code, crossterm::event::KeyCode::Char('k'));
                assert!(c.mods.is_empty());
            }
            other => panic!("expected Synthesize('k'); got {other:?}"),
        }
    }

    /// A scroll-down wheel event maps to `Char('j')`.
    #[test]
    fn mouse_scroll_down_translates_to_j() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let m = mouse_event(crossterm::event::MouseEventKind::ScrollDown, 10, 10);
        let outcome = route_mouse_event(&sid_app, full_area(), m);
        match outcome {
            MouseRouting::Synthesize(c) => {
                assert_eq!(c.code, crossterm::event::KeyCode::Char('j'));
                assert!(c.mods.is_empty());
            }
            other => panic!("expected Synthesize('j'); got {other:?}"),
        }
    }

    /// A LeftDown click on the tab strip's `ssh` label returns
    /// `SwitchToTab(1)` — the SSH tab is the second tab (index 1).
    ///
    /// The tab strip is laid out as
    /// `[marker(1)][space(1)][title(N)][gap(2)][marker(1)][space(1)][title(N)]...`
    /// starting at `inner.x` == full_area.x + 1, on row `inner.y` == 1.
    /// "Workspaces" is 10 chars → starts at col 1, occupies cols 1..=12.
    /// gap = 2 → next tab starts at col 15. "SSH" is 3 chars → occupies 15..=19.
    #[test]
    fn mouse_left_click_on_tab_strip_switches_tab() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        // Click on the "SSH" tab label (around col 16-18, row 1).
        let m = mouse_event(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            17,
            1,
        );
        let outcome = route_mouse_event(&sid_app, full_area(), m);
        assert_eq!(outcome, MouseRouting::SwitchToTab(1));
    }

    /// A left click on a row that is NOT the tab strip row is dropped
    /// (until focus-on-click lands in a future PR).
    #[test]
    fn mouse_left_click_off_tab_strip_drops() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let m = mouse_event(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            10,
            5,
        );
        let outcome = route_mouse_event(&sid_app, full_area(), m);
        assert_eq!(outcome, MouseRouting::Drop);
    }

    /// Mouse-move events are dropped silently.
    #[test]
    fn mouse_move_is_dropped() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let m = mouse_event(crossterm::event::MouseEventKind::Moved, 0, 0);
        let outcome = route_mouse_event(&sid_app, full_area(), m);
        assert_eq!(outcome, MouseRouting::Drop);
    }

    // ─── Phase 5 — Database tab modals ──────────────────────────────────────

    /// `N` on the Database tab opens the `database.new` modal.
    #[test]
    fn database_new_modal_for_key_opens_on_database() {
        let sid_app = build_test_sid_app(Some("database"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'))
            .expect("N on database opens add-connection modal");
        assert_eq!(modal.id.0, "database.new");
        assert_eq!(modal.fields.len(), 5);
    }

    /// `N` on a different tab does not produce the database modal.
    #[test]
    fn database_new_modal_for_key_does_not_open_on_other_tabs() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert_ne!(id, "database.new");
    }

    /// Submitting `database.new` writes a record to the store and refreshes
    /// the widget. With a non-empty Postgres password, the secret is also
    /// persisted via the SecretStore.
    #[test]
    fn database_new_submit_persists_and_refreshes_postgres_with_password() {
        use sid_core::adapters::secrets::SecretId;
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("database"));
        assert!(sid_app.store.list_db_connections().unwrap().is_empty());

        let id = ModalId("database.new".to_string());
        let values = vec![
            ("id".to_string(), FieldValue::Text("local-pg".into())),
            ("name".to_string(), FieldValue::Text("Local PG".into())),
            ("kind".to_string(), FieldValue::Choice("Postgres".into())),
            (
                "dsn".to_string(),
                FieldValue::Text("postgres://u@h/db".into()),
            ),
            ("password".to_string(), FieldValue::Password("pw".into())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let conns = sid_app.store.list_db_connections().unwrap();
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].id, "local-pg");
        let secret = SecretId::new("db.connection.local-pg.password");
        assert_eq!(
            sid_app.secrets.get(&secret).unwrap().as_deref(),
            Some(b"pw".as_slice())
        );

        // Widget reflects the change.
        let widget_conns: Vec<String> = sid_app
            .app
            .tabs()
            .active()
            .layout
            .iter_widgets()
            .next()
            .and_then(|w| w.as_any().downcast_ref::<DatabaseWidget>())
            .map(|d| {
                d.state()
                    .connections()
                    .iter()
                    .map(|c| c.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        assert!(widget_conns.contains(&"local-pg".to_string()));
    }

    /// SQLite connections ignore the password field even if supplied — no
    /// secret is written.
    #[test]
    fn database_new_submit_sqlite_ignores_password() {
        use sid_core::adapters::secrets::SecretId;
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("database"));
        let id = ModalId("database.new".to_string());
        let values = vec![
            ("id".to_string(), FieldValue::Text("scratch".into())),
            ("name".to_string(), FieldValue::Text("Scratch".into())),
            ("kind".to_string(), FieldValue::Choice("SQLite".into())),
            ("dsn".to_string(), FieldValue::Text(":memory:".into())),
            (
                "password".to_string(),
                FieldValue::Password("ignored".into()),
            ),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let secret = SecretId::new("db.connection.scratch.password");
        assert!(sid_app.secrets.get(&secret).unwrap().is_none());
        let conn = sid_app.store.get_db_connection("scratch").unwrap().unwrap();
        assert!(conn.secret_ref.is_none());
    }

    /// "No, cancel" on `database.remove:<id>` does not delete.
    #[test]
    fn database_remove_cancel_does_not_delete() {
        use sid_core::adapters::db_client::DbKind;
        use sid_store::{DbConnection, now_epoch};
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("database"));
        sid_app
            .store
            .upsert_db_connection(&DbConnection {
                id: "keep".into(),
                kind: DbKind::Sqlite,
                name: "Keep".into(),
                dsn: ":memory:".into(),
                secret_ref: None,
                created_at: now_epoch(),
            })
            .unwrap();
        let id = ModalId("database.remove:keep".to_string());
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("No, cancel".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert_eq!(sid_app.store.list_db_connections().unwrap().len(), 1);
    }

    /// "Yes, remove" on `database.remove:<id>` deletes the row AND the
    /// associated secret.
    #[test]
    fn database_remove_yes_deletes_and_clears_secret() {
        use sid_core::adapters::db_client::DbKind;
        use sid_core::adapters::secrets::SecretId;
        use sid_store::{DbConnection, now_epoch};
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("database"));
        let secret = SecretId::new("db.connection.doomed.password");
        sid_app.secrets.put(&secret, b"pw").unwrap();
        sid_app
            .store
            .upsert_db_connection(&DbConnection {
                id: "doomed".into(),
                kind: DbKind::Postgres,
                name: "Doomed".into(),
                dsn: "postgres://u@h/db".into(),
                secret_ref: Some(secret.clone()),
                created_at: now_epoch(),
            })
            .unwrap();
        let id = ModalId("database.remove:doomed".to_string());
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("Yes, remove".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert!(sid_app.store.list_db_connections().unwrap().is_empty());
        assert!(sid_app.secrets.get(&secret).unwrap().is_none());
    }

    // ─── Phase 5 — System tab modals ────────────────────────────────────────

    /// On the System tab with PinnedConfigs focused, `N` opens the
    /// `system.pin_config` modal.
    #[test]
    fn system_pin_modal_for_key_opens_on_system() {
        let sid_app = build_test_sid_app(Some("system"));
        // Default focused pane is PinnedConfigs.
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'))
            .expect("N on system PinnedConfigs opens pin modal");
        assert_eq!(modal.id.0, "system.pin_config");
        assert_eq!(modal.fields.len(), 2);
    }

    /// `N` on a non-System tab does not open the pin modal.
    #[test]
    fn system_pin_modal_for_key_does_not_open_on_other_tabs() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert_ne!(id, "system.pin_config");
    }

    /// Submitting `system.pin_config` writes the pin to the store and the
    /// widget reflects it.
    #[test]
    fn system_pin_submit_persists_and_refreshes() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        let dir = tempdir().unwrap();
        let target = dir.path().to_path_buf();
        let id = ModalId("system.pin_config".to_string());
        let values = vec![
            (
                "path".to_string(),
                FieldValue::Picker(target.to_string_lossy().into_owned()),
            ),
            ("label".to_string(), FieldValue::Text("zshrc".into())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let pins = sid_app.store.list_pinned_configs().unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].label, "zshrc");

        // Widget mirrors it.
        let widget_labels: Vec<String> = sid_app
            .app
            .tabs()
            .active()
            .layout
            .iter_widgets()
            .next()
            .and_then(|w| w.as_any().downcast_ref::<SystemWidget>())
            .map(|s| {
                s.pinned_configs()
                    .items()
                    .iter()
                    .map(|p| p.label.clone())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(widget_labels, vec!["zshrc".to_string()]);
    }

    /// `system.pin_config` with an empty path is rejected.
    #[test]
    fn system_pin_rejects_empty_path() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        let id = ModalId("system.pin_config".to_string());
        let values = vec![
            ("path".to_string(), FieldValue::Picker(String::new())),
            ("label".to_string(), FieldValue::Text(String::new())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().contains("path"));
    }

    /// `system.pin_config` defaults the label to the path's basename when
    /// none is supplied.
    #[test]
    fn system_pin_defaults_label_to_basename() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        let dir = tempdir().unwrap();
        let target = dir.path().join("nginx.conf");
        std::fs::write(&target, "stub").unwrap();
        let id = ModalId("system.pin_config".to_string());
        let values = vec![
            (
                "path".to_string(),
                FieldValue::Picker(target.to_string_lossy().into_owned()),
            ),
            ("label".to_string(), FieldValue::Text(String::new())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let pins = sid_app.store.list_pinned_configs().unwrap();
        assert_eq!(pins[0].label, "nginx.conf");
    }

    /// "Yes, remove" on `system.remove_pin:<path>` deletes the pin.
    #[test]
    fn system_pin_remove_yes_deletes() {
        use sid_store::{PinnedConfig, now_epoch};
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        let dir = tempdir().unwrap();
        let target = dir.path().to_path_buf();
        sid_app
            .store
            .upsert_pinned_config(&PinnedConfig {
                path: target.clone(),
                label: "victim".into(),
                opener_cmd: None,
                created_at: now_epoch(),
            })
            .unwrap();

        let id = ModalId(format!("system.remove_pin:{}", target.display()));
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("Yes, remove".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert!(sid_app.store.list_pinned_configs().unwrap().is_empty());
    }

    /// "No, cancel" on `system.remove_pin:<path>` keeps the pin.
    #[test]
    fn system_pin_remove_cancel_does_not_delete() {
        use sid_store::{PinnedConfig, now_epoch};
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        let dir = tempdir().unwrap();
        let target = dir.path().to_path_buf();
        sid_app
            .store
            .upsert_pinned_config(&PinnedConfig {
                path: target.clone(),
                label: "keep".into(),
                opener_cmd: None,
                created_at: now_epoch(),
            })
            .unwrap();
        let id = ModalId(format!("system.remove_pin:{}", target.display()));
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("No, cancel".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert_eq!(sid_app.store.list_pinned_configs().unwrap().len(), 1);
    }

    /// `N` on the System tab with QuickActions focused opens the quick-action
    /// modal.
    #[test]
    fn system_quick_action_modal_for_key_opens_when_focused() {
        let mut sid_app = build_test_sid_app(Some("system"));
        // Cycle focus forward twice: PinnedConfigs → Services → QuickActions.
        if let Some(w) = sid_app
            .app
            .tabs_mut()
            .active_mut()
            .layout
            .iter_widgets_mut()
            .next()
        {
            let any_ref = w as &mut dyn std::any::Any;
            if let Some(sw) = any_ref.downcast_mut::<SystemWidget>() {
                sw.state_mut().cycle_focus_forward();
                sw.state_mut().cycle_focus_forward();
            }
        }
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'))
            .expect("N on system QuickActions opens add modal");
        assert_eq!(modal.id.0, "system.quick_action.new");
        assert_eq!(modal.fields.len(), 5);
    }

    /// `N` on the System tab with Services focused is a no-op (services are
    /// not stored).
    #[test]
    fn system_modal_for_key_returns_none_when_services_focused() {
        let mut sid_app = build_test_sid_app(Some("system"));
        if let Some(w) = sid_app
            .app
            .tabs_mut()
            .active_mut()
            .layout
            .iter_widgets_mut()
            .next()
        {
            let any_ref = w as &mut dyn std::any::Any;
            if let Some(sw) = any_ref.downcast_mut::<SystemWidget>() {
                sw.state_mut().cycle_focus_forward(); // → Services
            }
        }
        assert!(modal_for_active_tab_key(&sid_app, plain_chord('N')).is_none());
    }

    /// Submitting `system.quick_action.new` persists the action AND adds it
    /// to the global palette when scope=Global.
    #[test]
    fn system_quick_action_new_submit_persists_and_rehydrates_palette() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        let id = ModalId("system.quick_action.new".to_string());
        let values = vec![
            ("id".to_string(), FieldValue::Text("qa-reload".into())),
            ("label".to_string(), FieldValue::Text("Reload".into())),
            ("command".to_string(), FieldValue::Text("sid reload".into())),
            ("scope".to_string(), FieldValue::Choice("Global".into())),
            ("keybind".to_string(), FieldValue::Text(String::new())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let actions = sid_app.store.list_quick_actions().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "qa-reload");
        // Palette contains the new entry.
        assert!(!sid_app.app.actions().fuzzy("Reload").is_empty());
    }

    /// `system.quick_action.new` requires id, label, and command.
    #[test]
    fn system_quick_action_new_rejects_missing_fields() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        let id = ModalId("system.quick_action.new".to_string());
        let values = vec![
            ("id".to_string(), FieldValue::Text("qa-x".into())),
            ("label".to_string(), FieldValue::Text(String::new())),
            ("command".to_string(), FieldValue::Text("echo".into())),
            ("scope".to_string(), FieldValue::Choice("Global".into())),
            ("keybind".to_string(), FieldValue::Text(String::new())),
        ];
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        assert!(err.to_string().contains("label"));
    }

    /// "Yes, remove" on `system.remove_quick_action:<id>` deletes the action.
    #[test]
    fn system_quick_action_remove_yes_deletes() {
        use sid_store::{QuickAction, QuickActionScope};
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        sid_app
            .store
            .upsert_quick_action(&QuickAction {
                id: "qa-bye".into(),
                label: "Goodbye".into(),
                cmd: "echo bye".into(),
                keybind: None,
                scope: QuickActionScope::Global,
            })
            .unwrap();
        let id = ModalId("system.remove_quick_action:qa-bye".to_string());
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("Yes, remove".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert!(sid_app.store.list_quick_actions().unwrap().is_empty());
    }

    /// "No, cancel" on `system.remove_quick_action:<id>` keeps the action.
    #[test]
    fn system_quick_action_remove_cancel_does_not_delete() {
        use sid_store::{QuickAction, QuickActionScope};
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("system"));
        sid_app
            .store
            .upsert_quick_action(&QuickAction {
                id: "qa-keep".into(),
                label: "Keep".into(),
                cmd: "echo keep".into(),
                keybind: None,
                scope: QuickActionScope::Global,
            })
            .unwrap();
        let id = ModalId("system.remove_quick_action:qa-keep".to_string());
        let values = vec![(
            "confirm".to_string(),
            FieldValue::Choice("No, cancel".into()),
        )];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert_eq!(sid_app.store.list_quick_actions().unwrap().len(), 1);
    }

    // ─── Live Network data + toasts + async jobs ──────────────────────────────

    use sid_core::adapters::sys::{
        ListeningPort, NetInterface, Pid as SysPid, ProcessInfo, Protocol, Signal, SocketState,
        SysError, SysProvider,
    };
    use sid_core::sys_probe::{SysProbe, SysSnapshot};
    use std::sync::Mutex as StdMutex;

    /// Trivial provider returning fixed, non-empty data on every call so
    /// snapshots arriving at the widget are detectable.
    struct StubSysProvider;
    impl SysProvider for StubSysProvider {
        fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
            Ok(vec![ProcessInfo {
                pid: SysPid::from_u32(1),
                name: "init".into(),
                cmd: "/sbin/init".into(),
                cpu_pct: 0.0,
                rss_bytes: 0,
                started_unix_secs: 0,
                parent: None,
                user: Some("0".into()),
            }])
        }
        fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
            Ok(vec![ListeningPort {
                port: 22,
                pid: Some(SysPid::from_u32(1)),
                command: "sshd".into(),
                protocol: Protocol::Tcp,
                state: SocketState::Listen,
                local_addr: "0.0.0.0".into(),
            }])
        }
        fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
            Ok(vec![NetInterface {
                name: "lo".into(),
                addrs: vec!["127.0.0.1".into()],
                rx_bytes: 0,
                tx_bytes: 0,
                is_up: true,
            }])
        }
        fn kill_process(&mut self, _: SysPid, _: Signal) -> Result<(), SysError> {
            Ok(())
        }
    }

    fn fixed_snapshot() -> SysSnapshot {
        SysSnapshot {
            processes: vec![ProcessInfo {
                pid: SysPid::from_u32(42),
                name: "fixture".into(),
                cmd: "/usr/bin/fixture".into(),
                cpu_pct: 0.0,
                rss_bytes: 0,
                started_unix_secs: 0,
                parent: None,
                user: None,
            }],
            listening_ports: vec![ListeningPort {
                port: 1234,
                pid: Some(SysPid::from_u32(42)),
                command: "fixture".into(),
                protocol: Protocol::Tcp,
                state: SocketState::Listen,
                local_addr: "0.0.0.0".into(),
            }],
            interfaces: vec![NetInterface {
                name: "eth0".into(),
                addrs: vec!["10.0.0.1".into()],
                rx_bytes: 0,
                tx_bytes: 0,
                is_up: true,
            }],
            captured_at_unix_secs: 1,
        }
    }

    /// `drain_sys_snapshots` forwards each broadcast snapshot to the Network
    /// widget. After a single tick the widget's three tables reflect the
    /// snapshot. This is the integration that makes the Network tab show data.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn network_widget_consumes_broadcast_snapshot() {
        let provider: Arc<StdMutex<dyn SysProvider>> = Arc::new(StdMutex::new(StubSysProvider));
        let probe = Arc::new(SysProbe::new(provider, Duration::from_millis(50)));
        let rx = probe.subscribe();

        // Spawn the probe run-loop on the test runtime; paused time lets the
        // first interval tick fire deterministically.
        let probe_for_task = Arc::clone(&probe);
        let handle = tokio::spawn(async move { probe_for_task.run().await });

        // Wait for the first snapshot to arrive in the receiver.
        let mut rx = rx;
        let first = rx.recv().await.expect("first snapshot");
        assert_eq!(first.listening_ports.len(), 1);

        // Build a SidApp pointed at the Network tab, then attach the receiver
        // we drained from above. Replace its first slot with a fresh receiver
        // — we already consumed one snapshot so simulate by handing the
        // existing `rx` over to `sid_app.sys_rx` for the drain test.
        let mut sid_app = build_test_sid_app(Some("network"));
        sid_app.sys_rx = Some(rx);

        // Manually trigger one drain to forward whatever the channel holds.
        // Inject a snapshot through the broadcast so the drain has something
        // to take. (We can't easily re-publish; rely on the next probe tick.)
        tokio::time::sleep(Duration::from_millis(60)).await;
        drain_sys_snapshots(&mut sid_app);

        // Network widget should now have rows.
        let layout = &sid_app.app.tabs().active().layout;
        let widget = layout
            .iter_widgets()
            .next()
            .expect("network tab has widget");
        let net = widget
            .as_any()
            .downcast_ref::<NetworkWidget>()
            .expect("network downcast");
        assert!(
            !net.ports().rows().is_empty(),
            "ports table should be populated after drain"
        );
        assert!(
            !net.processes().rows().is_empty(),
            "processes table should be populated after drain"
        );
        assert!(
            !net.interfaces().rows().is_empty(),
            "interfaces sidebar should be populated after drain"
        );

        handle.abort();
    }

    /// `refresh_network_widget` is a no-op when the active tab manager has
    /// no Network tab (e.g., a custom in-memory `TabManager` fixture). It
    /// also forwards into the widget exactly when one is present.
    #[test]
    fn refresh_network_widget_applies_snapshot_when_present() {
        let mut sid_app = build_test_sid_app(Some("network"));
        let snap = fixed_snapshot();
        refresh_network_widget(&mut sid_app, snap);
        let widget = sid_app
            .app
            .tabs()
            .active()
            .layout
            .iter_widgets()
            .next()
            .unwrap();
        let net = widget.as_any().downcast_ref::<NetworkWidget>().unwrap();
        assert_eq!(net.ports().rows().len(), 1);
        assert_eq!(net.processes().rows().len(), 1);
        assert_eq!(net.interfaces().rows().len(), 1);
    }

    /// `drain_sys_snapshots` handles the `Lagged` case without panicking and
    /// recovers by continuing to drain. The broadcast channel buffer in
    /// `SysProbe` is 16; flooding past that and draining must not panic.
    #[tokio::test(flavor = "current_thread")]
    async fn network_widget_handles_lagged_receiver_gracefully() {
        let provider: Arc<StdMutex<dyn SysProvider>> = Arc::new(StdMutex::new(StubSysProvider));
        let probe = Arc::new(SysProbe::new(provider, Duration::from_millis(50)));
        let rx = probe.subscribe();

        // Spawn the probe so the broadcast channel fills naturally with the
        // probe's emitted snapshots; the buffer (16) will overflow if the
        // subscriber doesn't drain.
        let probe_for_task = Arc::clone(&probe);
        let handle = tokio::spawn(async move { probe_for_task.run().await });

        // Give the probe time to emit a few snapshots.
        tokio::time::sleep(Duration::from_millis(120)).await;

        let mut sid_app = build_test_sid_app(Some("network"));
        sid_app.sys_rx = Some(rx);
        // Drain must not panic regardless of channel state.
        drain_sys_snapshots(&mut sid_app);

        handle.abort();
    }

    /// `dispatch_modal_submit` on `workspaces.new` with a valid path pushes
    /// a success toast that mentions the new workspace's name.
    #[test]
    fn dispatch_workspaces_new_pushes_success_toast() {
        use sid_widgets::{FieldValue, ModalId};
        let dir = tempdir().unwrap();
        std::mem::forget(dir);
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let abs = std::env::temp_dir();
        let id = ModalId("workspaces.new".to_string());
        let values = vec![
            ("name".into(), FieldValue::Text("acme".into())),
            (
                "path".into(),
                FieldValue::Picker(abs.to_string_lossy().to_string()),
            ),
            ("kind".into(), FieldValue::Choice("Repo".into())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let messages: Vec<String> = sid_app.toasts.iter().map(|t| t.message.clone()).collect();
        assert!(
            messages.iter().any(|m| m.contains("acme")),
            "expected a toast mentioning 'acme'; got: {messages:?}"
        );
        let kinds: Vec<crate::toast::ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert!(
            kinds.contains(&crate::toast::ToastKind::Success),
            "expected at least one Success toast; got: {kinds:?}"
        );
    }

    /// A validation failure (empty name + path) yields an Error toast in the
    /// drain stage (the binding happens in `drain_pending_submits`, not in
    /// `dispatch_modal_submit` itself; we exercise the queue indirectly).
    #[test]
    fn dispatch_workspaces_new_pushes_error_toast_on_validation_failure() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        sid_app.pending_submits.push((
            ModalId("workspaces.new".to_string()),
            vec![
                ("name".into(), FieldValue::Text(String::new())),
                ("path".into(), FieldValue::Picker(String::new())),
                ("kind".into(), FieldValue::Choice("Repo".into())),
            ],
        ));
        drain_pending_submits(&mut sid_app);
        let kinds: Vec<crate::toast::ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert!(
            kinds.contains(&crate::toast::ToastKind::Error),
            "expected an Error toast on validation failure; got: {kinds:?}"
        );
    }

    /// A completed `JobOutcome::Success` is converted into a Success toast by
    /// `drain_job_outcomes`. We bypass `tokio::spawn` and inject directly via
    /// `JobQueue::spawn` with a ready future.
    #[tokio::test(flavor = "current_thread")]
    async fn async_job_completion_pushes_success_outcome_toast() {
        let mut sid_app = build_test_sid_app(None);
        let outcome = JobOutcome::Success {
            label: "ssh-copy-id".into(),
            message: "copied key to acme".into(),
        };
        // Spawn a tokio task that pushes the outcome; we hand the queue to
        // the task via clone of the Arc.
        let jobs = Arc::clone(&sid_app.jobs);
        jobs.spawn(async move { outcome });
        // Yield until the spawned task gets to run.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        drain_job_outcomes(&mut sid_app);
        let messages: Vec<String> = sid_app.toasts.iter().map(|t| t.message.clone()).collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("ssh-copy-id") && m.contains("acme")),
            "expected a toast mentioning ssh-copy-id + acme; got: {messages:?}"
        );
        let kinds: Vec<crate::toast::ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&crate::toast::ToastKind::Success));
    }

    /// A completed `JobOutcome::Failure` is converted into an Error toast.
    #[tokio::test(flavor = "current_thread")]
    async fn async_job_completion_pushes_error_outcome_toast() {
        let mut sid_app = build_test_sid_app(None);
        let outcome = JobOutcome::Failure {
            label: "ssh-copy-id".into(),
            message: "permission denied".into(),
        };
        let jobs = Arc::clone(&sid_app.jobs);
        jobs.spawn(async move { outcome });
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        drain_job_outcomes(&mut sid_app);
        let kinds: Vec<crate::toast::ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert!(
            kinds.contains(&crate::toast::ToastKind::Error),
            "expected an Error toast; got: {kinds:?}"
        );
    }

    /// `submit_ssh_debug` pushes a `Toast::Info` synchronously when an
    /// asynchronous action is dispatched. The actual subprocess result is
    /// surfaced later via the job queue; this test only asserts the immediate
    /// info-toast feedback that tells the user something is running.
    #[tokio::test(flavor = "current_thread")]
    async fn async_ssh_debug_pushes_running_toast_immediately() {
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let values = vec![(
            "action".to_string(),
            FieldValue::Choice("Show identity diagnostics".into()),
        )];
        submit_ssh_debug(&mut sid_app, "alias-x", &values).unwrap();
        let messages: Vec<String> = sid_app.toasts.iter().map(|t| t.message.clone()).collect();
        assert!(
            messages.iter().any(|m| m.contains("ssh-add -l")),
            "expected a running-toast mentioning ssh-add -l; got: {messages:?}"
        );
        let kinds: Vec<crate::toast::ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert!(
            kinds.contains(&crate::toast::ToastKind::Info),
            "expected an Info toast on submit; got: {kinds:?}"
        );
    }

    /// `dispatch_modal_submit` on `database.new` with valid fields pushes a
    /// success toast whose message contains the new connection id.
    #[test]
    fn dispatch_database_new_pushes_success_toast() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("database"));
        let id = ModalId("database.new".to_string());
        let values = vec![
            ("id".into(), FieldValue::Text("prod-pg".into())),
            ("name".into(), FieldValue::Text("Prod".into())),
            ("kind".into(), FieldValue::Choice("SQLite".into())),
            ("dsn".into(), FieldValue::Text(":memory:".into())),
            ("password".into(), FieldValue::Password(String::new())),
        ];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        let messages: Vec<String> = sid_app.toasts.iter().map(|t| t.message.clone()).collect();
        assert!(
            messages.iter().any(|m| m.contains("prod-pg")),
            "expected toast mentioning 'prod-pg'; got: {messages:?}"
        );
    }
}
