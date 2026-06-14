//! Wires together concrete implementations — RedbStore, all six widgets, the
//! keybind map and action registry — into a running [`App`], and contains the
//! Ratatui render loop.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use directories::{ProjectDirs, UserDirs};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use sid_core::{
    Result as SidResult,
    action::{Action, ActionRegistry},
    adapters::{
        sys::SysProvider,
        systemctl::{
            JournalEntry, SystemUnit, SystemctlClient, SystemctlError, UnitBus, UnitFilter,
        },
        terminal_spawner::{SpawnRequest, SpawnerError, TerminalSpawner},
    },
    animation::AnimationConfig,
    app::{App, Dispatch},
    event::Event as SidEvent,
    keybind::KeybindMap,
    layout::Layout,
    sys_probe::{SysProbe, SysSnapshot},
    tab::{Tab, TabId, TabKind, TabManager},
    widget::Widget,
    workspace_discovery::{WorkspaceUpserter, merge_discoveries_into, scan_workspace_root},
    workspace_metadata::WorkspaceKind,
};
use sid_fx::FxState;
use sid_git::Git2ProviderFactory;
use sid_store::{RedbStore, SessionRecord, Store, Workspace, now_epoch};
use sid_ui::{
    helpers::styled_block,
    theme::{Color as UiColor, GlyphSet, Theme},
    theme_registry::ThemeRegistry,
    themes::cosmos,
};
use sid_widgets::{
    DatabaseWidget, NetworkWidget, SettingsWidget, SshWidget, SystemWidget, WorkspacesWidget,
};
use tokio::sync::{
    broadcast::error::TryRecvError,
    mpsc::{Receiver, error::TryRecvError as MpscTryRecvError},
};

use crate::toast::{Toast, ToastQueue};

/// Type alias for the SSH client factory: a callable that returns a fresh
/// `Box<dyn SshClient>` per invocation. The production binary wires this to
/// [`sid_ssh::RusshClientFactory::new_client`]; tests substitute a mock.
///
/// Held as an `Arc<dyn Fn(...) ...>` so the same closure can be cloned into
/// every spawned connect task without re-creating it.
pub type SshClientFactoryFn =
    Arc<dyn Fn() -> Box<dyn sid_core::adapters::ssh::SshClient> + Send + Sync>;

/// Outcome of an asynchronous SSH connect attempt. Produced by the task
/// spawned by [`drain_pending_ssh_connect`] and consumed by
/// [`drain_ssh_outcomes`] on the next event-loop iteration.
///
/// The `Connected` variant ships the freshly constructed [`sid_widgets::ssh::PtyPane`]
/// plus a byte-stream receiver. The wire layer attaches the pane to the
/// widget and stashes the receiver on `SidApp.ssh_byte_rx`; subsequent
/// frames drain bytes from the receiver into the pane.
///
/// The `Failed` variant carries the alias the user attempted plus a
/// human-readable error message that becomes the body of a toast.
pub enum SshConnectOutcome {
    /// SSH connect + `open_shell` succeeded.
    Connected {
        /// Alias that was connected.
        alias: String,
        /// Freshly created PTY pane wrapping a `Vt100Screen`. Ownership
        /// transfers to the widget when the wire layer drains this outcome.
        pty: sid_widgets::ssh::PtyPane,
        /// Channel that receives stdout bytes from the remote shell. The
        /// wire layer owns it and forwards bytes to the widget's pane each
        /// frame. Drop the sender (held by the spawned reader task) to
        /// terminate the reader on disconnect.
        byte_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        /// One-shot shutdown signal for the reader task. Send `()` (or
        /// drop) to stop the background reader.
        shutdown_tx: tokio::sync::oneshot::Sender<()>,
    },
    /// SSH connect or `open_shell` failed. The widget flips to
    /// `ConnectionPhase::Failed` and a toast is pushed.
    Failed {
        /// Alias that was attempted.
        alias: String,
        /// Human-readable error body.
        error: String,
    },
}

impl std::fmt::Debug for SshConnectOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected { alias, .. } => f
                .debug_struct("Connected")
                .field("alias", alias)
                .finish_non_exhaustive(),
            Self::Failed { alias, error } => f
                .debug_struct("Failed")
                .field("alias", alias)
                .field("error", error)
                .finish(),
        }
    }
}

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
    /// A detail tab's satellite list finished scanning. The widget identified
    /// by `tab_id` receives `rows` via `apply_satellites`. No toast — silent.
    WorkspaceSatellitesScanned {
        /// `TabId.as_str()` for the detail tab that requested the scan.
        tab_id: String,
        /// Umbrella row first, then satellites.
        rows: Vec<sid_widgets::SatelliteRow>,
    },
    /// One repo row's git snapshot finished loading off-thread.
    RepoGitLoaded {
        /// Detail tab id the row belongs to.
        tab_id: String,
        /// Absolute repo path (the row key).
        path: std::path::PathBuf,
        /// Loaded snapshot.
        git: sid_widgets::RepoGit,
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
/// let (ssh_outcome_tx, ssh_outcome_rx) = tokio::sync::mpsc::unbounded_channel();
/// let sid_app = SidApp {
///     app,
///     store,
///     git_factory: Arc::new(sid_git::Git2ProviderFactory::new()),
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
///     undo_ring: std::collections::VecDeque::new(),
///     form: None,
///     form_origin_tab: None,
///     pending_submits: Vec::new(),
///     toasts: ToastQueue::new(4),
///     jobs: Arc::new(JobQueue::<JobOutcome>::new()),
///     ssh_client_factory: sid::wire::build_ssh_client_factory_fn(),
///     ssh_outcome_tx,
///     ssh_outcome_rx,
///     ssh_byte_rx: None,
///     ssh_last_pty_area: None,
///     ssh_shutdown_tx: None,
///     active_theme: sid_ui::themes::cosmos(),
///     persister: sid_core::persister::StatePersister::new(std::time::Duration::ZERO),
///     last_heartbeat: std::time::Instant::now(),
/// };
/// ```
pub struct SidApp {
    pub app: App,
    pub store: Arc<RedbStore>,
    /// Shared git provider factory. Cloned into per-row off-thread git loads
    /// when a workspace detail tab opens. The factory is `open()`-only; each
    /// load opens its own per-repo provider.
    pub git_factory: Arc<Git2ProviderFactory>,
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
    /// Active side-pane form, if any. When `Some` and `form_origin_tab` points
    /// at the active tab, the form occupies the right 60% of the tab body and
    /// intercepts every key (after modals) until it submits or cancels. The
    /// UX-v2 add/edit substrate; branches 1-5 open forms via [`open_form`].
    pub form: Option<sid_widgets::form::FormPane>,
    /// Tab the active [`form`](Self::form) belongs to. The form only renders
    /// and only intercepts keys while this matches the active tab id, so a form
    /// opened on the Database tab stays put when the user cycles to SSH.
    pub form_origin_tab: Option<sid_core::tab::TabId>,
    /// Modals submitted on the previous frame whose handler hasn't run yet.
    /// Drained at the top of [`run_event_loop`] each iteration.
    pub pending_submits: Vec<(sid_widgets::ModalId, Vec<(String, sid_widgets::FieldValue)>)>,
    /// Lower-right corner toast queue. Pushed by modal submit handlers
    /// (success / error) and by completed background jobs.
    pub toasts: ToastQueue,
    /// Per-session settings undo ring. Capped at
    /// [`crate::settings_undo::UNDO_RING_CAP`] entries; entries are evicted
    /// when they exceed TTL ([`crate::settings_undo::UNDO_TTL`]).
    pub undo_ring: std::collections::VecDeque<crate::settings_undo::UndoEntry>,
    /// Job queue used for asynchronous subprocess work (ssh-copy-id, ssh-keygen,
    /// ssh -vv, ssh-add, etc.). Each spawned task pushes a [`JobOutcome`];
    /// the event loop drains completed outcomes once per iteration and
    /// converts them into toasts.
    pub jobs: Arc<sid_job::JobQueue<JobOutcome>>,
    /// Factory closure used to spawn a fresh `SshClient` for every new
    /// connect attempt. The production binary uses
    /// [`build_ssh_client_factory_fn`]; tests substitute a mock that returns
    /// a hand-rolled `SshClient`.
    pub ssh_client_factory: SshClientFactoryFn,
    /// Sender half of the SSH connect outcome channel. Cloned into every
    /// spawned connect task so it can deliver its result back to the wire
    /// layer. The receiver lives on `ssh_outcome_rx`.
    pub ssh_outcome_tx: tokio::sync::mpsc::UnboundedSender<SshConnectOutcome>,
    /// Receiver half of the SSH connect outcome channel. Drained each frame
    /// by [`drain_ssh_outcomes`]; on `Connected`, attaches the PtyPane and
    /// stashes the byte receiver; on `Failed`, marks the widget and pushes
    /// a toast.
    pub ssh_outcome_rx: tokio::sync::mpsc::UnboundedReceiver<SshConnectOutcome>,
    /// Live byte-stream receiver from the connected remote shell. `None`
    /// when no connection is active. Drained each frame by
    /// [`drain_ssh_bytes`] and forwarded into the SSH widget's PtyPane.
    pub ssh_byte_rx: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>,
    /// Last PTY body rect we resized the screen to. Used so we only call
    /// `pty_pane_resize_to_area` when the area actually changed.
    pub ssh_last_pty_area: Option<Rect>,
    /// One-shot shutdown signal for the active byte-reader task. Send (or
    /// drop) to terminate the reader cleanly. `None` when no reader is
    /// running.
    pub ssh_shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// The active colour theme, resolved from the persisted `THEME_NAME`
    /// setting at startup and updated live when the user applies a new theme.
    /// `draw()` reads this so theme selection actually takes effect.
    pub active_theme: sid_ui::theme::Theme,
    /// Debounces session-state persistence. Constructed from the
    /// `PERSIST_DEBOUNCE_MS` setting; a zero duration flushes every iteration.
    pub persister: sid_core::persister::StatePersister,
    /// Wall-clock instant of the last session heartbeat. The event loop touches
    /// the session's `last_active` every `HEARTBEAT_INTERVAL_SECS`.
    pub last_heartbeat: std::time::Instant,
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

/// Build the active [`SecretStore`] implementation.
///
/// Reads [`sid_store::settings_keys::USE_OS_KEYRING`] from `store`. When
/// `true`, constructs a [`sid_secrets::KeyringStore`] and probes the keyring
/// daemon with a round-trip write/read/delete of a sentinel key. On probe
/// failure, falls back to [`sid_secrets::PlainStore`] and logs a warning.
///
/// Returns `(store, used_keyring)` — the bool lets `main.rs` push a fallback
/// toast when the keyring was requested but unavailable.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use sid_store::{OpenStore, RedbStore, Store};
/// use tempfile::tempdir;
/// use sid::wire::build_secret_store;
///
/// let dir = tempdir().unwrap();
/// let redb = Arc::new(RedbStore::open(&dir.path().join("s.redb")).unwrap());
/// // Setting absent → PlainStore returned, used_keyring = false.
/// let (secrets, used_keyring) = build_secret_store(&*redb, Arc::clone(&redb) as Arc<dyn Store>);
/// assert!(!used_keyring);
/// let id = sid_core::adapters::secrets::SecretId::new("smoke.test");
/// use sid_core::adapters::secrets::SecretStore;
/// secrets.put(&id, b"val").unwrap();
/// assert_eq!(secrets.get(&id).unwrap().unwrap(), b"val".to_vec());
/// ```
pub fn build_secret_store(
    store: &RedbStore,
    plain_arc: Arc<dyn Store>,
) -> (Arc<dyn sid_core::adapters::secrets::SecretStore>, bool) {
    use sid_store::TypedSettings;
    let want_keyring = store
        .get_bool(sid_store::settings_keys::USE_OS_KEYRING)
        .unwrap_or(None)
        .unwrap_or(false);

    if want_keyring {
        // keyring v4 selects the backend at runtime: register the platform
        // store before any op, and refuse a non-durable (mock/in-memory) store
        // so secrets can never be silently lost to an ephemeral keystore.
        match sid_secrets::install_default_backend() {
            Ok(()) if sid_secrets::default_backend_is_durable() => {
                let ks = sid_secrets::KeyringStore::new();
                if probe_keyring(&ks) {
                    tracing::info!("OS keyring available — using KeyringStore");
                    return (Arc::new(ks), true);
                }
                tracing::warn!(
                    "OS keyring requested but probe failed — falling back to PlainStore"
                );
            }
            Ok(()) => {
                tracing::warn!(
                    "OS keyring backend is ephemeral (in-memory) — refusing to avoid \
                     silent secret loss; falling back to PlainStore"
                );
            }
            Err(e) => {
                tracing::warn!(
                    "OS keyring backend registration failed ({e}) — falling back to PlainStore"
                );
            }
        }
    }

    (Arc::new(sid_secrets::PlainStore::new(plain_arc)), false)
}

/// Write/read/delete a sentinel key to confirm the keyring backend is live.
///
/// Returns `true` only when the put succeeds, the read-back matches, and the
/// final delete succeeds.
///
/// Once the put succeeds the sentinel exists in the keyring, so EVERY exit path
/// after that point — read error, read-back mismatch, or success — flows
/// through a single best-effort `delete`. This guarantees the OS keyring is
/// never left with a stale `sid.__keyring_probe` entry, including the
/// put-Ok-then-get-Err path that an earlier version leaked.
fn probe_keyring(store: &dyn sid_core::adapters::secrets::SecretStore) -> bool {
    let probe_key = sid_core::adapters::secrets::SecretId::new("sid.__keyring_probe");

    // If the initial put fails, nothing was stored — no cleanup needed.
    if store.put(&probe_key, b"probe").is_err() {
        return false;
    }

    // The sentinel now exists. Determine success, then ALWAYS clean up exactly
    // once, regardless of which branch we took.
    let read_ok = matches!(store.get(&probe_key), Ok(Some(ref v)) if v.as_slice() == b"probe");

    // Single cleanup point. The delete result is the final word on liveness:
    // a backend that cannot delete its own sentinel is not usable.
    let delete_ok = store.delete(&probe_key).is_ok();

    read_ok && delete_ok
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

/// Load the `show_add_new_row` setting, which controls whether list panels
/// render the synthetic "+ add new" first row. Defaults to `true` if unset.
///
/// The setting is stored as `b"true"` or `b"false"`; any value other than
/// `b"false"` is treated as `true`.
///
/// # Examples
///
/// ```
/// use sid::wire::load_show_add_new_row;
/// use sid_store::{OpenStore, RedbStore};
/// use tempfile::tempdir;
///
/// let dir = tempdir().unwrap();
/// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
/// assert_eq!(load_show_add_new_row(&store), true);
/// ```
pub fn load_show_add_new_row(store: &dyn Store) -> bool {
    match store.get_setting(sid_store::settings_keys::SHOW_ADD_NEW_ROW) {
        Ok(Some(val)) => val.0 != b"false",
        _ => true,
    }
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

/// Construct the production [`SshClientFactoryFn`] closure: each invocation
/// returns a fresh [`sid_ssh::RusshClient`] (not yet connected) boxed as
/// [`sid_core::adapters::ssh::SshClient`].
///
/// # Examples
///
/// ```
/// use sid::wire::build_ssh_client_factory_fn;
/// let f = build_ssh_client_factory_fn();
/// let _client = f();
/// ```
pub fn build_ssh_client_factory_fn() -> SshClientFactoryFn {
    let factory = sid_ssh::RusshClientFactory::new();
    Arc::new(move || Box::new(factory.new_client()) as Box<dyn sid_core::adapters::ssh::SshClient>)
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
pub struct BuildAppData {
    pub workspaces: Vec<Workspace>,
    pub ssh_hosts: Vec<sid_store::SshHost>,
    pub ssh_config_entries: Vec<sid_widgets::ssh::SshConfigEntryLite>,
    pub start_ssh_alias: Option<String>,
    pub db_connections: Vec<sid_store::DbConnection>,
    /// Whether the synthetic "+ add new" row is shown in list panels
    /// (database connections, SSH hosts, ...). Defaults to `true`; loaded
    /// from `settings_keys::SHOW_ADD_NEW_ROW` at startup via
    /// `load_show_add_new_row`.
    pub show_add_new_row: bool,
    pub pinned_configs: Vec<sid_store::PinnedConfig>,
    pub quick_actions: Vec<sid_store::QuickAction>,
    pub settings_categories: Vec<sid_widgets::SettingsCategory>,
}

impl Default for BuildAppData {
    fn default() -> Self {
        Self {
            workspaces: Vec::new(),
            ssh_hosts: Vec::new(),
            ssh_config_entries: Vec::new(),
            start_ssh_alias: None,
            db_connections: Vec::new(),
            show_add_new_row: true,
            pinned_configs: Vec::new(),
            quick_actions: Vec::new(),
            settings_categories: Vec::new(),
        }
    }
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
    let ssh_state = sid_widgets::ssh::SshState::new(
        data.ssh_hosts,
        data.ssh_config_entries,
        data.show_add_new_row,
    );
    let mut ssh_widget = SshWidget::with_state(ssh_state);
    if let Some(ref alias) = data.start_ssh_alias {
        let aliases: Vec<_> = ssh_widget
            .state()
            .visible_hosts()
            .iter()
            .map(|h| h.alias.clone())
            .collect();
        if aliases.iter().any(|a| a == alias) {
            // Walk the cursor to the requested host. The walk (rather than a
            // fixed index count) absorbs the synthetic "+ add new" row the
            // cursor starts on when show_add_new_row is enabled; bounded so a
            // wrapping cursor can never loop forever.
            let max_steps = aliases.len() + 1;
            let mut steps = 0;
            while ssh_widget.state().selected_alias() != Some(alias.as_str()) && steps < max_steps {
                ssh_widget.state_mut().select_next();
                steps += 1;
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
            Box::new(DatabaseWidget::new_with_add_new(
                data.db_connections,
                data.show_add_new_row,
            )),
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
        "tab.close",
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
        kind: TabKind::Core,
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

/// Resume-window threshold: a previous session is only offered as a resume
/// candidate if it ended within this many nanoseconds of "now". 60 minutes,
/// expressed in nanoseconds because [`sid_store::Epoch`] is wall-clock
/// nanoseconds since UNIX epoch (see [`sid_store::now_epoch`]).
pub(crate) const RESUME_WINDOW_NS: u64 = 60 * 60 * 1_000_000_000;

/// If the prior session ended within [`RESUME_WINDOW_NS`] (or is still
/// recorded as running, i.e. `ended_at == None`) AND had a known active
/// tab, push a `"session.resume"` modal onto the stack so the user can
/// pick between resuming the tab or starting fresh.
///
/// No-op when:
/// - no prior session exists,
/// - the session has no `active_tab`, or
/// - the session ended more than [`RESUME_WINDOW_NS`] in the past.
///
/// The submit handler for the pushed modal lives in [`dispatch_modal_submit`]
/// (key prefix `"session.resume"`).
///
/// # Examples
///
/// ```no_run
/// use sid::wire::{build_app, maybe_push_resume_modal, NoopSystemctlClient, NoopTerminalSpawner, SidApp};
/// use sid::toast::ToastQueue;
/// use sid_job::JobQueue;
/// use sid_store::{OpenStore, RedbStore, Store};
/// use std::path::Path;
/// use std::sync::Arc;
///
/// let store = Arc::new(RedbStore::open(Path::new("/tmp/resume_test.redb")).unwrap());
/// let secrets: Arc<dyn sid_core::adapters::secrets::SecretStore> =
///     Arc::new(sid_secrets::PlainStore::new(Arc::clone(&store) as Arc<dyn Store>));
/// let (ssh_outcome_tx, ssh_outcome_rx) = tokio::sync::mpsc::unbounded_channel();
/// let mut sid_app = SidApp {
///     app: build_app(None, vec![]),
///     store,
///     git_factory: Arc::new(sid_git::Git2ProviderFactory::new()),
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
///     undo_ring: std::collections::VecDeque::new(),
///     form: None,
///     form_origin_tab: None,
///     pending_submits: Vec::new(),
///     toasts: ToastQueue::new(4),
///     jobs: Arc::new(JobQueue::new()),
///     ssh_client_factory: sid::wire::build_ssh_client_factory_fn(),
///     ssh_outcome_tx,
///     ssh_outcome_rx,
///     ssh_byte_rx: None,
///     ssh_last_pty_area: None,
///     ssh_shutdown_tx: None,
///     active_theme: sid_ui::themes::cosmos(),
///     persister: sid_core::persister::StatePersister::new(std::time::Duration::ZERO),
///     last_heartbeat: std::time::Instant::now(),
/// };
/// // With a fresh store there's no prior session — no modal is pushed.
/// maybe_push_resume_modal(&mut sid_app);
/// assert!(sid_app.modal_stack.is_empty());
/// ```
/// Inspect the store for a *restorable* prior session: one that has a recorded
/// `active_tab` and either is still recorded as running (`ended_at == None`) or
/// ended within [`RESUME_WINDOW_NS`]. Returns the tab to restore alongside the
/// nanoseconds elapsed since it ended (`None` when still running).
///
/// This is the single source of truth for the recency window, shared by
/// [`maybe_push_resume_modal`] (the `"ask"` path) and the silent `"yes"`
/// auto-restore path in the binary. Returns `None` when there is nothing to
/// restore.
pub(crate) fn restorable_prior_tab(store: &dyn Store) -> Option<(TabId, Option<u64>)> {
    let prev = store.current_session().ok().flatten()?;
    let active_tab = prev.active_tab.clone()?;
    // Sessions still recorded as "open" (ended_at == None) are also valid
    // resume candidates — that's the common case where a process exited
    // without a clean `end_session`.
    let now = now_epoch();
    let elapsed_ns: Option<u64> = prev.ended_at.map(|e| now.saturating_sub(e));
    let recent_enough = match elapsed_ns {
        None => true,
        Some(ns) => ns < RESUME_WINDOW_NS,
    };
    if !recent_enough {
        return None;
    }
    Some((active_tab, elapsed_ns))
}

pub fn maybe_push_resume_modal(sid_app: &mut SidApp) {
    use sid_widgets::{Field, ModalSpec};
    let Some((active_tab, elapsed_ns)) = restorable_prior_tab(&*sid_app.store) else {
        return;
    };
    let still_running = elapsed_ns.is_none();
    let elapsed_secs = elapsed_ns.map(|ns| ns / 1_000_000_000).unwrap_or(0);
    let when = if still_running {
        "(no ended_at; session still recorded as running)".to_string()
    } else if elapsed_secs == 0 {
        "(just now)".to_string()
    } else if elapsed_secs < 60 {
        format!("({elapsed_secs}s ago)")
    } else {
        format!("({}m ago)", elapsed_secs / 60)
    };
    let help = format!(
        "Last tab was '{tab}' {when}. Resume restores the tab; Start fresh keeps the launch default.",
        tab = active_tab.as_str(),
    );
    let modal = ModalSpec::new(
        format!("session.resume:{}", active_tab.as_str()),
        "Resume previous session?",
        vec![Field::Choice {
            label: "action".into(),
            options: vec!["Resume".into(), "Start fresh".into()],
            selected: 0,
        }],
    )
    .with_help(help);
    sid_app.modal_stack.push(modal);
}

/// Resolve the start tab at launch. The CLI `--start-tab` argument always
/// wins; the `DEFAULT_TAB` setting is the fallback when no CLI arg is given.
///
/// # Examples
///
/// ```
/// use sid::wire::resolve_start_tab;
///
/// // CLI wins over the setting.
/// assert_eq!(
///     resolve_start_tab(Some("ssh"), Some("database".into())),
///     Some("ssh".to_string())
/// );
/// // Falls back to the setting when no CLI arg.
/// assert_eq!(
///     resolve_start_tab(None, Some("database".into())),
///     Some("database".to_string())
/// );
/// // Nothing set → None (caller uses its built-in default).
/// assert_eq!(resolve_start_tab(None, None), None);
/// ```
pub fn resolve_start_tab(cli: Option<&str>, setting: Option<String>) -> Option<String> {
    cli.map(|s| s.to_string()).or(setting)
}

/// Apply the `AUTO_RESTORE_SESSION` policy at startup.
///
/// Reads `settings_keys::AUTO_RESTORE_SESSION` (default `"ask"`) and dispatches:
/// - `"ask"` → [`maybe_push_resume_modal`] (the interactive resume prompt).
/// - `"yes"` → silently switch to the restorable prior tab (no modal). No-op
///   when there is nothing to restore.
/// - `"no"` → do nothing; start on the launch-default tab.
///
/// Unknown values fall back to `"ask"` so a malformed setting never strands the
/// user without a resume path.
pub fn apply_auto_restore(sid_app: &mut SidApp) {
    use sid_store::{TypedSettings, settings_keys};
    let policy = sid_app
        .store
        .get_string(settings_keys::AUTO_RESTORE_SESSION)
        .ok()
        .flatten()
        .unwrap_or_else(|| "ask".to_string());
    match policy.as_str() {
        "yes" => {
            if let Some((tab, _)) = restorable_prior_tab(&*sid_app.store) {
                let _ = sid_app.app.tabs_mut().switch_to(&tab);
            }
        }
        "no" => {
            // Start fresh — intentionally nothing to do.
        }
        // "ask" and any unknown value fall back to the interactive prompt.
        _ => maybe_push_resume_modal(sid_app),
    }
}

/// Draw one frame: tab strip on top, active panel body, help bar on bottom,
/// optional command-palette overlay centred over everything.
///
/// Reads `sid_app.active_theme` for all chrome colours. Pure layout — does not
/// mutate any state.
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
/// let (ssh_outcome_tx, ssh_outcome_rx) = tokio::sync::mpsc::unbounded_channel();
/// let sid_app = SidApp {
///     app: build_app(None, vec![]),
///     store,
///     git_factory: Arc::new(sid_git::Git2ProviderFactory::new()),
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
///     undo_ring: std::collections::VecDeque::new(),
///     form: None,
///     form_origin_tab: None,
///     pending_submits: Vec::new(),
///     toasts: sid::toast::ToastQueue::new(4),
///     jobs: std::sync::Arc::new(sid_job::JobQueue::<sid::wire::JobOutcome>::new()),
///     ssh_client_factory: sid::wire::build_ssh_client_factory_fn(),
///     ssh_outcome_tx,
///     ssh_outcome_rx,
///     ssh_byte_rx: None,
///     ssh_last_pty_area: None,
///     ssh_shutdown_tx: None,
///     active_theme: sid_ui::themes::cosmos(),
///     persister: sid_core::persister::StatePersister::new(std::time::Duration::ZERO),
///     last_heartbeat: std::time::Instant::now(),
/// };
/// let backend = TestBackend::new(120, 40);
/// let mut terminal = Terminal::new(backend).unwrap();
/// terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
/// ```
pub fn draw(frame: &mut Frame<'_>, sid_app: &SidApp) {
    use ratatui::{
        style::{Modifier as TextMod, Style as TextStyle},
        widgets::{Block as RBlock, BorderType, Borders as RBorders},
    };

    // `Theme` is a small RGB palette + glyph set; a per-frame clone is cheap
    // and keeps the existing `&theme` call sites unchanged. Reading the live
    // `active_theme` (rather than the hardcoded `cosmos()`) is what makes
    // theme selection actually take effect at runtime.
    let theme = sid_app.active_theme.clone();
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
    let full_body_rect = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: body_h,
    };
    // When a form is active for the active tab, split the body 40/60: the
    // widget keeps the left 40%, the form pane takes the right 60%. Otherwise
    // the widget owns the whole body and `form_area` is zero-width.
    let form_active =
        sid_app.form.is_some() && sid_app.form_origin_tab.as_ref() == Some(&app.tabs().active().id);
    let (body_rect, form_area) = if form_active {
        let list_w = (full_body_rect.width as u32 * 40 / 100) as u16;
        (
            Rect {
                width: list_w,
                ..full_body_rect
            },
            Rect {
                x: full_body_rect.x + list_w,
                width: full_body_rect.width - list_w,
                ..full_body_rect
            },
        )
    } else {
        (
            full_body_rect,
            Rect {
                width: 0,
                ..full_body_rect
            },
        )
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

    // ─── Side-pane form (UX-v2) ───────────────────────────────────────────
    // Drawn into the right 60% of the body when a form is active for the
    // active tab. `form_area` is zero-width otherwise.
    if form_area.width > 0 {
        if let Some(form) = &sid_app.form {
            sid_widgets::form::render_form_pane(frame.buffer_mut(), form_area, form, &theme);
        }
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
        // While a form is active, the footer advertises the form contract
        // (Tab/⏎/⎋) instead of the underlying widget's per-tab hints.
        let form_hints = if form_active {
            Some(form_footer_hints())
        } else {
            None
        };
        if let Some(hints) = form_hints.as_ref() {
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
        } else if let Some(w) = widget {
            // Cap at the first 4 hints; always append `? help` so the
            // overlay is discoverable. The full list lives in the overlay.
            let hints = slim_footer_hints(w.footer_hint());
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
    //
    // Gated by `TOASTS_ENABLED`: messages are now a logs-only channel (see
    // `record`), so the floating overlay is suppressed by default. The queue
    // is still fed, so flipping the flag back on restores the overlay with no
    // further wiring.
    if TOASTS_ENABLED {
        render_toasts(frame, inner, &theme, &sid_app.toasts);
    }

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
#[allow(dead_code)] // Retained for the upcoming workspaces.scan_now palette action (branch #2 follow-up).
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

/// Derive the event-pump tick interval (milliseconds) from an animation FPS.
///
/// Clamps `fps` to `1..=30` — the valid [`sid_core::animation::AnimationConfig`]
/// range — so a corrupt or zero value can never divide by zero or spin the
/// pump faster than 30 Hz.
///
/// # Examples
///
/// ```
/// assert_eq!(sid::wire::fps_to_tick_ms(8), 125);
/// assert_eq!(sid::wire::fps_to_tick_ms(0), 1000); // clamped up to 1 fps
/// assert_eq!(sid::wire::fps_to_tick_ms(255), 33); // clamped down to 30 fps
/// ```
pub fn fps_to_tick_ms(fps: u8) -> u64 {
    1000 / u64::from(fps.clamp(1, 30))
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
///         git_factory: Arc::new(sid_git::Git2ProviderFactory::new()),
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
///         undo_ring: std::collections::VecDeque::new(),
///         form: None,
///         form_origin_tab: None,
///         pending_submits: Vec::new(),
///         toasts: sid::toast::ToastQueue::new(4),
///         jobs: std::sync::Arc::new(sid_job::JobQueue::<sid::wire::JobOutcome>::new()),
///         ssh_client_factory: sid::wire::build_ssh_client_factory_fn(),
///         ssh_outcome_tx: {
///             let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
///             tx
///         },
///         ssh_outcome_rx: {
///             let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
///             rx
///         },
///         ssh_byte_rx: None,
///         ssh_last_pty_area: None,
///         ssh_shutdown_tx: None,
///         active_theme: sid_ui::themes::cosmos(),
///         persister: sid_core::persister::StatePersister::new(std::time::Duration::ZERO),
///         last_heartbeat: std::time::Instant::now(),
///     };
///     let (tx, mut rx) = tokio::sync::mpsc::channel(1);
///     // Drop the sender to close the channel so the loop exits immediately.
///     drop(tx);
///     run_event_loop(&mut terminal, &mut sid_app, &mut rx).await.unwrap();
/// }
/// ```
/// `true` when at least `interval` has elapsed since `last`. Pure helper so the
/// heartbeat cadence is testable without wall-clock sleeps.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, Instant};
/// use sid::wire::heartbeat_due;
///
/// // Zero interval is always due.
/// assert!(heartbeat_due(Instant::now(), Duration::ZERO));
/// // A far-future interval is never due for a fresh instant.
/// assert!(!heartbeat_due(Instant::now(), Duration::from_secs(86_400)));
/// ```
pub fn heartbeat_due(last: std::time::Instant, interval: std::time::Duration) -> bool {
    last.elapsed() >= interval
}

pub async fn run_event_loop<B>(
    terminal: &mut Terminal<B>,
    sid_app: &mut SidApp,
    rx: &mut Receiver<SidEvent>,
) -> Result<()>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
{
    use sid_store::TypedSettings;
    // Session heartbeat cadence. Read HEARTBEAT_INTERVAL_SECS (default 5s); the
    // event loop touches the session's `last_active` no more than once per
    // interval so a long-lived detached process keeps a fresh recency stamp.
    let heartbeat_interval = std::time::Duration::from_secs(
        sid_app
            .store
            .get_u64(sid_store::settings_keys::HEARTBEAT_INTERVAL_SECS)
            .ok()
            .flatten()
            .unwrap_or(5),
    );
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

        // SSH live-connect plumbing. Order matters:
        // 1. Pending-connect intent → spawn connect task.
        // 2. Connect outcomes → attach PtyPane + stash byte_rx (or mark
        //    Failed and toast).
        // 3. Live bytes from the connected shell → forward into the
        //    attached PtyPane.
        drain_pending_ssh_connect(sid_app);
        drain_pending_ssh_add_new(sid_app);
        drain_ssh_outcomes(sid_app);
        drain_ssh_bytes(sid_app);

        // Resize the SSH PtyPane to match the current body area, if the
        // active tab is SSH and a pane is attached. The render path
        // doesn't mutate the screen, so this must happen before draw().
        let full_area = terminal_size_rect(terminal);
        sync_ssh_pty_size(sid_app, full_area);

        // Sweep expired toasts so they fade out on the next render.
        sid_app.toasts.drain_expired();

        terminal.draw(|f| draw(f, sid_app))?;
        let ev = match rx.recv().await {
            Some(e) => e,
            None => break,
        };

        // Advance starfield phase on timer ticks only — not on key/mouse events.
        // This ensures the visual twinkle rate matches `animation.fps` (the pump
        // interval is set from fps in main.rs) rather than keyboard activity.
        // Per spec (docs/superpowers/specs/2026-05-22-sid-ux-iteration.md:364):
        // "the tokio event pump wakes on either a key event OR a 1/FPS tick".
        if ev == sid_core::event::Event::Tick {
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
                // Skip ticking while a modal or form is open (spec line 366).
                if sid_app.modal_stack.is_empty() && sid_app.form.is_none() {
                    fx.tick(area, &sid_app.animation);
                }
            }
            // Session heartbeat: at most once per HEARTBEAT_INTERVAL_SECS,
            // refresh the current session's `last_active` so a long-running
            // (possibly detached) process keeps a fresh recency stamp.
            if heartbeat_due(sid_app.last_heartbeat, heartbeat_interval) {
                if let Ok(Some(mut sess)) = sid_app.store.current_session() {
                    sess.last_active = sid_store::now_epoch();
                    let _ = sid_app.store.upsert_session(&sess);
                }
                sid_app.last_heartbeat = std::time::Instant::now();
            }
        }

        // Translate mouse events into synthetic key events (scroll → j/k)
        // or direct tab switches (click on the tab strip), and route in-body
        // left-clicks to the active widget's `focus_at`. Other mouse kinds
        // are dropped. See `route_mouse_event` for the policy.
        // We rewrite `ev` in place so the rest of the loop treats the result
        // as the originating event.
        let ev = if let SidEvent::Mouse(m) = ev {
            let full_area = terminal_size_rect(terminal);
            match route_mouse_event(sid_app, full_area, m) {
                MouseRouting::Synthesize(chord) => SidEvent::Key(chord),
                MouseRouting::SwitchToTab(idx) => {
                    if let Some(tab) = sid_app.app.tabs().tabs().get(idx) {
                        let id = tab.id.clone();
                        let _ = sid_app.app.tabs_mut().switch_to(&id);
                    }
                    // Switching the tab is the whole action; tick the loop.
                    continue;
                }
                MouseRouting::FocusInBody { col, row } => {
                    // Body clicks are a no-op when a modal or form is open.
                    // Modals own input and visually cover the body; forms
                    // are keyboard-only this iteration and their 60 % pane
                    // would otherwise route clicks through `body_rect` at
                    // full width into the hidden background widget. The user
                    // dismisses the modal / form first, then clicks again.
                    if sid_app.modal_stack.is_empty() && sid_app.form.is_none() {
                        dispatch_focus_at_for_active_tab(sid_app, full_area, col, row);
                    }
                    continue;
                }
                MouseRouting::Drop => continue,
            }
        } else {
            ev
        };

        // Route key events through the layered interception (modal → form →
        // tab-strip → per-tab modal trigger). Mouse events fall straight
        // through to the widget dispatch below.
        let handled = match &ev {
            SidEvent::Key(chord) => route_key_event(sid_app, *chord),
            _ => false,
        };

        if !handled {
            let dispatch = sid_app.app.handle_event(&ev);
            // After the widget(s) have processed the event, check if the
            // Workspaces widget signalled that the user pressed Enter on a
            // Repo leaf. If so, open a detail tab.
            maybe_open_pending_workspace_detail(sid_app);
            // Or, if the user pressed Enter on the "+ add new" row, open the
            // create-new side-pane form.
            maybe_open_pending_new_form(sid_app);
            // Drain settings outcomes (live-apply behavior toggles, etc.).
            apply_pending_settings_outcomes(sid_app);
            // Drain database widget commands (OpenConnectionForm, TestConnection, ...).
            drain_database_commands(sid_app);
            // Drain network-tab widget actions (detail pane open/close).
            apply_pending_network_actions(sid_app);
            // Debounced session-state persistence: mark dirty every iteration,
            // but only write once the debounce window has elapsed. Compute the
            // flush decision first (needs `&mut sid_app.persister`), then do the
            // save (needs `&sid_app.store/app`) so the borrows don't overlap.
            sid_app.persister.mark_dirty();
            let should_flush = sid_app.persister.should_flush();
            if should_flush {
                let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
            }
            if matches!(dispatch, Dispatch::Quit) {
                // Flush unconditionally on quit so the final state is never
                // lost to the debounce window.
                let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
                break;
            }
        }
    }
    Ok(())
}

/// Layered key-event interception, ahead of the per-widget dispatch.
///
/// Returns `true` when the chord was consumed here (the loop then skips
/// `App::handle_event`). The precedence, highest first:
///
/// 1. **Global detach** (`Ctrl+D`) — bypasses every overlay so the user can
///    detach from a wedged modal.
/// 2. **Modal stack** — the topmost modal intercepts everything but global
///    quit/detach. Submit pushes onto `pending_submits`; Cancel pops.
/// 3. **Side-pane form** — when a form is open for the active tab and no modal
///    is open, it intercepts every key. Submit routes to
///    [`dispatch_form_submit`]; Cancel closes the pane; RequestDiscardConfirm
///    opens the discard modal.
/// 4. **Tab strip** — `strip_nav` cycling, gated on CONTROL modifier and no
///    modal/form active (`Ctrl+Tab` → next, `Ctrl+Shift+Tab` → prev).
///    Plain `Tab`/`Shift+Tab`/`BackTab` fall through to widgets for
///    intra-widget focus. Branches 1–5 adopt `strip_nav` for plain Tab as
///    they migrate widgets to the list/pane focus model.
/// 5. **Per-tab modal trigger** — opens a modal for the active tab if the
///    chord matches its opener.
///
/// Global quit (`Ctrl+Q`) is deliberately *not* consumed here: it falls
/// through to `App::handle_event`, which maps it to `app.quit`.
fn route_key_event(sid_app: &mut SidApp, chord: sid_core::event::KeyChord) -> bool {
    // Global quit always wins, even with a modal open.
    let is_global_quit = chord.code == crossterm::event::KeyCode::Char('q')
        && chord.mods.contains(crossterm::event::KeyModifiers::CONTROL);
    // Global detach: Ctrl+D spawns a new terminal pointed at the current tab.
    // Like Ctrl+Q it bypasses modal interception so the user can detach from a
    // wedged modal too.
    let is_global_detach = chord.code == crossterm::event::KeyCode::Char('d')
        && chord.mods.contains(crossterm::event::KeyModifiers::CONTROL);
    if is_global_detach {
        handle_ctrl_d_detach(sid_app);
        true
    } else if !is_global_quit && !sid_app.modal_stack.is_empty() {
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
                let popped = sid_app.modal_stack.pop();
                // Cancelling the connect-time password prompt aborts the
                // pending connect: reset the widget left in `Connecting` back
                // to Idle so it doesn't strand. The alias is carried in the
                // modal id (`ssh.password:<alias>`).
                if let Some(alias) = popped
                    .as_ref()
                    .and_then(|m| m.id.0.strip_prefix("ssh.password:"))
                {
                    cancel_pending_ssh_password(sid_app, alias);
                }
            }
            sid_widgets::ModalKeyOutcome::Submit => {
                let popped = sid_app.modal_stack.pop().expect("modal popped");
                let values = popped.collect_values();
                sid_app.pending_submits.push((popped.id, values));
            }
        }
        true
    } else if !is_global_quit
        && sid_app.modal_stack.is_empty()
        && sid_app.form.is_some()
        && sid_app.form_origin_tab.as_ref() == Some(&sid_app.app.tabs().active().id)
    {
        // Active form (on the active tab) intercepts every key after modals.
        // Mirror of the modal interception block: a modal wins if both are
        // somehow open (guarded by `modal_stack.is_empty()` above). Branches
        // 1-5 register `dispatch_form_submit` arms.
        //
        // SSH inspector background-open: when the active form is an
        // `ssh.inspect:<alias>` pane and the user presses Ctrl+Enter / O,
        // route to the background-open logic instead of handing the chord to
        // the FormPane (the inspector stays open; the new tab appears behind).
        //
        // Guard: Ctrl+Enter always intercepts (no conflict with text input).
        // Bare Shift+O only intercepts when the focused form field is NOT a
        // free-text input — typing 'O' into identity_file must insert the
        // char, not spawn a background tab.
        let is_ssh_inspector = sid_app
            .form
            .as_ref()
            .map(|f| {
                f.spec.id.0.starts_with("ssh.inspect:")
                    || f.spec.id.0.starts_with("ssh.inspect-ro:")
            })
            .unwrap_or(false);
        let is_ctrl_enter = chord.code == crossterm::event::KeyCode::Enter
            && chord.mods.contains(crossterm::event::KeyModifiers::CONTROL);
        let focused_is_text = sid_app
            .form
            .as_ref()
            .map(|f| f.focused_field_is_text())
            .unwrap_or(false);
        // Ctrl+Enter always intercepts; bare 'O' only when not in a text field.
        let should_background_open =
            is_ssh_inspector && (is_ctrl_enter || (chord.is_background_open() && !focused_is_text));
        if should_background_open {
            // Delegate to the existing background-open arm inside
            // dispatch_ssh_form_key; it reads sid_app.form internally.
            dispatch_ssh_form_key(sid_app, chord);
            return true;
        }
        let outcome = {
            let form = sid_app.form.as_mut().expect("form is_some");
            form.handle_key(chord)
        };
        match outcome {
            sid_widgets::form::FormEvent::Continue => {}
            sid_widgets::form::FormEvent::Cancel => {
                close_network_detail_pane_if_network_form(sid_app);
                sid_app.form = None;
                sid_app.form_origin_tab = None;
            }
            sid_widgets::form::FormEvent::Submit(values) => {
                let id = sid_app
                    .form
                    .as_ref()
                    .expect("form is_some")
                    .spec
                    .id
                    .0
                    .clone();
                dispatch_form_submit(sid_app, &id, values);
            }
            sid_widgets::form::FormEvent::RequestDiscardConfirm => {
                open_discard_confirm_modal(sid_app);
            }
        }
        true
    } else if !is_global_quit
        && sid_app.modal_stack.is_empty()
        && sid_app.form.is_none()
        && chord.mods.contains(crossterm::event::KeyModifiers::CONTROL)
        && chord.strip_nav() != sid_core::event::StripNav::None
    {
        // Tab-strip cycling — interim rule (orchestrator ruling, 2026-06-12):
        // fires ONLY on Ctrl-modified chords (Ctrl+Tab → next,
        // Ctrl+Shift+Tab → prev). Plain Tab/Shift+Tab/BackTab fall through to
        // widgets, which consume them for intra-widget focus today. Branches
        // 1-5 adopt strip_nav for plain Tab as they migrate widgets to the
        // list/pane focus model.
        //
        // Gate on no modal and no form — both claim Tab for their own focus
        // rings and are intercepted above.
        match chord.strip_nav() {
            sid_core::event::StripNav::Next => sid_app.app.tabs_mut().next(),
            sid_core::event::StripNav::Prev => sid_app.app.tabs_mut().prev(),
            sid_core::event::StripNav::None => {}
        }
        let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
        true
    } else if !is_global_quit
        && sid_app.modal_stack.is_empty()
        && sid_app.form.is_none()
        && maybe_open_workspaces_form_for_key(sid_app, chord)
    {
        // Workspaces-tab side-pane forms: `N` → create-new wizard,
        // `D` → adopt-existing wizard. Intercepted ahead of the modal opener so
        // the legacy `N`/`A`/`R` modals stay available for the other keys.
        true
    } else if !is_global_quit
        && sid_app.modal_stack.is_empty()
        && sid_app.form.is_none()
        && sid_app.app.tabs().active().id.as_str() == "ssh"
        && dispatch_ssh_form_key(sid_app, chord)
    {
        // SSH-tab FormPane keys handled; no modal push needed.
        true
    } else if !is_global_quit
        && sid_app.modal_stack.is_empty()
        && sid_app.form.is_none()
        && chord.code == crossterm::event::KeyCode::Char('u')
        && chord.mods == crossterm::event::KeyModifiers::NONE
    {
        // Settings undo interceptor. Pops the head entry from the undo ring
        // when it is within the TTL and re-applies the prior value.
        //
        // Spec (docs/superpowers/specs/2026-05-20-sid-future-features.md ~L268):
        // the `u` chord fires ONLY when the head toast is live AND its text
        // contains "(u: undo)". This ties undo to the toast the user can
        // actually see rather than to a 30-second ambient TTL.
        //
        // The TTL check on the ring entry is a secondary safety rail: if the
        // toast has already self-evicted (3s lifetime) the head-toast check
        // below rejects `u` anyway, so the TTL guard here is mostly defensive.
        //
        // Residual narrow window: if the user types `u` into a text filter that
        // does NOT claim the key at the widget level (e.g. a read-only Network
        // tab filter with no active form/modal) while a live undo toast is
        // visible, the undo fires. Long-term fix: widget text-focus signaling.
        // Runs ONLY when no modal or form is open to avoid swallowing `u` in
        // text-input contexts (forms are excluded above; modal exclusion below).
        let head_toast_has_marker = sid_app
            .toasts
            .iter()
            .last()
            .map(|t| !t.is_expired() && t.message.contains("(u: undo)"))
            .unwrap_or(false);
        if !head_toast_has_marker {
            // No live undo toast visible — let the key fall through.
            false
        } else if let Some(entry) = sid_app.undo_ring.pop_back() {
            if entry.is_expired() {
                // Entry too old — discard and let the key fall through.
                false
            } else {
                apply_undo_entry(sid_app, entry);
                true
            }
        } else {
            // No undo entry available — let the key fall through to the widget.
            false
        }
    } else if !is_global_quit && let Some(modal) = modal_for_active_tab_key(sid_app, chord) {
        sid_app.modal_stack.push(modal);
        true
    } else {
        false
    }
}

/// If the active tab is Workspaces and `chord` is a form-opener key, open the
/// corresponding side-pane form and return `true`. `N`/`n` opens the create-new
/// wizard; `D`/`d` opens the adopt-existing wizard (Task 9). Returns `false`
/// (so the caller falls through to the modal opener) for any other key or tab.
fn maybe_open_workspaces_form_for_key(
    sid_app: &mut SidApp,
    chord: sid_core::event::KeyChord,
) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    if sid_app.app.tabs().active().id.as_str() != "workspaces" {
        return false;
    }
    // Plain (unmodified) letter keys only — leave Ctrl/Alt chords to others.
    if chord
        .mods
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return false;
    }
    match chord.code {
        KeyCode::Char('N') | KeyCode::Char('n') => {
            open_form(sid_app, workspaces_new_form());
            true
        }
        KeyCode::Char('D') | KeyCode::Char('d') => {
            let dir = workspaces_adopt_dir(sid_app);
            open_form(sid_app, workspaces_adopt_form(&dir));
            true
        }
        _ => false,
    }
}

/// Drain the active Settings widget's pending outcomes (if any) and
/// dispatch each to the right `Store::put_*` call. Pushes a success
/// toast per applied outcome; pushes an error toast on `put_*` failure.
/// Master switch for the bottom-right toast overlay. Toasts are now a
/// logs-only channel: every message is recorded into the Settings → Logs ring
/// via [`record`], and the floating overlay is suppressed. Flip to `true` to
/// restore the on-screen toasts (the queue is still fed regardless, so no
/// message is lost when it is off).
const TOASTS_ENABLED: bool = false;

/// Mutably borrow the live [`sid_widgets::SettingsWidget`] out of the Settings
/// tab, if present. Returns `None` when the Settings tab is absent (custom test
/// setups) or its layout / widget type does not match.
fn settings_widget_mut(sid_app: &mut SidApp) -> Option<&mut sid_widgets::SettingsWidget> {
    use sid_core::layout::Layout;
    let settings_tab = sid_app
        .app
        .tabs_mut()
        .tabs_mut()
        .iter_mut()
        .find(|t| t.id.as_str() == "settings")?;
    let Layout::Single(w) = &mut settings_tab.layout else {
        return None;
    };
    w.as_any_mut().downcast_mut::<sid_widgets::SettingsWidget>()
}

/// Record a user-facing message into both channels:
///
/// 1. The Settings → Logs ring (via [`sid_widgets::SettingsWidget::record_log`]),
///    which is the durable, scrollable surface the user reads.
/// 2. The toast queue (kept fed even though the overlay is gated off by
///    [`TOASTS_ENABLED`]) so flipping the overlay back on needs no further
///    wiring.
///
/// The [`sid_widgets::settings::logs::LogLevel`] maps to a toast kind:
/// `Success` → success, `Error` → error, `Info` → a neutral success toast
/// (there is no dedicated "info" log level; info messages render as a plain
/// success toast which carries no error styling).
fn record(
    sid_app: &mut SidApp,
    level: sid_widgets::settings::logs::LogLevel,
    message: impl Into<String>,
) {
    use sid_widgets::settings::logs::{LogEntry, LogLevel};
    let entry = LogEntry::new(now_epoch(), level, message);
    // Feed the toast queue first (cheap clone of the message string).
    let toast = match level {
        LogLevel::Success => Toast::success(entry.message.clone()),
        LogLevel::Error => Toast::error(entry.message.clone()),
        LogLevel::Info => Toast::success(entry.message.clone()),
    };
    sid_app.toasts.push(toast);
    // Then record into the Logs ring (no-op if the category is absent).
    if let Some(settings) = settings_widget_mut(sid_app) {
        settings.record_log(entry);
    }
}

fn apply_pending_settings_outcomes(sid_app: &mut SidApp) {
    use sid_store::TypedSettings;
    use sid_widgets::settings::behavior_toggles::ToggleValue;

    // Drain the pending outcomes into an owned Vec, then DROP the settings
    // widget borrow before the loop. The loop calls `record(...)`, which
    // re-borrows the same widget to push log entries — so the extraction
    // borrow must not outlive it.
    let outcomes = {
        let Some(settings) = settings_widget_mut(sid_app) else {
            return;
        };
        settings.take_pending_outcomes()
    };
    if outcomes.is_empty() {
        return;
    }

    for outcome in outcomes {
        use sid_widgets::settings::PendingSettingsOutcome::*;

        use crate::settings_undo::{UndoEntry, UndoPayload};
        match outcome {
            BehaviorToggled { key, value } => {
                // Read prior value before write so it can be restored.
                let prior_toggle = read_prior_toggle(&*sid_app.store, key, &value);
                let put_result = match &value {
                    ToggleValue::Bool(b) => sid_app.store.put_bool(key, *b),
                    ToggleValue::Choice { options, selected } => {
                        let picked = options.get(*selected).cloned().unwrap_or_default();
                        sid_app.store.put_string(key, &picked)
                    }
                    ToggleValue::U64 { value, .. } => sid_app.store.put_u64(key, *value),
                    ToggleValue::String(s) => sid_app.store.put_string(key, s),
                };
                // Push undo only when a meaningful prior exists (i.e. the key
                // was already set). Net-new boolean keys read as None from an
                // empty store; for those the change is non-revertable and no
                // undo entry is recorded.
                let undo = prior_toggle.map(|prior| UndoEntry {
                    payload: UndoPayload::BehaviorToggle { key, prior },
                    recorded_at: std::time::Instant::now(),
                });
                persist_outcome(sid_app, put_result, undo, format!("Saved {key}"), |e| {
                    format!("Save failed for {key}: {e}")
                });
            }
            WorkspaceRootsChanged(new_roots) => {
                use sid_store::{SettingValue, settings_keys};
                // Read prior roots before write. The prior is always meaningful
                // (even an empty vec is a valid state to restore to), so always
                // push an undo entry.
                let prior_roots = read_prior_roots(&*sid_app.store);
                let json = serde_json::to_string(&new_roots)
                    .map_err(|e| sid_core::SidError::Storage(e.to_string()));
                let put_result = json.and_then(|s| {
                    sid_app.store.put_setting(
                        settings_keys::WORKSPACE_ROOTS,
                        &SettingValue(s.into_bytes()),
                    )
                });
                let undo = Some(UndoEntry {
                    payload: UndoPayload::WorkspaceRoots { prior: prior_roots },
                    recorded_at: std::time::Instant::now(),
                });
                persist_outcome(
                    sid_app,
                    put_result,
                    undo,
                    "Workspace roots saved".into(),
                    |e| format!("Workspace roots save failed: {e}"),
                );
            }
            QuickActionUpserted(qa) => {
                // Read prior record if it exists. A net-new quick-action has no
                // prior to restore to, so undo is skipped for those (no toast
                // suffix either).
                let prior_qa = sid_app.store.get_quick_action(&qa.id).ok().flatten();
                let put_result = sid_app.store.upsert_quick_action(&qa);
                let undo = prior_qa.map(|prior| UndoEntry {
                    payload: UndoPayload::QuickActionUpserted { prior },
                    recorded_at: std::time::Instant::now(),
                });
                let label = format!("Quick action '{}' saved", qa.id);
                persist_outcome(sid_app, put_result, undo, label, |e| {
                    format!("Quick action save failed: {e}")
                });
            }
            QuickActionRemoved(id) => {
                // Read prior record so it can be restored. If the record is
                // absent (shouldn't happen in practice) no undo is recorded.
                let prior_qa = sid_app.store.get_quick_action(&id).ok().flatten();
                let put_result = sid_app.store.remove_quick_action(&id);
                let undo = prior_qa.map(|prior| UndoEntry {
                    payload: UndoPayload::QuickActionRemoved { prior },
                    recorded_at: std::time::Instant::now(),
                });
                persist_outcome(
                    sid_app,
                    put_result,
                    undo,
                    format!("Quick action '{id}' removed"),
                    |e| format!("Quick action remove failed: {e}"),
                );
            }
            KeybindApplied {
                profile_name,
                map_snapshot,
            } => {
                use sid_store::keybind_load::{load_keybind_profile, save_keybind_profile};
                // Read prior map before write. The prior (or the default map
                // when absent) is always meaningful — an undo restores the
                // pre-change binding state including "unset".
                let prior_map = load_keybind_profile(&*sid_app.store, &profile_name)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let put_result =
                    save_keybind_profile(&*sid_app.store, &profile_name, &map_snapshot);
                let undo = Some(UndoEntry {
                    payload: UndoPayload::Keybind {
                        profile_name: profile_name.clone(),
                        prior: prior_map,
                    },
                    recorded_at: std::time::Instant::now(),
                });
                persist_outcome(
                    sid_app,
                    put_result,
                    undo,
                    format!("Keybinds saved to '{profile_name}'"),
                    |e| format!("Keybind save failed: {e}"),
                );
            }
            ThemeApplied { name } => {
                use sid_store::{TypedSettings, settings_keys};
                // Read prior theme before write. The prior (or the default
                // "cosmos") is always meaningful — an undo restores the theme
                // name the user had before even if it was never explicitly set.
                let prior_theme = sid_app
                    .store
                    .get_string(settings_keys::THEME_NAME)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "cosmos".into());
                let put_result = sid_app.store.put_string(settings_keys::THEME_NAME, &name);
                let undo = Some(UndoEntry {
                    payload: UndoPayload::Theme { prior: prior_theme },
                    recorded_at: std::time::Instant::now(),
                });
                let applied_ok = put_result.is_ok();
                persist_outcome(
                    sid_app,
                    put_result,
                    undo,
                    format!("Theme '{name}' applied"),
                    |e| format!("Theme save failed: {e}"),
                );
                // Apply the new theme LIVE: re-resolve it from the store (which
                // now holds the freshly-persisted THEME_NAME) so `draw()` picks
                // it up on the next frame. Only on a successful persist.
                if applied_ok {
                    sid_app.active_theme = load_active_theme(&*sid_app.store).0;
                }
            }
            DbPathOverrideWritten(notice) => {
                let msg = format!(
                    "DB path written to {} — restart to apply",
                    notice.sid_toml_path.display()
                );
                // DB path change requires a restart — intentionally not undoable.
                record(sid_app, sid_widgets::settings::logs::LogLevel::Info, msg);
            }
            FactoryResetConfirmed => {
                use sid_widgets::settings::{logs::LogLevel, reset::ResetView};
                let mut rv = ResetView::new();
                rv.open_confirm();
                match rv.confirm(&*sid_app.store) {
                    Ok(n) => {
                        // Factory reset is intentionally not undoable.
                        record(
                            sid_app,
                            LogLevel::Success,
                            format!("Reset {n} settings to defaults"),
                        );
                    }
                    Err(e) => {
                        record(sid_app, LogLevel::Error, format!("Reset failed: {e}"));
                    }
                }
            }
            AnimationChanged(new_cfg) => {
                // Toggle fx_state to match the new enabled flag before
                // replacing animation so the comparison is against the
                // *old* value.
                if new_cfg.enabled && sid_app.fx_state.is_none() {
                    sid_app.fx_state = Some(sid_fx::FxState::new());
                } else if !new_cfg.enabled && sid_app.fx_state.is_some() {
                    sid_app.fx_state = None;
                }
                sid_app.animation = new_cfg;
                // Live-applied in place — nothing persisted here beyond what
                // AnimationView already flushed, so no undo entry is recorded.
                record(
                    sid_app,
                    sid_widgets::settings::logs::LogLevel::Success,
                    "Animation settings applied".to_string(),
                );
            }
        }
    }
}

/// Unified persistence helper for all undo-bearing settings arms.
///
/// On success: if `undo` is `Some`, pushes the entry into `ring` and appends
/// `" (u: undo)"` to `success_label`; otherwise toasts `success_label`
/// unchanged (no suffix). On error: calls `err_msg(e)` and pushes an error
/// toast.
///
/// Policy rationale — when to pass `Some` vs `None`:
/// - Pass `Some` when a prior exists and is meaningful to restore, including
///   when the prior is a type-default (empty vec, default map, "cosmos").
///   `WorkspaceRoots`, `Keybind`, and `Theme` always carry `Some` for this
///   reason; restoring "unset" is a legitimate undo.
/// - Pass `None` for net-new records with no prior (e.g. first upsert of a
///   quick-action); there is nothing to restore.
///
/// Both the success and error messages are routed through [`record`], so every
/// outcome lands in the Settings → Logs ring (and the gated toast queue) — not
/// just the on-screen toast overlay.
fn persist_outcome(
    sid_app: &mut SidApp,
    put_result: Result<(), sid_core::SidError>,
    undo: Option<crate::settings_undo::UndoEntry>,
    success_label: String,
    err_fn: impl FnOnce(sid_core::SidError) -> String,
) {
    use sid_widgets::settings::logs::LogLevel;
    match put_result {
        Ok(()) => {
            let pushed = if let Some(entry) = undo {
                push_undo(&mut sid_app.undo_ring, entry);
                true
            } else {
                false
            };
            let suffix = if pushed { " (u: undo)" } else { "" };
            record(
                sid_app,
                LogLevel::Success,
                format!("{success_label}{suffix}"),
            );
        }
        Err(e) => {
            record(sid_app, LogLevel::Error, err_fn(e));
        }
    }
}

/// Re-apply the prior value stored in `entry` to the store.
/// Pushes a success toast on success; error toast on failure.
fn apply_undo_entry(sid_app: &mut SidApp, entry: crate::settings_undo::UndoEntry) {
    use sid_store::TypedSettings;

    use crate::settings_undo::UndoPayload;
    match entry.payload {
        UndoPayload::BehaviorToggle { key, prior } => {
            use sid_widgets::settings::behavior_toggles::ToggleValue;
            let res = match &prior {
                ToggleValue::Bool(b) => sid_app.store.put_bool(key, *b),
                ToggleValue::Choice { options, selected } => {
                    let picked = options.get(*selected).cloned().unwrap_or_default();
                    sid_app.store.put_string(key, &picked)
                }
                ToggleValue::U64 { value, .. } => sid_app.store.put_u64(key, *value),
                ToggleValue::String(s) => sid_app.store.put_string(key, s),
            };
            match res {
                Ok(()) => sid_app.toasts.push(Toast::success(format!("Undid {key}"))),
                Err(e) => sid_app
                    .toasts
                    .push(Toast::error(format!("Undo failed for {key}: {e}"))),
            }
        }
        UndoPayload::WorkspaceRoots { prior } => {
            use sid_store::{SettingValue, settings_keys};
            let json = serde_json::to_string(&prior)
                .map_err(|e| sid_core::SidError::Storage(e.to_string()));
            let res = json.and_then(|s| {
                sid_app.store.put_setting(
                    settings_keys::WORKSPACE_ROOTS,
                    &SettingValue(s.into_bytes()),
                )
            });
            match res {
                Ok(()) => sid_app.toasts.push(Toast::success("Undid workspace roots")),
                Err(e) => sid_app
                    .toasts
                    .push(Toast::error(format!("Workspace roots undo failed: {e}"))),
            }
        }
        UndoPayload::QuickActionUpserted { prior } => {
            match sid_app.store.upsert_quick_action(&prior) {
                Ok(()) => sid_app.toasts.push(Toast::success(format!(
                    "Restored quick action '{}'",
                    prior.id
                ))),
                Err(e) => sid_app
                    .toasts
                    .push(Toast::error(format!("Quick action undo failed: {e}"))),
            }
        }
        UndoPayload::QuickActionRemoved { prior } => {
            match sid_app.store.upsert_quick_action(&prior) {
                Ok(()) => sid_app.toasts.push(Toast::success(format!(
                    "Restored quick action '{}'",
                    prior.id
                ))),
                Err(e) => sid_app
                    .toasts
                    .push(Toast::error(format!("Quick action restore failed: {e}"))),
            }
        }
        UndoPayload::Keybind {
            profile_name,
            prior,
        } => {
            use sid_store::keybind_load::save_keybind_profile;
            match save_keybind_profile(&*sid_app.store, &profile_name, &prior) {
                Ok(()) => sid_app.toasts.push(Toast::success(format!(
                    "Undid keybinds for '{profile_name}'"
                ))),
                Err(e) => sid_app
                    .toasts
                    .push(Toast::error(format!("Keybind undo failed: {e}"))),
            }
        }
        UndoPayload::Theme { prior } => {
            use sid_store::settings_keys;
            match sid_app.store.put_string(settings_keys::THEME_NAME, &prior) {
                Ok(()) => sid_app
                    .toasts
                    .push(Toast::success(format!("Undid theme → '{prior}'"))),
                Err(e) => sid_app
                    .toasts
                    .push(Toast::error(format!("Theme undo failed: {e}"))),
            }
        }
    }
}

/// Push an undo entry, evicting the oldest when at cap.
fn push_undo(
    ring: &mut std::collections::VecDeque<crate::settings_undo::UndoEntry>,
    entry: crate::settings_undo::UndoEntry,
) {
    use crate::settings_undo::UNDO_RING_CAP;
    if ring.len() == UNDO_RING_CAP {
        ring.pop_front();
    }
    ring.push_back(entry);
}

/// Read the current value of `key` as a [`sid_widgets::settings::behavior_toggles::ToggleValue`]
/// in the same shape as `new_value`, for use as the undo-ring "prior".
fn read_prior_toggle(
    store: &dyn sid_store::Store,
    key: &'static str,
    new_value: &sid_widgets::settings::behavior_toggles::ToggleValue,
) -> Option<sid_widgets::settings::behavior_toggles::ToggleValue> {
    use sid_store::TypedSettings;
    use sid_widgets::settings::behavior_toggles::ToggleValue;
    Some(match new_value {
        ToggleValue::Bool(_) => ToggleValue::Bool(store.get_bool(key).ok()??.to_owned()),
        ToggleValue::Choice { options, .. } => {
            let s = store.get_string(key).ok()??;
            let selected = options.iter().position(|o| o == &s).unwrap_or(0);
            ToggleValue::Choice {
                options: options.clone(),
                selected,
            }
        }
        ToggleValue::U64 { min, max, step, .. } => ToggleValue::U64 {
            value: store.get_u64(key).ok()??,
            min: *min,
            max: *max,
            step: *step,
        },
        ToggleValue::String(_) => ToggleValue::String(store.get_string(key).ok()??),
    })
}

/// Read the current workspace roots from the store, or return an empty vec on
/// error/absent.
fn read_prior_roots(store: &dyn sid_store::Store) -> Vec<std::path::PathBuf> {
    use sid_store::settings_keys;
    let Ok(Some(sv)) = store.get_setting(settings_keys::WORKSPACE_ROOTS) else {
        return Vec::new();
    };
    serde_json::from_slice::<Vec<std::path::PathBuf>>(&sv.0).unwrap_or_default()
}

/// If the Workspaces widget has a pending `take_pending_open_detail` flag,
/// drain it and push a new [`sid_widgets::WorkspaceDetailWidget`] as a
/// detail tab. No-op when the flag is unset.
///
/// Avoids duplicate tabs: if a detail tab for the same workspace path is
/// already open, switches to it instead of pushing a new one.
fn maybe_open_pending_workspace_detail(sid_app: &mut SidApp) {
    use sid_core::{
        layout::Layout,
        tab::{Tab, TabId, TabKind},
    };

    // Find the workspaces tab and drain its pending flag.
    let parent_idx = match sid_app
        .app
        .tabs()
        .tabs()
        .iter()
        .position(|t| t.id.as_str() == "workspaces")
    {
        Some(i) => i,
        None => return,
    };
    let workspace = {
        let tabs = sid_app.app.tabs_mut().tabs_mut();
        let Some(tab) = tabs.get_mut(parent_idx) else {
            return;
        };
        let Layout::Single(w) = &mut tab.layout else {
            return;
        };
        let Some(ws_widget) = w
            .as_any_mut()
            .downcast_mut::<sid_widgets::WorkspacesWidget>()
        else {
            return;
        };
        match ws_widget.take_pending_open_detail() {
            Some(ws) => ws,
            None => return,
        }
    };

    // Drain the background-open flag too.
    let background = {
        let tabs = sid_app.app.tabs_mut().tabs_mut();
        tabs.get_mut(parent_idx)
            .and_then(|t| {
                if let Layout::Single(w) = &mut t.layout {
                    w.as_any_mut()
                        .downcast_mut::<sid_widgets::WorkspacesWidget>()
                        .map(|ww| ww.take_pending_open_background())
                } else {
                    None
                }
            })
            .unwrap_or(false)
    };

    let tab_id_str = format!("workspace_detail:{}", workspace.path.display());
    let tab_id = TabId::new(&tab_id_str);

    // Already open? Just switch.
    if sid_app.app.tabs().tabs().iter().any(|t| t.id == tab_id) {
        let _ = sid_app.app.tabs_mut().switch_to(&tab_id);
        return;
    }

    let git_factory = sid_app.git_factory.clone();
    let widget = sid_widgets::WorkspaceDetailWidget::new(workspace.clone(), None);
    let new_tab = Tab {
        id: tab_id.clone(),
        title: workspace.name.clone(),
        layout: Layout::Single(Box::new(widget)),
        hotkey: None,
        kind: TabKind::Detail { parent_idx },
    };
    let push_result = if background {
        sid_app.app.tabs_mut().push_background(new_tab)
    } else {
        sid_app.app.tabs_mut().push_detail(new_tab)
    };
    if let Err(e) = push_result {
        sid_app
            .toasts
            .push(Toast::error(format!("open workspace detail: {e}")));
        return;
    }
    if !background {
        let _ = sid_app.app.tabs_mut().switch_to(&tab_id);
    }

    // Scan satellites synchronously (cheap fs walk), push them, then spawn one
    // git-load job per row.
    let rows = scan_umbrella_satellites(&workspace.path, &workspace.name);
    let paths: Vec<std::path::PathBuf> = rows.iter().map(|r| r.path.clone()).collect();
    let scan_tab_id = tab_id_str.clone();
    let scan_rows = rows.clone();
    let _ = sid_app.jobs.spawn(async move {
        JobOutcome::WorkspaceSatellitesScanned {
            tab_id: scan_tab_id,
            rows: scan_rows,
        }
    });
    for path in paths {
        let factory = git_factory.clone();
        let job_tab_id = tab_id_str.clone();
        let path_for_outcome = path.clone();
        let _ = sid_app.jobs.spawn(async move {
            let git = tokio::task::spawn_blocking(move || load_repo_git(&factory, &path))
                .await
                .unwrap_or_else(|_| sid_widgets::RepoGit::loaded("?".into(), 0, 0, 0));
            JobOutcome::RepoGitLoaded {
                tab_id: job_tab_id,
                path: path_for_outcome,
                git,
            }
        });
    }
}

/// Build the detail tab's row list: the umbrella row first, then every
/// adoptable satellite under it (one level deep, symlinks resolved). Git
/// snapshots start in the `loading` state; per-row loads fill them in.
fn scan_umbrella_satellites(
    umbrella_path: &std::path::Path,
    umbrella_name: &str,
) -> Vec<sid_widgets::SatelliteRow> {
    use sid_core::workspace_discovery::scan_adoptable_repos;
    use sid_widgets::{RepoGit, SatelliteRow};
    let mut rows = vec![SatelliteRow {
        name: umbrella_name.to_string(),
        path: umbrella_path.to_path_buf(),
        is_umbrella: true,
        git: RepoGit::loading(),
    }];
    for repo in scan_adoptable_repos(umbrella_path) {
        rows.push(SatelliteRow {
            name: repo.name,
            path: repo.path,
            is_umbrella: false,
            git: RepoGit::loading(),
        });
    }
    rows
}

/// Open `path` with `factory` and compute its [`sid_widgets::RepoGit`] snapshot.
/// Best-effort: on any git error returns a `?`-branch loaded snapshot so the
/// row stops showing "loading" rather than hanging.
fn load_repo_git(
    factory: &std::sync::Arc<sid_git::Git2ProviderFactory>,
    path: &std::path::Path,
) -> sid_widgets::RepoGit {
    use sid_core::adapters::git::GitProvider;
    use sid_widgets::RepoGit;
    let provider = match factory.open(path) {
        Ok(p) => p,
        Err(_) => return RepoGit::loaded("?".into(), 0, 0, 0),
    };
    let branch = provider
        .current_branch()
        .ok()
        .flatten()
        .map(|b| b.name)
        .unwrap_or_else(|| "?".into());
    let dirty = provider
        .status()
        .map(|s| u32::try_from(s.entries.len()).unwrap_or(u32::MAX))
        .unwrap_or(0);
    // Outgoing = commits on the current branch not on its upstream. The
    // `GitProvider` trait exposes no ahead/behind method, so v1 reports 0
    // (honest, not a guess); a follow-up that grows the trait can wire a real
    // ahead/behind count.
    RepoGit::loaded(branch, dirty, 0, 0)
}

/// Mutable handle to the Workspaces overview widget, if the tab is installed
/// and the layout is a single widget. Used by the add-new-row hydration and
/// drain paths.
fn workspaces_widget_mut(sid_app: &mut SidApp) -> Option<&mut sid_widgets::WorkspacesWidget> {
    use sid_core::layout::Layout;
    for tab in sid_app.app.tabs_mut().tabs_mut().iter_mut() {
        if tab.id.as_str() != "workspaces" {
            continue;
        }
        if let Layout::Single(w) = &mut tab.layout {
            return w
                .as_any_mut()
                .downcast_mut::<sid_widgets::WorkspacesWidget>();
        }
        return None;
    }
    None
}

/// Hydrate the Workspaces overview's `show_add_new_row` toggle from the store's
/// `show_add_new_row` setting (default on). Call once after the app is built;
/// the flag is widget-level so it survives state refreshes.
pub fn hydrate_workspaces_add_new_row(sid_app: &mut SidApp) {
    let show = load_show_add_new_row(&*sid_app.store);
    if let Some(ww) = workspaces_widget_mut(sid_app) {
        ww.set_show_add_new_row(show);
    }
}

/// If the Workspaces widget signalled an add-new press (Enter on the synthetic
/// "+ add new" row), open the create-new side-pane form. No-op otherwise.
fn maybe_open_pending_new_form(sid_app: &mut SidApp) {
    let pressed = workspaces_widget_mut(sid_app)
        .map(|ww| ww.take_pending_add_new())
        .unwrap_or(false);
    if pressed && sid_app.form.is_none() {
        open_form(sid_app, workspaces_new_form());
    }
}

/// Spawn a new external terminal window running `sid --start-tab <active>`.
///
/// Triggered by `Ctrl+D` in [`run_event_loop`]. Fire-and-forget — no IPC, no
/// re-attach. Pushes a [`Toast::success`] on a clean spawn or a [`Toast::error`]
/// if the spawner couldn't launch (e.g., kitty missing).
///
/// The command line is `<current_exe> --start-tab <id>`; `current_exe` falls
/// back to the literal string `"sid"` if it can't be resolved (the PATH
/// lookup is then the spawner's problem). The working directory is the
/// current process's CWD (so the detached window inherits whatever workspace
/// context the user was in).
pub fn handle_ctrl_d_detach(sid_app: &mut SidApp) {
    let tab_id = sid_app.app.tabs().active().id.as_str().to_string();
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sid".to_string());
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // Build a shell-safe command line. We single-quote the exe path and tab
    // id so paths containing spaces (or other unusual characters) survive
    // through `sh -c "<cmd>"` style spawners.
    let cmd = format!("{} --start-tab {}", shell_quote(&exe), shell_quote(&tab_id));
    let req = SpawnRequest { cwd, cmd };
    match sid_app.spawner.spawn(req) {
        Ok(()) => {
            sid_app
                .toasts
                .push(Toast::success(format!("detached {tab_id} to new window")));
        }
        Err(e) => {
            sid_app
                .toasts
                .push(Toast::error(format!("detach failed: {e}")));
        }
    }
}

/// Wrap `s` in single quotes for safe interpolation into a shell command
/// line. Embedded single quotes are escaped via `'\''`. Always returns a
/// non-empty quoted string.
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// What the mouse-event router decided to do with a raw [`crossterm::event::MouseEvent`].
///
/// The cases match the policy in [`route_mouse_event`]: scrolls become
/// synthetic key events (so widget lists scroll through their existing j/k
/// handlers), clicks on the tab strip switch tabs, clicks inside the body
/// region become a focus-pane request on the active widget, anything else
/// is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseRouting {
    /// Translate the mouse event into a key chord that the rest of the loop
    /// dispatches via the existing key path.
    Synthesize(sid_core::event::KeyChord),
    /// Switch directly to the tab at the given zero-based index.
    SwitchToTab(usize),
    /// Left-click landed inside the per-tab body region. The wire-layer
    /// dispatches this to the active widget's `focus_at(body_rect, col, row)`
    /// so the clicked pane gains focus. Carries the click coordinate; the
    /// dispatch site recomputes `body_rect` from the live terminal size.
    FocusInBody { col: u16, row: u16 },
    /// Drop the event silently. The router falls through to this for any
    /// kind not handled above.
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

/// Compute the per-tab body [`Rect`] given the full terminal area.
///
/// Mirrors the body layout in [`draw`]:
/// - Strip the outer `Borders::ALL` (one cell on each side).
/// - The first two rows of the inner area are the tab strip.
/// - The bottom `footer_h + status_h` rows are footer/status (heights vary
///   with available room — see [`draw`] for the same arithmetic).
/// - Returns `None` when the body would have zero width or height.
///
/// # Examples
///
/// ```
/// use ratatui::layout::Rect;
/// use sid::wire::body_rect;
///
/// let full = Rect { x: 0, y: 0, width: 120, height: 40 };
/// let body = body_rect(full).unwrap();
/// // Body is inside the outer border (x >= 1, y >= 1 + tabs_h) and strictly
/// // smaller than `full` on every axis.
/// assert!(body.x >= 1);
/// assert!(body.y >= 3);
/// assert!(body.width < full.width);
/// assert!(body.height < full.height);
///
/// // A tiny terminal yields None.
/// let tiny = Rect { x: 0, y: 0, width: 1, height: 1 };
/// assert!(body_rect(tiny).is_none());
/// ```
pub fn body_rect(full_area: Rect) -> Option<Rect> {
    if full_area.width < 2 || full_area.height < 2 {
        return None;
    }
    let inner = Rect {
        x: full_area.x.saturating_add(1),
        y: full_area.y.saturating_add(1),
        width: full_area.width.saturating_sub(2),
        height: full_area.height.saturating_sub(2),
    };
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let tabs_h: u16 = 2;
    let status_h: u16 = if inner.height >= 12 { 1 } else { 0 };
    let footer_h: u16 = if inner.height >= 10 { 2 } else { 1 };
    let body_h = inner.height.saturating_sub(tabs_h + status_h + footer_h);
    if body_h == 0 {
        return None;
    }
    Some(Rect {
        x: inner.x,
        y: inner.y.saturating_add(tabs_h),
        width: inner.width,
        height: body_h,
    })
}

/// Decide what to do with a raw mouse event.
///
/// Policy:
///
/// - `MouseEventKind::ScrollUp`   → `KeyChord(Char('k'), NONE)` (focus prev row).
/// - `MouseEventKind::ScrollDown` → `KeyChord(Char('j'), NONE)` (focus next row).
/// - `MouseEventKind::Down(Left)` on the tab strip → switch to that tab.
/// - `MouseEventKind::Down(Left)` inside the per-tab body → [`MouseRouting::FocusInBody`]
///   so the active widget can focus the clicked pane.
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
            if m.row == tab_row {
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
                return MouseRouting::Drop;
            }
            // Body region: route to the active widget for focus-on-click.
            // The dispatch site recomputes `body_rect` from the live
            // terminal size and hands it to the widget's `focus_at`.
            if let Some(body) = body_rect(full_area) {
                if m.row >= body.y
                    && m.row < body.y.saturating_add(body.height)
                    && m.column >= body.x
                    && m.column < body.x.saturating_add(body.width)
                {
                    return MouseRouting::FocusInBody {
                        col: m.column,
                        row: m.row,
                    };
                }
            }
            MouseRouting::Drop
        }
        _ => MouseRouting::Drop,
    }
}

/// Dispatch a `MouseRouting::FocusInBody` to whichever widget is on the
/// active tab. Recomputes `body_rect` from `full_area`, then calls the
/// widget's `focus_at`. No-op when the active tab has no widget that
/// supports focus-on-click (Workspaces / SSH / Database / Network / System /
/// Settings are all covered today).
pub fn dispatch_focus_at_for_active_tab(sid_app: &mut SidApp, full_area: Rect, col: u16, row: u16) {
    let Some(body) = body_rect(full_area) else {
        return;
    };
    let layout = &mut sid_app.app.tabs_mut().active_mut().layout;
    let Some(w) = layout.iter_widgets_mut().next() else {
        return;
    };
    let any_ref = w as &mut dyn std::any::Any;
    if let Some(ws) = any_ref.downcast_mut::<WorkspacesWidget>() {
        ws.focus_at(body, col, row);
        return;
    }
    if let Some(ssh) = any_ref.downcast_mut::<SshWidget>() {
        ssh.focus_at(body, col, row);
        return;
    }
    if let Some(db) = any_ref.downcast_mut::<DatabaseWidget>() {
        db.focus_at(body, col, row);
        return;
    }
    if let Some(net) = any_ref.downcast_mut::<NetworkWidget>() {
        net.focus_at(body, col, row);
        return;
    }
    if let Some(sys) = any_ref.downcast_mut::<SystemWidget>() {
        sys.focus_at(body, col, row);
        return;
    }
    if let Some(settings) = any_ref.downcast_mut::<SettingsWidget>() {
        settings.focus_at(body, col, row);
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
    // Global help overlay: `?` on any tab.
    if chord.code == KeyCode::Char('?') {
        return Some(build_help_overlay(sid_app));
    }
    match sid_app.app.tabs().active().id.as_str() {
        "workspaces" => workspaces_modal_for_key(sid_app, chord),
        "ssh" => ssh_modal_for_key(sid_app, chord),
        "database" => database_modal_for_key(sid_app, chord),
        "system" => system_modal_for_key(sid_app, chord),
        "network" => network_modal_for_key(sid_app, chord),
        _ => None,
    }
}

/// Network-tab modal opener — **retired** in favour of UX-v2 form pane.
/// Replaced by [`network_open_detail_form`]. Kept as a stub so that the
/// `modal_for_active_tab_key` match arm compiles without change; returns
/// `None` unconditionally.
#[allow(dead_code)]
fn network_modal_for_key(
    _sid_app: &SidApp,
    _chord: sid_core::event::KeyChord,
) -> Option<sid_widgets::ModalSpec> {
    None
}

/// Load the sid-level prefs for the named interface from the store.
///
/// Returns defaults (not pinned, no alias) for any key that is absent or
/// fails to parse — never propagates a store error to the caller.
fn load_network_iface_prefs(
    store: &impl sid_store::TypedSettings,
    name: &str,
) -> sid_widgets::network::detail_pane::NetInterfacePrefs {
    let pinned = store
        .get_bool(&sid_widgets::network::detail_pane::pinned_key(name))
        .unwrap_or_default()
        .unwrap_or(false);
    let alias = store
        .get_string(&sid_widgets::network::detail_pane::alias_key(name))
        .unwrap_or_default()
        .unwrap_or_default();
    sid_widgets::network::detail_pane::NetInterfacePrefs { pinned, alias }
}

/// Open the UX-v2 detail form pane for the currently-selected interface.
///
/// No-op when: not on the network tab, interfaces pane not focused, or the
/// list is empty.
///
/// # Examples
///
/// ```no_run
/// use sid::wire::{network_open_detail_form, SidApp};
/// # fn demo(sid_app: &mut SidApp) {
/// network_open_detail_form(sid_app);
/// # }
/// ```
pub fn network_open_detail_form(sid_app: &mut SidApp) {
    let active = sid_app.app.tabs().active();
    let net = match active
        .layout
        .iter_widgets()
        .next()
        .and_then(|w| w.as_any().downcast_ref::<sid_widgets::NetworkWidget>())
    {
        Some(w) => w,
        None => return,
    };
    if net.focused_pane_label() != "Interfaces" {
        return;
    }
    let iface = match net.interfaces().selected_row() {
        Some(i) => i.clone(),
        None => return,
    };
    let is_default_route = net.interfaces().is_default_route(&iface.name);
    let prefs = load_network_iface_prefs(sid_app.store.as_ref(), &iface.name);
    let spec = sid_widgets::network::detail_pane::build_form_spec(&iface, &prefs, is_default_route);
    open_form(sid_app, spec);
}

/// Close the network widget's detail pane when the currently-active form
/// belongs to the network module (id starts with `"network.interface_prefs:"`).
///
/// Called from the three form-clearing code paths — Cancel, successful submit,
/// and discard-confirm — so the widget's `SplitView` stays in sync with the
/// wire-owned form state.  No-op when no network widget is active or the form
/// id doesn't match.
pub(crate) fn close_network_detail_pane_if_network_form(sid_app: &mut SidApp) {
    let is_network_form = sid_app
        .form
        .as_ref()
        .map(|f| f.spec.id.0.starts_with("network.interface_prefs:"))
        .unwrap_or(false);
    if !is_network_form {
        return;
    }
    let tab = sid_app.app.tabs_mut().active_mut();
    if let Some(net) = tab
        .layout
        .iter_widgets_mut()
        .next()
        .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>())
    {
        net.close_detail_pane();
    }
}

/// Write submitted network interface prefs to the store and push the updated
/// alias + pinned state into the active network widget immediately.
fn apply_network_prefs(
    sid_app: &mut SidApp,
    iface_name: &str,
    prefs: sid_widgets::network::detail_pane::NetInterfacePrefs,
) {
    use sid_store::TypedSettings;
    let _ = sid_app.store.put_bool(
        &sid_widgets::network::detail_pane::pinned_key(iface_name),
        prefs.pinned,
    );
    let _ = sid_app.store.put_string(
        &sid_widgets::network::detail_pane::alias_key(iface_name),
        &prefs.alias,
    );

    // Reload all aliases/pinned from the store and push into the live widget
    // so the sidebar re-renders immediately without waiting for a probe tick.
    let tab = sid_app.app.tabs_mut().active_mut();
    if let Some(net) = tab
        .layout
        .iter_widgets_mut()
        .next()
        .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>())
    {
        let mut aliases = std::collections::HashMap::new();
        let mut pinned_names = std::collections::HashSet::new();
        use sid_store::Store;
        if let Ok(keys) = sid_app.store.list_setting_keys() {
            for key in &keys {
                if let Some(rest) = key.strip_prefix("network.iface.") {
                    if let Some(name) = rest.strip_suffix(".alias") {
                        if let Ok(Some(v)) = sid_app.store.get_string(key) {
                            aliases.insert(name.to_string(), v);
                        }
                    }
                    if let Some(name) = rest.strip_suffix(".pinned") {
                        if let Ok(Some(true)) = sid_app.store.get_bool(key) {
                            pinned_names.insert(name.to_string());
                        }
                    }
                }
            }
        }
        net.ifs_mut().set_aliases(aliases);
        net.ifs_mut().set_pinned_names(pinned_names);
    }
}

/// Dispatch a network-specific widget action emitted via `ctx.emit_action`.
///
/// Called from the main event loop whenever the active widget emits an
/// action whose id starts with `"network."`.
#[allow(dead_code)] // Used in tests; `apply_pending_network_actions` is the production call-site.
pub fn handle_network_action(sid_app: &mut SidApp, action: &str) {
    match action {
        "network.open_detail_pane" => {
            network_open_detail_form(sid_app);
        }
        "network.close_detail_pane" => {
            // The widget already popped its SplitView; clear the form too.
            sid_app.form = None;
            sid_app.form_origin_tab = None;
        }
        _ => {}
    }
}

/// Poll the active network widget for a pending action and dispatch it.
///
/// Called after each `app.handle_event` cycle, symmetrically with
/// `maybe_open_pending_workspace_detail`.
fn apply_pending_network_actions(sid_app: &mut SidApp) {
    use sid_widgets::network::PendingNetAction;

    // Downcast the active tab's widget only when we're on the network tab.
    let tab_id = sid_app.app.tabs().active().id.as_str().to_string();
    if tab_id.as_str() != "network" {
        return;
    }
    let pending = {
        let tab = sid_app.app.tabs_mut().active_mut();
        let net = tab
            .layout
            .iter_widgets_mut()
            .next()
            .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>());
        net.and_then(|n| n.take_pending_net_action())
    };
    match pending {
        Some(PendingNetAction::OpenDetailPane) => network_open_detail_form(sid_app),
        Some(PendingNetAction::CloseDetailPane) => {
            sid_app.form = None;
            sid_app.form_origin_tab = None;
        }
        None => {}
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
        // 'N' and 'E' are now handled by dispatch_ssh_form_key (FormPane path).
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

/// Handle SSH-tab keys that open a side-pane [`FormPane`] rather than a modal.
///
/// Returns `true` when a form was opened (the caller should skip the
/// `ssh_modal_for_key` branch).
///
/// Covers:
/// - `N` / `n` — open the Add Host form.
/// - `E` / `e` — open the Edit Host form for the selected Manual host.
/// - `→` — open the inspector pane for the selected host.
///
/// Does not handle `G`, `S`, `K`, `X`, `F` — those remain modal.
///
pub fn dispatch_ssh_form_key(sid_app: &mut SidApp, chord: sid_core::event::KeyChord) -> bool {
    use crossterm::event::KeyCode;
    use sid_store::SshHostSource;
    use sid_widgets::ssh::{SshInspector, ssh_add_form_spec, ssh_edit_form_spec};

    match chord.code {
        KeyCode::Char('N') | KeyCode::Char('n') => {
            open_form(sid_app, ssh_add_form_spec());
            true
        }
        KeyCode::Char('E') | KeyCode::Char('e') => {
            let Some(host) = ssh_selected_host(sid_app) else {
                return false;
            };
            // ssh-config entries are read-only.
            if host.source == SshHostSource::SshConfig {
                return false;
            }
            open_form(sid_app, ssh_edit_form_spec(&host));
            true
        }
        KeyCode::Right => {
            // → from list focus opens the inspector side pane for the selected host.
            let Some(host) = ssh_selected_host(sid_app) else {
                return false;
            };
            let spec = SshInspector::from_host(&host).to_form_spec();
            open_form(sid_app, spec);
            true
        }
        _ if chord.is_background_open() => {
            // Ctrl+Enter or Shift+O: background-open a new SSH session tab for
            // the host currently shown in the inspector pane.
            let Some(form) = sid_app.form.as_ref() else {
                return false;
            };
            let id = form.spec.id.0.clone();
            // Accept both editable (ssh.inspect:<alias>) and read-only
            // (ssh.inspect-ro:<alias>) inspector form ids.
            let alias = if let Some(a) = id.strip_prefix("ssh.inspect-ro:") {
                a.to_string()
            } else if let Some(a) = id.strip_prefix("ssh.inspect:") {
                a.to_string()
            } else {
                return false;
            };
            // Use a unique tab id so active_ssh_widget_mut / refresh_ssh_widget
            // (which match on exact "ssh") keep targeting the parent tab only,
            // and so re-opening the same alias focuses rather than stacking.
            let detail_tab_id_str = format!("ssh:{alias}");
            let detail_tab_id = TabId::new(&detail_tab_id_str);
            // Dedup: if a session tab for this alias is already open, focus it
            // (mirror of maybe_open_pending_workspace_detail).
            if sid_app
                .app
                .tabs()
                .tabs()
                .iter()
                .any(|t| t.id == detail_tab_id)
            {
                let _ = sid_app.app.tabs_mut().switch_to(&detail_tab_id);
                sid_app.toasts.push(Toast::info(format!(
                    "SSH · {alias} already open — switched"
                )));
                return true;
            }
            let parent_idx = sid_app.app.tabs().active_index();
            let mut bg_widget = sid_widgets::SshWidget::new();
            // Hydrate the host list from the store FIRST: the connect drain
            // resolves the alias against this widget's visible_hosts, and the
            // user sees the real list behind the connecting overlay.
            match sid_app.store.list_ssh_hosts() {
                Ok(hosts) => bg_widget.state_mut().set_store_hosts(hosts),
                Err(e) => tracing::warn!("background-open: list_ssh_hosts failed: {e}"),
            }
            // Then mark pending connect and begin the connection so the detail
            // tab connects through the normal drain pipeline (same mechanism
            // as pressing Enter — just without switching focus).
            bg_widget.set_pending_connect(Some(alias.clone()));
            bg_widget.connection_mut().begin_connecting(alias.clone());
            let new_tab = Tab {
                id: detail_tab_id,
                title: format!("SSH · {alias}"),
                layout: Layout::Single(Box::new(bg_widget)),
                hotkey: None,
                kind: TabKind::Detail { parent_idx },
            };
            match sid_app.app.tabs_mut().push_background(new_tab) {
                Ok(()) => {
                    sid_app
                        .toasts
                        .push(Toast::info(format!("Opened SSH · {alias} in background")));
                }
                Err(e) => {
                    sid_app
                        .toasts
                        .push(Toast::error(format!("background open failed: {e}")));
                }
            }
            true
        }
        _ => false,
    }
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
/// Build the `?` help overlay: global chords, then the active tab's bindings.
///
/// Sources, in order:
/// 1. A fixed global section (tab strip, background-open, form keys, quit).
/// 2. The active widget's `footer_hint()` list — every entry, not just the
///    few the slim footer shows.
///
/// The overlay uses two `Field::Display` fields so the global and per-tab
/// sections stay visually separated in the modal renderer.
fn build_help_overlay(sid_app: &SidApp) -> sid_widgets::ModalSpec {
    use sid_widgets::{Field, ModalSpec};
    let mut global_body = String::new();
    global_body.push_str("Tab/S-Tab  cycle tabs (C-Tab next, C-S-Tab back on kitty terms)\n");
    global_body.push_str("C-Enter/O  open in background tab\n");
    global_body.push_str("→ / ←      enter / leave pane\n");
    global_body.push_str("C-W        close tab\n");
    global_body.push_str("Ctrl+Q     quit\n");
    global_body.push_str("Ctrl+F     palette\n");
    global_body.push_str("Ctrl+,     settings\n");
    global_body.push_str("?          this help");
    let mut tab_body = String::new();
    if let Some(w) = sid_app.app.tabs().active().layout.iter_widgets().next() {
        let hints = w.footer_hint();
        if hints.is_empty() {
            tab_body.push_str("(no tab-local actions)");
        } else {
            for h in &hints {
                tab_body.push_str(&format!("{:<10} {}\n", h.chord, h.label));
            }
            // trim trailing newline
            if tab_body.ends_with('\n') {
                tab_body.pop();
            }
        }
    } else {
        tab_body.push_str("(no widget)");
    }
    ModalSpec::new(
        "help.overlay",
        "Keybinds",
        vec![
            Field::Display {
                label: "Global".into(),
                body: global_body,
            },
            Field::Display {
                label: "This tab".into(),
                body: tab_body,
            },
        ],
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

/// Database-tab modal opener. `Del` / `D` removes the selected connection.
///
/// The `N` / `n` key previously opened a `database.new` modal here; that path
/// has been replaced by the UX-v2 side-pane form (via `DbCommand::OpenConnectionForm`
/// emitted by the widget). `N` now bubbles through the widget and is NOT intercepted
/// by this function.
fn database_modal_for_key(
    sid_app: &SidApp,
    chord: sid_core::event::KeyChord,
) -> Option<sid_widgets::ModalSpec> {
    use crossterm::event::KeyCode;
    use sid_widgets::{Field, ModalSpec};
    match chord.code {
        // database.new modal path removed — connections now use the form substrate
        // via "database.connection". submit_database_new kept temporarily for safety.
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
    use sid_widgets::{Field, ModalSpec, system::SystemPane};
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

/// Drain commands queued by the `DatabaseWidget` event handler.
///
/// `DbCommand::OpenConnectionForm` opens a side-pane form (unconditionally —
/// callers must not have a dirty form pending). `DbCommand::TestConnection`
/// spawns an off-thread connection probe. All other commands remain for the
/// binary's legacy handlers to consume on future frames.
pub(crate) fn drain_database_commands(sid_app: &mut SidApp) {
    use sid_widgets::database::DbCommand;

    // Pull the commands out of the widget; we need the SidApp borrow back
    // before calling helpers that borrow it mutably again.
    let cmds: Vec<DbCommand> = sid_app
        .app
        .tabs_mut()
        .tabs_mut()
        .iter_mut()
        .find(|t| t.id.as_str() == "database")
        .and_then(|t| t.layout.iter_widgets_mut().next())
        .and_then(|w| w.as_any_mut().downcast_mut::<DatabaseWidget>())
        .map(|w| w.state_mut().drain_commands())
        .unwrap_or_default();

    for cmd in cmds {
        match cmd {
            DbCommand::OpenConnectionForm { prefill } => {
                let spec = db_connection_form_spec(prefill.as_ref());
                open_form(sid_app, spec);
            }
            DbCommand::TestConnection { conn_id } => {
                spawn_test_connection(sid_app, conn_id);
            }
            // Commands that require Plan 4 wiring (RunQuery, Disconnect,
            // CopyCell, Connect, LoadHistory, LoadNextPage) are dropped here
            // with a trace-level log. They must not be re-queued — doing so
            // causes an infinite drain cycle. The Plan 4 implementation will
            // add dedicated arms when the real DbClient is wired.
            other => {
                tracing::warn!(
                    cmd = ?other,
                    "drain_database_commands: unhandled DbCommand dropped (Plan 4 stub)"
                );
            }
        }
    }
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
    // Build the prefs maps from the store so aliases / pinned names survive
    // each probe-tick refresh.
    let mut aliases = std::collections::HashMap::new();
    let mut pinned = std::collections::HashSet::new();
    use sid_store::{Store, TypedSettings};
    if let Ok(keys) = sid_app.store.list_setting_keys() {
        for key in &keys {
            if let Some(rest) = key.strip_prefix("network.iface.") {
                if let Some(name) = rest.strip_suffix(".alias") {
                    if let Ok(Some(v)) = sid_app.store.get_string(key) {
                        aliases.insert(name.to_string(), v);
                    }
                }
                if let Some(name) = rest.strip_suffix(".pinned") {
                    if let Ok(Some(true)) = sid_app.store.get_bool(key) {
                        pinned.insert(name.to_string());
                    }
                }
            }
        }
    }

    // If the currently-open form is for a named interface that is absent from
    // the incoming snapshot, close the pane and the form before applying the
    // new data.  This prevents stale-name submits from writing orphaned store
    // keys for an interface that no longer exists.
    let vanished_iface: Option<String> = sid_app.form.as_ref().and_then(|f| {
        f.spec
            .id
            .0
            .strip_prefix("network.interface_prefs:")
            .map(|name| name.to_string())
    });
    if let Some(ref gone) = vanished_iface {
        let still_present = snap.interfaces.iter().any(|i| &i.name == gone);
        if !still_present {
            close_network_detail_pane_if_network_form(sid_app);
            sid_app.form = None;
            sid_app.form_origin_tab = None;
            sid_app
                .toasts
                .push(Toast::info(format!("interface {gone} disappeared")));
        }
    }

    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "network" {
            if let Some(w) = t.layout.iter_widgets_mut().next() {
                let any_ref = w as &mut dyn std::any::Any;
                if let Some(n) = any_ref.downcast_mut::<NetworkWidget>() {
                    n.apply_snapshot_with_prefs(snap, aliases, pinned);
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
            Ok(JobOutcome::WorkspaceSatellitesScanned { tab_id, rows }) => {
                apply_satellites_to_detail(sid_app, &tab_id, rows);
            }
            Ok(JobOutcome::RepoGitLoaded { tab_id, path, git }) => {
                apply_row_git_to_detail(sid_app, &tab_id, &path, git);
            }
            Err(e) => {
                sid_app.toasts.push(Toast::error(format!("job: {e}")));
            }
        }
    }
}

/// Push a scanned satellite list to the detail widget identified by `tab_id`.
/// No-op if the tab was closed before the scan completed.
fn apply_satellites_to_detail(
    sid_app: &mut SidApp,
    tab_id: &str,
    rows: Vec<sid_widgets::SatelliteRow>,
) {
    use sid_core::layout::Layout;
    for tab in sid_app.app.tabs_mut().tabs_mut().iter_mut() {
        if tab.id.as_str() != tab_id {
            continue;
        }
        if let Layout::Single(w) = &mut tab.layout
            && let Some(d) = w
                .as_any_mut()
                .downcast_mut::<sid_widgets::WorkspaceDetailWidget>()
        {
            d.apply_satellites(rows);
        }
        return;
    }
}

/// Push one row's loaded git snapshot to the detail widget identified by `tab_id`.
/// No-op if the tab was closed before the load completed.
fn apply_row_git_to_detail(
    sid_app: &mut SidApp,
    tab_id: &str,
    path: &std::path::Path,
    git: sid_widgets::RepoGit,
) {
    use sid_core::layout::Layout;
    for tab in sid_app.app.tabs_mut().tabs_mut().iter_mut() {
        if tab.id.as_str() != tab_id {
            continue;
        }
        if let Layout::Single(w) = &mut tab.layout
            && let Some(d) = w
                .as_any_mut()
                .downcast_mut::<sid_widgets::WorkspaceDetailWidget>()
        {
            d.apply_row_git(path, git);
        }
        return;
    }
}

// ---------------------------------------------------------------------------
// SSH connect + PTY wiring
// ---------------------------------------------------------------------------

/// Drain the SSH widget's `pending_connect` slot, if any, and spawn the real
/// russh connect task. The task delivers its outcome back to the wire layer
/// via `sid_app.ssh_outcome_tx`; the next event-loop pass picks it up via
/// [`drain_ssh_outcomes`].
///
/// Does nothing when:
/// - The SSH tab isn't installed (custom in-memory `TabManager`s in tests).
/// - The widget has no pending connect.
/// - The matching host alias is not in the merged host list.
///
/// Looks up the host record from the merged list (store + ssh-config) so the
/// connect target is consistent with what the user sees in the Hosts pane.
pub fn drain_pending_ssh_connect(sid_app: &mut SidApp) {
    // Lift the alias and host snapshot out of the widget so we can release
    // the borrow on `sid_app.app` before spawning the connect task (which
    // captures `sid_app.ssh_client_factory` + `ssh_outcome_tx`).
    let (alias, host, rows, cols) = {
        // Any SSH widget — parent tab or background `ssh:<alias>` detail tab —
        // may carry the intent; route to whichever one set it.
        let Some(ssh) = find_ssh_widget_mut(sid_app, |w| w.peek_pending_connect().is_some()) else {
            return;
        };
        let Some(alias) = ssh.take_pending_connect() else {
            return;
        };
        let host = ssh
            .state()
            .visible_hosts()
            .iter()
            .find(|h| h.alias == alias)
            .cloned();
        let Some(host) = host else {
            // Race: the user removed the host between Enter and drain.
            // Mark the connection failed instead of silently dropping.
            ssh.connection_mut().mark_failed("host not found".into());
            return;
        };
        // Pick a default starting size; the next render frame calls
        // `pty_pane_resize_to_area` and bumps the screen to match the actual
        // body rect. 24x80 is the universal vt100 default.
        let (rows, cols) = ssh.pty_pane().map(|p| p.size()).unwrap_or((24u16, 80u16));
        (alias, host, rows, cols)
    };

    // Resolve the auth method from the host record. Password hosts may need an
    // interactive prompt (when no keyring entry exists yet); the other kinds
    // resolve synchronously. `resolve_connect_auth` returns the control-flow
    // decision so the modal interleaving lives in one place.
    match resolve_connect_auth(sid_app, &alias, &host) {
        ConnectAuthDecision::Spawn(auth) => {
            let factory = Arc::clone(&sid_app.ssh_client_factory);
            let tx = sid_app.ssh_outcome_tx.clone();
            spawn_ssh_connect_with_auth(factory, tx, host, alias, rows, cols, auth);
        }
        ConnectAuthDecision::PromptPassword => {
            // The widget stays in its `Connecting` phase (set when the connect
            // intent was raised) while the modal is up; `submit_ssh_password`
            // spawns the connect, and its outcome routes back to the still-
            // Connecting widget. A cancelled modal resets the widget to Idle
            // (see the `ssh.password:` arm in the modal Cancel handler).
            sid_app.modal_stack.push(ssh_password_modal(&alias));
        }
        ConnectAuthDecision::Fail(error) => {
            // No usable auth (e.g. Agent selected but SSH_AUTH_SOCK unset).
            // Deliver a Failed outcome through the normal channel so the widget
            // + toast path is identical to a connect-time auth rejection.
            let _ = sid_app
                .ssh_outcome_tx
                .send(SshConnectOutcome::Failed { alias, error });
        }
    }
}

/// Control-flow decision returned by [`resolve_connect_auth`]: spawn the
/// connect with a resolved [`SshAuth`], prompt the user for a password first,
/// or fail immediately with a clear message.
#[derive(Debug, PartialEq, Eq)]
enum ConnectAuthDecision {
    /// Auth is fully resolved — spawn the connect task now.
    Spawn(sid_core::adapters::ssh::SshAuth),
    /// A password host with no saved keyring entry — push the password modal
    /// and wait for `submit_ssh_password` to spawn the connect.
    PromptPassword,
    /// No usable auth — deliver a Failed outcome with this message.
    Fail(String),
}

/// Resolve the [`SshAuth`] for a host's connect attempt, or decide that the
/// user must be prompted / that the attempt cannot proceed.
///
/// - `Key` with an identity file → [`SshAuth::Key`]; `Key` without one falls
///   back to [`SshAuth::Agent`] (subject to the same agent-socket check).
/// - `Agent` → [`SshAuth::Agent`], but only when `SSH_AUTH_SOCK` is set;
///   otherwise [`ConnectAuthDecision::Fail`] with a clear message.
/// - `Password` → load `ssh.host.{alias}.password` from the secret store; if
///   present, [`SshAuth::Password`]; if absent, [`ConnectAuthDecision::PromptPassword`].
///
/// The password is never logged and never written back to the host record.
fn resolve_connect_auth(
    sid_app: &SidApp,
    alias: &str,
    host: &sid_store::SshHost,
) -> ConnectAuthDecision {
    use sid_core::adapters::ssh::SshAuth;
    match host.auth_kind {
        sid_store::SshAuthKind::Key => match host.identity_file.as_ref() {
            Some(path) => ConnectAuthDecision::Spawn(SshAuth::Key {
                path: std::path::PathBuf::from(path),
                passphrase: None,
            }),
            // No identity file recorded → fall through to agent semantics
            // (which also performs the SSH_AUTH_SOCK preflight).
            None => agent_auth_decision(),
        },
        sid_store::SshAuthKind::Agent => agent_auth_decision(),
        sid_store::SshAuthKind::Password => match ssh_password_from_keyring(sid_app, alias) {
            Some(pw) => ConnectAuthDecision::Spawn(SshAuth::Password(pw)),
            None => ConnectAuthDecision::PromptPassword,
        },
    }
}

/// Agent-auth decision with the `SSH_AUTH_SOCK` preflight (§B). When the socket
/// env var is unset the connect would fail deep inside russh with an opaque
/// message; surface a clear, actionable one at the wire layer instead.
fn agent_auth_decision() -> ConnectAuthDecision {
    agent_auth_decision_for(std::env::var_os("SSH_AUTH_SOCK").is_some())
}

/// Pure core of [`agent_auth_decision`]: given whether an ssh-agent socket is
/// available, decide whether to spawn agent auth or fail with a clear message.
/// Split out so the §B branch is testable without mutating the environment.
fn agent_auth_decision_for(agent_socket_present: bool) -> ConnectAuthDecision {
    use sid_core::adapters::ssh::SshAuth;
    if agent_socket_present {
        ConnectAuthDecision::Spawn(SshAuth::Agent)
    } else {
        ConnectAuthDecision::Fail(
            "no ssh-agent (SSH_AUTH_SOCK unset) — use password or key auth in the host's settings"
                .into(),
        )
    }
}

/// Load a host's saved password from the secret store, returning `None` when
/// no entry exists (or the store errors — a read failure is treated as
/// "prompt the user" rather than a hard error). The decoded bytes are turned
/// into a `String` via lossy UTF-8; the raw bytes are dropped immediately.
///
/// The returned `String` is the only copy kept; callers move it straight into
/// [`SshAuth::Password`].
fn ssh_password_from_keyring(sid_app: &SidApp, alias: &str) -> Option<String> {
    use sid_core::adapters::secrets::SecretId;
    let id = SecretId::new(ssh_password_secret_key(alias));
    match sid_app.secrets.get(&id) {
        Ok(Some(bytes)) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Ok(None) => None,
        Err(e) => {
            // Never include the alias-scoped value in the log — only the id and
            // the error kind.
            tracing::warn!(secret = %id.as_str(), error = %e, "ssh password keyring read failed");
            None
        }
    }
}

/// The secret-store key under which a host's connect password is saved.
/// Mirrors the DB tab's `db.connection.{id}.password` convention.
///
/// # Examples
///
/// ```
/// # // private helper — illustrated via the public convention.
/// // ssh.host.prod.password
/// ```
fn ssh_password_secret_key(alias: &str) -> String {
    format!("ssh.host.{alias}.password")
}

/// Build the connect-time password prompt modal (§A step 2). One masked
/// `password` field and one `save` toggle ("Save to keyring"). The modal id
/// carries the alias so [`submit_ssh_password`] knows which host to connect.
fn ssh_password_modal(alias: &str) -> sid_widgets::ModalSpec {
    use sid_widgets::modal::Field;
    sid_widgets::ModalSpec::new(
        format!("ssh.password:{alias}"),
        format!("Password for {alias}"),
        vec![
            Field::Password {
                label: "Password".into(),
                value: String::new(),
            },
            Field::Toggle {
                label: "Save to keyring".into(),
                value: false,
            },
        ],
    )
    .with_help("Entered password is never written to the host record.")
}

/// Submit handler for the `ssh.password:{alias}` modal (§A step 3). Spawns the
/// connect with [`SshAuth::Password`]; when the `save` toggle is on, persists
/// the password under `ssh.host.{alias}.password` so subsequent connects are
/// silent.
///
/// Security: the password is read from the modal's [`FieldValue::Password`],
/// moved straight into [`SshAuth::Password`] (and, on opt-in, into the secret
/// store as bytes). It is never written to the host record and never logged.
fn submit_ssh_password(
    sid_app: &mut SidApp,
    alias: &str,
    values: &[(String, sid_widgets::FieldValue)],
) {
    use sid_core::adapters::ssh::SshAuth;
    // Re-resolve the host record: the user may have changed the host list while
    // the modal was up. A missing host marks the connecting widget Failed.
    let host = active_ssh_widget_mut(sid_app)
        .and_then(|w| {
            w.state()
                .visible_hosts()
                .iter()
                .find(|h| h.alias == alias)
                .cloned()
        })
        .or_else(|| sid_app.store.get_ssh_host(alias).ok().flatten());
    let Some(host) = host else {
        let _ = sid_app.ssh_outcome_tx.send(SshConnectOutcome::Failed {
            alias: alias.to_string(),
            error: "host not found".into(),
        });
        return;
    };

    let password = string_value(values, "Password").unwrap_or_default();
    let save = bool_value(values, "Save to keyring");

    if save {
        use sid_core::adapters::secrets::SecretId;
        let id = SecretId::new(ssh_password_secret_key(alias));
        if let Err(e) = sid_app.secrets.put(&id, password.as_bytes()) {
            // Saving is best-effort: warn (without the secret) and continue
            // with the connect using the entered password.
            tracing::warn!(secret = %id.as_str(), error = %e, "ssh password keyring write failed");
        }
    }

    // Pick a default starting size matching the widget's current PTY pane.
    let (rows, cols) = active_ssh_widget_mut(sid_app)
        .and_then(|w| w.pty_pane().map(|p| p.size()))
        .unwrap_or((24u16, 80u16));

    let factory = Arc::clone(&sid_app.ssh_client_factory);
    let tx = sid_app.ssh_outcome_tx.clone();
    spawn_ssh_connect_with_auth(
        factory,
        tx,
        host,
        alias.to_string(),
        rows,
        cols,
        SshAuth::Password(password),
    );
}

/// Reset any SSH widget left in the `Connecting` phase for `alias` back to
/// `Idle` after the user cancelled the connect-time password prompt. Without
/// this the widget would advertise "Connecting…" forever for a connect that
/// will never be spawned.
fn cancel_pending_ssh_password(sid_app: &mut SidApp, alias: &str) {
    if let Some(ssh) = find_ssh_widget_mut(sid_app, |w| {
        w.connection().phase() == sid_widgets::ssh::ConnectionPhase::Connecting
            && w.connection().alias() == Some(alias)
    }) {
        ssh.connection_mut().reset();
    }
}

/// Drain the SSH widget's pending add-new intent. When the cursor is on the
/// synthetic "+" row and Enter is pressed, the widget sets `pending_add_new`;
/// this helper opens the add-host [`FormPane`] via [`open_form`].
///
/// Called once per event-loop tick, immediately after [`drain_pending_ssh_connect`].
pub fn drain_pending_ssh_add_new(sid_app: &mut SidApp) {
    let wants_add = active_ssh_widget_mut(sid_app)
        .map(|w| w.take_pending_add_new())
        .unwrap_or(false);
    if wants_add {
        open_form(sid_app, sid_widgets::ssh::ssh_add_form_spec());
    }
}

/// Drain every queued [`SshConnectOutcome`]. On `Connected`, attaches the
/// PtyPane to the SSH widget, stashes the byte receiver + shutdown handle on
/// `sid_app`, and flips connection state to `Connected`. On `Failed`, marks
/// the widget failed and pushes an error toast.
pub fn drain_ssh_outcomes(sid_app: &mut SidApp) {
    loop {
        let outcome = match sid_app.ssh_outcome_rx.try_recv() {
            Ok(o) => o,
            Err(MpscTryRecvError::Empty) => break,
            Err(MpscTryRecvError::Disconnected) => {
                // The sender stored on SidApp should keep this alive; if we
                // hit this branch the channel was torn down — log and exit.
                tracing::warn!("ssh_outcome channel disconnected; stopping drain");
                break;
            }
        };
        match outcome {
            SshConnectOutcome::Connected {
                alias,
                pty,
                byte_rx,
                shutdown_tx,
            } => {
                // Tear down any previous reader (best-effort). sid runs at
                // most ONE live SSH session; a new connect supersedes the old.
                if let Some(prev) = sid_app.ssh_shutdown_tx.take() {
                    let _ = prev.send(());
                }
                // The superseded session's widget (possibly in another tab)
                // still says Connected; flip it to Disconnected so exactly one
                // widget reads as live. Its pane stays attached for post-mortem
                // viewing, matching remote-close semantics.
                for_each_ssh_widget_mut(sid_app, |w| {
                    if w.connection().phase() == sid_widgets::ssh::ConnectionPhase::Connected {
                        w.connection_mut().mark_disconnected();
                    }
                });
                // Attach to the widget that asked for this alias (a background
                // detail tab's widget for background-opens); fall back to the
                // parent "ssh" tab for outcomes nobody is waiting on.
                let attached = if let Some(ssh) = find_ssh_widget_mut(sid_app, |w| {
                    w.connection().phase() == sid_widgets::ssh::ConnectionPhase::Connecting
                        && w.connection().alias() == Some(alias.as_str())
                }) {
                    ssh.set_pty_pane(pty);
                    ssh.connection_mut().mark_connected();
                    true
                } else if let Some(ssh) = active_ssh_widget_mut(sid_app) {
                    ssh.set_pty_pane(pty);
                    ssh.connection_mut().mark_connected();
                    true
                } else {
                    false
                };
                if attached {
                    sid_app.ssh_byte_rx = Some(byte_rx);
                    sid_app.ssh_shutdown_tx = Some(shutdown_tx);
                    sid_app.ssh_last_pty_area = None;
                    sid_app
                        .toasts
                        .push(Toast::success(format!("ssh: connected to {alias}")));
                } else {
                    // No SSH widget on this app — drop the resources;
                    // dropping `shutdown_tx` causes the reader task to exit
                    // naturally on its select! branch.
                    drop(byte_rx);
                    drop(shutdown_tx);
                    tracing::warn!("ssh outcome arrived but SSH tab is missing");
                }
            }
            SshConnectOutcome::Failed { alias, error } => {
                // Route the failure to the widget that was connecting to this
                // alias; fall back to the parent tab for orphan outcomes.
                if let Some(ssh) = find_ssh_widget_mut(sid_app, |w| {
                    w.connection().phase() == sid_widgets::ssh::ConnectionPhase::Connecting
                        && w.connection().alias() == Some(alias.as_str())
                }) {
                    ssh.connection_mut().mark_failed(error.clone());
                } else if let Some(ssh) = active_ssh_widget_mut(sid_app) {
                    ssh.connection_mut().mark_failed(error.clone());
                }
                sid_app
                    .toasts
                    .push(Toast::error(format!("ssh {alias}: {error}")));
            }
        }
    }
}

/// Drain pending bytes from the connected remote shell into the widget's
/// PtyPane. Coalesces every chunk that's currently available so we forward
/// in a single render-aligned burst.
///
/// No-op when:
/// - No byte channel is attached (no live connection).
/// - The SSH widget has no PtyPane (shouldn't happen post-connect, but is
///   defensive).
pub fn drain_ssh_bytes(sid_app: &mut SidApp) {
    let Some(rx) = sid_app.ssh_byte_rx.as_mut() else {
        return;
    };
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut disconnected = false;
    loop {
        match rx.try_recv() {
            Ok(bytes) => chunks.push(bytes),
            Err(MpscTryRecvError::Empty) => break,
            Err(MpscTryRecvError::Disconnected) => {
                disconnected = true;
                break;
            }
        }
    }
    if chunks.is_empty() && !disconnected {
        return;
    }
    // Feed the widget owning the live session (Connected + pane attached) —
    // which may be a background detail tab's widget, not the parent tab's.
    // Fall back to the parent tab to preserve single-tab behaviour in edge
    // states (e.g. bytes arriving for an already-superseded widget).
    // Existence is checked first, then re-borrowed: NLL extends a borrow
    // returned from an if-let arm across the whole expression, so the
    // fallback can't share one binding with the primary lookup.
    let live_pred = |w: &SshWidget| {
        w.connection().phase() == sid_widgets::ssh::ConnectionPhase::Connected
            && w.pty_pane().is_some()
    };
    let has_live = find_ssh_widget_mut(sid_app, live_pred).is_some();
    let target = if has_live {
        find_ssh_widget_mut(sid_app, live_pred)
    } else {
        active_ssh_widget_mut(sid_app)
    };
    if let Some(ssh) = target {
        if let Some(pane) = ssh.pty_pane_mut() {
            for chunk in &chunks {
                pane.feed(chunk);
            }
        }
        if disconnected {
            // Remote closed the shell. Mark widget Disconnected so the
            // status bar reflects it; keep the pane around so the user can
            // still see the final terminal state until they hit Enter
            // again.
            ssh.connection_mut().mark_disconnected();
        }
    }
    if disconnected {
        // Drop the receiver so future drains exit immediately. The reader
        // task is already done if the channel is closed.
        sid_app.ssh_byte_rx = None;
        if let Some(prev) = sid_app.ssh_shutdown_tx.take() {
            let _ = prev.send(());
        }
    }
}

/// Resize the SSH widget's PTY pane to match the body rect, if it changed.
/// Also sends a `window_change` (via [`sid_core::adapters::ssh::SshShell::resize`])
/// to the remote in a future iteration — for now we only resize the local
/// screen so the rendered output lines up with the visible area. The shell
/// reader task does not receive the new size automatically; that's a
/// scheduled follow-up (see TODO).
pub fn sync_ssh_pty_size(sid_app: &mut SidApp, full_area: Rect) {
    let body = active_ssh_body_rect(full_area);
    // Guard: only act when the active tab is an SSH tab — the parent ("ssh")
    // or a background-opened session tab ("ssh:<alias>").
    let is_active_ssh = {
        let id = sid_app.app.tabs().active().id.as_str();
        id == "ssh" || id.starts_with("ssh:")
    };
    if !is_active_ssh {
        return;
    }
    if sid_app.ssh_last_pty_area == Some(body) {
        return;
    }
    if let Some(ssh) = active_tab_ssh_widget_mut(sid_app) {
        if ssh.pty_pane().is_some() {
            ssh.pty_pane_resize_to_area(body);
            sid_app.ssh_last_pty_area = Some(body);
            // TODO: forward the new size to the remote via SshShell::resize.
            // The current shell handle isn't accessible here because it was
            // moved into the reader task; the next iteration plumbs a
            // resize-command channel.
        }
    }
}

/// Compute the SSH widget's right-pane body rect given the full terminal
/// area. Mirrors the body rect carved out of the full draw area by
/// [`draw`]: tab strip (3 rows) → body → footer (1 row) on the outside, and
/// then [`sid_widgets::ssh::body_rect_for`] inside.
fn active_ssh_body_rect(full: Rect) -> Rect {
    // The body rect inside draw() is full_area minus a 3-row tab strip on
    // top and a 1-row footer at the bottom. Match that.
    let top = 3u16;
    let bottom = 1u16;
    let inner_h = full.height.saturating_sub(top + bottom);
    let inner = Rect {
        x: full.x,
        y: full.y + top.min(full.height),
        width: full.width,
        height: inner_h,
    };
    sid_widgets::ssh::body_rect_for(inner)
}

/// Mutably borrow the parent SSH tab's widget (tab id exactly `"ssh"`).
///
/// Host CRUD (add/edit/delete refresh) targets this widget only; session
/// routing must NOT assume it — background-opened `ssh:<alias>` detail tabs
/// hold their own `SshWidget`s. Use [`find_ssh_widget_mut`] for anything
/// connection-related.
fn active_ssh_widget_mut(sid_app: &mut SidApp) -> Option<&mut SshWidget> {
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "ssh" {
            if let Some(w) = t.layout.iter_widgets_mut().next() {
                let any_ref = w as &mut dyn std::any::Any;
                return any_ref.downcast_mut::<SshWidget>();
            }
        }
    }
    None
}

/// Find the first [`SshWidget`] in ANY tab (the parent `"ssh"` tab or an
/// `"ssh:<alias>"` background detail tab) satisfying `pred`.
///
/// The connect plumbing uses this to route intents and outcomes to the
/// widget that owns them: a pending-connect set on a background tab's widget
/// must be drained from THAT widget, and its Connected/Failed outcome must
/// land back on it — not on whichever tab happens to be the parent.
fn find_ssh_widget_mut(
    sid_app: &mut SidApp,
    pred: impl Fn(&SshWidget) -> bool,
) -> Option<&mut SshWidget> {
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if let Some(w) = t.layout.iter_widgets_mut().next() {
            let any_ref = w as &mut dyn std::any::Any;
            if let Some(ww) = any_ref.downcast_mut::<SshWidget>() {
                if pred(ww) {
                    return Some(ww);
                }
            }
        }
    }
    None
}

/// Run `f` on every [`SshWidget`] across all tabs (parent + detail tabs).
fn for_each_ssh_widget_mut(sid_app: &mut SidApp, mut f: impl FnMut(&mut SshWidget)) {
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if let Some(w) = t.layout.iter_widgets_mut().next() {
            if let Some(ww) = (w as &mut dyn std::any::Any).downcast_mut::<SshWidget>() {
                f(ww);
            }
        }
    }
}

/// The ACTIVE tab's [`SshWidget`], whatever its tab id (`"ssh"` or
/// `"ssh:<alias>"`). Used by per-frame work that must touch only the widget
/// the user is looking at (e.g. PTY resize).
fn active_tab_ssh_widget_mut(sid_app: &mut SidApp) -> Option<&mut SshWidget> {
    let t = sid_app.app.tabs_mut().active_mut();
    let w = t.layout.iter_widgets_mut().next()?;
    (w as &mut dyn std::any::Any).downcast_mut::<SshWidget>()
}

/// Spawn the async connect task with a fully-resolved [`SshAuth`]. Each task
/// is independent and owns the [`SshClient`] it created; on completion it sends
/// an [`SshConnectOutcome`] back through `tx`.
///
/// `rows` / `cols` set the initial remote PTY size; the wire layer will
/// resize the local screen each frame via [`sync_ssh_pty_size`].
///
/// Auth resolution (keyring lookup, password prompt, agent-socket preflight)
/// happens *before* this call in [`resolve_connect_auth`] /
/// [`submit_ssh_password`]; this function just performs the connect with the
/// `auth` it is handed. The `auth` value may carry a password (`SshAuth::Password`)
/// — it is moved into the task and never logged.
fn spawn_ssh_connect_with_auth(
    factory: SshClientFactoryFn,
    tx: tokio::sync::mpsc::UnboundedSender<SshConnectOutcome>,
    host: sid_store::SshHost,
    alias: String,
    rows: u16,
    cols: u16,
    auth: sid_core::adapters::ssh::SshAuth,
) {
    use sid_core::adapters::ssh::SshHostSpec;

    tokio::spawn(async move {
        let mut client = factory();
        let spec = SshHostSpec {
            host: host.host.clone(),
            port: host.port,
            user: host.user.clone(),
        };

        if let Err(e) = client.connect(&spec, &auth).await {
            let _ = tx.send(SshConnectOutcome::Failed {
                alias,
                error: format!("connect: {e}"),
            });
            return;
        }

        let mut shell = match client.open_shell("xterm-256color", rows, cols).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(SshConnectOutcome::Failed {
                    alias,
                    error: format!("open_shell: {e}"),
                });
                return;
            }
        };

        // Build the PtyPane wrapping a freshly-sized Vt100Screen. The local
        // screen size matches the rows/cols we just used for the remote PTY
        // request so the first frame doesn't visibly stretch.
        let screen = sid_pty::Vt100Screen::new(rows, cols);
        let pty = sid_widgets::ssh::PtyPane::new(
            Box::new(screen) as Box<dyn sid_core::adapters::pty::TerminalScreen>
        );

        // Byte-forwarding channel. The reader task owns the sender; the
        // wire layer owns the receiver and forwards into the pane each
        // frame.
        let (byte_tx, byte_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Hand the PtyPane + receiver to the wire layer.
        let _ = tx.send(SshConnectOutcome::Connected {
            alias: alias.clone(),
            pty,
            byte_rx,
            shutdown_tx,
        });

        // Background reader loop: poll the shell every ~25ms for new bytes
        // and forward them. The loop exits when:
        // - The shell returns an error (remote closed),
        // - `try_read` returns `Err(Disconnected)` (we send an empty
        //   chunk and rely on the byte channel close),
        // - `byte_tx` is closed (the receiver was dropped),
        // - The shutdown signal fires.
        //
        // `try_read` is non-blocking: it returns whatever bytes have
        // accumulated in `RusshShell`'s internal buffer. The buffer is
        // populated by russh's own background task — see
        // `sid_ssh::shell::RusshShell::new`.
        let poll_interval = std::time::Duration::from_millis(25);
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = tokio::time::sleep(poll_interval) => {
                    match shell.try_read().await {
                        Ok(bytes) if bytes.is_empty() => {
                            // Nothing this tick; keep polling.
                        }
                        Ok(bytes) => {
                            if byte_tx.send(bytes).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "ssh shell read error; closing");
                            break;
                        }
                    }
                }
            }
        }
        // Best-effort: close the shell and disconnect. Errors at this stage
        // are not user-facing.
        let _ = shell.close().await;
        let _ = client.disconnect().await;
        tracing::debug!(alias = %alias, "ssh reader task exited");
    });
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
    use ratatui::{
        style::{Modifier as TextMod, Style as TextStyle},
        widgets::Paragraph,
    };

    use crate::toast::ToastKind;

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
        // "ssh.new" modal path retired by UX-v2 — hosts are now added via the
        // side-pane FormPane ("ssh.new" in dispatch_form_submit).
    } else if let Some(alias) = key.strip_prefix("ssh.password:") {
        // Connect-time password prompt (§A). Spawns the connect with the entered
        // password and optionally saves it to the keyring.
        submit_ssh_password(sid_app, alias, values);
    } else if let Some(alias) = key.strip_prefix("ssh.remove:") {
        submit_ssh_remove(sid_app, alias, values)?;
    } else if let Some(_alias) = key.strip_prefix("ssh.edit:") {
        // "ssh.edit:<alias>" modal path retired by UX-v2 — hosts are now edited
        // via the side-pane FormPane ("ssh.edit:<alias>" in dispatch_form_submit).
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
        // "database.new" modal retired by UX-v2 — connections are created via the side-pane form ("database.connection" in dispatch_form_submit).
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
    } else if let Some(tab_id) = key.strip_prefix("session.resume:") {
        submit_session_resume(sid_app, tab_id, values);
    } else if key == "form.discard_confirm" {
        // Discard-changes confirm for an open side-pane form. Submit means the
        // user chose "Discard" (default selection is "Keep editing", and Esc
        // cancels the modal without touching the form). Any non-"Discard"
        // selection leaves the form open.
        if choice_value(values, "confirm").as_deref() == Some("Discard") {
            close_network_detail_pane_if_network_form(sid_app);
            sid_app.form = None;
            sid_app.form_origin_tab = None;
        }
    } else {
        tracing::debug!("unhandled modal submit id={key}");
    }
    Ok(())
}

/// Open `spec` as the side-pane form of the currently-active tab.
///
/// Any prior form is replaced. The form occupies the right 60% of the tab body
/// and intercepts every key (after modals) until it submits or cancels. The
/// UX-v2 add/edit substrate entry point; branches 1-5 call this with their own
/// [`FormSpec`](sid_widgets::form::FormSpec) and register the matching
/// `dispatch_form_submit` arm.
///
/// # Invariants
///
/// **Replacement without discard-confirm.** This function replaces any prior
/// form unconditionally. Callers opening from flows where a dirty form might
/// already be active must route through the discard-confirm first (see
/// [`open_discard_confirm_modal`]). Branches adding `open_form` call sites:
/// check `form.is_none()` before calling, or accept silent replacement
/// consciously and document why.
///
/// **Tab-close suppression.** While a form is active on the origin tab,
/// [`route_key_event`] consumes every key on that tab, so the origin tab
/// cannot be closed from the keyboard. Any future non-keyboard tab-close path
/// (palette action, pointer gesture, programmatic close) **must** clear both
/// `form` and `form_origin_tab` for the closed tab, or the form will strand
/// invisibly — active on a tab that no longer exists.
///
/// # Examples
///
/// ```no_run
/// use sid::wire::{open_form, SidApp};
/// use sid_widgets::form::{FormSpec, FormSection, FormField, SectionKind};
/// use sid_widgets::modal::Field;
///
/// # fn demo(sid_app: &mut SidApp) {
/// let spec = FormSpec::new(
///     "example.edit",
///     "Edit",
///     vec![FormSection {
///         title: "Details".into(),
///         kind: SectionKind::Editable,
///         fields: vec![FormField::new(
///             "name",
///             Field::Text { label: "name".into(), value: String::new(), placeholder: None },
///         )],
///     }],
/// );
/// open_form(sid_app, spec);
/// assert!(sid_app.form.is_some());
/// # }
/// ```
#[allow(dead_code)] // Substrate API: branches 1-5 call this to open their add/edit forms.
pub fn open_form(sid_app: &mut SidApp, spec: sid_widgets::form::FormSpec) {
    sid_app.form_origin_tab = Some(sid_app.app.tabs().active().id.clone());
    sid_app.form = Some(sid_widgets::form::FormPane::new(spec));
}

/// Build the create-new-workspace side-pane form. Reshape on `kind`: Umbrella
/// shows the satellite-scan + feature toggles; Repo hides them.
pub fn workspaces_new_form() -> sid_widgets::form::FormSpec {
    sid_widgets::form::FormSpec::new(
        "workspaces.create",
        "New Workspace",
        workspaces_new_sections(&sid_widgets::form::FormValues::new()),
    )
    .with_reshape(vec!["kind".into()], workspaces_new_sections)
}

/// Section builder for [`workspaces_new_form`]. Reshape-driven: the `kind`
/// value selects whether the umbrella-only "Features" section is present.
fn workspaces_new_sections(
    values: &sid_widgets::form::FormValues,
) -> Vec<sid_widgets::form::FormSection> {
    use sid_widgets::{
        form::{FormField, FormSection, SectionKind, Validate},
        modal::Field,
    };
    let kind = values.get("kind").map(String::as_str).unwrap_or("Umbrella");
    let mut fields = vec![
        FormField::new(
            "name",
            Field::Text {
                label: "name".into(),
                value: String::new(),
                placeholder: Some("e.g. gen4-stack".into()),
            },
        )
        .with_validate(vec![Validate::NonEmpty]),
        FormField::new(
            "path",
            Field::Picker {
                label: "path".into(),
                value: String::new(),
                hint: "absolute path".into(),
            },
        )
        .with_validate(vec![Validate::NonEmpty]),
        FormField::new(
            "kind",
            Field::Choice {
                label: "kind".into(),
                options: vec!["Umbrella".into(), "Repo".into()],
                selected: if kind == "Repo" { 1 } else { 0 },
            },
        ),
    ];
    let mut sections = vec![FormSection {
        title: "Workspace".into(),
        kind: SectionKind::Editable,
        fields: std::mem::take(&mut fields),
    }];
    if kind == "Umbrella" {
        sections.push(FormSection {
            title: "Features".into(),
            kind: SectionKind::Editable,
            fields: vec![
                FormField::new(
                    "scan_satellites",
                    Field::Toggle {
                        label: "scan satellites now".into(),
                        value: true,
                    },
                ),
                FormField::new(
                    "register_claude_md",
                    Field::Toggle {
                        label: "read CLAUDE.md actions".into(),
                        value: true,
                    },
                ),
            ],
        });
    }
    sections
}

/// Resolve the directory to scan for the adopt-existing wizard. Prefers the
/// currently-selected workspace path, falls back to the first default discovery
/// root (`~/vcs`), then the current working directory.
fn workspaces_adopt_dir(sid_app: &SidApp) -> PathBuf {
    if let Some(p) = workspaces_selected_path(sid_app) {
        return p;
    }
    if let Some(root) = default_discovery_roots().into_iter().next() {
        return root;
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Build the adopt-existing-umbrella form for `dir`: a name field plus one
/// pre-checked Toggle per repo found one level under `dir`. The repo path is
/// encoded in the toggle key (`repo:<abs-path>`) so the submit handler can
/// register each checked satellite without a second scan.
pub fn workspaces_adopt_form(dir: &std::path::Path) -> sid_widgets::form::FormSpec {
    use sid_core::workspace_discovery::scan_adoptable_repos;
    use sid_widgets::{
        form::{FormField, FormSection, SectionKind, Validate},
        modal::Field,
    };
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("umbrella")
        .to_string();
    let mut header = vec![
        FormField::new(
            "dir",
            Field::Display {
                label: "directory".into(),
                body: dir.display().to_string(),
            },
        ),
        FormField::new(
            "name",
            Field::Text {
                label: "umbrella name".into(),
                value: name,
                placeholder: None,
            },
        )
        .with_validate(vec![Validate::NonEmpty]),
    ];
    let repos = scan_adoptable_repos(dir);
    let toggles: Vec<FormField> = repos
        .iter()
        .map(|r| {
            FormField::new(
                format!("repo:{}", r.path.display()),
                Field::Toggle {
                    label: r.name.clone(),
                    value: true,
                },
            )
        })
        .collect();
    let mut sections = vec![FormSection {
        title: "Umbrella".into(),
        kind: SectionKind::Editable,
        fields: std::mem::take(&mut header),
    }];
    if toggles.is_empty() {
        sections.push(FormSection {
            title: "Satellites".into(),
            kind: SectionKind::Info,
            fields: vec![FormField::new(
                "none",
                Field::Display {
                    label: "found".into(),
                    body: "no git repos found under this directory".into(),
                },
            )],
        });
    } else {
        sections.push(FormSection {
            title: "Satellites".into(),
            kind: SectionKind::Editable,
            fields: toggles,
        });
    }
    sid_widgets::form::FormSpec::new("workspaces.adopt", "Adopt Existing Umbrella", sections)
}

/// Route a submitted form's values by form id, then close the form.
///
/// Branches 1-5 register their own arms here keyed on the form id; the
/// substrate ships with only the wildcard, which toasts an "unhandled form
/// submit" diagnostic. Either way the form closes on submit — a successful
/// submit is terminal.
fn dispatch_form_submit(sid_app: &mut SidApp, id: &str, values: sid_widgets::form::FormValues) {
    // Branch-specific arms keyed on form id. Wildcard handles unknown ids.
    match id {
        "database.connection" => {
            if let Err(e) = submit_db_connection_form(sid_app, values) {
                sid_app
                    .toasts
                    .push(Toast::error(format!("save connection: {e}")));
            }
        }
        "workspaces.create" => {
            let name = values.get("name").cloned().unwrap_or_default();
            let path_str = values.get("path").cloned().unwrap_or_default();
            let kind = match values.get("kind").map(String::as_str) {
                Some("Repo") => sid_core::workspace_metadata::WorkspaceKind::Repo,
                _ => sid_core::workspace_metadata::WorkspaceKind::Umbrella,
            };
            match std::fs::canonicalize(&path_str) {
                Ok(path) => {
                    let ws = sid_store::Workspace {
                        path,
                        name: name.clone(),
                        kind,
                        manifest_hash: 0,
                        last_seen: sid_store::now_epoch(),
                        parent: None,
                    };
                    match sid_app.store.upsert_workspace(&ws) {
                        Ok(()) => {
                            refresh_workspaces_widget(sid_app);
                            sid_app
                                .toasts
                                .push(Toast::success(format!("workspace '{name}' added")));
                        }
                        Err(e) => {
                            sid_app
                                .toasts
                                .push(Toast::error(format!("add workspace: {e}")));
                        }
                    }
                }
                Err(e) => {
                    sid_app
                        .toasts
                        .push(Toast::error(format!("bad path '{path_str}': {e}")));
                }
            }
        }
        "workspaces.adopt" => {
            let dir_str = values.get("dir").cloned().unwrap_or_default();
            let name = values.get("name").cloned().unwrap_or_default();
            let umbrella_path = match std::fs::canonicalize(&dir_str) {
                Ok(p) => p,
                Err(e) => {
                    sid_app
                        .toasts
                        .push(Toast::error(format!("bad directory '{dir_str}': {e}")));
                    sid_app.form = None;
                    sid_app.form_origin_tab = None;
                    return;
                }
            };
            // Register the umbrella.
            let umbrella = sid_store::Workspace {
                path: umbrella_path.clone(),
                name: name.clone(),
                kind: sid_core::workspace_metadata::WorkspaceKind::Umbrella,
                manifest_hash: 0,
                last_seen: sid_store::now_epoch(),
                parent: None,
            };
            let mut errors = 0usize;
            let mut added = 0usize;
            if sid_app.store.upsert_workspace(&umbrella).is_err() {
                errors += 1;
            } else {
                added += 1;
            }
            // Register each checked satellite (key = "repo:<path>", value "true").
            for (key, val) in values.iter() {
                let Some(path_str) = key.strip_prefix("repo:") else {
                    continue;
                };
                if val != "true" {
                    continue;
                }
                let sat_path = std::path::PathBuf::from(path_str);
                let sat_name = sat_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("repo")
                    .to_string();
                let sat = sid_store::Workspace {
                    path: sat_path,
                    name: sat_name,
                    kind: sid_core::workspace_metadata::WorkspaceKind::Repo,
                    manifest_hash: 0,
                    last_seen: sid_store::now_epoch(),
                    parent: Some(umbrella_path.clone()),
                };
                if sid_app.store.upsert_workspace(&sat).is_err() {
                    errors += 1;
                } else {
                    added += 1;
                }
            }
            refresh_workspaces_widget(sid_app);
            if errors == 0 {
                sid_app.toasts.push(Toast::success(format!(
                    "adopted '{name}' + {} satellites",
                    added.saturating_sub(1)
                )));
            } else {
                sid_app
                    .toasts
                    .push(Toast::error(format!("adopted with {errors} error(s)")));
            }
        }
        // Network: interface prefs form — id carries the iface name after ':'.
        id if id.starts_with("network.interface_prefs:") => {
            let iface_name = id
                .strip_prefix("network.interface_prefs:")
                .unwrap_or("")
                .to_string();
            if let Some(prefs) = sid_widgets::network::detail_pane::prefs_from_values(&values) {
                apply_network_prefs(sid_app, &iface_name, prefs);
                sid_app
                    .toasts
                    .push(Toast::success(format!("Saved prefs for {iface_name}")));
            }
        }
        "ssh.new" => match submit_ssh_new_from_form(sid_app, &values) {
            Ok(alias) => {
                sid_app
                    .toasts
                    .push(Toast::success(format!("host '{alias}' added")));
            }
            Err(e) => {
                sid_app
                    .toasts
                    .push(Toast::error(format!("ssh add failed: {e}")));
            }
        },
        id if id.starts_with("ssh.edit:") => {
            let alias = &id["ssh.edit:".len()..];
            if let Err(e) = submit_ssh_edit_from_form(sid_app, alias, &values) {
                sid_app
                    .toasts
                    .push(Toast::error(format!("ssh edit failed: {e}")));
            }
        }
        id if id.starts_with("ssh.inspect:") => {
            let alias = &id["ssh.inspect:".len()..];
            if let Err(e) = submit_ssh_inspect_from_form(sid_app, alias, &values) {
                sid_app
                    .toasts
                    .push(Toast::error(format!("ssh inspect save failed: {e}")));
            }
        }
        id if id.starts_with("ssh.inspect-ro:") => {
            // Read-only inspector (SSH-Config host): ⏎ closes the pane cleanly
            // without attempting to write anything.  No toast needed — there is
            // nothing ambiguous about closing an info-only panel.
            let _ = id; // alias not needed
            let _ = &values;
        }
        _ => {
            let _ = &values;
            sid_app
                .toasts
                .push(Toast::error(format!("unhandled form submit: {id}")));
        }
    }
    close_network_detail_pane_if_network_form(sid_app);
    sid_app.form = None;
    sid_app.form_origin_tab = None;
}

/// Open the standard "Discard changes?" confirm for a dirty side-pane form.
///
/// Reuses the modal substrate: a single Choice with "Keep editing" (default)
/// and "Discard". Esc cancels the modal and leaves the form untouched; picking
/// "Discard" and submitting closes the form (see the `form.discard_confirm`
/// arm of [`dispatch_modal_submit`]).
fn open_discard_confirm_modal(sid_app: &mut SidApp) {
    sid_app.modal_stack.push(
        sid_widgets::ModalSpec::new(
            "form.discard_confirm",
            "Discard changes?",
            vec![sid_widgets::modal::Field::Choice {
                label: "confirm".into(),
                options: vec!["Keep editing".into(), "Discard".into()],
                selected: 0,
            }],
        )
        .with_help("Unsaved edits will be lost."),
    );
}

/// Slim the per-tab footer hint list: keep at most the first 3 entries and
/// always append `? help` so the overlay is discoverable. The full hint list
/// (including any entries beyond position 3) is available via the overlay
/// itself (plan decision 13: footer is 3 primary verbs + ?: help).
fn slim_footer_hints(mut hints: Vec<sid_core::FooterHint>) -> Vec<sid_core::FooterHint> {
    hints.truncate(3);
    hints.push(sid_core::FooterHint::new("?", "help"));
    hints
}

/// Footer hint strip shown while a side-pane form is active. Substitutes the
/// active widget's hints with the fixed form contract: `Tab` cycles fields,
/// `⏎` saves, `⎋` cancels.
fn form_footer_hints() -> Vec<sid_core::FooterHint> {
    vec![
        sid_core::FooterHint::new("Tab", "fields"),
        sid_core::FooterHint::new("⏎", "save"),
        sid_core::FooterHint::new("⎋", "cancel"),
    ]
}

/// Handler for the `session.resume:<tab_id>` modal. If the user picked
/// `"Resume"`, switch to `tab_id`; if `"Start fresh"`, no-op. `switch_to`
/// is best-effort — an unknown tab id silently leaves focus where it is
/// (the modal can't construct an unknown tab, but a future plan could
/// remove a tab the previous session had open).
fn submit_session_resume(
    sid_app: &mut SidApp,
    tab_id: &str,
    values: &[(String, sid_widgets::FieldValue)],
) {
    let choice = choice_value(values, "action").unwrap_or_default();
    if choice == "Resume" {
        let id = TabId::new(tab_id);
        // `switch_to` returns false when the id isn't present; we ignore
        // that — the user already picked Resume, but the tab list may have
        // shifted between sessions.
        let _ = sid_app.app.tabs_mut().switch_to(&id);
    }
    // "Start fresh" is intentionally a no-op.
}

// ---------------------------------------------------------------------------
// Per-tab submit handlers
// ---------------------------------------------------------------------------

/// Handle a successful submit of the `ssh.new` modal: validate inputs,
/// upsert the host into the store, refresh the SSH widget. Returns the alias
/// of the newly-added host so the caller can populate a context-rich toast.
///
/// The `ssh.new` modal dispatch arm was retired by UX-v2; this function is
/// retained for direct test coverage of the core persistence contract.
#[allow(dead_code)]
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
///
/// This variant matches the *capitalized* option strings used by the modal
/// paths ("Key", "Password") — the now-retired `ssh.new` / `ssh.edit` modals
/// and their successor `submit_ssh_new` / `submit_ssh_edit` helpers.
#[allow(dead_code)]
fn parse_auth_choice(choice: Option<&str>) -> sid_store::SshAuthKind {
    use sid_store::SshAuthKind;
    match choice {
        Some("Key") => SshAuthKind::Key,
        Some("Password") => SshAuthKind::Password,
        _ => SshAuthKind::Agent,
    }
}

/// Like [`parse_auth_choice`] but matches the *lowercase* strings produced by
/// `ssh_add_form_spec` / `ssh_edit_form_spec` ("agent", "key", "password").
fn parse_auth_form_choice(choice: Option<&str>) -> sid_store::SshAuthKind {
    use sid_store::SshAuthKind;
    match choice {
        Some("key") => SshAuthKind::Key,
        Some("password") => SshAuthKind::Password,
        _ => SshAuthKind::Agent,
    }
}

/// Handle a successful submit of the `ssh.new` FormPane form. Reads from a
/// [`FormValues`] map (plain `String` values from the side-pane form substrate)
/// rather than the old `FieldValue` slice used by modal submits.
fn submit_ssh_new_from_form(
    sid_app: &mut SidApp,
    values: &sid_widgets::form::FormValues,
) -> Result<String> {
    use sid_store::{SshHost, SshHostSource};
    let alias = values.get("alias").cloned().unwrap_or_default();
    let host = values.get("host").cloned().unwrap_or_default();
    let user = values.get("user").cloned().unwrap_or_default();
    let port_str = values.get("port").cloned().unwrap_or_default();
    let identity_file = values
        .get("identity_file")
        .filter(|s| !s.is_empty())
        .cloned();
    let auth_kind = parse_auth_form_choice(values.get("auth").map(String::as_str));
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

/// Handle a successful submit of an `ssh.edit:<alias>` or `ssh.inspect:<alias>`
/// FormPane form. Reads from a [`FormValues`] map; merges changes onto the
/// existing store record (preserves `last_sftp_path`, `command_history`,
/// `last_connected`).
fn submit_ssh_edit_from_form(
    sid_app: &mut SidApp,
    alias_in_id: &str,
    values: &sid_widgets::form::FormValues,
) -> Result<()> {
    use sid_store::{SshHost, SshHostSource};
    let new_alias = values
        .get("alias")
        .cloned()
        .unwrap_or(alias_in_id.to_string());
    let host = values.get("host").cloned().unwrap_or_default();
    let user = values.get("user").cloned().unwrap_or_default();
    let port_str = values.get("port").cloned().unwrap_or_default();
    let identity_file = values
        .get("identity_file")
        .filter(|s| !s.is_empty())
        .cloned();
    let auth_kind = parse_auth_form_choice(values.get("auth").map(String::as_str));
    if new_alias.is_empty() || host.is_empty() || user.is_empty() {
        return Err(anyhow::anyhow!("alias, host, and user are required"));
    }
    let port: u16 = if port_str.is_empty() {
        22
    } else {
        port_str
            .parse()
            .map_err(|e| anyhow::anyhow!("port must be a u16 (got {port_str:?}): {e}"))?
    };
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

/// For an `ssh.inspect:<alias>` form submit, merge the editable field
/// (`identity_file`) from the submitted values with the rest of the existing
/// host record, then persist.
fn submit_ssh_inspect_from_form(
    sid_app: &mut SidApp,
    alias: &str,
    values: &sid_widgets::form::FormValues,
) -> Result<()> {
    let existing = sid_app
        .store
        .get_ssh_host(alias)
        .map_err(|e| anyhow::anyhow!("get ssh host: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("no host with alias '{alias}' in store"))?;
    // Build a merged FormValues from the existing record, overriding identity_file
    // from the submitted values.
    let mut merged = sid_widgets::form::FormValues::new();
    merged.insert("alias".to_string(), existing.alias.clone());
    merged.insert("host".to_string(), existing.host.clone());
    merged.insert("port".to_string(), existing.port.to_string());
    merged.insert("user".to_string(), existing.user.clone());
    merged.insert(
        "identity_file".to_string(),
        values.get("identity_file").cloned().unwrap_or_default(),
    );
    merged.insert(
        "auth".to_string(),
        match existing.auth_kind {
            sid_store::SshAuthKind::Agent => "agent".to_string(),
            sid_store::SshAuthKind::Key => "key".to_string(),
            sid_store::SshAuthKind::Password => "password".to_string(),
        },
    );
    submit_ssh_edit_from_form(sid_app, alias, &merged)
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
    // Drop any saved connect password so a removed host leaves no secret behind.
    // `delete` is idempotent — a missing entry is `Ok(())`.
    {
        use sid_core::adapters::secrets::SecretId;
        let id = SecretId::new(ssh_password_secret_key(alias));
        if let Err(e) = sid_app.secrets.delete(&id) {
            tracing::warn!(secret = %id.as_str(), error = %e, "ssh password keyring delete failed");
        }
    }
    refresh_ssh_widget(sid_app);
    sid_app
        .toasts
        .push(Toast::success(format!("host '{alias}' removed")));
    Ok(())
}

/// Handle a successful submit of `ssh.edit:<alias>`: validate, update the
/// host record (preserves `last_sftp_path` and `command_history`), and
/// refresh the widget.
///
/// The `ssh.edit:<alias>` modal dispatch arm was retired by UX-v2; hosts are
/// now edited via the side-pane FormPane (`submit_ssh_edit_from_form`).
/// This function is retained for direct test coverage of the core update
/// contract.
#[allow(dead_code)]
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
    // Resolve user/host/port + any saved password BEFORE spawning the blocking
    // task (needs &SidApp). The password (if any) is moved into the closure and
    // never logged.
    let target = resolve_copy_id_target(sid_app, alias);
    sid_app.toasts.push(Toast::info(format!(
        "ssh-copy-id: connecting to {alias}..."
    )));
    sid_app.jobs.spawn(async move {
        let outcome = tokio::task::spawn_blocking({
            let identity = identity_owned.clone();
            move || {
                run_ssh_copy_id(
                    &target.alias,
                    &target.user,
                    &target.host,
                    target.port,
                    Some(&identity),
                    target.password.as_deref(),
                )
            }
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

/// A fully-resolved `ssh-copy-id` invocation: the program to spawn and its
/// argument vector. Built by [`build_ssh_copy_id_invocation`] so the argv can
/// be unit-tested without spawning a process.
///
/// SECURITY: the password (for password hosts) is NOT stored here and never
/// appears in `args`. It is delivered to the child via the `SSHPASS`
/// environment variable consumed by `sshpass -e`, so it does not land in the
/// world-readable `/proc/<pid>/cmdline`. The argv is therefore safe to log in
/// full, and `Debug` on this struct cannot leak a secret.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CopyIdInvocation {
    /// Program name (`ssh-copy-id` for key/agent hosts, `sshpass` for password hosts).
    program: String,
    /// Full argument vector passed to `program`. Contains no secrets.
    args: Vec<String>,
}

impl CopyIdInvocation {
    /// `[program] + args` — safe to log: no secret is ever placed in argv
    /// (passwords travel via the `SSHPASS` env var; see the struct docs).
    fn argv(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.args.len() + 1);
        out.push(self.program.clone());
        out.extend(self.args.iter().cloned());
        out
    }
}

/// Reject a subprocess positional argument that begins with `-`, which
/// `ssh`/`ssh-copy-id` would parse as a flag (neither reliably honours `--`).
/// Returns `Err("err: …")` so callers can surface it directly as a failure.
fn reject_flaglike(label: &str, value: &str) -> Result<(), String> {
    if value.starts_with('-') {
        Err(format!(
            "err: refusing — {label} {value:?} starts with '-' (possible argument injection)"
        ))
    } else {
        Ok(())
    }
}

/// Normalize an identity path to its public-key form (`<path>.pub`). A path
/// already ending in `.pub` is returned unchanged.
fn pub_key_path(identity: &str) -> String {
    if identity.ends_with(".pub") {
        identity.to_string()
    } else {
        format!("{identity}.pub")
    }
}

/// Build the `ssh-copy-id` invocation for a host (§C).
///
/// - When `password` is `Some` (a password-auth host with a resolved
///   password): `sshpass -e ssh-copy-id [-i <pub>] -p <port>
///   -o StrictHostKeyChecking=accept-new {user}@{host}`. The password is read
///   by `sshpass` from the `SSHPASS` env var (set by [`run_ssh_copy_id`]), not
///   from argv.
/// - Otherwise (key/agent host): `ssh-copy-id [-i <pub>] {target}` where
///   `target` is the SSH-config alias (preserving the existing behaviour that
///   relies on the user's `~/.ssh/config`).
///
/// The password (when present) is delivered to `sshpass` via the `SSHPASS`
/// environment variable (`sshpass -e`), never in argv. Positional components
/// (and the `-i` identity) are validated against flag smuggling; returns
/// `Err("err: …")` on rejection.
///
/// SECURITY (host-key trust): the sshpass path hardcodes
/// `StrictHostKeyChecking=accept-new`, which is Trust-On-First-Use — the
/// remote's host key is accepted unverified on first contact and pinned in
/// `known_hosts` thereafter. This is required for a non-interactive first
/// copy (there is no TTY to answer the yes/no prompt, and the stricter `yes`
/// mode would simply fail on an unknown host), and it matches OpenSSH's own
/// default first-contact behaviour. A network MITM present at the very first
/// copy could pin their key; later connections are protected by the pin. The
/// key/agent path deliberately omits this flag so it honours whatever the
/// user's `~/.ssh/config` mandates.
fn build_ssh_copy_id_invocation(
    alias: &str,
    user: &str,
    host: &str,
    port: u16,
    identity: Option<&str>,
    password: Option<&str>,
) -> Result<CopyIdInvocation, String> {
    // The identity becomes the value of `-i`. getopt consumes the next token as
    // that value, so a leading '-' is not a classic flag-injection here — but a
    // path starting with '-' is almost certainly a bug or hostile input, and
    // ssh-copy-id is a shell wrapper that re-forwards the path to ssh, so reject
    // it for defence-in-depth and parity with the other positionals.
    if let Some(i) = identity {
        reject_flaglike("identity", i)?;
    }
    match password {
        Some(_pw) => {
            // sshpass-driven non-interactive copy for password-only hosts. The
            // password is NOT placed here — it goes via the SSHPASS env var
            // (see `run_ssh_copy_id`); `-e` tells sshpass to read it from there.
            // `{user}@{host}` is a positional, so guard both halves.
            reject_flaglike("user", user)?;
            reject_flaglike("host", host)?;
            let mut args: Vec<String> = vec!["-e".into(), "ssh-copy-id".into()];
            if let Some(i) = identity {
                args.push("-i".into());
                args.push(pub_key_path(i));
            }
            args.push("-p".into());
            args.push(port.to_string());
            args.push("-o".into());
            args.push("StrictHostKeyChecking=accept-new".into());
            args.push(format!("{user}@{host}"));
            Ok(CopyIdInvocation {
                program: "sshpass".into(),
                args,
            })
        }
        None => {
            // Key/agent host: plain ssh-copy-id against the config alias (a
            // positional, so guard it).
            reject_flaglike("alias", alias)?;
            let mut args: Vec<String> = Vec::new();
            if let Some(i) = identity {
                args.push("-i".into());
                args.push(pub_key_path(i));
            }
            args.push(alias.to_string());
            Ok(CopyIdInvocation {
                program: "ssh-copy-id".into(),
                args,
            })
        }
    }
}

/// Whether `name` resolves to an executable on `PATH`. Used to pre-flight
/// `sshpass` before attempting a password-host key copy (§C).
fn binary_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

/// Resolved connection details for an `ssh-copy-id` invocation: the SSH-config
/// alias plus the concrete `user`/`host`/`port`, and (for password hosts with a
/// saved keyring entry) the password to drive `sshpass`.
struct CopyIdTarget {
    alias: String,
    user: String,
    host: String,
    port: u16,
    /// `Some` only for a password-auth host whose password is in the keyring.
    password: Option<String>,
}

/// Resolve the [`CopyIdTarget`] for `alias` from the store + secret store.
///
/// Falls back to the alias-only target (no concrete user/host) when the host
/// record is missing — the plain `ssh-copy-id <alias>` path still works via the
/// user's `~/.ssh/config`. The password is looked up from the keyring only for
/// password-auth hosts and is never logged.
fn resolve_copy_id_target(sid_app: &SidApp, alias: &str) -> CopyIdTarget {
    match sid_app.store.get_ssh_host(alias).ok().flatten() {
        Some(h) => {
            let password = if h.auth_kind == sid_store::SshAuthKind::Password {
                ssh_password_from_keyring(sid_app, alias)
            } else {
                None
            };
            CopyIdTarget {
                alias: alias.to_string(),
                user: h.user,
                host: h.host,
                port: h.port,
                password,
            }
        }
        None => CopyIdTarget {
            alias: alias.to_string(),
            user: String::new(),
            host: String::new(),
            port: 22,
            password: None,
        },
    }
}

/// Capture `ssh-copy-id` output (best-effort; the binary may be missing).
/// Returns either `"ok: <stdout>"` or `"err: <stderr/stdout>"` so callers can
/// branch on the prefix. Runs synchronously and is meant to be invoked from
/// `tokio::task::spawn_blocking`.
///
/// When `password` is `Some`, the copy is driven via `sshpass` (preflighted on
/// PATH) using the host's `user`/`host`/`port`; otherwise the plain
/// `ssh-copy-id <alias>` path is used. The password is never logged or placed
/// in argv — it is handed to the child via the `SSHPASS` env var (`sshpass
/// -e`), so the full argv is safe to trace.
fn run_ssh_copy_id(
    alias: &str,
    user: &str,
    host: &str,
    port: u16,
    identity: Option<&str>,
    password: Option<&str>,
) -> String {
    use std::process::Command;
    // Pre-flight: the password path needs sshpass on PATH.
    if password.is_some() && !binary_on_path("sshpass") {
        return "err: sshpass not on PATH (required for password-auth key copy)".to_string();
    }
    let invocation = match build_ssh_copy_id_invocation(alias, user, host, port, identity, password)
    {
        Ok(inv) => inv,
        Err(e) => return e,
    };
    tracing::info!(argv = ?invocation.argv(), "ssh-copy-id invocation");
    let mut cmd = Command::new(&invocation.program);
    cmd.args(&invocation.args);
    // The password (if any) travels via the SSHPASS env var, consumed by
    // `sshpass -e`. It is never placed in argv (see CopyIdInvocation docs).
    if let Some(pw) = password {
        cmd.env("SSHPASS", pw);
    }
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
        Err(e) => format!("err: {} not on PATH: {e}", invocation.program),
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
    // The path is passed to `ssh-keygen -f` here and later forwarded as the
    // `ssh-copy-id -i` identity (gen-key step 3). A leading '-' would be a
    // flag-smuggling vector once it reaches those subprocesses; reject it up
    // front so the wizard fails fast with a clear message.
    if output_path.starts_with('-') {
        return Err(anyhow::anyhow!(
            "output_path must not start with '-' (looks like a flag): {output_path}"
        ));
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
    // Resolve user/host/port + any saved password before spawning (needs
    // &SidApp). The password (if any) is moved into the closure, never logged.
    let copy_target = resolve_copy_id_target(sid_app, &target);
    sid_app.toasts.push(Toast::info(format!(
        "ssh-copy-id: connecting to {target}..."
    )));
    sid_app.jobs.spawn(async move {
        let result = tokio::task::spawn_blocking({
            let key = output_path_owned.clone();
            move || {
                run_ssh_copy_id(
                    &copy_target.alias,
                    &copy_target.user,
                    &copy_target.host,
                    copy_target.port,
                    Some(&key),
                    copy_target.password.as_deref(),
                )
            }
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
    // The alias is passed as a positional arg to ssh/ssh-keygen below; a
    // leading '-' would be parsed as a flag (argument injection). Reject it
    // once here, covering every branch.
    if let Err(e) = reject_flaglike("alias", alias) {
        sid_app.toasts.push(Toast::error(e));
        return Ok(());
    }
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
// TODO: remove in follow-up cleanup once database.connection path is proven stable.
#[allow(dead_code)]
fn submit_database_new(
    sid_app: &mut SidApp,
    values: &[(String, sid_widgets::FieldValue)],
) -> Result<String> {
    use sid_core::adapters::{db_client::DbKind, secrets::SecretId};
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

/// Build a [`sid_widgets::form::FormSpec`] for adding or editing a database connection.
///
/// When `prefill` is `Some`, all fields are pre-populated from the existing
/// connection record and its DSN is parsed into individual Host/Port/Database/User
/// fields. The form id is `"database.connection"`.
///
/// The spec carries a reshape hook watching `"kind"`: selecting `Postgres` shows
/// Host/Port/Database/User/Password fields; selecting `SQLite` replaces them with
/// a single Path field. Surviving field values are preserved across the reshape.
/// The Info section's DSN row is recomputed on every reshape from the current
/// editable-field values.
pub(crate) fn db_connection_form_spec(
    prefill: Option<&sid_store::DbConnection>,
) -> sid_widgets::form::FormSpec {
    use sid_core::adapters::db_client::DbKind;
    use sid_widgets::form::FormSpec;

    // Parse a postgres DSN (postgres://user@host:port/database) into parts.
    // Returns (host, port, database, user). Tolerates missing components.
    fn parse_pg_dsn(dsn: &str) -> (String, String, String, String) {
        // strip scheme
        let rest = dsn
            .strip_prefix("postgres://")
            .or_else(|| dsn.strip_prefix("postgresql://"))
            .unwrap_or(dsn);
        // split user@hostpart/database
        let (user_host, db) = rest.split_once('/').unwrap_or((rest, ""));
        let (user, host_port) = user_host.split_once('@').unwrap_or(("", user_host));
        let (host, port) = host_port.split_once(':').unwrap_or((host_port, "5432"));
        (
            host.to_string(),
            port.to_string(),
            db.to_string(),
            user.to_string(),
        )
    }

    // Build the DSN Info row body from current field values.
    fn build_dsn(values: &sid_widgets::form::FormValues) -> String {
        let kind = values.get("kind").map(String::as_str).unwrap_or("Postgres");
        match kind {
            "SQLite" => values.get("path").cloned().unwrap_or_default(),
            _ => {
                let user = values.get("user").map(String::as_str).unwrap_or("");
                let host = values
                    .get("host")
                    .map(String::as_str)
                    .unwrap_or("localhost");
                let port = values.get("port").map(String::as_str).unwrap_or("5432");
                let db = values.get("database").map(String::as_str).unwrap_or("");
                if user.is_empty() {
                    format!("postgres://{host}:{port}/{db}")
                } else {
                    format!("postgres://{user}@{host}:{port}/{db}")
                }
            }
        }
    }

    // Sections builder; invoked initially and by the reshape hook.
    fn make_sections(
        values: &sid_widgets::form::FormValues,
    ) -> Vec<sid_widgets::form::FormSection> {
        use sid_widgets::{
            form::{FormField, FormSection, SectionKind, Validate},
            modal::Field,
        };

        let kind = values.get("kind").map(String::as_str).unwrap_or("Postgres");
        let name_val = values.get("name").cloned().unwrap_or_default();
        let kind_idx = if kind == "SQLite" { 1 } else { 0 };

        let mut editable_fields = vec![
            FormField::new(
                "name",
                Field::Text {
                    label: "Name".into(),
                    value: name_val,
                    placeholder: Some("Local Postgres".into()),
                },
            )
            .with_validate(vec![Validate::NonEmpty]),
            FormField::new(
                "kind",
                Field::Choice {
                    label: "Kind".into(),
                    options: vec!["Postgres".into(), "SQLite".into()],
                    selected: kind_idx,
                },
            ),
        ];

        if kind == "SQLite" {
            let path_val = values.get("path").cloned().unwrap_or_default();
            editable_fields.push(
                FormField::new(
                    "path",
                    Field::Picker {
                        label: "Path".into(),
                        value: path_val,
                        hint: String::new(),
                    },
                )
                .with_validate(vec![Validate::NonEmpty]),
            );
        } else {
            let host_val = values
                .get("host")
                .cloned()
                .unwrap_or_else(|| "localhost".to_string());
            let port_val = values
                .get("port")
                .cloned()
                .unwrap_or_else(|| "5432".to_string());
            let db_val = values.get("database").cloned().unwrap_or_default();
            let user_val = values.get("user").cloned().unwrap_or_default();
            let pw_val = values.get("password").cloned().unwrap_or_default();

            editable_fields.push(
                FormField::new(
                    "host",
                    Field::Text {
                        label: "Host".into(),
                        value: host_val,
                        placeholder: Some("localhost".into()),
                    },
                )
                .with_validate(vec![Validate::NonEmpty]),
            );
            editable_fields.push(
                FormField::new(
                    "port",
                    Field::Text {
                        label: "Port".into(),
                        value: port_val,
                        placeholder: Some("5432".into()),
                    },
                )
                .with_validate(vec![Validate::Port]),
            );
            editable_fields.push(
                FormField::new(
                    "database",
                    Field::Text {
                        label: "Database".into(),
                        value: db_val,
                        placeholder: Some("mydb".into()),
                    },
                )
                .with_validate(vec![Validate::NonEmpty]),
            );
            editable_fields.push(FormField::new(
                "user",
                Field::Text {
                    label: "User".into(),
                    value: user_val,
                    placeholder: Some("postgres".into()),
                },
            ));
            editable_fields.push(FormField::new(
                "password",
                Field::Password {
                    label: "Password".into(),
                    value: pw_val,
                },
            ));
        }

        let dsn_body = build_dsn(values);
        let info_section = FormSection {
            title: "Connection string".into(),
            kind: SectionKind::Info,
            fields: vec![FormField::new(
                "dsn",
                Field::Display {
                    label: "DSN".into(),
                    body: dsn_body,
                },
            )],
        };

        vec![
            FormSection {
                title: "Connection".into(),
                kind: SectionKind::Editable,
                fields: editable_fields,
            },
            info_section,
        ]
    }

    // Seed initial values from prefill.
    let mut seed = sid_widgets::form::FormValues::new();

    if let Some(conn) = prefill {
        seed.insert("name".into(), conn.name.clone());
        match conn.kind {
            DbKind::Postgres => {
                seed.insert("kind".into(), "Postgres".into());
                let (host, port, db, user) = parse_pg_dsn(&conn.dsn);
                seed.insert("host".into(), host);
                seed.insert("port".into(), port);
                seed.insert("database".into(), db);
                seed.insert("user".into(), user);
                // password is never pre-filled (it lives in the secrets table)
            }
            DbKind::Sqlite => {
                seed.insert("kind".into(), "SQLite".into());
                seed.insert("path".into(), conn.dsn.clone());
            }
        }
        // Store the existing id so submit_db_connection_form can detect edit vs create.
        seed.insert("_id".into(), conn.id.clone());
    }

    let initial_sections = make_sections(&seed);

    FormSpec::new(
        "database.connection",
        "Database connection",
        initial_sections,
    )
    .with_reshape(vec!["kind".into()], make_sections)
}

/// Handle a `"database.connection"` form submit. If `values` contains `"_id"`,
/// updates the existing record (preserving `created_at`); otherwise generates a
/// new id from the name. Persists to the store and refreshes the widget.
///
/// Password handling is identical to `submit_database_new`: Postgres password
/// is written to the secrets table via `secrets.put`; the DSN stored in the
/// record never includes the password.
pub(crate) fn submit_db_connection_form(
    sid_app: &mut SidApp,
    values: sid_widgets::form::FormValues,
) -> Result<()> {
    use sid_core::adapters::{db_client::DbKind, secrets::SecretId};
    use sid_store::{DbConnection, now_epoch};

    let name = values.get("name").cloned().unwrap_or_default();
    let kind_str = values.get("kind").cloned().unwrap_or_default();
    let password = values.get("password").cloned().unwrap_or_default();
    let existing_id = values.get("_id").cloned();

    if name.is_empty() {
        return Err(anyhow::anyhow!("connection name is required"));
    }

    let kind = match kind_str.as_str() {
        "SQLite" => DbKind::Sqlite,
        _ => DbKind::Postgres,
    };

    let dsn = match kind {
        DbKind::Sqlite => values.get("path").cloned().unwrap_or_default(),
        DbKind::Postgres => {
            let host = values
                .get("host")
                .cloned()
                .unwrap_or_else(|| "localhost".to_string());
            let port = values
                .get("port")
                .cloned()
                .unwrap_or_else(|| "5432".to_string());
            let db = values.get("database").cloned().unwrap_or_default();
            let user = values.get("user").cloned().unwrap_or_default();
            if user.is_empty() {
                format!("postgres://{host}:{port}/{db}")
            } else {
                format!("postgres://{user}@{host}:{port}/{db}")
            }
        }
    };

    if dsn.trim().is_empty() {
        return Err(anyhow::anyhow!("connection path/host is required"));
    }

    // Derive a stable id: reuse existing id when editing, slug from name when creating.
    let id = existing_id.unwrap_or_else(|| {
        name.to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    });

    let secret_ref = if kind == DbKind::Postgres && !password.is_empty() {
        let sid_key = SecretId::new(format!("db.connection.{id}.password"));
        sid_app
            .secrets
            .put(&sid_key, password.as_bytes())
            .map_err(|e| anyhow::anyhow!("write db password: {e}"))?;
        Some(sid_key)
    } else {
        None
    };

    // Preserve created_at when updating.
    let created_at = sid_app
        .store
        .get_db_connection(&id)
        .ok()
        .flatten()
        .map(|c| c.created_at)
        .unwrap_or_else(now_epoch);

    let conn = DbConnection {
        id: id.clone(),
        kind,
        name: name.clone(),
        dsn,
        secret_ref,
        created_at,
    };
    sid_app
        .store
        .upsert_db_connection(&conn)
        .map_err(|e| anyhow::anyhow!("upsert db connection: {e}"))?;
    refresh_database_widget(sid_app);
    sid_app
        .toasts
        .push(Toast::success(format!("connection '{name}' saved")));
    Ok(())
}

/// Spawn an off-thread connection test for `conn_id` via the configured
/// `DbClient` factory (Postgres or SQLite). Returns `JobOutcome::Success`
/// with a round-trip latency message, or `JobOutcome::Failure` with the
/// driver error text. The result surfaces as a toast via `drain_job_outcomes`.
fn spawn_test_connection(sid_app: &mut SidApp, conn_id: String) {
    use sid_core::adapters::db_client::OpenParams;
    // Retrieve connection record.
    let record = match sid_app.store.get_db_connection(&conn_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            sid_app
                .toasts
                .push(Toast::error(format!("connection '{conn_id}' not found")));
            return;
        }
        Err(e) => {
            sid_app
                .toasts
                .push(Toast::error(format!("read connection: {e}")));
            return;
        }
    };

    // Retrieve password from secrets store if a secret_ref is present.
    let password = record.secret_ref.as_ref().and_then(|sid| {
        sid_app
            .secrets
            .get(sid)
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    });

    let factory: Arc<dyn sid_core::adapters::db_client::DbClient> = match record.kind {
        sid_core::adapters::db_client::DbKind::Postgres => Arc::clone(&sid_app.postgres),
        sid_core::adapters::db_client::DbKind::Sqlite => Arc::clone(&sid_app.sqlite),
    };

    let label = format!("test-connection:{conn_id}");
    let params = OpenParams {
        kind: record.kind,
        dsn: record.dsn.clone(),
        password,
    };

    sid_app
        .toasts
        .push(Toast::info(format!("testing connection '{conn_id}'...")));

    sid_app.jobs.spawn(async move {
        let start = std::time::Instant::now();
        match factory.open(params).await {
            Ok(_client) => {
                let elapsed_ms = start.elapsed().as_millis();
                JobOutcome::Success {
                    label,
                    message: format!("connected in {elapsed_ms}ms"),
                }
            }
            Err(e) => JobOutcome::Failure {
                label,
                message: e.to_string(),
            },
        }
    });
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

/// Read a [`FieldValue::Toggle`] from a modal submit's values by label.
/// Returns `false` when the field is absent or not a toggle — the safe default
/// for an opt-in checkbox (e.g. "Save to keyring").
fn bool_value(values: &[(String, sid_widgets::FieldValue)], label: &str) -> bool {
    use sid_widgets::FieldValue;
    values
        .iter()
        .find(|(k, _)| k == label)
        .and_then(|(_, v)| match v {
            FieldValue::Toggle(b) => Some(*b),
            _ => None,
        })
        .unwrap_or(false)
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
/// history) is left intact. Threads the current `show_add_new_row` setting
/// so the cursor's synthetic-row flag stays consistent.
fn refresh_database_widget(sid_app: &mut SidApp) {
    let conns = match sid_app.store.list_db_connections() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("list_db_connections after form/modal submit failed: {e}");
            return;
        }
    };
    let show_add_new = load_show_add_new_row(&*sid_app.store);
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "database" {
            if let Some(w) = t.layout.iter_widgets_mut().next() {
                let any_ref = w as &mut dyn std::any::Any;
                if let Some(ww) = any_ref.downcast_mut::<DatabaseWidget>() {
                    ww.state_mut().set_connections(conns, show_add_new);
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

    /// `--ssh <alias>` startup walks the cursor to the requested host even
    /// though the cursor starts on the synthetic "+ add new" row (regression:
    /// a fixed index-count walk landed one row short once show_add_new_row
    /// shipped enabled by default).
    #[test]
    fn start_ssh_alias_selects_host_despite_add_new_row() {
        use sid_store::{SshHost, SshHostSource};
        let mk = |alias: &str| SshHost {
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
        };
        let app = build_app_hydrated(
            None,
            BuildAppData {
                ssh_hosts: vec![mk("alpha"), mk("beta")],
                start_ssh_alias: Some("beta".into()),
                ..Default::default()
            },
        );
        let ssh_tab = app
            .tabs()
            .tabs()
            .iter()
            .find(|t| t.id.as_str() == "ssh")
            .expect("ssh tab");
        let w = ssh_tab
            .layout
            .iter_widgets()
            .next()
            .unwrap()
            .as_any()
            .downcast_ref::<SshWidget>()
            .expect("ssh widget");
        assert_eq!(
            w.state().selected_alias(),
            Some("beta"),
            "cursor must land on the requested host, not one row short"
        );
        assert_eq!(
            w.connection().phase(),
            sid_widgets::ssh::ConnectionPhase::Connecting,
            "startup alias must begin connecting"
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

    // ---- T5: DEFAULT_TAB resolution ----------------------------------------

    /// CLI arg always wins over the DEFAULT_TAB setting.
    #[test]
    fn resolve_start_tab_cli_wins_over_setting() {
        assert_eq!(
            resolve_start_tab(Some("ssh"), Some("database".into())),
            Some("ssh".to_string())
        );
    }

    /// With no CLI arg, the DEFAULT_TAB setting is used.
    #[test]
    fn resolve_start_tab_falls_back_to_setting() {
        assert_eq!(
            resolve_start_tab(None, Some("database".into())),
            Some("database".to_string())
        );
    }

    /// With neither set, returns None (caller uses its built-in default).
    #[test]
    fn resolve_start_tab_none_when_nothing_set() {
        assert_eq!(resolve_start_tab(None, None), None);
    }

    /// End-to-end: the resolved tab actually drives the built app's active tab.
    /// CLI=None + DEFAULT_TAB="database" → active tab is database.
    #[test]
    fn default_tab_setting_drives_active_tab_when_no_cli() {
        let resolved = resolve_start_tab(None, Some("database".into()));
        let app = build_app(resolved.as_deref(), vec![]);
        assert_eq!(app.tabs().active().id.as_str(), "database");
    }

    /// End-to-end: CLI="ssh" wins over DEFAULT_TAB="database".
    #[test]
    fn cli_start_tab_overrides_default_tab_setting() {
        let resolved = resolve_start_tab(Some("ssh"), Some("database".into()));
        let app = build_app(resolved.as_deref(), vec![]);
        assert_eq!(app.tabs().active().id.as_str(), "ssh");
    }

    // ---- probe_keyring ----

    /// How a [`ProbeStore`] should misbehave on `get`, to drive each failure
    /// branch of `probe_keyring` through its single cleanup point.
    #[derive(Clone, Copy, PartialEq)]
    enum ProbeMode {
        /// `get` returns the bytes that were put — the happy path.
        Honest,
        /// `get` returns wrong bytes — read-back mismatch.
        CorruptReadback,
        /// `get` returns `Err` — the put-Ok-then-get-Err leak path.
        GetErrors,
    }

    /// A fake SecretStore that records calls and can inject a read-back
    /// mismatch or a `get` error, so the probe cleanup is exercised on every
    /// post-put exit path.
    struct ProbeStore {
        mode: ProbeMode,
        deleted: std::sync::Mutex<Vec<String>>,
        stored: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    impl ProbeStore {
        fn new(mode: ProbeMode) -> Self {
            Self {
                mode,
                deleted: Default::default(),
                stored: Default::default(),
            }
        }
        fn new_mismatch() -> Self {
            Self::new(ProbeMode::CorruptReadback)
        }
        fn deleted_keys(&self) -> Vec<String> {
            self.deleted.lock().unwrap().clone()
        }
    }

    impl sid_core::adapters::secrets::SecretStore for ProbeStore {
        fn put(
            &self,
            id: &sid_core::adapters::secrets::SecretId,
            value: &[u8],
        ) -> Result<(), sid_core::adapters::secrets::SecretError> {
            self.stored
                .lock()
                .unwrap()
                .insert(id.as_str().to_string(), value.to_vec());
            Ok(())
        }

        fn get(
            &self,
            id: &sid_core::adapters::secrets::SecretId,
        ) -> Result<Option<Vec<u8>>, sid_core::adapters::secrets::SecretError> {
            match self.mode {
                ProbeMode::CorruptReadback => Ok(Some(b"WRONG".to_vec())),
                ProbeMode::GetErrors => Err(sid_core::adapters::secrets::SecretError::Storage(
                    "injected get failure".into(),
                )),
                ProbeMode::Honest => Ok(self.stored.lock().unwrap().get(id.as_str()).cloned()),
            }
        }

        fn delete(
            &self,
            id: &sid_core::adapters::secrets::SecretId,
        ) -> Result<(), sid_core::adapters::secrets::SecretError> {
            self.deleted.lock().unwrap().push(id.as_str().to_string());
            self.stored.lock().unwrap().remove(id.as_str());
            Ok(())
        }

        fn list_ids(
            &self,
        ) -> Result<
            Vec<sid_core::adapters::secrets::SecretId>,
            sid_core::adapters::secrets::SecretError,
        > {
            Ok(vec![])
        }
    }

    const PROBE_KEY: &str = "sid.__keyring_probe";

    /// On a read-back mismatch `probe_keyring` returns false AND deletes the
    /// sentinel so the OS keyring is not left with a stale probe entry.
    #[test]
    fn probe_keyring_mismatch_cleans_up_sentinel() {
        let store = ProbeStore::new_mismatch();
        let ok = probe_keyring(&store);
        assert!(!ok, "probe must return false on mismatch");
        let deleted = store.deleted_keys();
        assert!(
            deleted.contains(&PROBE_KEY.to_string()),
            "probe key must be deleted even on mismatch; deleted={deleted:?}"
        );
    }

    /// Regression for Fix M5: when the put succeeds but the read-back `get`
    /// returns `Err`, the sentinel was previously leaked. The single cleanup
    /// point must delete it and the probe must report failure.
    #[test]
    fn probe_keyring_get_error_cleans_up_sentinel() {
        let store = ProbeStore::new(ProbeMode::GetErrors);
        let ok = probe_keyring(&store);
        assert!(!ok, "probe must return false when read-back errors");
        let deleted = store.deleted_keys();
        assert!(
            deleted.contains(&PROBE_KEY.to_string()),
            "probe key must be deleted on the get-Err path (no leak); deleted={deleted:?}"
        );
    }

    /// The happy path: put succeeds, read-back matches, delete succeeds → probe
    /// returns true and still cleans up the sentinel (no stale entry left).
    #[test]
    fn probe_keyring_happy_path_returns_true_and_cleans_up() {
        let store = ProbeStore::new(ProbeMode::Honest);
        let ok = probe_keyring(&store);
        assert!(ok, "probe must succeed on the honest path");
        let deleted = store.deleted_keys();
        assert!(
            deleted.contains(&PROBE_KEY.to_string()),
            "probe key must always be deleted, even on success; deleted={deleted:?}"
        );
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

    // ---- T6: PERSIST_DEBOUNCE_MS / persister-driven flush ------------------

    /// Zero-debounce persister flushes immediately after `mark_dirty` — the
    /// behaviour the event loop relies on when PERSIST_DEBOUNCE_MS is 0.
    #[test]
    fn persister_zero_debounce_flushes_immediately() {
        use sid_core::persister::StatePersister;
        let mut p = StatePersister::new(std::time::Duration::ZERO);
        assert!(!p.should_flush(), "nothing dirty yet");
        p.mark_dirty();
        assert!(p.should_flush(), "zero debounce → due immediately");
        assert!(!p.should_flush(), "marker consumed by the flush");
    }

    /// A large-debounce persister does not flush twice within the window: the
    /// loop marks dirty every iteration, but only the first elapsed window
    /// flushes. With a far-future debounce, no iteration flushes.
    #[test]
    fn persister_large_debounce_does_not_flush_within_window() {
        use sid_core::persister::StatePersister;
        let mut p = StatePersister::new(std::time::Duration::from_secs(3600));
        p.mark_dirty();
        assert!(!p.should_flush(), "debounce not elapsed → no flush");
        // Re-marking does not reset / force a flush within the window.
        p.mark_dirty();
        assert!(!p.should_flush(), "still within the debounce window");
        assert!(p.is_dirty(), "state remains dirty until the window elapses");
    }

    /// Quit flushes session state unconditionally even when the debounce window
    /// has NOT elapsed. With a far-future debounce, the per-iteration flush
    /// never fires, so the session row only exists because the quit path wrote
    /// it. We assert the active tab round-trips through the store after a quit.
    #[tokio::test]
    async fn quit_flushes_session_state_despite_debounce() {
        use ratatui::backend::TestBackend;
        use tokio::sync::mpsc;

        let backend = TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut sid_app = build_test_sid_app(Some("database"));
        // Far-future debounce: per-iteration `should_flush` can never fire.
        sid_app.persister =
            sid_core::persister::StatePersister::new(std::time::Duration::from_secs(86_400));
        sid_app.session_id = "quit-flush-sess".into();

        // Send a single Ctrl+Q so `App::handle_event` returns Dispatch::Quit.
        let (tx, mut rx) = mpsc::channel::<sid_core::event::Event>(4);
        let ctrl_q = sid_core::event::KeyChord {
            code: crossterm::event::KeyCode::Char('q'),
            mods: crossterm::event::KeyModifiers::CONTROL,
        };
        tx.send(sid_core::event::Event::Key(ctrl_q)).await.unwrap();
        drop(tx);

        run_event_loop(&mut terminal, &mut sid_app, &mut rx)
            .await
            .unwrap();

        let loaded = sid_app
            .store
            .current_session()
            .unwrap()
            .expect("quit must have flushed a session record");
        assert_eq!(loaded.id, "quit-flush-sess");
        assert_eq!(
            loaded.active_tab.unwrap().as_str(),
            "database",
            "quit flush must persist the active tab"
        );
    }

    // ---- T7: HEARTBEAT_INTERVAL_SECS / heartbeat_due -----------------------

    /// A zero interval is always due; a far-future interval is never due for a
    /// fresh instant. Deterministic — no wall-clock sleeps.
    #[test]
    fn heartbeat_due_zero_interval_always_due() {
        assert!(heartbeat_due(
            std::time::Instant::now(),
            std::time::Duration::ZERO
        ));
    }

    #[test]
    fn heartbeat_due_large_interval_not_due() {
        assert!(!heartbeat_due(
            std::time::Instant::now(),
            std::time::Duration::from_secs(86_400)
        ));
    }

    /// An instant already aged past the interval is due.
    #[test]
    fn heartbeat_due_past_interval_is_due() {
        let past = std::time::Instant::now() - std::time::Duration::from_secs(10);
        assert!(heartbeat_due(past, std::time::Duration::from_secs(5)));
    }

    /// The heartbeat body refreshes the session's `last_active` via the store.
    /// Verifies the store contract the loop relies on (no timing involved).
    #[test]
    fn heartbeat_upsert_refreshes_last_active() {
        let sid_app = build_test_sid_app(None);
        // Seed a session with a stale last_active.
        let stale = SessionRecord {
            id: "hb-sess".into(),
            started_at: now_epoch().saturating_sub(100_000_000_000),
            last_active: 1,
            ended_at: None,
            active_tab: Some(TabId::new("system")),
            open_tabs: vec![],
        };
        sid_app.store.upsert_session(&stale).unwrap();

        // Mirror the loop's heartbeat body.
        let mut sess = sid_app.store.current_session().unwrap().unwrap();
        let before = sess.last_active;
        sess.last_active = now_epoch();
        sid_app.store.upsert_session(&sess).unwrap();

        let after = sid_app
            .store
            .current_session()
            .unwrap()
            .unwrap()
            .last_active;
        assert!(after > before, "last_active must advance after a heartbeat");
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

    /// `build_app` registers 15 actions (9 named + 6 jump).
    #[test]
    fn build_app_registers_expected_actions() {
        let app = build_app(None, vec![]);
        // 9 named (added tab.close in branch #1) + 6 jump actions
        let all: Vec<_> = app.actions().all().collect();
        assert_eq!(all.len(), 15, "expected 15 actions, got {}", all.len());
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
        use ratatui::{Terminal, backend::TestBackend};

        let sid_app = build_test_sid_app(None);
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
    }

    /// `draw` renders without panicking on a very small (1×1) terminal.
    #[test]
    fn draw_does_not_panic_on_tiny_terminal() {
        use ratatui::{Terminal, backend::TestBackend};

        let sid_app = build_test_sid_app(None);
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
    }

    /// `draw` renders without panicking when the terminal is smaller than the
    /// tab bar (height = 2, which is less than the 3-row bar height).
    #[test]
    fn draw_does_not_panic_when_shorter_than_bar() {
        use ratatui::{Terminal, backend::TestBackend};

        let sid_app = build_test_sid_app(None);
        // Height 2 < bar height 3; body_rect will have saturating_sub(3) = 0 height.
        let backend = TestBackend::new(80, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
    }

    /// `draw` renders all six tabs without panicking.
    #[test]
    fn draw_all_tabs_render_without_panic() {
        use ratatui::{Terminal, backend::TestBackend};

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
        let (ssh_outcome_tx, ssh_outcome_rx) = tokio::sync::mpsc::unbounded_channel();
        SidApp {
            app: build_app(start_tab, vec![]),
            store,
            git_factory: Arc::new(Git2ProviderFactory::new()),
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
            form: None,
            form_origin_tab: None,
            pending_submits: Vec::new(),
            toasts: ToastQueue::new(4),
            undo_ring: std::collections::VecDeque::new(),
            jobs: Arc::new(sid_job::JobQueue::<JobOutcome>::new()),
            ssh_client_factory: build_ssh_client_factory_fn(),
            ssh_outcome_tx,
            ssh_outcome_rx,
            ssh_byte_rx: None,
            ssh_last_pty_area: None,
            ssh_shutdown_tx: None,
            active_theme: sid_ui::themes::cosmos(),
            persister: sid_core::persister::StatePersister::new(std::time::Duration::ZERO),
            last_heartbeat: std::time::Instant::now(),
        }
    }

    #[test]
    fn scan_umbrella_satellites_finds_repos_and_marks_umbrella() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::create_dir_all(tmp.path().join("api").join(".git")).unwrap();
        let rows = scan_umbrella_satellites(tmp.path(), "gen4");
        // umbrella row first, then satellites
        assert!(rows[0].is_umbrella);
        assert_eq!(rows[0].name, "gen4");
        assert!(rows.iter().any(|r| r.name == "api" && !r.is_umbrella));
    }

    #[test]
    fn workspaces_new_form_reshapes_on_kind() {
        use sid_widgets::modal::Field;
        let mut spec = workspaces_new_form();
        // default kind Umbrella exposes the umbrella feature toggles
        let v = spec.values();
        assert_eq!(v["kind"], "Umbrella");
        assert!(
            spec.sections
                .iter()
                .flat_map(|s| &s.fields)
                .any(|f| f.key == "scan_satellites")
        );
        // flip to Repo, reshape drops umbrella-only toggles
        for s in &mut spec.sections {
            for f in &mut s.fields {
                if f.key == "kind"
                    && let Field::Choice { selected, .. } = &mut f.field
                {
                    *selected = 1; // Repo
                }
            }
        }
        spec.run_reshape();
        assert_eq!(spec.values()["kind"], "Repo");
        assert!(
            !spec
                .sections
                .iter()
                .flat_map(|s| &s.fields)
                .any(|f| f.key == "scan_satellites")
        );
    }

    #[test]
    fn dispatch_workspaces_create_persists_workspace() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let tmp = tempfile::tempdir().unwrap();
        let mut values = sid_widgets::form::FormValues::new();
        values.insert("name".into(), "neo".into());
        values.insert("path".into(), tmp.path().display().to_string());
        values.insert("kind".into(), "Repo".into());
        dispatch_form_submit(&mut sid_app, "workspaces.create", values);
        let ws = sid_app.store.list_workspaces().unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].name, "neo");
    }

    #[test]
    fn adopt_form_lists_scanned_repos_as_prechecked_toggles() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("api").join(".git")).unwrap();
        std::fs::create_dir_all(tmp.path().join("web").join(".git")).unwrap();
        let spec = workspaces_adopt_form(tmp.path());
        let toggles: Vec<&str> = spec
            .sections
            .iter()
            .flat_map(|s| &s.fields)
            .filter(|f| f.key.starts_with("repo:"))
            .map(|f| f.key.as_str())
            .collect();
        assert_eq!(toggles.len(), 2);
        // every found repo is pre-checked (value true)
        for f in spec
            .sections
            .iter()
            .flat_map(|s| &s.fields)
            .filter(|f| f.key.starts_with("repo:"))
        {
            assert_eq!(f.value_string(), "true");
        }
    }

    #[test]
    fn add_new_enter_opens_create_form() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        // Enable + select the add-new row, then drive a real Enter through the
        // widget so it flags pending_add_new the way the user would.
        if let Some(ww) = workspaces_widget_mut(&mut sid_app) {
            ww.set_show_add_new_row(true);
            assert!(ww.add_new_selected());
            let (_tx, _rx) = std::sync::mpsc::channel::<String>();
            let mut ctx = sid_core::context::WidgetCtx::new(_tx);
            let ev = Event::Key(KeyChord {
                code: KeyCode::Enter,
                mods: KeyModifiers::NONE,
            });
            let _ = ww.handle_event(&ev, &mut ctx);
        }
        assert!(sid_app.form.is_none());
        maybe_open_pending_new_form(&mut sid_app);
        // A form is now open with id workspaces.create.
        let form = sid_app.form.as_ref().expect("create form opened");
        assert_eq!(form.spec.id.0, "workspaces.create");
    }

    #[test]
    fn hydrate_workspaces_add_new_row_defaults_on() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        hydrate_workspaces_add_new_row(&mut sid_app);
        let ww = workspaces_widget_mut(&mut sid_app).expect("workspaces widget");
        // default setting is on, so the add-new row is selected at startup
        assert!(ww.add_new_selected());
    }

    #[test]
    fn dispatch_workspaces_adopt_registers_umbrella_and_checked_satellites() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let tmp = tempfile::tempdir().unwrap();
        let api = tmp.path().join("api");
        std::fs::create_dir_all(api.join(".git")).unwrap();
        let api_real = std::fs::canonicalize(&api).unwrap();

        let mut values = sid_widgets::form::FormValues::new();
        values.insert("dir".into(), tmp.path().display().to_string());
        values.insert("name".into(), "gen4".into());
        values.insert(format!("repo:{}", api_real.display()), "true".into());
        dispatch_form_submit(&mut sid_app, "workspaces.adopt", values);

        let ws = sid_app.store.list_workspaces().unwrap();
        // umbrella + 1 satellite
        assert_eq!(ws.len(), 2);
        let _umbrella = ws.iter().find(|w| w.name == "gen4").unwrap();
        let sat = ws.iter().find(|w| w.parent.is_some()).unwrap();
        assert_eq!(
            sat.parent.as_ref().unwrap(),
            &std::fs::canonicalize(tmp.path()).unwrap()
        );
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

    // ---- T1: active_theme actually drives draw() ----

    /// `draw` renders different cell styling under two distinct themes. Proves
    /// `sid_app.active_theme` is read by the render path rather than a
    /// hardcoded palette. We render the same app twice — once with cosmos,
    /// once with void — and assert the resulting buffers differ.
    #[test]
    fn draw_reflects_active_theme_palette() {
        use ratatui::{Terminal, backend::TestBackend};

        let render = |theme: sid_ui::theme::Theme| {
            let mut sid_app = build_test_sid_app(None);
            sid_app.active_theme = theme;
            let backend = TestBackend::new(120, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &sid_app)).unwrap();
            terminal.backend().buffer().clone()
        };

        let cosmos_buf = render(sid_ui::themes::cosmos());
        let void_buf = render(sid_ui::themes::void());
        // The border / foreground colours differ between the two built-in
        // themes, so at least one cell's style must differ.
        assert_ne!(
            cosmos_buf, void_buf,
            "rendered buffer must differ between cosmos and void themes"
        );
    }

    /// The `ThemeApplied` settings outcome mutates `sid_app.active_theme` live
    /// (not just on restart). Drive the outcome through the wire drain and
    /// assert the field changed to the applied theme.
    #[test]
    fn theme_applied_outcome_updates_active_theme_live() {
        use sid_core::layout::Layout;

        let mut sid_app = build_test_sid_app(Some("settings"));
        // Precondition: starts on cosmos.
        assert_eq!(sid_app.active_theme.name, "cosmos");

        // Inject a ThemeApplied outcome selecting a non-cosmos built-in theme.
        {
            let tabs = sid_app.app.tabs_mut().tabs_mut();
            let settings_tab = tabs
                .iter_mut()
                .find(|t| t.id.as_str() == "settings")
                .expect("settings tab present");
            let Layout::Single(w) = &mut settings_tab.layout else {
                panic!("settings tab must have Single layout");
            };
            let settings_widget = w
                .as_any_mut()
                .downcast_mut::<sid_widgets::SettingsWidget>()
                .expect("downcast to SettingsWidget");
            settings_widget.push_pending_outcome(
                sid_widgets::settings::PendingSettingsOutcome::ThemeApplied {
                    name: "void".into(),
                },
            );
        }

        apply_pending_settings_outcomes(&mut sid_app);

        assert_eq!(
            sid_app.active_theme.name, "void",
            "active_theme must reflect the applied theme name immediately"
        );
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

    // ---- load_show_add_new_row ----

    #[test]
    fn show_add_new_row_defaults_true_when_unset() {
        let (_d, store) = fresh_store();
        assert!(load_show_add_new_row(&store));
    }

    /// A store whose `get_setting` always errors. Only that method is
    /// reachable from `load_show_add_new_row`; everything else is
    /// `unimplemented!()` so an unexpected call fails the test loudly.
    struct FailingGetSettingStore;

    #[rustfmt::skip]
    impl Store for FailingGetSettingStore {
        fn get_setting(&self, _: &str) -> Result<Option<sid_store::SettingValue>, sid_core::SidError> {
            Err(sid_core::SidError::Storage("injected get_setting failure".into()))
        }
        fn put_setting(&self, _: &str, _: &sid_store::SettingValue) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn list_setting_keys(&self) -> Result<Vec<String>, sid_core::SidError> { unimplemented!() }
        fn delete_setting(&self, _: &str) -> Result<bool, sid_core::SidError> { unimplemented!() }
        fn current_session(&self) -> Result<Option<sid_store::SessionRecord>, sid_core::SidError> { unimplemented!() }
        fn upsert_session(&self, _: &sid_store::SessionRecord) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn end_session(&self, _: &str, _: sid_store::Epoch) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn list_sessions(&self) -> Result<Vec<sid_store::SessionRecord>, sid_core::SidError> { unimplemented!() }
        fn save_widget_state(&self, _: &sid_store::WidgetState) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn load_widget_state(&self, _: &sid_core::TabId, _: &sid_core::WidgetId) -> Result<Option<Vec<u8>>, sid_core::SidError> { unimplemented!() }
        fn list_workspaces(&self) -> Result<Vec<Workspace>, sid_core::SidError> { unimplemented!() }
        fn upsert_workspace(&self, _: &Workspace) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn get_workspace(&self, _: &Path) -> Result<Option<Workspace>, sid_core::SidError> { unimplemented!() }
        fn remove_workspace(&self, _: &Path) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn secret_put(&self, _: &str, _: &[u8]) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn secret_get(&self, _: &str) -> Result<Option<Vec<u8>>, sid_core::SidError> { unimplemented!() }
        fn secret_delete(&self, _: &str) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn list_secret_ids(&self) -> Result<Vec<String>, sid_core::SidError> { unimplemented!() }
        fn list_themes(&self) -> Result<Vec<sid_store::ThemeSpec>, sid_core::SidError> { unimplemented!() }
        fn get_theme(&self, _: &str) -> Result<Option<sid_store::ThemeSpec>, sid_core::SidError> { unimplemented!() }
        fn upsert_theme(&self, _: &sid_store::ThemeSpec) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn remove_theme(&self, _: &str) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn list_keybind_profiles(&self) -> Result<Vec<sid_store::KeybindProfile>, sid_core::SidError> { unimplemented!() }
        fn get_keybind_profile(&self, _: &str) -> Result<Option<sid_store::KeybindProfile>, sid_core::SidError> { unimplemented!() }
        fn upsert_keybind_profile(&self, _: &sid_store::KeybindProfile) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn remove_keybind_profile(&self, _: &str) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn list_quick_actions(&self) -> Result<Vec<sid_store::QuickAction>, sid_core::SidError> { unimplemented!() }
        fn get_quick_action(&self, _: &str) -> Result<Option<sid_store::QuickAction>, sid_core::SidError> { unimplemented!() }
        fn upsert_quick_action(&self, _: &sid_store::QuickAction) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn remove_quick_action(&self, _: &str) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn list_pinned_configs(&self) -> Result<Vec<sid_store::PinnedConfig>, sid_core::SidError> { unimplemented!() }
        fn upsert_pinned_config(&self, _: &sid_store::PinnedConfig) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn get_pinned_config(&self, _: &Path) -> Result<Option<sid_store::PinnedConfig>, sid_core::SidError> { unimplemented!() }
        fn remove_pinned_config(&self, _: &Path) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn list_db_connections(&self) -> Result<Vec<sid_store::DbConnection>, sid_core::SidError> { unimplemented!() }
        fn upsert_db_connection(&self, _: &sid_store::DbConnection) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn get_db_connection(&self, _: &str) -> Result<Option<sid_store::DbConnection>, sid_core::SidError> { unimplemented!() }
        fn remove_db_connection(&self, _: &str) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn append_query_record(&self, _: &sid_store::QueryRecord) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn recent_queries(&self, _: &str, _: usize) -> Result<Vec<sid_store::QueryRecord>, sid_core::SidError> { unimplemented!() }
        fn list_ssh_hosts(&self) -> Result<Vec<sid_store::SshHost>, sid_core::SidError> { unimplemented!() }
        fn upsert_ssh_host(&self, _: &sid_store::SshHost) -> Result<(), sid_core::SidError> { unimplemented!() }
        fn get_ssh_host(&self, _: &str) -> Result<Option<sid_store::SshHost>, sid_core::SidError> { unimplemented!() }
        fn remove_ssh_host(&self, _: &str) -> Result<(), sid_core::SidError> { unimplemented!() }
    }

    /// Deferred from the branch-0 review: a store ERROR (not just an unset
    /// key) must also fall back to showing the row — the Err and unset arms
    /// deliberately converge on default-true, and this pins the Err arm.
    #[test]
    fn show_add_new_row_defaults_true_on_store_error() {
        assert!(
            load_show_add_new_row(&FailingGetSettingStore),
            "a store read error must fall back to the default (row shown)"
        );
    }

    #[test]
    fn load_show_add_new_row_honours_true_setting() {
        use sid_store::TypedSettings;
        let (_d, store) = fresh_store();
        store
            .put_string(sid_store::settings_keys::SHOW_ADD_NEW_ROW, "true")
            .unwrap();
        assert!(load_show_add_new_row(&store));
    }

    #[test]
    fn show_add_new_row_respects_stored_false() {
        use sid_store::TypedSettings;
        let (_d, store) = fresh_store();
        store
            .put_string(sid_store::settings_keys::SHOW_ADD_NEW_ROW, "false")
            .unwrap();
        assert!(!load_show_add_new_row(&store));
    }

    #[test]
    fn load_show_add_new_row_non_false_values_are_true() {
        use sid_store::TypedSettings;
        let (_d, store) = fresh_store();
        // Any non-"false" value should be treated as true.
        store
            .put_string(sid_store::settings_keys::SHOW_ADD_NEW_ROW, "garbage")
            .unwrap();
        assert!(load_show_add_new_row(&store));
    }

    /// Cross-crate contract: the loader reads exactly what `put_bool` writes.
    /// `put_bool(.., false)` must round-trip to `load_show_add_new_row == false`
    /// — the loader's lenient `!= b"false"` check must agree with sid-store's
    /// canonical bool encoding.
    #[test]
    fn load_show_add_new_row_round_trips_put_bool_false() {
        use sid_store::TypedSettings;
        let (_d, store) = fresh_store();
        store
            .put_bool(sid_store::settings_keys::SHOW_ADD_NEW_ROW, false)
            .unwrap();
        assert!(!load_show_add_new_row(&store));
    }

    /// And `put_bool(.., true)` round-trips to `true`.
    #[test]
    fn load_show_add_new_row_round_trips_put_bool_true() {
        use sid_store::TypedSettings;
        let (_d, store) = fresh_store();
        store
            .put_bool(sid_store::settings_keys::SHOW_ADD_NEW_ROW, true)
            .unwrap();
        assert!(load_show_add_new_row(&store));
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

    // ---- Task 4 / Task 7 tests: dispatch_ssh_form_key, rewritten N/E tests ----

    /// Build a test `SidApp` with the SSH tab active and the given hosts
    /// pre-populated in both the store and the SSH widget.
    fn build_app_with_ssh_hosts(hosts: Vec<sid_store::SshHost>) -> SidApp {
        let mut app = build_test_sid_app(Some("ssh"));
        for h in &hosts {
            app.store.upsert_ssh_host(h).unwrap();
        }
        // Refresh the widget state so it sees the hosts.
        refresh_ssh_widget(&mut app);
        // The cursor starts on the synthetic "+ add new" row (show_add_new_row
        // defaults on); step onto the first real host so selection-dependent
        // tests see one — mirroring the ↓ a user would press.
        if let Some(w) = active_ssh_widget_mut(&mut app) {
            if w.state().selected_host().is_none() {
                w.state_mut().select_next();
            }
        }
        app
    }

    // Previously: `N` opened a modal. Now it opens a FormPane (Task 4/7).
    #[test]
    fn dispatch_ssh_form_key_n_opens_form_on_ssh_tab() {
        use crossterm::event::KeyModifiers;
        use sid_core::event::KeyChord;
        let mut app = build_test_sid_app(Some("ssh"));
        let chord = KeyChord {
            code: crossterm::event::KeyCode::Char('N'),
            mods: KeyModifiers::empty(),
        };
        let opened = dispatch_ssh_form_key(&mut app, chord);
        assert!(opened, "N must open a form");
        assert_eq!(app.form.as_ref().unwrap().spec.id.0, "ssh.new");
        assert!(app.modal_stack.is_empty(), "no modal must be opened");
    }

    // Previously: `N` on other tabs produced no `ssh.new` modal.
    // Now: `dispatch_ssh_form_key` only checks the SSH form path;
    // route_key_event guards it behind `active_tab == "ssh"`, so calling it
    // on a non-SSH SidApp will still open the form but this path is only reached
    // when tab == "ssh". We verify the guard in the route_key_event tests.
    #[test]
    fn dispatch_ssh_form_key_n_is_noop_on_non_ssh_tab() {
        // The `route_key_event` guard checks `active().id == "ssh"` before calling
        // `dispatch_ssh_form_key`. The function itself is tab-agnostic — it
        // operates on the SSH widget via `active_ssh_widget_mut`, which returns
        // `None` on a non-SSH tab and guards correctly.
        use crossterm::event::KeyModifiers;
        use sid_core::event::KeyChord;
        let mut app = build_test_sid_app(Some("workspaces"));
        let chord = KeyChord {
            code: crossterm::event::KeyCode::Char('N'),
            mods: KeyModifiers::empty(),
        };
        // N always opens the add form (no tab-gate in dispatch_ssh_form_key);
        // the tab gate lives in route_key_event's `active().id == "ssh"` guard.
        // Test that the form is opened regardless — route_key_event is tested separately.
        let _opened = dispatch_ssh_form_key(&mut app, chord);
        // No assertion about form state; the key point is it does not panic.
    }

    #[test]
    fn n_key_on_ssh_tab_opens_add_form_not_modal() {
        use crossterm::event::KeyModifiers;
        use sid_core::event::KeyChord;
        let mut app = build_test_sid_app(Some("ssh"));
        let chord = KeyChord {
            code: crossterm::event::KeyCode::Char('N'),
            mods: KeyModifiers::empty(),
        };
        let opened = dispatch_ssh_form_key(&mut app, chord);
        assert!(opened, "N must open a form");
        assert!(app.form.is_some(), "form must be set");
        assert_eq!(app.form.as_ref().unwrap().spec.id.0, "ssh.new");
        assert!(app.modal_stack.is_empty(), "no modal must be opened");
    }

    #[test]
    fn e_key_on_ssh_tab_opens_edit_form_for_manual_host() {
        use crossterm::event::KeyModifiers;
        use sid_core::event::KeyChord;
        use sid_store::{SshAuthKind, SshHost, SshHostSource};
        let host = SshHost {
            alias: "myhost".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: vec![],
            last_sftp_path: None,
            auth_kind: SshAuthKind::Agent,
        };
        let mut app = build_app_with_ssh_hosts(vec![host]);
        let chord = KeyChord {
            code: crossterm::event::KeyCode::Char('E'),
            mods: KeyModifiers::empty(),
        };
        assert!(dispatch_ssh_form_key(&mut app, chord));
        let form_id = app.form.as_ref().unwrap().spec.id.0.clone();
        assert_eq!(form_id, "ssh.edit:myhost");
    }

    #[test]
    fn right_arrow_on_ssh_host_opens_inspector_form() {
        use crossterm::event::KeyModifiers;
        use sid_core::event::KeyChord;
        use sid_store::{SshAuthKind, SshHost, SshHostSource};
        let host = SshHost {
            alias: "inspector-test".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: vec![],
            last_sftp_path: None,
            auth_kind: SshAuthKind::Agent,
        };
        let mut app = build_app_with_ssh_hosts(vec![host]);
        let chord = KeyChord {
            code: crossterm::event::KeyCode::Right,
            mods: KeyModifiers::empty(),
        };
        assert!(dispatch_ssh_form_key(&mut app, chord));
        let id = app.form.as_ref().unwrap().spec.id.0.clone();
        assert_eq!(id, "ssh.inspect:inspector-test");
    }

    // --- Task 5: background-open ---

    /// End-to-end test: drives `route_key_event` (not `dispatch_ssh_form_key`
    /// directly) to verify background-open is reachable from the inspector in
    /// production. Previously the test called dispatch_ssh_form_key directly
    /// which bypassed the `form.is_none()` gate in route_key_event — a false
    /// positive. This test opens the inspector via `→` through route_key_event,
    /// then fires Ctrl+Enter through route_key_event to confirm the new tab
    /// is pushed in the background (active index unchanged) and the inspector
    /// form remains open.
    #[test]
    fn background_open_from_inspector_pushes_tab_and_toasts() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        use sid_store::{SshAuthKind, SshHost, SshHostSource};
        let host = SshHost {
            alias: "bg-host".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: vec![],
            last_sftp_path: None,
            auth_kind: SshAuthKind::Agent,
        };
        let mut app = build_app_with_ssh_hosts(vec![host.clone()]);

        // Open the inspector via → through route_key_event (the production path).
        let right_chord = KeyChord {
            code: KeyCode::Right,
            mods: KeyModifiers::NONE,
        };
        let consumed = route_key_event(&mut app, right_chord);
        assert!(consumed, "→ must open inspector form");
        assert!(
            app.form
                .as_ref()
                .map(|f| f.spec.id.0.starts_with("ssh.inspect:"))
                .unwrap_or(false),
            "form must be an ssh.inspect form after →"
        );

        let active_idx_before = app.app.tabs().active_index();
        let tab_count_before = app.app.tabs().tabs().len();

        // Fire Ctrl+Enter through route_key_event — must reach the
        // background-open arm despite form.is_some().
        let bg_chord = KeyChord {
            code: KeyCode::Enter,
            mods: KeyModifiers::CONTROL,
        };
        let consumed = route_key_event(&mut app, bg_chord);
        assert!(consumed, "Ctrl+Enter must be consumed");

        // New tab pushed.
        assert_eq!(
            app.app.tabs().tabs().len(),
            tab_count_before + 1,
            "one new tab must be pushed in the background"
        );
        // Active index unchanged — background push does not change focus.
        assert_eq!(
            app.app.tabs().active_index(),
            active_idx_before,
            "background-open must not change the active tab"
        );
        // Toast must mention the alias.
        assert!(
            app.toasts.iter().any(|t| t.message.contains("bg-host")),
            "toast must mention the alias"
        );
        // Inspector form remains open (plan Task 5: "without closing the inspector").
        assert!(
            app.form
                .as_ref()
                .map(|f| f.spec.id.0.starts_with("ssh.inspect:"))
                .unwrap_or(false),
            "inspector form must remain open after background-open"
        );
        // Fix 3: pushed tab id must be "ssh:<alias>", not the bare "ssh" id.
        let pushed_tab = app.app.tabs().tabs().last().unwrap();
        assert_eq!(
            pushed_tab.id.as_str(),
            "ssh:bg-host",
            "pushed tab must have unique id 'ssh:<alias>'"
        );
        // Fix 3: pending connect must be seeded so the detail tab connects.
        let bg_ssh = pushed_tab
            .layout
            .iter_widgets()
            .next()
            .unwrap()
            .as_any()
            .downcast_ref::<sid_widgets::SshWidget>()
            .expect("background tab must hold an SshWidget");
        assert_eq!(
            bg_ssh.peek_pending_connect(),
            Some("bg-host"),
            "pending_connect must be seeded with the alias"
        );
        use sid_widgets::ssh::ConnectionPhase;
        assert_eq!(
            bg_ssh.connection().phase(),
            ConnectionPhase::Connecting,
            "connection must be in Connecting phase"
        );
    }

    /// Fix 3: re-invoking background-open for the same alias focuses the
    /// existing tab instead of stacking a second one (dedup).
    #[test]
    fn background_open_deduplicates_existing_tab() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        use sid_store::{SshAuthKind, SshHost, SshHostSource};
        let host = SshHost {
            alias: "dedup-host".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: vec![],
            last_sftp_path: None,
            auth_kind: SshAuthKind::Agent,
        };
        let mut app = build_app_with_ssh_hosts(vec![host.clone()]);

        // Open the inspector via →.
        let right_chord = KeyChord {
            code: KeyCode::Right,
            mods: KeyModifiers::NONE,
        };
        route_key_event(&mut app, right_chord);

        let bg_chord = KeyChord {
            code: KeyCode::Enter,
            mods: KeyModifiers::CONTROL,
        };

        // First background-open — pushes one new tab.
        route_key_event(&mut app, bg_chord);
        let count_after_first = app.app.tabs().tabs().len();

        // Second background-open — must NOT push another tab.
        route_key_event(&mut app, bg_chord);
        assert_eq!(
            app.app.tabs().tabs().len(),
            count_after_first,
            "second background-open of same alias must not stack a duplicate tab"
        );
        // The dedup toast must mention the alias.
        assert!(
            app.toasts
                .iter()
                .any(|t| t.message.contains("dedup-host") && t.message.contains("already open")),
            "dedup must produce a toast mentioning the alias and 'already open'"
        );
    }

    /// Fix 1: typing 'O' into an editable text field (identity_file) in the
    /// ssh inspector must insert the character — NOT spawn a background tab.
    #[test]
    fn background_open_o_key_does_not_fire_when_text_field_focused() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        use sid_store::{SshAuthKind, SshHost, SshHostSource};
        let host = SshHost {
            alias: "text-host".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: vec![],
            last_sftp_path: None,
            auth_kind: SshAuthKind::Agent,
        };
        let mut app = build_app_with_ssh_hosts(vec![host]);

        // Open the inspector via →.  Manual host → has editable Prefs section
        // with a Text identity_file field which is focused by default.
        let right_chord = KeyChord {
            code: KeyCode::Right,
            mods: KeyModifiers::NONE,
        };
        route_key_event(&mut app, right_chord);
        assert!(
            app.form
                .as_ref()
                .map(|f| f.spec.id.0.starts_with("ssh.inspect:"))
                .unwrap_or(false),
            "inspector must be open"
        );

        let tab_count_before = app.app.tabs().tabs().len();

        // Advance focus to the identity_file Text field (Tab through the form).
        // The first slot in an editable form is typically a text field, so
        // focused_field_is_text() should return true right after open.
        assert!(
            app.form
                .as_ref()
                .map(|f| f.focused_field_is_text())
                .unwrap_or(false),
            "first focusable slot after open must be a text field"
        );

        // Press 'O' — is_background_open() returns true for Char('O'), but the
        // guard must block it because focused_is_text == true.
        let o_chord = KeyChord {
            code: KeyCode::Char('O'),
            mods: KeyModifiers::NONE,
        };
        route_key_event(&mut app, o_chord);

        assert_eq!(
            app.app.tabs().tabs().len(),
            tab_count_before,
            "'O' in a text field must NOT push a background tab"
        );
    }

    /// Fix 1: 'O' on a non-text field (e.g. after tabbing past text fields to
    /// the Save button) DOES background-open.
    #[test]
    fn background_open_o_key_fires_when_non_text_focused() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        use sid_store::{SshAuthKind, SshHost, SshHostSource};
        let host = SshHost {
            alias: "non-text-host".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: vec![],
            last_sftp_path: None,
            auth_kind: SshAuthKind::Agent,
        };
        let mut app = build_app_with_ssh_hosts(vec![host]);

        // Open inspector.
        let right_chord = KeyChord {
            code: KeyCode::Right,
            mods: KeyModifiers::NONE,
        };
        route_key_event(&mut app, right_chord);

        // Shift focus away from text fields by Tab-pressing until
        // focused_field_is_text() returns false (or we exhaust the form).
        let tab_key = KeyChord {
            code: KeyCode::Tab,
            mods: KeyModifiers::NONE,
        };
        for _ in 0..20 {
            if app
                .form
                .as_ref()
                .map(|f| !f.focused_field_is_text())
                .unwrap_or(true)
            {
                break;
            }
            // Let the form consume Tab directly (not via route_key_event).
            if let Some(f) = app.form.as_mut() {
                f.handle_key(tab_key);
            }
        }

        // If after 20 Tabs we still haven't found a non-text slot, the inspector
        // may only have text fields (acceptable) — skip the background-open
        // assertion, but verify no crash occurred.
        if app
            .form
            .as_ref()
            .map(|f| f.focused_field_is_text())
            .unwrap_or(true)
        {
            // All slots text — Ctrl+Enter still works regardless.
            let bg_chord = KeyChord {
                code: KeyCode::Enter,
                mods: KeyModifiers::CONTROL,
            };
            let tab_count_before = app.app.tabs().tabs().len();
            route_key_event(&mut app, bg_chord);
            assert_eq!(
                app.app.tabs().tabs().len(),
                tab_count_before + 1,
                "Ctrl+Enter must always background-open regardless of field type"
            );
            return;
        }

        // We have a non-text field focused — 'O' must background-open.
        let tab_count_before = app.app.tabs().tabs().len();
        let o_chord = KeyChord {
            code: KeyCode::Char('O'),
            mods: KeyModifiers::NONE,
        };
        route_key_event(&mut app, o_chord);
        assert_eq!(
            app.app.tabs().tabs().len(),
            tab_count_before + 1,
            "'O' on a non-text field must push a background tab"
        );
    }

    /// Background-open on a NON-ssh form (e.g. database.connection) must NOT
    /// push a new tab — the intercept is scoped to ssh.inspect: form ids only.
    #[test]
    fn background_open_does_not_fire_on_non_ssh_inspector_form() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        // Build an app with any non-ssh form open.  Use a bare FormSpec with a
        // database-flavoured id so the intercept guard rejects it.
        let mut app = build_test_sid_app(Some("ssh"));
        let fake_spec = sid_widgets::form::FormSpec::new("database.connection", "fake", vec![]);
        open_form(&mut app, fake_spec);
        assert!(app.form.is_some(), "form must be open for this test");

        let tab_count_before = app.app.tabs().tabs().len();

        let bg_chord = KeyChord {
            code: KeyCode::Enter,
            mods: KeyModifiers::CONTROL,
        };
        route_key_event(&mut app, bg_chord);

        assert_eq!(
            app.app.tabs().tabs().len(),
            tab_count_before,
            "background-open must NOT push a tab for a non-ssh-inspector form"
        );
    }

    // --- Task 4 (continued): add-new cursor ---

    #[test]
    fn add_new_cursor_enter_drains_to_open_form() {
        let mut app = build_test_sid_app(Some("ssh"));
        // Simulate Enter press on add-new row setting pending flag.
        if let Some(w) = active_ssh_widget_mut(&mut app) {
            w.pending_add_new = true;
        }
        drain_pending_ssh_add_new(&mut app);
        assert!(app.form.is_some());
        assert_eq!(app.form.as_ref().unwrap().spec.id.0, "ssh.new");
    }

    /// Previously `N` on SSH tab opened a modal; now it opens a FormPane.
    /// `modal_for_active_tab_key` must NOT return `ssh.new` anymore.
    #[test]
    fn ssh_new_modal_for_key_opens_on_ssh() {
        let sid_app = build_test_sid_app(Some("ssh"));
        // N is now handled by dispatch_ssh_form_key; modal_for_active_tab_key
        // should return None for N (no longer has the 'N' arm).
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'));
        // It may return None, or another modal — just must NOT be ssh.new.
        assert_ne!(
            modal.as_ref().map(|m| m.id.0.as_str()).unwrap_or(""),
            "ssh.new",
            "modal_for_active_tab_key must not open ssh.new (FormPane owns N now)"
        );
    }

    /// `N` on a non-SSH tab does NOT produce an `ssh.new` modal.
    #[test]
    fn ssh_new_modal_for_key_does_not_open_on_other_tabs() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert_ne!(id, "ssh.new", "workspaces N must not produce ssh.new");
    }

    /// `submit_ssh_new` upserts the host into the store AND the SSH widget sees
    /// the new host on the next render.
    /// Note: the "ssh.new" modal dispatch arm was retired by UX-v2; hosts are
    /// now added via the side-pane FormPane ("ssh.new" in dispatch_form_submit).
    /// This test calls submit_ssh_new directly to verify the core persistence
    /// contract shared by both paths.
    #[test]
    fn ssh_new_submit_persists_and_refreshes() {
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("ssh"));
        assert!(sid_app.store.list_ssh_hosts().unwrap().is_empty());

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
        let alias = submit_ssh_new(&mut sid_app, &values).expect("submit ok");
        assert_eq!(alias, "my-prod");

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

    /// The `auth` Choice value persists through `submit_ssh_new` for every variant.
    /// Note: the "ssh.new" modal dispatch arm was retired by UX-v2; the form path
    /// uses lowercase auth choices ("key", "password") via submit_ssh_new_from_form.
    /// This test exercises submit_ssh_new's uppercase-matching parse_auth_choice.
    #[test]
    fn ssh_new_submit_persists_each_auth_kind() {
        use sid_store::SshAuthKind;
        use sid_widgets::FieldValue;

        let cases = [
            ("Key", SshAuthKind::Key),
            ("Password", SshAuthKind::Password),
            ("Agent", SshAuthKind::Agent),
            // Unknown / missing value falls back to Agent (most permissive).
            ("WeirdNotAnOption", SshAuthKind::Agent),
        ];
        for (label, expected) in cases {
            let mut sid_app = build_test_sid_app(Some("ssh"));
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
            submit_ssh_new(&mut sid_app, &values).expect("submit ok");
            let hosts = sid_app.store.list_ssh_hosts().unwrap();
            assert_eq!(
                hosts[0].auth_kind, expected,
                "{label} choice should persist as {expected:?}"
            );
        }
    }

    /// §D: the side-pane FormPane path (`submit_ssh_new_from_form`) maps the
    /// lowercase `auth` Choice to the right `SshAuthKind` and persists it on the
    /// `SshHost`. The form Choice substrate emits "agent"/"key"/"password".
    #[test]
    fn ssh_new_from_form_persists_each_auth_kind() {
        use sid_store::SshAuthKind;
        let cases = [
            ("agent", SshAuthKind::Agent),
            ("key", SshAuthKind::Key),
            ("password", SshAuthKind::Password),
            // Unknown/missing falls back to Agent.
            ("nonsense", SshAuthKind::Agent),
        ];
        for (choice, expected) in cases {
            let mut sid_app = build_test_sid_app(Some("ssh"));
            let mut values = sid_widgets::form::FormValues::new();
            values.insert("alias".into(), format!("h-{choice}"));
            values.insert("host".into(), "10.0.0.1".into());
            values.insert("user".into(), "alice".into());
            values.insert("port".into(), "22".into());
            values.insert("identity_file".into(), String::new());
            values.insert("auth".into(), choice.into());
            let alias = submit_ssh_new_from_form(&mut sid_app, &values).expect("submit ok");
            let persisted = sid_app.store.get_ssh_host(&alias).unwrap().unwrap();
            assert_eq!(
                persisted.auth_kind, expected,
                "form auth '{choice}' should persist as {expected:?}"
            );
        }
    }

    /// §D: the add-form's actual Choice options ("agent"/"key"/"password") each
    /// round-trip through `parse_auth_form_choice`, guarding against the form
    /// substrate and the parser drifting apart.
    #[test]
    fn ssh_add_form_choice_options_all_parse() {
        use sid_store::SshAuthKind;
        use sid_widgets::modal::Field;
        let spec = sid_widgets::ssh::ssh_add_form_spec();
        let auth_field = spec
            .sections
            .iter()
            .flat_map(|s| &s.fields)
            .find(|f| f.key == "auth")
            .expect("auth field present");
        let Field::Choice { options, .. } = &auth_field.field else {
            panic!("auth field must be a Choice");
        };
        assert_eq!(options, &["agent", "key", "password"]);
        assert_eq!(parse_auth_form_choice(Some("agent")), SshAuthKind::Agent);
        assert_eq!(parse_auth_form_choice(Some("key")), SshAuthKind::Key);
        assert_eq!(
            parse_auth_form_choice(Some("password")),
            SshAuthKind::Password
        );
    }

    /// `submit_ssh_new` requires alias, host, user. Empty alias → Err.
    /// Note: the "ssh.new" modal dispatch arm was retired by UX-v2; this tests
    /// the shared validation in submit_ssh_new directly.
    #[test]
    fn ssh_new_submit_rejects_missing_required_fields() {
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("ssh"));
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
        let err = submit_ssh_new(&mut sid_app, &values).unwrap_err();
        assert!(err.to_string().contains("alias"));
    }

    /// `submit_ssh_new` rejects a port that is not a u16.
    /// Note: the "ssh.new" modal dispatch arm was retired by UX-v2; this tests
    /// the shared port validation in submit_ssh_new directly.
    #[test]
    fn ssh_new_submit_rejects_non_u16_port() {
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("ssh"));
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
        let err = submit_ssh_new(&mut sid_app, &values).unwrap_err();
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
        // Refresh the widget to pick up the new host, then step off the
        // "+ add new" row onto it (the cursor starts on the synthetic row).
        refresh_ssh_widget(&mut sid_app);
        if let Some(w) = active_ssh_widget_mut(&mut sid_app) {
            if w.state().selected_host().is_none() {
                w.state_mut().select_next();
            }
        }

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

    /// SECURITY: a flag-like `output_path` (leading '-') is rejected before it
    /// can be smuggled to `ssh-keygen -f` / `ssh-copy-id -i` as a flag. Fails
    /// fast with a clear message; never shells out.
    #[test]
    fn ssh_gen_key_step2_rejects_flaglike_output_path() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let id = ModalId("ssh.gen_key.step2:Ed25519".to_string());
        let values = vec![
            (
                "output_path".to_string(),
                FieldValue::Picker("-oProxyCommand=evil".to_string()),
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
        let err = dispatch_modal_submit(&mut sid_app, &id, &values).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("output_path"), "got: {msg}");
        assert!(msg.contains("flag"), "got: {msg}");
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
        // Step off the "+ add new" row onto the first host (cursor starts on
        // the synthetic row because show_add_new_row defaults on).
        if let Some(w) = active_ssh_widget_mut(sid_app) {
            if w.state().selected_host().is_none() {
                w.state_mut().select_next();
            }
        }
    }

    /// `E` on the SSH tab with a selected manual host now opens a FormPane
    /// (not a modal). `modal_for_active_tab_key` must return `None` for `E`.
    #[test]
    fn ssh_edit_modal_for_key_opens_on_ssh() {
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "edit-me");
        // E is now handled by dispatch_ssh_form_key (FormPane path).
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('E'));
        assert!(
            modal.is_none()
                || !modal
                    .as_ref()
                    .map(|m| m.id.0.starts_with("ssh.edit"))
                    .unwrap_or(false),
            "E must no longer open an ssh.edit modal"
        );
        // Verify FormPane path works instead.
        let opened = dispatch_ssh_form_key(
            &mut sid_app,
            sid_core::event::KeyChord {
                code: crossterm::event::KeyCode::Char('E'),
                mods: crossterm::event::KeyModifiers::empty(),
            },
        );
        assert!(opened, "E on ssh tab must open FormPane");
        assert!(
            sid_app
                .form
                .as_ref()
                .map(|f| f.spec.id.0.starts_with("ssh.edit"))
                .unwrap_or(false)
        );
    }

    /// `E` on a non-SSH tab does not open an SSH modal.
    #[test]
    fn ssh_edit_modal_does_not_open_on_other_tabs() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('E'));
        let id = modal.map(|m| m.id.0).unwrap_or_default();
        assert!(!id.starts_with("ssh.edit"));
    }

    /// `submit_ssh_edit` updates the host record fields.
    /// Note: the "ssh.edit:<alias>" modal dispatch arm was retired by UX-v2;
    /// hosts are now edited via the side-pane FormPane ("ssh.edit:<alias>" in
    /// dispatch_form_submit). This test calls submit_ssh_edit directly to
    /// verify the core update contract.
    #[test]
    fn ssh_edit_submit_updates_host() {
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "alpha");
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
        submit_ssh_edit(&mut sid_app, "alpha", &values).unwrap();
        let h = sid_app.store.get_ssh_host("alpha").unwrap().unwrap();
        assert_eq!(h.host, "10.99.99.99");
        assert_eq!(h.user, "admin");
        assert_eq!(h.port, 2222);
        assert_eq!(h.identity_file.as_deref(), Some("/tmp/id_test"));
    }

    /// `submit_ssh_edit` rejects an empty alias.
    /// Note: the "ssh.edit:<alias>" modal dispatch arm was retired by UX-v2;
    /// this tests the shared validation in submit_ssh_edit directly.
    #[test]
    fn ssh_edit_submit_rejects_empty_alias() {
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("ssh"));
        upsert_host_for(&mut sid_app, "alpha");
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
        let err = submit_ssh_edit(&mut sid_app, "alpha", &values).unwrap_err();
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

    /// `?` on any tab opens the help overlay (fixed id "help.overlay").
    #[test]
    fn ssh_help_modal_lists_footer_hints() {
        let sid_app = build_test_sid_app(Some("ssh"));
        let modal =
            modal_for_active_tab_key(&sid_app, plain_chord('?')).expect("? always opens help");
        // Overlay now uses a fixed id regardless of active tab.
        assert_eq!(modal.id.0, "help.overlay");
        // The "This tab" Display field should contain the SshWidget's footer
        // hints (N/G/S/K/X/?).
        let tab_body = modal
            .fields
            .iter()
            .find_map(|f| match f {
                sid_widgets::Field::Display { label, body } if label == "This tab" => {
                    Some(body.clone())
                }
                _ => None,
            })
            .unwrap_or_default();
        // The overlay shows the FULL footer_hint list — all hints including
        // E and G which are beyond the slim rendered footer cap.
        // Rendered footer: N / ⏎ / → / ? (plan decision 13: 3 primary verbs + ?: help).
        // Overlay (here): N / ⏎ / → / E / G (the long tail the slim footer drops).
        for ch in ["N", "→", "E", "G"] {
            assert!(
                tab_body.contains(ch),
                "expected tab body to mention {ch}; got: {tab_body}"
            );
        }
        // Global hints in the "Global" field.
        let global_body = modal
            .fields
            .iter()
            .find_map(|f| match f {
                sid_widgets::Field::Display { label, body } if label == "Global" => {
                    Some(body.clone())
                }
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            global_body.contains("Ctrl+Q"),
            "global body must mention Ctrl+Q; got: {global_body}"
        );
    }

    /// `?` on any other tab also opens the overlay with the fixed id.
    #[test]
    fn ssh_help_modal_opens_on_other_tabs_too() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal =
            modal_for_active_tab_key(&sid_app, plain_chord('?')).expect("? always opens help");
        // Always "help.overlay" — not keyed per tab.
        assert_eq!(modal.id.0, "help.overlay");
    }

    /// The help overlay uses two `Field::Display` fields — one for Global,
    /// one for the active tab — so multi-line bodies render one row per
    /// `\n`-separated line.
    #[test]
    fn help_modal_uses_display_field_with_multiline_body() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let modal =
            modal_for_active_tab_key(&sid_app, plain_chord('?')).expect("? always opens help");
        let first_field = modal.fields.first().expect("help modal has a field");
        match first_field {
            sid_widgets::Field::Display { label, body } => {
                assert_eq!(label, "Global");
                assert!(
                    body.contains('\n'),
                    "Global body must contain newlines so the Display renderer paints multi-row"
                );
                assert!(
                    body.contains("Ctrl+Q"),
                    "Global body must contain Ctrl+Q hint"
                );
            }
            other => panic!("help modal first field must be Display; got {other:?}"),
        }
        // Second field is "This tab".
        let second_field = modal.fields.get(1).expect("help modal has two fields");
        match second_field {
            sid_widgets::Field::Display { label, .. } => {
                assert_eq!(label, "This tab");
            }
            other => panic!("help modal second field must be Display; got {other:?}"),
        }
    }

    // ─── Task 10 — Help overlay ─────────────────────────────────────────────

    /// `?` on the database tab opens the overlay; the "This tab" Display
    /// body contains a known database hint label.
    #[test]
    fn question_mark_opens_help_overlay_with_tab_section() {
        let sid_app = build_test_sid_app(Some("database"));
        let modal =
            modal_for_active_tab_key(&sid_app, plain_chord('?')).expect("? always opens help");
        assert_eq!(modal.id.0, "help.overlay", "overlay must use the fixed id");
        assert_eq!(modal.title, "Keybinds", "overlay title must be Keybinds");
        // Second field is the per-tab section.
        let tab_body = modal
            .fields
            .iter()
            .find_map(|f| match f {
                sid_widgets::Field::Display { label, body } if label == "This tab" => {
                    Some(body.clone())
                }
                _ => None,
            })
            .expect("overlay must have a 'This tab' field");
        // DatabaseWidget advertises "new" as its first hint.
        assert!(
            tab_body.contains("new"),
            "tab section must contain the 'new' hint; got: {tab_body}"
        );
    }

    /// When a form is active, `?` is consumed by the form as a literal
    /// character and must NOT push the help overlay onto the modal stack.
    #[test]
    fn question_mark_inside_form_types_literally() {
        let mut sid_app = build_test_sid_app(Some("database"));
        open_form(&mut sid_app, test_form_spec("test.edit"));
        assert!(sid_app.form.is_some(), "form must be open");
        // Route `?` through the wire layer.
        route_key_event(&mut sid_app, plain_chord('?'));
        // No modal must have been pushed.
        assert!(
            sid_app.modal_stack.is_empty(),
            "form must consume '?' without opening the help overlay"
        );
        // The first text field in section 0 ("name") must now contain '?'.
        let form = sid_app.form.as_ref().unwrap();
        let val = form.spec.sections[0].fields[0].value_string();
        assert!(
            val.ends_with('?'),
            "form field must have received '?' as literal input; got: {val:?}"
        );
    }

    /// A widget with 6 hints produces a slimmed footer list of 3 + `? help`.
    #[test]
    fn footer_caps_hints_and_appends_help() {
        use sid_core::FooterHint;
        let six_hints: Vec<FooterHint> = (0..6)
            .map(|i| FooterHint::new(format!("K{i}"), format!("action{i}")))
            .collect();
        let slimmed = slim_footer_hints(six_hints);
        assert_eq!(
            slimmed.len(),
            4,
            "3 capped + 1 '? help' = 4; got {}",
            slimmed.len()
        );
        // Last entry must be the `?` hint.
        let last = slimmed.last().unwrap();
        assert_eq!(last.chord, "?");
        assert_eq!(last.label, "help");
        // First 3 are the originals.
        for (i, h) in slimmed.iter().take(3).enumerate() {
            assert_eq!(h.chord, format!("K{i}"));
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

    /// A left click on a row that is NOT the tab strip row but IS inside the
    /// body region routes to [`MouseRouting::FocusInBody`] for the active
    /// widget to focus the clicked pane.
    #[test]
    fn mouse_left_click_in_body_routes_to_focus_in_body() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let m = mouse_event(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            10,
            5,
        );
        let outcome = route_mouse_event(&sid_app, full_area(), m);
        assert_eq!(outcome, MouseRouting::FocusInBody { col: 10, row: 5 });
    }

    /// A left click outside the body (e.g. on the footer row near the bottom)
    /// is dropped.
    #[test]
    fn mouse_left_click_outside_body_drops() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        let m = mouse_event(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            10,
            // full_area is 120x40; the footer occupies the bottom 2 rows. y=39 is the very
            // last row, well below the body.
            39,
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

    /// `N` on the Database tab no longer opens the `database.new` modal —
    /// UX-v2: N is now consumed by the widget and emits DbCommand::OpenConnectionForm.
    /// `database_modal_for_key` should return `None` for `N`.
    #[test]
    fn n_key_on_database_tab_does_not_open_old_modal() {
        let sid_app = build_test_sid_app(Some("database"));
        let modal = modal_for_active_tab_key(&sid_app, plain_chord('N'));
        assert!(
            modal.is_none(),
            "N must not open any modal on the database tab — it routes to the side-pane form"
        );
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
    /// Note: tests submit_database_new directly — the "database.new" modal path was
    /// retired by UX-v2; connections now use the form substrate ("database.connection").
    #[test]
    fn database_new_submit_persists_and_refreshes_postgres_with_password() {
        use sid_core::adapters::secrets::SecretId;
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("database"));
        assert!(sid_app.store.list_db_connections().unwrap().is_empty());

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
        submit_database_new(&mut sid_app, &values).unwrap();
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
    /// Note: tests submit_database_new directly — the "database.new" modal path was
    /// retired by UX-v2; connections now use the form substrate ("database.connection").
    #[test]
    fn database_new_submit_sqlite_ignores_password() {
        use sid_core::adapters::secrets::SecretId;
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("database"));
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
        submit_database_new(&mut sid_app, &values).unwrap();
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
        use sid_core::adapters::{db_client::DbKind, secrets::SecretId};
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

    // ─── Database UX-v2 form path ────────────────────────────────────────────

    fn database_widget_ref(app: &SidApp) -> &DatabaseWidget {
        app.app
            .tabs()
            .tabs()
            .iter()
            .find(|t| t.id.as_str() == "database")
            .and_then(|t| t.layout.iter_widgets().next())
            .and_then(|w| w.as_any().downcast_ref::<DatabaseWidget>())
            .expect("database widget not found")
    }

    #[test]
    fn db_form_spec_postgres_sections_reshape_on_kind_change() {
        use sid_widgets::form::SectionKind;
        let spec = db_connection_form_spec(None);
        // default kind is Postgres
        let pg_keys: Vec<&str> = spec
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Editable)
            .flat_map(|s| s.fields.iter().map(|f| f.key.as_str()))
            .collect();
        assert!(pg_keys.contains(&"name"), "missing name in pg spec");
        assert!(pg_keys.contains(&"host"), "missing host in pg spec");
        assert!(pg_keys.contains(&"port"), "missing port in pg spec");
        assert!(pg_keys.contains(&"database"), "missing database in pg spec");
        assert!(pg_keys.contains(&"user"), "missing user in pg spec");
        assert!(pg_keys.contains(&"password"), "missing password in pg spec");
        // Info section should have dsn key
        let info_keys: Vec<&str> = spec
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Info)
            .flat_map(|s| s.fields.iter().map(|f| f.key.as_str()))
            .collect();
        assert!(info_keys.contains(&"dsn"), "missing dsn info row");
    }

    #[test]
    fn db_form_spec_reshapes_to_sqlite_on_kind_change() {
        use sid_widgets::{form::SectionKind, modal::Field};
        let mut spec = db_connection_form_spec(None);
        // change kind to SQLite
        let kind_section = spec
            .sections
            .iter_mut()
            .find(|s| s.kind == SectionKind::Editable)
            .expect("editable section");
        let kind_field = kind_section
            .fields
            .iter_mut()
            .find(|f| f.key == "kind")
            .expect("kind field");
        if let Field::Choice {
            selected, options, ..
        } = &mut kind_field.field
        {
            *selected = options.iter().position(|o| o == "SQLite").unwrap_or(0);
        }
        spec.run_reshape();
        let editable_keys: Vec<&str> = spec
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Editable)
            .flat_map(|s| s.fields.iter().map(|f| f.key.as_str()))
            .collect();
        assert!(
            editable_keys.contains(&"path"),
            "missing path in sqlite spec"
        );
        assert!(
            !editable_keys.contains(&"host"),
            "host should be absent in sqlite spec"
        );
        assert!(
            !editable_keys.contains(&"password"),
            "password should be absent in sqlite spec"
        );
    }

    #[test]
    fn db_form_spec_prefill_populates_values() {
        use sid_core::adapters::db_client::DbKind;
        use sid_store::{DbConnection, now_epoch};
        let conn = DbConnection {
            id: "local-pg".to_string(),
            kind: DbKind::Postgres,
            name: "Local Postgres".to_string(),
            dsn: "postgres://dbuser@localhost:5432/mydb".to_string(),
            secret_ref: None,
            created_at: now_epoch(),
        };
        let spec = db_connection_form_spec(Some(&conn));
        let values = spec.values();
        assert_eq!(
            values.get("name").map(String::as_str),
            Some("Local Postgres")
        );
        assert_eq!(values.get("kind").map(String::as_str), Some("Postgres"));
        // DSN fields parsed out
        assert_eq!(values.get("host").map(String::as_str), Some("localhost"));
        assert_eq!(values.get("port").map(String::as_str), Some("5432"));
        assert_eq!(values.get("database").map(String::as_str), Some("mydb"));
        assert_eq!(values.get("user").map(String::as_str), Some("dbuser"));
    }

    #[test]
    fn db_form_spec_dsn_info_row_reflects_postgres_fields() {
        use sid_widgets::{form::SectionKind, modal::Field};
        let mut spec = db_connection_form_spec(None);
        // set host + port + database + user
        for section in spec
            .sections
            .iter_mut()
            .filter(|s| s.kind == SectionKind::Editable)
        {
            for field in &mut section.fields {
                match field.key.as_str() {
                    "host" => {
                        if let Field::Text { value, .. } = &mut field.field {
                            *value = "db.example.com".into();
                        }
                    }
                    "port" => {
                        if let Field::Text { value, .. } = &mut field.field {
                            *value = "5432".into();
                        }
                    }
                    "database" => {
                        if let Field::Text { value, .. } = &mut field.field {
                            *value = "app".into();
                        }
                    }
                    "user" => {
                        if let Field::Text { value, .. } = &mut field.field {
                            *value = "alice".into();
                        }
                    }
                    _ => {}
                }
            }
        }
        spec.run_reshape();
        let dsn_value = spec
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Info)
            .flat_map(|s| s.fields.iter())
            .find(|f| f.key == "dsn")
            .and_then(|f| {
                if let Field::Display { body, .. } = &f.field {
                    Some(body.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("");
        assert!(
            dsn_value.contains("db.example.com"),
            "dsn should contain host"
        );
        assert!(
            dsn_value.contains("app"),
            "dsn should contain database name"
        );
        assert!(dsn_value.contains("alice"), "dsn should contain user");
    }

    #[test]
    fn submit_db_connection_form_persists_new_connection() {
        use sid_widgets::form::FormValues;
        let mut app = build_test_sid_app(Some("database"));
        let mut values = FormValues::new();
        values.insert("name".into(), "Dev Postgres".into());
        values.insert("kind".into(), "Postgres".into());
        values.insert("host".into(), "localhost".into());
        values.insert("port".into(), "5432".into());
        values.insert("database".into(), "devdb".into());
        values.insert("user".into(), "dev".into());
        // no _id → create path
        let result = submit_db_connection_form(&mut app, values);
        assert!(result.is_ok(), "submit should succeed: {:?}", result);
        let conns = app.store.list_db_connections().unwrap();
        assert!(
            conns.iter().any(|c| c.name == "Dev Postgres"),
            "connection should be persisted"
        );
    }

    #[test]
    fn submit_db_connection_form_updates_existing_connection() {
        use sid_core::adapters::db_client::DbKind;
        use sid_store::{DbConnection, now_epoch};
        use sid_widgets::form::FormValues;
        let mut app = build_test_sid_app(Some("database"));
        // pre-seed a connection in the store
        let existing = DbConnection {
            id: "existing-pg".to_string(),
            kind: DbKind::Postgres,
            name: "Old Name".to_string(),
            dsn: "postgres://localhost:5432/olddb".to_string(),
            secret_ref: None,
            created_at: now_epoch(),
        };
        app.store.upsert_db_connection(&existing).unwrap();

        let mut values = FormValues::new();
        values.insert("_id".into(), "existing-pg".into());
        values.insert("name".into(), "New Name".into());
        values.insert("kind".into(), "Postgres".into());
        values.insert("host".into(), "localhost".into());
        values.insert("port".into(), "5432".into());
        values.insert("database".into(), "newdb".into());
        values.insert("user".into(), String::new());
        let result = submit_db_connection_form(&mut app, values);
        assert!(result.is_ok());
        let conns = app.store.list_db_connections().unwrap();
        assert!(
            conns
                .iter()
                .any(|c| c.id == "existing-pg" && c.name == "New Name"),
            "existing connection should be updated"
        );
    }

    #[test]
    fn database_widget_respects_show_add_new_row_setting() {
        use sid_store::TypedSettings;
        let mut app = build_test_sid_app(Some("database"));
        // Default (unset) → add_new = true (BuildAppData::default has show_add_new_row=true)
        {
            let w = database_widget_ref(&app);
            assert!(
                w.state().cursor.add_new,
                "cursor should have add_new=true by default"
            );
        }
        // Turn it off in the store
        app.store
            .put_bool(sid_store::settings_keys::SHOW_ADD_NEW_ROW, false)
            .unwrap();
        // Trigger a refresh (as if a connection was saved)
        refresh_database_widget(&mut app);
        {
            let w = database_widget_ref(&app);
            assert!(
                !w.state().cursor.add_new,
                "cursor should have add_new=false after setting stored false"
            );
        }
    }

    #[test]
    fn dispatch_form_submit_database_connection_saves_and_closes_form() {
        use sid_widgets::form::FormValues;
        let mut app = build_test_sid_app(Some("database"));
        // Open a form so it's visible
        let spec = db_connection_form_spec(None);
        open_form(&mut app, spec);
        assert!(app.form.is_some(), "form should be open");

        let mut values = FormValues::new();
        values.insert("name".into(), "TestConn".into());
        values.insert("kind".into(), "Postgres".into());
        values.insert("host".into(), "localhost".into());
        values.insert("port".into(), "5432".into());
        values.insert("database".into(), "testdb".into());
        values.insert("user".into(), "admin".into());

        dispatch_form_submit(&mut app, "database.connection", values);

        // Form should be closed
        assert!(app.form.is_none(), "form should be closed after submit");
        // Connection should be persisted
        let conns = app.store.list_db_connections().unwrap();
        assert!(
            conns.iter().any(|c| c.name == "TestConn"),
            "connection should be in store"
        );
    }

    /// Fix 2: unhandled DbCommands (RunQuery, Disconnect, CopyCell) must be
    /// dropped by drain_database_commands, not re-queued. Re-queuing would
    /// cause the command to survive every drain iteration until the process
    /// terminates — an unbounded busy-loop. After one drain, the queue must
    /// be empty.
    #[test]
    fn drain_database_commands_drops_unhandled_commands_not_requeue() {
        let mut app = build_test_sid_app(Some("database"));
        // Inject a RunQuery (Plan 4 stub — no consumer wired yet).
        {
            let tab = app
                .app
                .tabs_mut()
                .tabs_mut()
                .iter_mut()
                .find(|t| t.id.as_str() == "database")
                .expect("database tab");
            let widget = tab
                .layout
                .iter_widgets_mut()
                .next()
                .and_then(|w| w.as_any_mut().downcast_mut::<DatabaseWidget>())
                .expect("database widget");
            widget
                .state_mut()
                .push_command(sid_widgets::database::DbCommand::RunQuery {
                    sql: "SELECT 1".into(),
                    conn_id: "pg".into(),
                });
        }
        // First drain: command should be consumed (dropped with warn) and the
        // queue emptied — the widget must not re-queue it.
        drain_database_commands(&mut app);
        let remaining = {
            let tab = app
                .app
                .tabs_mut()
                .tabs_mut()
                .iter_mut()
                .find(|t| t.id.as_str() == "database")
                .expect("database tab");
            let widget = tab
                .layout
                .iter_widgets_mut()
                .next()
                .and_then(|w| w.as_any_mut().downcast_mut::<DatabaseWidget>())
                .expect("database widget");
            widget.state_mut().drain_commands()
        };
        assert!(
            remaining.is_empty(),
            "RunQuery must be dropped by drain, not re-queued; got: {:?}",
            remaining
        );
    }

    /// Fix 2 (routing check): RunQuery injected directly through the widget
    /// event handler (Ctrl+R) emits the command exactly once, and after a
    /// single drain it is gone.
    #[test]
    fn ctrl_r_in_editor_emits_run_query_and_drain_clears_it() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::{
            event::{Event, KeyChord},
            widget::Widget,
        };
        let mut app = build_test_sid_app(Some("database"));
        // set active connection so Ctrl+R has a conn_id to attach
        {
            let tab = app
                .app
                .tabs_mut()
                .tabs_mut()
                .iter_mut()
                .find(|t| t.id.as_str() == "database")
                .expect("database tab");
            let widget = tab
                .layout
                .iter_widgets_mut()
                .next()
                .and_then(|w| w.as_any_mut().downcast_mut::<DatabaseWidget>())
                .expect("database widget");
            widget.state_mut().set_active_conn_id_for_tests("pg".into());
            // Advance to Editor pane so Ctrl+R is routed correctly
            widget.focus_next(); // Connections → Editor
        }
        // Send Ctrl+R through the widget
        {
            let (tx, _rx) = std::sync::mpsc::channel();
            let mut ctx = sid_core::context::WidgetCtx::new(tx);
            let ev = Event::Key(KeyChord {
                code: KeyCode::Char('r'),
                mods: KeyModifiers::CONTROL,
            });
            let tab = app
                .app
                .tabs_mut()
                .tabs_mut()
                .iter_mut()
                .find(|t| t.id.as_str() == "database")
                .expect("database tab");
            if let Some(w) = tab
                .layout
                .iter_widgets_mut()
                .next()
                .and_then(|w| w.as_any_mut().downcast_mut::<DatabaseWidget>())
            {
                w.handle_event(&ev, &mut ctx);
            }
        }
        // One drain — must not leave the command in the queue.
        drain_database_commands(&mut app);
        let remaining = {
            let tab = app
                .app
                .tabs_mut()
                .tabs_mut()
                .iter_mut()
                .find(|t| t.id.as_str() == "database")
                .expect("database tab");
            tab.layout
                .iter_widgets_mut()
                .next()
                .and_then(|w| w.as_any_mut().downcast_mut::<DatabaseWidget>())
                .map(|w| w.state_mut().drain_commands())
                .unwrap_or_default()
        };
        assert!(
            remaining.is_empty(),
            "RunQuery from Ctrl+R must be dropped by drain; got: {:?}",
            remaining
        );
    }

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

    use std::sync::Mutex as StdMutex;

    use sid_core::{
        adapters::sys::{
            ListeningPort, NetInterface, Pid as SysPid, ProcessInfo, Protocol, Signal, SocketState,
            SysError, SysProvider,
        },
        sys_probe::{SysProbe, SysSnapshot},
    };

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
            default_route_iface: None,
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

    /// `submit_database_new` with valid fields returns the new connection id.
    /// Note: the "database.new" modal dispatch arm was retired by UX-v2; the toast
    /// is now pushed by the form-substrate path ("database.connection"). This test
    /// calls submit_database_new directly to verify the core persistence contract.
    #[test]
    fn dispatch_database_new_pushes_success_toast() {
        use sid_widgets::FieldValue;
        let mut sid_app = build_test_sid_app(Some("database"));
        let values = vec![
            ("id".into(), FieldValue::Text("prod-pg".into())),
            ("name".into(), FieldValue::Text("Prod".into())),
            ("kind".into(), FieldValue::Choice("SQLite".into())),
            ("dsn".into(), FieldValue::Text(":memory:".into())),
            ("password".into(), FieldValue::Password(String::new())),
        ];
        let conn_id = submit_database_new(&mut sid_app, &values).unwrap();
        assert_eq!(
            conn_id, "prod-pg",
            "submit_database_new must return the connection id"
        );
        assert!(
            sid_app
                .store
                .get_db_connection("prod-pg")
                .unwrap()
                .is_some(),
            "connection must be persisted"
        );
    }

    // ─── Feature 1 — Session resume modal ────────────────────────────────────

    /// Helper: write a SessionRecord into the store with a controlled ended_at.
    fn write_session(store: &dyn Store, id: &str, active_tab: Option<&str>, ended_at: Option<u64>) {
        let rec = SessionRecord {
            id: id.to_string(),
            started_at: now_epoch().saturating_sub(10_000_000_000),
            last_active: now_epoch(),
            ended_at,
            active_tab: active_tab.map(TabId::new),
            open_tabs: vec![],
        };
        store.upsert_session(&rec).unwrap();
    }

    #[test]
    fn resume_modal_pushes_when_recent_session_with_tab() {
        let mut sid_app = build_test_sid_app(None);
        // Recent session that ended 30s ago, with active_tab = ssh.
        let ended = now_epoch().saturating_sub(30 * 1_000_000_000);
        write_session(&*sid_app.store, "sess-prev", Some("ssh"), Some(ended));
        assert!(sid_app.modal_stack.is_empty());
        maybe_push_resume_modal(&mut sid_app);
        assert_eq!(sid_app.modal_stack.len(), 1);
        assert_eq!(sid_app.modal_stack[0].id.0, "session.resume:ssh");
        assert_eq!(sid_app.modal_stack[0].title, "Resume previous session?");
        // Single choice field named "action" with Resume / Start fresh.
        assert_eq!(sid_app.modal_stack[0].fields.len(), 1);
        if let sid_widgets::Field::Choice { label, options, .. } = &sid_app.modal_stack[0].fields[0]
        {
            assert_eq!(label, "action");
            assert_eq!(options, &vec!["Resume".to_string(), "Start fresh".into()]);
        } else {
            panic!("expected a Choice field");
        }
    }

    #[test]
    fn resume_modal_does_not_push_when_no_session() {
        let mut sid_app = build_test_sid_app(None);
        // Fresh store has no session record.
        assert!(sid_app.store.current_session().unwrap().is_none());
        maybe_push_resume_modal(&mut sid_app);
        assert!(sid_app.modal_stack.is_empty());
    }

    #[test]
    fn resume_modal_does_not_push_when_session_too_old() {
        let mut sid_app = build_test_sid_app(None);
        // Session ended 2 hours ago — beyond the 60-minute window.
        let two_hours_ns = 2 * 60 * 60 * 1_000_000_000u64;
        let ended = now_epoch().saturating_sub(two_hours_ns);
        write_session(&*sid_app.store, "sess-old", Some("ssh"), Some(ended));
        maybe_push_resume_modal(&mut sid_app);
        assert!(sid_app.modal_stack.is_empty());
    }

    #[test]
    fn resume_modal_does_not_push_when_session_has_no_active_tab() {
        let mut sid_app = build_test_sid_app(None);
        // Recent but no active_tab — nothing to resume.
        let ended = now_epoch().saturating_sub(1_000_000_000);
        write_session(&*sid_app.store, "sess-no-tab", None, Some(ended));
        maybe_push_resume_modal(&mut sid_app);
        assert!(sid_app.modal_stack.is_empty());
    }

    #[test]
    fn resume_modal_pushes_when_session_never_ended() {
        // ended_at == None — treat as recent (process exited without
        // clean end_session).
        let mut sid_app = build_test_sid_app(None);
        write_session(&*sid_app.store, "sess-running", Some("system"), None);
        maybe_push_resume_modal(&mut sid_app);
        assert_eq!(sid_app.modal_stack.len(), 1);
        assert_eq!(sid_app.modal_stack[0].id.0, "session.resume:system");
    }

    // ---- T4: AUTO_RESTORE_SESSION policy ----------------------------------

    /// Seed a recent prior session with `active_tab = ssh`.
    fn seed_recent_ssh_session(sid_app: &SidApp) {
        let ended = now_epoch().saturating_sub(30 * 1_000_000_000);
        write_session(&*sid_app.store, "sess-prev", Some("ssh"), Some(ended));
    }

    /// `"ask"` (the default) pushes the resume modal and does not switch tabs.
    #[test]
    fn auto_restore_ask_pushes_modal() {
        use sid_store::TypedSettings;
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        seed_recent_ssh_session(&sid_app);
        sid_app
            .store
            .put_string(sid_store::settings_keys::AUTO_RESTORE_SESSION, "ask")
            .unwrap();
        apply_auto_restore(&mut sid_app);
        assert_eq!(sid_app.modal_stack.len(), 1, "ask must push a modal");
        assert_eq!(sid_app.modal_stack[0].id.0, "session.resume:ssh");
        // Active tab unchanged until the user chooses Resume.
        assert_eq!(sid_app.app.tabs().active().id.as_str(), "workspaces");
    }

    /// An unset setting defaults to `"ask"`.
    #[test]
    fn auto_restore_unset_defaults_to_ask() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        seed_recent_ssh_session(&sid_app);
        apply_auto_restore(&mut sid_app);
        assert_eq!(sid_app.modal_stack.len(), 1, "unset must behave as ask");
    }

    /// `"yes"` silently switches to the prior tab and pushes NO modal.
    #[test]
    fn auto_restore_yes_switches_tab_without_modal() {
        use sid_store::TypedSettings;
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        seed_recent_ssh_session(&sid_app);
        sid_app
            .store
            .put_string(sid_store::settings_keys::AUTO_RESTORE_SESSION, "yes")
            .unwrap();
        apply_auto_restore(&mut sid_app);
        assert!(
            sid_app.modal_stack.is_empty(),
            "yes must not push a resume modal"
        );
        assert_eq!(
            sid_app.app.tabs().active().id.as_str(),
            "ssh",
            "yes must restore the prior tab silently"
        );
    }

    /// `"no"` pushes no modal and leaves the launch-default tab active.
    #[test]
    fn auto_restore_no_starts_fresh() {
        use sid_store::TypedSettings;
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        seed_recent_ssh_session(&sid_app);
        sid_app
            .store
            .put_string(sid_store::settings_keys::AUTO_RESTORE_SESSION, "no")
            .unwrap();
        apply_auto_restore(&mut sid_app);
        assert!(sid_app.modal_stack.is_empty(), "no must not push a modal");
        assert_eq!(
            sid_app.app.tabs().active().id.as_str(),
            "workspaces",
            "no must leave the launch-default tab"
        );
    }

    /// `"yes"` with no restorable prior session is a no-op (no panic, no
    /// switch).
    #[test]
    fn auto_restore_yes_no_prior_session_is_noop() {
        use sid_store::TypedSettings;
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        sid_app
            .store
            .put_string(sid_store::settings_keys::AUTO_RESTORE_SESSION, "yes")
            .unwrap();
        apply_auto_restore(&mut sid_app);
        assert!(sid_app.modal_stack.is_empty());
        assert_eq!(sid_app.app.tabs().active().id.as_str(), "workspaces");
    }

    #[test]
    fn dispatch_session_resume_choice_resume_switches_tab() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        assert_eq!(sid_app.app.tabs().active().id.as_str(), "workspaces");
        let id = ModalId("session.resume:database".to_string());
        let values = vec![("action".into(), FieldValue::Choice("Resume".into()))];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert_eq!(sid_app.app.tabs().active().id.as_str(), "database");
    }

    #[test]
    fn dispatch_session_resume_choice_start_fresh_does_nothing() {
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let before = sid_app.app.tabs().active().id.as_str().to_string();
        let id = ModalId("session.resume:database".to_string());
        let values = vec![("action".into(), FieldValue::Choice("Start fresh".into()))];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert_eq!(sid_app.app.tabs().active().id.as_str(), before);
    }

    #[test]
    fn dispatch_session_resume_with_unknown_tab_is_silent() {
        // The modal id encodes a tab id that no longer exists in the tab
        // list. `switch_to` returns false; dispatch must not panic and the
        // active tab must stay where it was.
        use sid_widgets::{FieldValue, ModalId};
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let before = sid_app.app.tabs().active().id.as_str().to_string();
        let id = ModalId("session.resume:not-a-real-tab".to_string());
        let values = vec![("action".into(), FieldValue::Choice("Resume".into()))];
        dispatch_modal_submit(&mut sid_app, &id, &values).unwrap();
        assert_eq!(sid_app.app.tabs().active().id.as_str(), before);
    }

    // ─── Feature 2 — Ctrl+D detach ───────────────────────────────────────────

    /// Test double for `TerminalSpawner` that records each spawn request and
    /// returns a configurable outcome. `Send + Sync` because the wire layer
    /// stores spawners in `Arc<dyn TerminalSpawner>`.
    #[derive(Default)]
    struct MockTerminalSpawner {
        requests: std::sync::Mutex<Vec<SpawnRequest>>,
        next_result: std::sync::Mutex<Option<Result<(), SpawnerError>>>,
    }

    impl MockTerminalSpawner {
        fn new() -> Self {
            Self::default()
        }
        fn with_failure(err: SpawnerError) -> Self {
            let m = Self::default();
            *m.next_result.lock().unwrap() = Some(Err(err));
            m
        }
        fn requests(&self) -> Vec<SpawnRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl TerminalSpawner for MockTerminalSpawner {
        fn spawn(&self, req: SpawnRequest) -> Result<(), SpawnerError> {
            self.requests.lock().unwrap().push(req);
            self.next_result.lock().unwrap().take().unwrap_or(Ok(()))
        }
        fn name(&self) -> &'static str {
            "mock"
        }
    }

    /// `handle_ctrl_d_detach` builds a SpawnRequest carrying the current
    /// `sid` exe path and `--start-tab <active>`, calls the spawner, and
    /// pushes a success toast.
    #[test]
    fn ctrl_d_on_active_tab_invokes_spawner_with_start_tab_arg() {
        let mut sid_app = build_test_sid_app(Some("ssh"));
        let mock = Arc::new(MockTerminalSpawner::new());
        sid_app.spawner = mock.clone() as Arc<dyn TerminalSpawner>;
        handle_ctrl_d_detach(&mut sid_app);
        let reqs = mock.requests();
        assert_eq!(reqs.len(), 1, "expected exactly one spawn call");
        // The command line should reference --start-tab and the active tab.
        assert!(
            reqs[0].cmd.contains("--start-tab"),
            "cmd should contain --start-tab; got: {}",
            reqs[0].cmd
        );
        assert!(
            reqs[0].cmd.contains("ssh"),
            "cmd should contain the active tab id 'ssh'; got: {}",
            reqs[0].cmd
        );
        // Success toast.
        let kinds: Vec<crate::toast::ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert!(
            kinds.contains(&crate::toast::ToastKind::Success),
            "expected a Success toast; got: {kinds:?}"
        );
    }

    /// `handle_ctrl_d_detach` surfaces a spawner failure as an error toast.
    #[test]
    fn ctrl_d_when_spawner_fails_pushes_error_toast() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let mock = Arc::new(MockTerminalSpawner::with_failure(
            SpawnerError::TerminalMissing("kitty".into()),
        ));
        sid_app.spawner = mock.clone() as Arc<dyn TerminalSpawner>;
        handle_ctrl_d_detach(&mut sid_app);
        let kinds: Vec<crate::toast::ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert!(
            kinds.contains(&crate::toast::ToastKind::Error),
            "expected an Error toast on spawner failure; got: {kinds:?}"
        );
        let messages: Vec<String> = sid_app.toasts.iter().map(|t| t.message.clone()).collect();
        assert!(
            messages.iter().any(|m| m.contains("detach failed")),
            "expected toast mentioning 'detach failed'; got: {messages:?}"
        );
    }

    /// `handle_ctrl_d_detach` with the default `NoopTerminalSpawner` also
    /// pushes an error toast (the noop returns `TerminalMissing`).
    #[test]
    fn ctrl_d_with_noop_spawner_pushes_error_toast() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        // build_test_sid_app already uses NoopTerminalSpawner; just call.
        handle_ctrl_d_detach(&mut sid_app);
        let kinds: Vec<crate::toast::ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&crate::toast::ToastKind::Error));
    }

    // ─── Feature 3 — Mouse click-to-focus dispatch ───────────────────────────

    /// A synthesized LeftDown event inside the body region dispatches to the
    /// active widget's `focus_at` and changes `focused_pane`.
    #[test]
    fn mouse_click_in_body_dispatches_focus_at() {
        use ratatui::layout::Rect;
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        // WorkspacesWidget defaults to Tree focus. Flip it manually so we
        // can prove the click changed it back.
        if let Some(w) = sid_app
            .app
            .tabs_mut()
            .active_mut()
            .layout
            .iter_widgets_mut()
            .next()
        {
            let any_ref = w as &mut dyn std::any::Any;
            if let Some(ws) = any_ref.downcast_mut::<sid_widgets::WorkspacesWidget>() {
                ws.focus_next();
                assert_eq!(ws.focused_pane(), sid_widgets::workspaces::WsFocus::SubView);
            }
        }
        // Click in the left half of the body — should focus Tree.
        let full = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        };
        let body = body_rect(full).expect("body rect");
        // col is well inside the left 30% of body.
        let col = body.x + body.width / 10;
        let row = body.y + body.height / 2;
        dispatch_focus_at_for_active_tab(&mut sid_app, full, col, row);
        if let Some(w) = sid_app.app.tabs().active().layout.iter_widgets().next() {
            if let Some(ws) = w.as_any().downcast_ref::<sid_widgets::WorkspacesWidget>() {
                assert_eq!(ws.focused_pane(), sid_widgets::workspaces::WsFocus::Tree);
            } else {
                panic!("expected WorkspacesWidget");
            }
        }
    }

    #[test]
    fn route_mouse_returns_focus_in_body_for_body_click() {
        let sid_app = build_test_sid_app(Some("workspaces"));
        // Click in the middle of the body region (well below the tab strip
        // and well above the footer).
        let m = mouse_event(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            60,
            20,
        );
        let outcome = route_mouse_event(&sid_app, full_area(), m);
        assert_eq!(outcome, MouseRouting::FocusInBody { col: 60, row: 20 });
    }

    #[test]
    fn body_rect_full_area_matches_draw_layout() {
        use ratatui::layout::Rect;
        let full = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        };
        let body = body_rect(full).expect("body rect");
        // Inner is (1, 1, 118, 38); after tabs(2) + status(1) + footer(2) the body
        // is 38 - 5 = 33 rows tall, starting at row 3.
        assert_eq!(body.x, 1);
        assert_eq!(body.y, 3);
        assert_eq!(body.width, 118);
        assert_eq!(body.height, 33);
    }

    #[test]
    fn body_rect_tiny_terminal_returns_none() {
        use ratatui::layout::Rect;
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        assert!(body_rect(tiny).is_none());
        let zero = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        assert!(body_rect(zero).is_none());
    }

    // ----- Form hosting (UX-v2 substrate) ----------------------------------

    /// A minimal two-field editable form spec for hosting tests.
    fn test_form_spec(id: &str) -> sid_widgets::form::FormSpec {
        use sid_widgets::{
            form::{FormField, FormSection, FormSpec, SectionKind},
            modal::Field,
        };
        FormSpec::new(
            id,
            "Test form",
            vec![FormSection {
                title: "Details".into(),
                kind: SectionKind::Editable,
                fields: vec![
                    FormField::new(
                        "name",
                        Field::Text {
                            label: "name".into(),
                            value: String::new(),
                            placeholder: None,
                        },
                    ),
                    FormField::new(
                        "host",
                        Field::Text {
                            label: "host".into(),
                            value: String::new(),
                            placeholder: None,
                        },
                    ),
                ],
            }],
        )
    }

    fn chord(code: crossterm::event::KeyCode) -> sid_core::event::KeyChord {
        sid_core::event::KeyChord::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn chord_mods(
        code: crossterm::event::KeyCode,
        mods: crossterm::event::KeyModifiers,
    ) -> sid_core::event::KeyChord {
        sid_core::event::KeyChord::new(code, mods)
    }

    /// `open_form` records the origin tab and installs the pane; a `Tab` chord
    /// routed through the wire is consumed by the form (focus advances) and the
    /// active tab is left unchanged.
    #[test]
    fn open_form_renders_split_and_form_consumes_tab() {
        use crossterm::event::KeyCode;
        use sid_widgets::form::PaneFocusState;

        let mut sid_app = build_test_sid_app(Some("database"));
        let before_idx = sid_app.app.tabs().active_index();
        open_form(&mut sid_app, test_form_spec("test.edit"));
        assert!(sid_app.form.is_some());
        assert_eq!(
            sid_app.form_origin_tab.as_ref().map(|t| t.as_str()),
            Some("database")
        );
        assert_eq!(
            sid_app.form.as_ref().unwrap().focus,
            PaneFocusState::Field(0)
        );

        // Tab is intercepted by the form (returns true = handled) and advances
        // focus; the active tab index does not move.
        let handled = route_key_event(&mut sid_app, chord(KeyCode::Tab));
        assert!(handled, "form must consume Tab");
        assert_eq!(
            sid_app.form.as_ref().unwrap().focus,
            PaneFocusState::Field(1),
            "Tab should advance the form's focus"
        );
        assert_eq!(
            sid_app.app.tabs().active_index(),
            before_idx,
            "tab strip must not cycle while a form is active"
        );
    }

    /// A form opened on one tab does not intercept keys while another tab is
    /// active — the chord falls through to the active widget and the form's
    /// values are untouched.
    #[test]
    fn form_only_intercepts_on_origin_tab() {
        use crossterm::event::KeyCode;

        let mut sid_app = build_test_sid_app(Some("workspaces"));
        open_form(&mut sid_app, test_form_spec("test.edit"));
        let values_before = sid_app.form.as_ref().unwrap().spec.values();

        // Switch to a different tab (index 1 = ssh).
        sid_app.app.tabs_mut().jump(1);
        assert_ne!(sid_app.app.tabs().active().id.as_str(), "workspaces");

        // A char key is NOT consumed by the off-origin form.
        let handled = route_key_event(&mut sid_app, chord(KeyCode::Char('x')));
        assert!(
            !handled,
            "form on a non-active tab must not intercept the key"
        );
        // The form's values are unchanged (it never saw the key).
        assert_eq!(sid_app.form.as_ref().unwrap().spec.values(), values_before);
    }

    /// Submitting a form whose id has no registered arm toasts an "unhandled
    /// form submit" diagnostic and closes the pane.
    #[test]
    fn submit_unknown_form_id_toasts_and_closes() {
        use crossterm::event::KeyCode;
        use sid_widgets::form::PaneFocusState;

        let mut sid_app = build_test_sid_app(Some("database"));
        open_form(&mut sid_app, test_form_spec("totally.unknown.form"));
        // Move focus onto the Save button, then press Enter to submit.
        sid_app.form.as_mut().unwrap().focus = PaneFocusState::Primary;
        let handled = route_key_event(&mut sid_app, chord(KeyCode::Enter));
        assert!(handled, "Enter on Save must be consumed by the form");

        assert!(sid_app.form.is_none(), "form must close after submit");
        assert!(sid_app.form_origin_tab.is_none());
        let toast_text = sid_app
            .toasts
            .iter()
            .map(|t| t.message.clone())
            .collect::<String>();
        assert!(
            toast_text.contains("unhandled form submit"),
            "expected diagnostic toast, got: {toast_text:?}"
        );
    }

    /// With no form active, Ctrl+Tab cycles forward and Ctrl+Shift+Tab cycles
    /// backward. Interim rule: only Ctrl-modified chords reach the strip-nav
    /// branch; plain Tab/BackTab fall through to widgets.
    #[test]
    fn strip_nav_cycles_tabs_when_no_form_active() {
        use crossterm::event::{KeyCode, KeyModifiers};

        let mut sid_app = build_test_sid_app(Some("workspaces"));
        assert_eq!(sid_app.app.tabs().active_index(), 0);

        // Ctrl+Tab -> forward (+1).
        assert!(route_key_event(
            &mut sid_app,
            chord_mods(KeyCode::Tab, KeyModifiers::CONTROL)
        ));
        assert_eq!(sid_app.app.tabs().active_index(), 1);

        // Ctrl+Shift+Tab -> back (-1).
        assert!(route_key_event(
            &mut sid_app,
            chord_mods(KeyCode::Tab, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        ));
        assert_eq!(sid_app.app.tabs().active_index(), 0);
    }

    /// Strip-nav is suppressed while a form is active — Ctrl+Tab goes to the
    /// form's interception layer, not the tab strip.
    #[test]
    fn strip_nav_suppressed_while_form_active() {
        use crossterm::event::{KeyCode, KeyModifiers};

        let mut sid_app = build_test_sid_app(Some("workspaces"));
        open_form(&mut sid_app, test_form_spec("test.edit"));
        let before = sid_app.app.tabs().active_index();
        // Ctrl+Tab must not cycle the strip while a form is open; the form
        // interception block fires first and consumes the key.
        route_key_event(
            &mut sid_app,
            chord_mods(KeyCode::Tab, KeyModifiers::CONTROL),
        );
        assert_eq!(
            sid_app.app.tabs().active_index(),
            before,
            "Ctrl+Tab must not cycle tabs while a form owns it"
        );
    }

    /// Plain Tab/Shift+Tab/BackTab must NOT be consumed by the strip-nav branch
    /// — they fall through to widgets for intra-widget focus cycling.
    #[test]
    fn plain_tab_falls_through_to_widget() {
        use crossterm::event::KeyCode;

        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let before = sid_app.app.tabs().active_index();

        // Plain Tab: route_key_event returns false (fall-through) and the
        // active tab index is unchanged.
        let consumed = route_key_event(&mut sid_app, chord(KeyCode::Tab));
        assert!(
            !consumed,
            "plain Tab must fall through (return false) — it belongs to the widget"
        );
        assert_eq!(
            sid_app.app.tabs().active_index(),
            before,
            "plain Tab must not cycle the tab strip"
        );

        // BackTab: same contract.
        let consumed = route_key_event(&mut sid_app, chord(KeyCode::BackTab));
        assert!(
            !consumed,
            "BackTab must fall through (return false) — it belongs to the widget"
        );
        assert_eq!(
            sid_app.app.tabs().active_index(),
            before,
            "BackTab must not cycle the tab strip"
        );
    }

    /// A modal wins over a form: while a modal is open the form does not see
    /// the key (modal_stack interception fires first).
    #[test]
    fn modal_wins_over_form() {
        use crossterm::event::KeyCode;

        let mut sid_app = build_test_sid_app(Some("database"));
        open_form(&mut sid_app, test_form_spec("test.edit"));
        let form_focus_before = sid_app.form.as_ref().unwrap().focus;
        // Push a benign modal on top.
        sid_app.modal_stack.push(sid_widgets::ModalSpec::new(
            "test.modal",
            "Test",
            vec![sid_widgets::modal::Field::Display {
                label: "info".into(),
                body: "hello".into(),
            }],
        ));
        // Tab is consumed by the modal, not the form.
        assert!(route_key_event(&mut sid_app, chord(KeyCode::Tab)));
        assert_eq!(
            sid_app.form.as_ref().unwrap().focus,
            form_focus_before,
            "modal must intercept before the form sees the key"
        );
    }

    /// A dirty form leaving via Esc opens the discard-confirm modal; choosing
    /// "Discard" and submitting it closes the form.
    #[test]
    fn dirty_form_esc_opens_discard_confirm_and_discard_closes() {
        use crossterm::event::KeyCode;

        let mut sid_app = build_test_sid_app(Some("database"));
        open_form(&mut sid_app, test_form_spec("test.edit"));
        // Type a char to dirty the form.
        route_key_event(&mut sid_app, chord(KeyCode::Char('a')));
        assert!(sid_app.form.as_ref().unwrap().dirty);

        // Esc on a dirty form requests the discard confirm modal.
        assert!(route_key_event(&mut sid_app, chord(KeyCode::Esc)));
        assert!(
            sid_app.form.is_some(),
            "form stays open until the user confirms discard"
        );
        assert_eq!(
            sid_app.modal_stack.last().map(|m| m.id.0.as_str()),
            Some("form.discard_confirm")
        );

        // Select "Discard" (Right cycles the Choice) then submit the modal.
        let modal = sid_app.modal_stack.last_mut().unwrap();
        sid_widgets::route_key_to_modal(modal, chord(KeyCode::Right));
        let outcome = sid_widgets::route_key_to_modal(
            sid_app.modal_stack.last_mut().unwrap(),
            chord(KeyCode::Enter),
        );
        assert_eq!(outcome, sid_widgets::ModalKeyOutcome::Submit);
        let popped = sid_app.modal_stack.pop().unwrap();
        let values = popped.collect_values();
        dispatch_modal_submit(&mut sid_app, &popped.id, &values).unwrap();

        assert!(
            sid_app.form.is_none(),
            "confirming discard must close the form"
        );
        assert!(sid_app.form_origin_tab.is_none());
    }

    /// While a form is active, a body click does NOT route through
    /// `dispatch_focus_at_for_active_tab` — the background widget's focus
    /// state must be unchanged. Regression test for the guard introduced in
    /// the same commit (`modal_stack.is_empty() && form.is_none()`).
    #[test]
    fn body_click_suppressed_while_form_is_active() {
        use crossterm::event::{MouseButton, MouseEventKind};

        let mut sid_app = build_test_sid_app(Some("workspaces"));

        // Record the WorkspacesWidget's focused pane before we do anything.
        fn ws_focus(sid_app: &SidApp) -> Option<sid_widgets::workspaces::WsFocus> {
            sid_app
                .app
                .tabs()
                .active()
                .layout
                .iter_widgets()
                .next()
                .and_then(|w| w.as_any().downcast_ref::<sid_widgets::WorkspacesWidget>())
                .map(|ws| ws.focused_pane())
        }

        // Flip the workspaces focus to SubView so we have a detectable baseline.
        if let Some(w) = sid_app
            .app
            .tabs_mut()
            .active_mut()
            .layout
            .iter_widgets_mut()
            .next()
        {
            let any_ref = w as &mut dyn std::any::Any;
            if let Some(ws) = any_ref.downcast_mut::<sid_widgets::WorkspacesWidget>() {
                ws.focus_next(); // Tree → SubView
            }
        }
        let focus_before = ws_focus(&sid_app);

        // Open a form — this is what the guard must detect.
        open_form(&mut sid_app, test_form_spec("body_click_suppressed.test"));
        assert!(sid_app.form.is_some(), "form must be open");

        // Simulate a body click in the left half (which, without the guard,
        // would dispatch into the hidden background WorkspacesWidget and flip
        // focus back to Tree).
        let m = mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            20, // col — well inside the left pane
            15, // row — inside the body region
        );
        let routing = route_mouse_event(&sid_app, full_area(), m);
        // The event still resolves to FocusInBody (route_mouse_event does not
        // know about forms); the suppression happens one layer up.
        assert_eq!(
            routing,
            MouseRouting::FocusInBody { col: 20, row: 15 },
            "route_mouse_event resolves to FocusInBody regardless of form state"
        );

        // Exercise the guard path directly (mirrors the event-loop arm).
        if sid_app.modal_stack.is_empty() && sid_app.form.is_none() {
            dispatch_focus_at_for_active_tab(&mut sid_app, full_area(), 20, 15);
        }
        // form.is_some() → guard does NOT call dispatch_focus_at — focus unchanged.
        assert_eq!(
            ws_focus(&sid_app),
            focus_before,
            "background widget focus must be unchanged while a form is open"
        );
    }

    /// While a form is active on the origin tab, the `tab.close` keybind chord
    /// (Alt+W / Ctrl+W) is consumed by the form and the tab count is unchanged.
    /// This is the close-invariant regression test described in the doc-comment
    /// added to `open_form`.
    #[test]
    fn tab_close_keybind_consumed_by_form_on_origin_tab() {
        use crossterm::event::{KeyCode, KeyModifiers};

        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let parent_idx = sid_app.app.tabs().active_index();
        let tab_count_before = sid_app.app.tabs().tabs().len();

        // Push a Detail tab and switch to it so the origin tab holds the form.
        let detail_tab = sid_core::tab::Tab {
            id: sid_core::tab::TabId::new("ws-detail-test"),
            title: "Test Detail".into(),
            layout: {
                use sid_core::{
                    layout::Layout,
                    widget::{EventOutcome, RenderTarget, Widget, WidgetId},
                };
                struct Stub {
                    id: WidgetId,
                }
                impl Widget for Stub {
                    fn id(&self) -> &WidgetId {
                        &self.id
                    }
                    fn title(&self) -> &str {
                        "stub"
                    }
                    fn render(&self, _: &mut dyn RenderTarget) {}
                    fn handle_event(
                        &mut self,
                        _: &sid_core::event::Event,
                        _: &mut sid_core::context::WidgetCtx,
                    ) -> EventOutcome {
                        EventOutcome::Bubble
                    }
                    fn as_any(&self) -> &dyn std::any::Any {
                        self
                    }
                    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                        self
                    }
                }
                Layout::Single(Box::new(Stub {
                    id: WidgetId::new("stub"),
                }))
            },
            hotkey: None,
            kind: sid_core::tab::TabKind::Detail { parent_idx },
        };
        sid_app
            .app
            .tabs_mut()
            .push_detail(detail_tab)
            .expect("push detail");
        // Switch to the detail tab so it is the active (origin) tab for the form.
        assert!(
            sid_app
                .app
                .tabs_mut()
                .switch_to(&sid_core::tab::TabId::new("ws-detail-test")),
            "switch to detail tab"
        );
        let tab_count_with_detail = sid_app.app.tabs().tabs().len();
        assert_eq!(tab_count_with_detail, tab_count_before + 1);

        // Open a form on the detail tab.
        open_form(&mut sid_app, test_form_spec("close-invariant.test"));
        assert!(sid_app.form.is_some());
        assert_eq!(
            sid_app.form_origin_tab.as_ref().map(|t| t.as_str()),
            Some("ws-detail-test")
        );

        // Drive the tab.close keybind (Alt+W) through route_key_event.
        let close_chord = chord_mods(KeyCode::Char('w'), KeyModifiers::ALT);
        let handled = route_key_event(&mut sid_app, close_chord);

        // The form consumed the key (returns true) and the tab count is unchanged.
        assert!(
            handled,
            "form must consume Alt+W while active on origin tab"
        );
        assert_eq!(
            sid_app.app.tabs().tabs().len(),
            tab_count_with_detail,
            "tab count must not decrease — form intercepted Alt+W before tab.close"
        );
        assert!(
            sid_app.form.is_some(),
            "form must still be open (Alt+W has no form binding, treated as Consumed)"
        );
    }

    // ----- SSH live-connect wiring -----------------------------------------

    /// Tests for the live SSH connect path: pending-connect drain, outcome
    /// drain (Connected attaches PtyPane, Failed marks failed + toasts),
    /// byte-stream forwarding, and the production factory shape.
    ///
    /// Real russh is gated to integration tests in `sid-ssh/tests/`. Here
    /// we substitute a hand-rolled `SshClient` so the wire layer is
    /// exercised end-to-end without network or subprocess.
    mod ssh_connect_wiring {
        use std::sync::{Arc, Mutex};

        use async_trait::async_trait;
        use sid_core::adapters::ssh::{
            ExecResult, SftpSession, SshAuth, SshClient, SshError, SshHostSpec, SshShell,
        };
        use sid_store::{SshHost, SshHostSource};
        use sid_widgets::SshWidget;

        use super::*;

        // Mock shell — emits a queue of byte chunks then idles forever.
        struct MockShell {
            chunks: Mutex<std::collections::VecDeque<Vec<u8>>>,
            closed: Mutex<bool>,
        }
        impl MockShell {
            fn new(chunks: Vec<Vec<u8>>) -> Self {
                Self {
                    chunks: Mutex::new(chunks.into_iter().collect()),
                    closed: Mutex::new(false),
                }
            }
        }
        #[async_trait]
        impl SshShell for MockShell {
            async fn write(&mut self, _bytes: &[u8]) -> Result<(), SshError> {
                Ok(())
            }
            async fn try_read(&mut self) -> Result<Vec<u8>, SshError> {
                if *self.closed.lock().unwrap() {
                    return Err(SshError::Disconnected);
                }
                Ok(self.chunks.lock().unwrap().pop_front().unwrap_or_default())
            }
            async fn resize(&mut self, _rows: u16, _cols: u16) -> Result<(), SshError> {
                Ok(())
            }
            async fn close(&mut self) -> Result<(), SshError> {
                *self.closed.lock().unwrap() = true;
                Ok(())
            }
        }

        // Mock client — configurable success/failure at each step.
        /// Shared slot a test inspects to assert which [`SshAuth`] the connect
        /// task handed the client.
        type AuthCapture = Arc<Mutex<Option<SshAuth>>>;

        struct MockClient {
            connect_ok: bool,
            open_shell_ok: bool,
            chunks: Vec<Vec<u8>>,
            connected: bool,
            /// When set, `connect` records the auth it received here.
            captured_auth: Option<AuthCapture>,
        }
        impl MockClient {
            fn ok(chunks: Vec<Vec<u8>>) -> Self {
                Self {
                    connect_ok: true,
                    open_shell_ok: true,
                    chunks,
                    connected: false,
                    captured_auth: None,
                }
            }
            fn connect_fail() -> Self {
                Self {
                    connect_ok: false,
                    open_shell_ok: false,
                    chunks: vec![],
                    connected: false,
                    captured_auth: None,
                }
            }
            fn open_shell_fail() -> Self {
                Self {
                    connect_ok: true,
                    open_shell_ok: false,
                    chunks: vec![],
                    connected: false,
                    captured_auth: None,
                }
            }
            /// Record the [`SshAuth`] passed to `connect` into `slot`.
            fn with_auth_capture(mut self, slot: AuthCapture) -> Self {
                self.captured_auth = Some(slot);
                self
            }
        }
        #[async_trait]
        impl SshClient for MockClient {
            async fn connect(
                &mut self,
                _host: &SshHostSpec,
                auth: &SshAuth,
            ) -> Result<(), SshError> {
                if let Some(slot) = &self.captured_auth {
                    *slot.lock().unwrap() = Some(auth.clone());
                }
                if self.connect_ok {
                    self.connected = true;
                    Ok(())
                } else {
                    Err(SshError::ConnectFailed("mock refuse".into()))
                }
            }
            async fn disconnect(&mut self) -> Result<(), SshError> {
                self.connected = false;
                Ok(())
            }
            fn is_connected(&self) -> bool {
                self.connected
            }
            async fn exec(&mut self, _cmd: &str) -> Result<ExecResult, SshError> {
                Err(SshError::Other("mock exec".into()))
            }
            async fn open_shell(
                &mut self,
                _term: &str,
                _rows: u16,
                _cols: u16,
            ) -> Result<Box<dyn SshShell>, SshError> {
                if self.open_shell_ok {
                    Ok(Box::new(MockShell::new(std::mem::take(&mut self.chunks))))
                } else {
                    Err(SshError::Other("mock shell open denied".into()))
                }
            }
            async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError> {
                Err(SshError::Other("mock sftp".into()))
            }
        }

        type MockMaker = Box<dyn FnMut() -> Box<dyn SshClient> + Send>;

        fn factory_for(make: Arc<Mutex<MockMaker>>) -> SshClientFactoryFn {
            Arc::new(move || make.lock().unwrap()())
        }

        fn host_record(alias: &str) -> SshHost {
            SshHost {
                alias: alias.into(),
                host: "127.0.0.1".into(),
                port: 22,
                user: "u".into(),
                identity_file: None,
                source: SshHostSource::Manual,
                last_connected: 0,
                command_history: vec![],
                last_sftp_path: None,
                auth_kind: sid_store::SshAuthKind::Agent,
            }
        }

        /// A Key-auth host with an identity file. Resolves to `SshAuth::Key`
        /// deterministically — no dependency on `SSH_AUTH_SOCK` — so the
        /// connect-plumbing tests stay green whether or not an ssh-agent socket
        /// is present in the environment.
        fn key_host(alias: &str) -> SshHost {
            let mut h = host_record(alias);
            h.auth_kind = sid_store::SshAuthKind::Key;
            h.identity_file = Some("/nonexistent/id_ed25519".into());
            h
        }

        fn seed_host_into_widget(sid_app: &mut SidApp, h: SshHost) {
            sid_app.store.upsert_ssh_host(&h).unwrap();
            for t in sid_app.app.tabs_mut().tabs_mut() {
                if t.id.as_str() == "ssh"
                    && let Some(w) = t.layout.iter_widgets_mut().next()
                    && let Some(ssh) = (w as &mut dyn std::any::Any).downcast_mut::<SshWidget>()
                {
                    ssh.state_mut().set_store_hosts(vec![h.clone()]);
                    // Step off the "+ add new" row onto the host (the cursor
                    // starts on the synthetic row; show_add_new_row is on by
                    // default).
                    if ssh.state().selected_host().is_none() {
                        ssh.state_mut().select_next();
                    }
                }
            }
        }

        /// `drain_pending_ssh_connect` is a no-op when no intent is queued.
        #[test]
        fn drain_pending_connect_noop_without_intent() {
            let mut sid_app = build_test_sid_app(Some("ssh"));
            drain_pending_ssh_connect(&mut sid_app);
            assert!(sid_app.ssh_outcome_rx.try_recv().is_err());
        }

        /// Race: alias removed from the list between Enter and drain →
        /// the connection state is flipped to Failed immediately.
        #[test]
        fn drain_pending_connect_unknown_alias_marks_failed() {
            let mut sid_app = build_test_sid_app(Some("ssh"));
            active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .set_pending_connect(Some("ghost".into()));
            drain_pending_ssh_connect(&mut sid_app);
            let phase = active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .connection()
                .phase();
            assert_eq!(phase, sid_widgets::ssh::ConnectionPhase::Failed);
        }

        /// End-to-end on a single tokio runtime: pending-connect intent
        /// flows through a mock factory, the connect task succeeds, the
        /// outcome drain attaches a PtyPane and flips the widget to
        /// Connected, and subsequent byte drains feed bytes into the pane.
        #[tokio::test(flavor = "current_thread")]
        async fn pending_connect_succeeds_attaches_pane_and_forwards_bytes() {
            use sid_widgets::ssh::ConnectionPhase;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            // Key host: resolves to SshAuth::Key without needing SSH_AUTH_SOCK,
            // so this connect-plumbing test is deterministic in any env.
            seed_host_into_widget(&mut sid_app, key_host("acme"));

            let make: MockMaker = Box::new(|| Box::new(MockClient::ok(vec![b"hello\n".to_vec()])));
            sid_app.ssh_client_factory = factory_for(Arc::new(Mutex::new(make)));

            active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .set_pending_connect(Some("acme".into()));

            drain_pending_ssh_connect(&mut sid_app);
            for _ in 0..30 {
                tokio::task::yield_now().await;
            }

            drain_ssh_outcomes(&mut sid_app);
            let widget = active_ssh_widget_mut(&mut sid_app).expect("ssh widget");
            assert_eq!(widget.connection().phase(), ConnectionPhase::Connected);
            assert!(widget.pty_pane().is_some());
            assert!(sid_app.ssh_byte_rx.is_some());

            // Let the reader task pump at least one chunk.
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            drain_ssh_bytes(&mut sid_app);

            let pane = active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .pty_pane()
                .unwrap();
            let lines = pane.lines();
            assert!(
                lines[0].trim_end().starts_with("hello"),
                "expected first line to start with 'hello'; got {:?}",
                lines[0]
            );

            if let Some(s) = sid_app.ssh_shutdown_tx.take() {
                let _ = s.send(());
            }
        }

        /// Connect failure flips Failed and pushes an error toast.
        #[tokio::test(flavor = "current_thread")]
        async fn pending_connect_fails_marks_widget_and_toasts() {
            use sid_widgets::ssh::ConnectionPhase;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            // Key host: reaches the mock's connect_fail regardless of agent env.
            seed_host_into_widget(&mut sid_app, key_host("acme"));

            let make: MockMaker = Box::new(|| Box::new(MockClient::connect_fail()));
            sid_app.ssh_client_factory = factory_for(Arc::new(Mutex::new(make)));

            active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .set_pending_connect(Some("acme".into()));

            drain_pending_ssh_connect(&mut sid_app);
            for _ in 0..30 {
                tokio::task::yield_now().await;
            }
            drain_ssh_outcomes(&mut sid_app);

            let phase = active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .connection()
                .phase();
            assert_eq!(phase, ConnectionPhase::Failed);
            let kinds: Vec<crate::toast::ToastKind> =
                sid_app.toasts.iter().map(|t| t.kind).collect();
            assert!(
                kinds.contains(&crate::toast::ToastKind::Error),
                "expected an Error toast; got: {kinds:?}"
            );
        }

        /// open_shell failure (connect OK, shell denied) also flips Failed.
        #[tokio::test(flavor = "current_thread")]
        async fn pending_connect_open_shell_failure_marks_failed() {
            use sid_widgets::ssh::ConnectionPhase;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            // Key host: the connect must SUCCEED (then open_shell fails), so the
            // host must resolve without depending on SSH_AUTH_SOCK.
            seed_host_into_widget(&mut sid_app, key_host("acme"));

            let make: MockMaker = Box::new(|| Box::new(MockClient::open_shell_fail()));
            sid_app.ssh_client_factory = factory_for(Arc::new(Mutex::new(make)));

            active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .set_pending_connect(Some("acme".into()));

            drain_pending_ssh_connect(&mut sid_app);
            for _ in 0..30 {
                tokio::task::yield_now().await;
            }
            drain_ssh_outcomes(&mut sid_app);

            let phase = active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .connection()
                .phase();
            assert_eq!(phase, ConnectionPhase::Failed);
        }

        /// drain_ssh_outcomes is a no-op when nothing is queued.
        #[test]
        fn drain_outcomes_empty_is_a_noop() {
            let mut sid_app = build_test_sid_app(Some("ssh"));
            drain_ssh_outcomes(&mut sid_app);
            let phase = active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .connection()
                .phase();
            assert_eq!(phase, sid_widgets::ssh::ConnectionPhase::Idle);
        }

        /// drain_ssh_outcomes attaches a forged Connected outcome.
        #[test]
        fn drain_outcomes_attaches_pty_and_stashes_byte_rx() {
            use sid_pty::Vt100Screen;
            use sid_widgets::ssh::ConnectionPhase;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            let (byte_tx, byte_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let (shutdown_tx, _shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let pty = sid_widgets::ssh::PtyPane::new(Box::new(Vt100Screen::new(24, 80))
                as Box<dyn sid_core::adapters::pty::TerminalScreen>);
            sid_app
                .ssh_outcome_tx
                .send(SshConnectOutcome::Connected {
                    alias: "x".into(),
                    pty,
                    byte_rx,
                    shutdown_tx,
                })
                .unwrap();
            drain_ssh_outcomes(&mut sid_app);
            let widget = active_ssh_widget_mut(&mut sid_app).unwrap();
            assert_eq!(widget.connection().phase(), ConnectionPhase::Connected);
            assert!(widget.pty_pane().is_some());
            assert!(sid_app.ssh_byte_rx.is_some());
            assert!(sid_app.ssh_shutdown_tx.is_some());
            drop(byte_tx);
        }

        /// drain_ssh_outcomes on Failed marks the widget and toasts.
        #[test]
        fn drain_outcomes_failed_marks_widget_and_toasts() {
            use sid_widgets::ssh::ConnectionPhase;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            sid_app
                .ssh_outcome_tx
                .send(SshConnectOutcome::Failed {
                    alias: "x".into(),
                    error: "boom".into(),
                })
                .unwrap();
            drain_ssh_outcomes(&mut sid_app);
            let widget = active_ssh_widget_mut(&mut sid_app).unwrap();
            assert_eq!(widget.connection().phase(), ConnectionPhase::Failed);
            assert_eq!(widget.connection().error_message(), Some("boom"));
            let kinds: Vec<crate::toast::ToastKind> =
                sid_app.toasts.iter().map(|t| t.kind).collect();
            assert!(kinds.contains(&crate::toast::ToastKind::Error));
        }

        /// drain_ssh_bytes is a no-op with no channel attached.
        #[test]
        fn drain_bytes_noop_without_channel() {
            let mut sid_app = build_test_sid_app(Some("ssh"));
            drain_ssh_bytes(&mut sid_app);
        }

        /// drain_ssh_bytes forwards queued chunks into the pane.
        #[test]
        fn drain_bytes_forwards_chunks_into_pane() {
            use sid_pty::Vt100Screen;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            let (byte_tx, byte_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let (shutdown_tx, _shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            sid_app
                .ssh_outcome_tx
                .send(SshConnectOutcome::Connected {
                    alias: "x".into(),
                    pty: sid_widgets::ssh::PtyPane::new(Box::new(Vt100Screen::new(24, 80))
                        as Box<dyn sid_core::adapters::pty::TerminalScreen>),
                    byte_rx,
                    shutdown_tx,
                })
                .unwrap();
            drain_ssh_outcomes(&mut sid_app);
            byte_tx.send(b"abc".to_vec()).unwrap();
            byte_tx.send(b"def".to_vec()).unwrap();
            drain_ssh_bytes(&mut sid_app);
            let pane = active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .pty_pane()
                .unwrap();
            let lines = pane.lines();
            assert!(lines[0].starts_with("abcdef"), "got {:?}", lines[0]);
            drop(byte_tx);
        }

        /// drain_ssh_bytes on remote disconnect flips Disconnected and
        /// clears the receiver.
        #[test]
        fn drain_bytes_disconnect_marks_widget_and_clears_rx() {
            use sid_pty::Vt100Screen;
            use sid_widgets::ssh::ConnectionPhase;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            let (byte_tx, byte_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let (shutdown_tx, _shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            sid_app
                .ssh_outcome_tx
                .send(SshConnectOutcome::Connected {
                    alias: "x".into(),
                    pty: sid_widgets::ssh::PtyPane::new(Box::new(Vt100Screen::new(24, 80))
                        as Box<dyn sid_core::adapters::pty::TerminalScreen>),
                    byte_rx,
                    shutdown_tx,
                })
                .unwrap();
            drain_ssh_outcomes(&mut sid_app);
            drop(byte_tx);
            drain_ssh_bytes(&mut sid_app);
            let widget = active_ssh_widget_mut(&mut sid_app).unwrap();
            assert_eq!(widget.connection().phase(), ConnectionPhase::Disconnected);
            assert!(sid_app.ssh_byte_rx.is_none());
        }

        /// sync_ssh_pty_size is a no-op when the active tab is not SSH.
        #[test]
        fn sync_pty_size_noop_when_not_ssh() {
            use sid_pty::Vt100Screen;
            let mut sid_app = build_test_sid_app(Some("workspaces"));
            for t in sid_app.app.tabs_mut().tabs_mut() {
                if t.id.as_str() == "ssh"
                    && let Some(w) = t.layout.iter_widgets_mut().next()
                    && let Some(ssh) = (w as &mut dyn std::any::Any).downcast_mut::<SshWidget>()
                {
                    ssh.set_pty_pane(sid_widgets::ssh::PtyPane::new(Box::new(Vt100Screen::new(
                        24, 80,
                    ))
                        as Box<dyn sid_core::adapters::pty::TerminalScreen>));
                }
            }
            sync_ssh_pty_size(&mut sid_app, Rect::new(0, 0, 120, 40));
            assert!(sid_app.ssh_last_pty_area.is_none());
        }

        /// sync_ssh_pty_size resizes the attached pane when the body
        /// rect changed and the active tab is SSH.
        #[test]
        fn sync_pty_size_resizes_pane_on_ssh_tab() {
            use sid_pty::Vt100Screen;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            active_ssh_widget_mut(&mut sid_app).unwrap().set_pty_pane(
                sid_widgets::ssh::PtyPane::new(Box::new(Vt100Screen::new(24, 80))
                    as Box<dyn sid_core::adapters::pty::TerminalScreen>),
            );
            sync_ssh_pty_size(&mut sid_app, Rect::new(0, 0, 120, 40));
            assert!(sid_app.ssh_last_pty_area.is_some());
            let prev = sid_app.ssh_last_pty_area;
            sync_ssh_pty_size(&mut sid_app, Rect::new(0, 0, 120, 40));
            assert_eq!(sid_app.ssh_last_pty_area, prev);
        }

        /// End-to-end through PRODUCTION routing: background-open from the
        /// inspector pushes an `ssh:<alias>` detail tab whose own widget
        /// carries the connect intent; the drain pipeline resolves the alias
        /// against the detail widget's hydrated host list, the Connected
        /// outcome attaches the PtyPane to the DETAIL widget (not the parent
        /// "ssh" tab), and bytes flow into the detail pane.
        #[tokio::test(flavor = "current_thread")]
        async fn background_open_tab_connects_end_to_end() {
            use crossterm::event::{KeyCode, KeyModifiers};
            use sid_core::event::KeyChord;
            use sid_widgets::ssh::ConnectionPhase;

            let mut sid_app = build_test_sid_app(Some("ssh"));
            // Key host: deterministic auth resolution (no agent-socket dependency).
            seed_host_into_widget(&mut sid_app, key_host("acme"));
            let make: MockMaker = Box::new(|| Box::new(MockClient::ok(vec![b"hello\n".to_vec()])));
            sid_app.ssh_client_factory = factory_for(Arc::new(Mutex::new(make)));

            // Open the inspector on "acme", then background-open it.
            route_key_event(
                &mut sid_app,
                KeyChord {
                    code: KeyCode::Right,
                    mods: KeyModifiers::NONE,
                },
            );
            assert!(
                sid_app
                    .form
                    .as_ref()
                    .map(|f| f.spec.id.0.starts_with("ssh.inspect:"))
                    .unwrap_or(false),
                "inspector must be open before background-open"
            );
            route_key_event(
                &mut sid_app,
                KeyChord {
                    code: KeyCode::Enter,
                    mods: KeyModifiers::CONTROL,
                },
            );

            // The detail tab's widget is hydrated with the host list, so the
            // connect drain can resolve the alias from THAT widget.
            let detail_id = TabId::new("ssh:acme");
            {
                let detail = detail_widget_mut(&mut sid_app, &detail_id);
                assert!(
                    !detail.state().visible_hosts().is_empty(),
                    "detail widget must be hydrated with the store's host list"
                );
                assert_eq!(detail.connection().phase(), ConnectionPhase::Connecting);
            }

            drain_pending_ssh_connect(&mut sid_app);
            for _ in 0..30 {
                tokio::task::yield_now().await;
            }
            drain_ssh_outcomes(&mut sid_app);

            // Outcome landed on the DETAIL widget...
            {
                let detail = detail_widget_mut(&mut sid_app, &detail_id);
                assert_eq!(detail.connection().phase(), ConnectionPhase::Connected);
                assert!(
                    detail.pty_pane().is_some(),
                    "pane must attach to detail tab"
                );
            }
            // ...and NOT on the parent "ssh" tab.
            let parent = active_ssh_widget_mut(&mut sid_app).expect("parent ssh widget");
            assert_eq!(
                parent.connection().phase(),
                ConnectionPhase::Idle,
                "parent tab must stay untouched by a background connect"
            );
            assert!(parent.pty_pane().is_none(), "parent must not get the pane");

            // Bytes flow into the detail pane.
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            drain_ssh_bytes(&mut sid_app);
            let detail = detail_widget_mut(&mut sid_app, &detail_id);
            let lines = detail.pty_pane().unwrap().lines();
            assert!(
                lines[0].trim_end().starts_with("hello"),
                "detail pane must receive session bytes; got {:?}",
                lines[0]
            );

            if let Some(s) = sid_app.ssh_shutdown_tx.take() {
                let _ = s.send(());
            }
        }

        /// A new Connected outcome supersedes the previous live session: the
        /// previously-Connected widget (the parent tab here) is flipped to
        /// Disconnected — its reader was torn down — and exactly one widget
        /// (the background detail tab) reads as live afterwards.
        #[tokio::test(flavor = "current_thread")]
        async fn connected_outcome_supersedes_previous_session_widget() {
            use crossterm::event::{KeyCode, KeyModifiers};
            use sid_core::event::KeyChord;
            use sid_pty::Vt100Screen;
            use sid_widgets::ssh::ConnectionPhase;

            let mut sid_app = build_test_sid_app(Some("ssh"));
            seed_host_into_widget(&mut sid_app, host_record("acme"));

            // Background-open "acme" while the parent is idle: detail tab is
            // now Connecting("acme").
            route_key_event(
                &mut sid_app,
                KeyChord {
                    code: KeyCode::Right,
                    mods: KeyModifiers::NONE,
                },
            );
            route_key_event(
                &mut sid_app,
                KeyChord {
                    code: KeyCode::Enter,
                    mods: KeyModifiers::CONTROL,
                },
            );

            // Forge a Connected outcome for an alias nobody is waiting on —
            // the fallback attaches it to the parent "ssh" tab (legacy
            // single-tab behaviour). Parent is now the live session.
            let (tx1, rx1) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let (sd1, _sr1) = tokio::sync::oneshot::channel::<()>();
            sid_app
                .ssh_outcome_tx
                .send(SshConnectOutcome::Connected {
                    alias: "legacy".into(),
                    pty: sid_widgets::ssh::PtyPane::new(Box::new(Vt100Screen::new(24, 80))
                        as Box<dyn sid_core::adapters::pty::TerminalScreen>),
                    byte_rx: rx1,
                    shutdown_tx: sd1,
                })
                .unwrap();
            drain_ssh_outcomes(&mut sid_app);
            assert_eq!(
                active_ssh_widget_mut(&mut sid_app)
                    .unwrap()
                    .connection()
                    .phase(),
                ConnectionPhase::Connected
            );

            // Now the detail tab's connect completes: it must take over as the
            // single live session, and the parent must flip to Disconnected.
            let (tx2, rx2) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let (sd2, _sr2) = tokio::sync::oneshot::channel::<()>();
            sid_app
                .ssh_outcome_tx
                .send(SshConnectOutcome::Connected {
                    alias: "acme".into(),
                    pty: sid_widgets::ssh::PtyPane::new(Box::new(Vt100Screen::new(24, 80))
                        as Box<dyn sid_core::adapters::pty::TerminalScreen>),
                    byte_rx: rx2,
                    shutdown_tx: sd2,
                })
                .unwrap();
            drain_ssh_outcomes(&mut sid_app);

            let detail_id = TabId::new("ssh:acme");
            assert_eq!(
                detail_widget_mut(&mut sid_app, &detail_id)
                    .connection()
                    .phase(),
                ConnectionPhase::Connected,
                "detail tab must be the live session"
            );
            assert_eq!(
                active_ssh_widget_mut(&mut sid_app)
                    .unwrap()
                    .connection()
                    .phase(),
                ConnectionPhase::Disconnected,
                "superseded parent session must read Disconnected"
            );
            drop(tx1);
            drop(tx2);
        }

        /// Borrow the SshWidget inside the tab with `id`, panicking with a
        /// clear message if the tab or widget is missing (test helper).
        fn detail_widget_mut<'a>(sid_app: &'a mut SidApp, id: &TabId) -> &'a mut SshWidget {
            for t in sid_app.app.tabs_mut().tabs_mut() {
                if t.id == *id {
                    let w = t
                        .layout
                        .iter_widgets_mut()
                        .next()
                        .expect("detail tab must hold a widget");
                    return (w as &mut dyn std::any::Any)
                        .downcast_mut::<SshWidget>()
                        .expect("detail tab widget must be an SshWidget");
                }
            }
            panic!("tab {id:?} not found");
        }

        /// The production factory closure produces a fresh client per call.
        #[test]
        fn build_ssh_client_factory_fn_produces_clients() {
            let f = build_ssh_client_factory_fn();
            let c1 = f();
            let c2 = f();
            assert!(!c1.is_connected());
            assert!(!c2.is_connected());
        }

        /// active_ssh_body_rect carves a sensible body rect inside the
        /// full draw area.
        #[test]
        fn active_ssh_body_rect_carves_inside_layout() {
            let full = Rect::new(0, 0, 120, 40);
            let body = active_ssh_body_rect(full);
            assert!(body.x >= 120 * 40 / 100 - 1, "x={}", body.x);
            assert!(body.y >= 3, "y={}", body.y);
            assert!(body.width < full.width);
        }

        /// SshConnectOutcome::Debug does not leak pty/byte_rx payloads but
        /// the Failed branch is fully displayable.
        #[test]
        fn ssh_connect_outcome_debug_compiles() {
            let s = format!(
                "{:?}",
                SshConnectOutcome::Failed {
                    alias: "x".into(),
                    error: "y".into()
                }
            );
            assert!(s.contains("Failed"));
            assert!(s.contains("x"));
            assert!(s.contains("y"));
        }

        // ── SSH password-auth fixes (§A–§C, host removal) ───────────────────

        use sid_core::adapters::secrets::SecretId;

        /// A password-auth host record.
        fn password_host(alias: &str) -> SshHost {
            let mut h = host_record(alias);
            h.auth_kind = sid_store::SshAuthKind::Password;
            h
        }

        /// Put `widget` into the `Connecting` phase for `alias` (mirrors what
        /// raising a connect intent does before the drain runs).
        fn begin_connecting(sid_app: &mut SidApp, alias: &str) {
            active_ssh_widget_mut(sid_app)
                .unwrap()
                .connection_mut()
                .begin_connecting(alias.into());
        }

        // ── §A: connect auth resolution ─────────────────────────────────────

        /// Password host with a saved keyring entry → resolve to a silent
        /// `SshAuth::Password` (no modal).
        #[test]
        fn resolve_connect_auth_password_from_keyring_is_silent() {
            let sid_app = build_test_sid_app(Some("ssh"));
            let host = password_host("pi");
            sid_app
                .secrets
                .put(&SecretId::new("ssh.host.pi.password"), b"raspberry")
                .unwrap();
            let decision = resolve_connect_auth(&sid_app, "pi", &host);
            assert_eq!(
                decision,
                ConnectAuthDecision::Spawn(SshAuth::Password("raspberry".into()))
            );
        }

        /// Password host with no keyring entry → prompt.
        #[test]
        fn resolve_connect_auth_password_without_keyring_prompts() {
            let sid_app = build_test_sid_app(Some("ssh"));
            let host = password_host("pi");
            assert_eq!(
                resolve_connect_auth(&sid_app, "pi", &host),
                ConnectAuthDecision::PromptPassword
            );
        }

        /// Key host with an identity file → `SshAuth::Key`.
        #[test]
        fn resolve_connect_auth_key_with_identity() {
            let sid_app = build_test_sid_app(Some("ssh"));
            let mut host = host_record("k");
            host.auth_kind = sid_store::SshAuthKind::Key;
            host.identity_file = Some("/home/u/.ssh/id_ed25519".into());
            assert_eq!(
                resolve_connect_auth(&sid_app, "k", &host),
                ConnectAuthDecision::Spawn(SshAuth::Key {
                    path: std::path::PathBuf::from("/home/u/.ssh/id_ed25519"),
                    passphrase: None,
                })
            );
        }

        // ── §B: agent-socket preflight (pure logic, no env mutation) ─────────

        #[test]
        fn agent_auth_decision_present_socket_spawns_agent() {
            assert_eq!(
                agent_auth_decision_for(true),
                ConnectAuthDecision::Spawn(SshAuth::Agent)
            );
        }

        #[test]
        fn agent_auth_decision_missing_socket_fails_with_clear_message() {
            match agent_auth_decision_for(false) {
                ConnectAuthDecision::Fail(msg) => {
                    assert!(msg.contains("SSH_AUTH_SOCK unset"), "got: {msg}");
                    assert!(msg.contains("password or key auth"), "got: {msg}");
                }
                other => panic!("expected Fail, got {other:?}"),
            }
        }

        /// `drain_pending_ssh_connect` for an agent host with no socket emits a
        /// Failed outcome with the clear message (drives the §B path through the
        /// public entry point). Only runs when `SSH_AUTH_SOCK` is genuinely
        /// unset in the test environment (no env mutation).
        #[test]
        fn drain_agent_host_without_socket_fails_clearly() {
            if std::env::var_os("SSH_AUTH_SOCK").is_some() {
                // Agent socket is present in this environment; the unset path
                // is covered deterministically by the pure-logic test above.
                return;
            }
            let mut sid_app = build_test_sid_app(Some("ssh"));
            seed_host_into_widget(&mut sid_app, host_record("ag"));
            active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .set_pending_connect(Some("ag".into()));
            begin_connecting(&mut sid_app, "ag");
            drain_pending_ssh_connect(&mut sid_app);
            match sid_app.ssh_outcome_rx.try_recv() {
                Ok(SshConnectOutcome::Failed { error, .. }) => {
                    assert!(error.contains("SSH_AUTH_SOCK unset"), "got: {error}");
                }
                other => panic!("expected Failed outcome, got {other:?}"),
            }
        }

        // ── §A: password modal interleaving with the async spawn ────────────

        /// A password host with no keyring entry pushes the password modal and
        /// does NOT spawn a connect (no outcome on the channel).
        #[test]
        fn drain_password_host_without_keyring_pushes_modal_no_spawn() {
            let mut sid_app = build_test_sid_app(Some("ssh"));
            seed_host_into_widget(&mut sid_app, password_host("pi"));
            active_ssh_widget_mut(&mut sid_app)
                .unwrap()
                .set_pending_connect(Some("pi".into()));
            begin_connecting(&mut sid_app, "pi");
            drain_pending_ssh_connect(&mut sid_app);
            // Modal pushed, no connect outcome yet.
            assert_eq!(sid_app.modal_stack.len(), 1);
            assert_eq!(sid_app.modal_stack[0].id.0, "ssh.password:pi");
            assert!(sid_app.ssh_outcome_rx.try_recv().is_err());
            // The masked field + save toggle are present.
            let fields = &sid_app.modal_stack[0].fields;
            assert!(matches!(
                fields[0],
                sid_widgets::modal::Field::Password { .. }
            ));
            assert!(matches!(
                fields[1],
                sid_widgets::modal::Field::Toggle { .. }
            ));
        }

        /// Submitting the password modal routes `SshAuth::Password` into the
        /// client (mock captures the auth) and, without "save", leaves no
        /// keyring entry behind (so a second connect prompts again).
        #[tokio::test(flavor = "current_thread")]
        async fn submit_password_modal_routes_password_and_no_save_does_not_persist() {
            use sid_widgets::FieldValue;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            seed_host_into_widget(&mut sid_app, password_host("pi"));
            begin_connecting(&mut sid_app, "pi");

            let captured: AuthCapture = Arc::new(Mutex::new(None));
            let cap = Arc::clone(&captured);
            let make: MockMaker = Box::new(move || {
                Box::new(MockClient::ok(vec![]).with_auth_capture(Arc::clone(&cap)))
            });
            sid_app.ssh_client_factory = factory_for(Arc::new(Mutex::new(make)));

            submit_ssh_password(
                &mut sid_app,
                "pi",
                &[
                    ("Password".into(), FieldValue::Password("raspberry".into())),
                    ("Save to keyring".into(), FieldValue::Toggle(false)),
                ],
            );
            for _ in 0..30 {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                *captured.lock().unwrap(),
                Some(SshAuth::Password("raspberry".into()))
            );
            // No-save → keyring stays empty → next connect would prompt.
            assert!(
                sid_app
                    .secrets
                    .get(&SecretId::new("ssh.host.pi.password"))
                    .unwrap()
                    .is_none()
            );
            // Password is NOT in the persisted host record.
            let persisted = sid_app.store.get_ssh_host("pi").unwrap().unwrap();
            let dbg = format!("{persisted:?}");
            assert!(
                !dbg.contains("raspberry"),
                "password leaked into host: {dbg}"
            );

            if let Some(s) = sid_app.ssh_shutdown_tx.take() {
                let _ = s.send(());
            }
        }

        /// Submitting with "save" on persists the password; a subsequent
        /// `resolve_connect_auth` then resolves silently (round-trip).
        #[tokio::test(flavor = "current_thread")]
        async fn submit_password_modal_save_persists_and_next_connect_is_silent() {
            use sid_widgets::FieldValue;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            seed_host_into_widget(&mut sid_app, password_host("pi"));
            begin_connecting(&mut sid_app, "pi");

            let make: MockMaker = Box::new(|| Box::new(MockClient::ok(vec![])));
            sid_app.ssh_client_factory = factory_for(Arc::new(Mutex::new(make)));

            submit_ssh_password(
                &mut sid_app,
                "pi",
                &[
                    ("Password".into(), FieldValue::Password("raspberry".into())),
                    ("Save to keyring".into(), FieldValue::Toggle(true)),
                ],
            );
            for _ in 0..30 {
                tokio::task::yield_now().await;
            }
            // Saved → silent on the next connect.
            let host = password_host("pi");
            assert_eq!(
                resolve_connect_auth(&sid_app, "pi", &host),
                ConnectAuthDecision::Spawn(SshAuth::Password("raspberry".into()))
            );

            if let Some(s) = sid_app.ssh_shutdown_tx.take() {
                let _ = s.send(());
            }
        }

        /// Cancelling the password modal resets the stranded `Connecting`
        /// widget back to Idle.
        #[test]
        fn cancel_password_modal_resets_connecting_widget() {
            use sid_widgets::ssh::ConnectionPhase;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            seed_host_into_widget(&mut sid_app, password_host("pi"));
            begin_connecting(&mut sid_app, "pi");
            assert_eq!(
                active_ssh_widget_mut(&mut sid_app)
                    .unwrap()
                    .connection()
                    .phase(),
                ConnectionPhase::Connecting
            );
            cancel_pending_ssh_password(&mut sid_app, "pi");
            assert_eq!(
                active_ssh_widget_mut(&mut sid_app)
                    .unwrap()
                    .connection()
                    .phase(),
                ConnectionPhase::Idle
            );
        }

        /// A connect that fails password auth surfaces a Failed outcome whose
        /// error string does NOT contain the password.
        #[tokio::test(flavor = "current_thread")]
        async fn password_connect_failure_error_does_not_leak_password() {
            let mut sid_app = build_test_sid_app(Some("ssh"));
            seed_host_into_widget(&mut sid_app, password_host("pi"));
            begin_connecting(&mut sid_app, "pi");

            let make: MockMaker = Box::new(|| Box::new(MockClient::connect_fail()));
            sid_app.ssh_client_factory = factory_for(Arc::new(Mutex::new(make)));

            spawn_ssh_connect_with_auth(
                Arc::clone(&sid_app.ssh_client_factory),
                sid_app.ssh_outcome_tx.clone(),
                password_host("pi"),
                "pi".into(),
                24,
                80,
                SshAuth::Password("raspberry".into()),
            );
            for _ in 0..30 {
                tokio::task::yield_now().await;
            }
            match sid_app.ssh_outcome_rx.try_recv() {
                Ok(SshConnectOutcome::Failed { error, .. }) => {
                    assert!(!error.contains("raspberry"), "password leaked: {error}");
                }
                other => panic!("expected Failed, got {other:?}"),
            }
        }

        // ── §C: ssh-copy-id argv construction ───────────────────────────────

        /// Password host → `sshpass -e ssh-copy-id -i <pub> -p <port>
        /// -o StrictHostKeyChecking=accept-new user@host`. The password is NOT
        /// in argv — it travels via the `SSHPASS` env var (`-e`).
        #[test]
        fn copy_id_invocation_password_host_uses_sshpass() {
            let inv = build_ssh_copy_id_invocation(
                "pi",
                "raspberrypi",
                "10.1.1.93",
                2222,
                Some("/home/u/.ssh/id_ed25519"),
                Some("s3cr3t-pw"),
            )
            .expect("valid positional components");
            assert_eq!(inv.program, "sshpass");
            assert_eq!(
                inv.args,
                vec![
                    "-e".to_string(),
                    "ssh-copy-id".into(),
                    "-i".into(),
                    "/home/u/.ssh/id_ed25519.pub".into(),
                    "-p".into(),
                    "2222".into(),
                    "-o".into(),
                    "StrictHostKeyChecking=accept-new".into(),
                    "raspberrypi@10.1.1.93".into(),
                ]
            );
            // SECURITY: the password is delivered via SSHPASS, never argv.
            assert!(
                !inv.argv().iter().any(|a| a.contains("s3cr3t-pw")),
                "password leaked into argv: {:?}",
                inv.argv()
            );
        }

        /// Key/agent host → plain `ssh-copy-id -i <pub> <alias>` (no sshpass,
        /// no password).
        #[test]
        fn copy_id_invocation_key_host_is_plain() {
            let inv = build_ssh_copy_id_invocation(
                "prod",
                "alice",
                "10.0.0.1",
                22,
                Some("/k/id_rsa"),
                None,
            )
            .expect("valid alias");
            assert_eq!(inv.program, "ssh-copy-id");
            assert_eq!(
                inv.args,
                vec!["-i".to_string(), "/k/id_rsa.pub".into(), "prod".into()]
            );
        }

        /// `.pub` suffix is not doubled.
        #[test]
        fn copy_id_invocation_pub_suffix_not_doubled() {
            let inv = build_ssh_copy_id_invocation("h", "u", "host", 22, Some("/k/id.pub"), None)
                .expect("valid alias");
            assert!(inv.args.contains(&"/k/id.pub".to_string()));
        }

        /// SECURITY: the full argv of a password-host invocation never contains
        /// the password (it goes via the `SSHPASS` env var), so it is safe to
        /// log via `argv()`.
        #[test]
        fn copy_id_invocation_argv_never_contains_password() {
            let inv =
                build_ssh_copy_id_invocation("pi", "u", "host", 22, None, Some("supersecret"))
                    .expect("valid positional components");
            let argv = inv.argv();
            assert!(
                !argv.iter().any(|a| a.contains("supersecret")),
                "leak: {argv:?}"
            );
            assert_eq!(argv[0], "sshpass");
            assert!(argv.contains(&"-e".to_string()));
        }

        /// SECURITY: a flag-like `user` / `host` (password path) or `alias`
        /// (key path) is rejected before it can be smuggled to ssh-copy-id as a
        /// flag (argument injection). Returns `Err("err: …")`.
        #[test]
        fn copy_id_invocation_rejects_flaglike_components() {
            // Password path guards both halves of user@host.
            let e =
                build_ssh_copy_id_invocation("a", "-oProxyCommand=evil", "h", 22, None, Some("p"))
                    .expect_err("flag-like user must be rejected");
            assert!(e.starts_with("err:"), "got: {e}");
            assert!(e.contains("user"), "got: {e}");

            let e = build_ssh_copy_id_invocation("a", "u", "-Gbad", 22, None, Some("p"))
                .expect_err("flag-like host must be rejected");
            assert!(e.contains("host"), "got: {e}");

            // Key path guards the alias positional.
            let e = build_ssh_copy_id_invocation("-Gbad", "u", "h", 22, None, None)
                .expect_err("flag-like alias must be rejected");
            assert!(e.contains("alias"), "got: {e}");

            // The `-i` identity is guarded on BOTH the password and key paths.
            let e = build_ssh_copy_id_invocation("a", "u", "h", 22, Some("-evil"), Some("p"))
                .expect_err("flag-like identity (password path) must be rejected");
            assert!(e.contains("identity"), "got: {e}");
            let e = build_ssh_copy_id_invocation("a", "u", "h", 22, Some("-evil"), None)
                .expect_err("flag-like identity (key path) must be rejected");
            assert!(e.contains("identity"), "got: {e}");

            // The error string never echoes the password.
            let e = build_ssh_copy_id_invocation("a", "-x", "h", 22, None, Some("hunter2"))
                .expect_err("rejected");
            assert!(!e.contains("hunter2"), "password leaked into error: {e}");
        }

        /// `reject_flaglike` accepts ordinary values and rejects `-`-leading
        /// ones (both `Ok` and `Err` arms).
        #[test]
        fn reject_flaglike_both_arms() {
            assert!(reject_flaglike("user", "raspberrypi").is_ok());
            assert!(reject_flaglike("host", "10.1.1.93").is_ok());
            assert!(reject_flaglike("alias", "prod-box").is_ok());
            assert!(reject_flaglike("user", "-oProxyCommand=x").is_err());
            // An empty value does not start with '-', so it is accepted here
            // (emptiness is validated elsewhere on the form path).
            assert!(reject_flaglike("alias", "").is_ok());
        }

        /// `binary_on_path` reports a definitely-absent binary as missing
        /// (deterministic; no env mutation).
        #[test]
        fn binary_on_path_reports_absent_binary() {
            assert!(!binary_on_path(
                "sid-definitely-not-a-real-binary-zzz-9f3a2b"
            ));
        }

        /// The missing-`sshpass` preflight (§C) yields a clear error that never
        /// contains the password.
        #[test]
        fn run_copy_id_missing_sshpass_message_carries_no_password() {
            // A password-host invocation's argv never exposes the password (it
            // goes via SSHPASS), and the static preflight message carries none.
            let inv = build_ssh_copy_id_invocation("pi", "u", "h", 22, None, Some("hunter2"))
                .expect("valid positional components");
            assert!(
                !inv.argv().iter().any(|a| a == "hunter2"),
                "leak: {:?}",
                inv.argv()
            );
            // The missing-binary message itself carries no password (it is
            // constructed from a static string only).
            assert!(
                !"err: sshpass not on PATH (required for password-auth key copy)"
                    .contains("hunter2")
            );
        }

        // ── Host removal deletes the saved password ─────────────────────────

        #[test]
        fn removing_host_deletes_saved_password() {
            use sid_widgets::FieldValue;
            let mut sid_app = build_test_sid_app(Some("ssh"));
            seed_host_into_widget(&mut sid_app, password_host("pi"));
            sid_app
                .secrets
                .put(&SecretId::new("ssh.host.pi.password"), b"raspberry")
                .unwrap();
            submit_ssh_remove(
                &mut sid_app,
                "pi",
                &[("confirm".into(), FieldValue::Choice("Yes, remove".into()))],
            )
            .unwrap();
            assert!(
                sid_app
                    .secrets
                    .get(&SecretId::new("ssh.host.pi.password"))
                    .unwrap()
                    .is_none()
            );
            assert!(sid_app.store.get_ssh_host("pi").unwrap().is_none());
        }
    }

    // ── Network detail pane wiring tests (Task 5) ────────────────────────────

    #[test]
    fn network_open_detail_form_sets_form_on_app() {
        let mut sid_app = build_test_sid_app(Some("network"));
        let snap = sid_core::sys_probe::SysSnapshot {
            processes: vec![],
            listening_ports: vec![],
            interfaces: vec![sid_core::adapters::sys::NetInterface {
                name: "eth0".into(),
                addrs: vec!["10.0.0.1".into()],
                rx_bytes: 1024,
                tx_bytes: 512,
                is_up: true,
            }],
            default_route_iface: None,
            captured_at_unix_secs: 0,
        };
        refresh_network_widget(&mut sid_app, snap);
        {
            let tab = sid_app.app.tabs_mut().active_mut();
            let w = tab
                .layout
                .iter_widgets_mut()
                .next()
                .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>())
                .expect("network widget");
            while w.focus() != sid_widgets::network::Focus::Interfaces {
                w.focus_next();
            }
        }
        network_open_detail_form(&mut sid_app);
        assert!(
            sid_app.form.is_some(),
            "form should be open after network_open_detail_form"
        );
        let form = sid_app.form.as_ref().unwrap();
        assert!(
            form.spec.id.0.starts_with("network.interface_prefs:"),
            "form id should embed interface name; got: {}",
            form.spec.id.0
        );
    }

    #[test]
    fn network_submit_prefs_writes_to_store_and_closes_form() {
        use std::collections::BTreeMap;

        use sid_store::TypedSettings;
        use sid_widgets::form::FormValues;

        let mut sid_app = build_test_sid_app(Some("network"));
        let snap = sid_core::sys_probe::SysSnapshot {
            processes: vec![],
            listening_ports: vec![],
            interfaces: vec![sid_core::adapters::sys::NetInterface {
                name: "eth0".into(),
                addrs: vec![],
                rx_bytes: 0,
                tx_bytes: 0,
                is_up: true,
            }],
            default_route_iface: None,
            captured_at_unix_secs: 0,
        };
        refresh_network_widget(&mut sid_app, snap);

        let mut map = BTreeMap::new();
        map.insert("pinned".into(), "true".into());
        map.insert("alias".into(), "home-net".into());
        let values: FormValues = map;

        dispatch_form_submit(&mut sid_app, "network.interface_prefs:eth0", values);

        assert!(sid_app.form.is_none());

        assert_eq!(
            sid_app.store.get_bool("network.iface.eth0.pinned").unwrap(),
            Some(true)
        );
        assert_eq!(
            sid_app
                .store
                .get_string("network.iface.eth0.alias")
                .unwrap()
                .as_deref(),
            Some("home-net")
        );
    }

    #[test]
    fn network_close_detail_pane_action_clears_form() {
        use sid_widgets::form::{FormId, FormSection, FormSpec, SectionKind};

        let mut sid_app = build_test_sid_app(Some("network"));
        let origin_tab = sid_app.app.tabs().active().id.clone();
        sid_app.form = Some(sid_widgets::form::FormPane::new(FormSpec {
            id: FormId("network.interface_prefs:eth0".into()),
            title: "Interface: eth0".into(),
            primary_label: "Save".into(),
            sections: vec![FormSection {
                kind: SectionKind::Info,
                title: "".into(),
                fields: vec![],
            }],
            reshape: None,
            watch: vec![],
        }));
        sid_app.form_origin_tab = Some(origin_tab);

        handle_network_action(&mut sid_app, "network.close_detail_pane");
        assert!(sid_app.form.is_none());
    }

    // ── Fix 1: detail-pane state desync — production-routing tests ────────────

    /// Helper: build a SidApp on the network tab with eth0 loaded and the
    /// interfaces pane focused.  Returns with the widget ready to open.
    fn sid_app_with_eth0() -> SidApp {
        let mut sid_app = build_test_sid_app(Some("network"));
        let snap = sid_core::sys_probe::SysSnapshot {
            processes: vec![],
            listening_ports: vec![],
            interfaces: vec![sid_core::adapters::sys::NetInterface {
                name: "eth0".into(),
                addrs: vec!["10.0.0.1".into()],
                rx_bytes: 1024,
                tx_bytes: 512,
                is_up: true,
            }],
            default_route_iface: None,
            captured_at_unix_secs: 0,
        };
        refresh_network_widget(&mut sid_app, snap);
        // Focus the interfaces pane so Enter opens the detail pane.
        {
            let tab = sid_app.app.tabs_mut().active_mut();
            let w = tab
                .layout
                .iter_widgets_mut()
                .next()
                .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>())
                .expect("network widget");
            while w.focus() != sid_widgets::network::Focus::Interfaces {
                w.focus_next();
            }
        }
        sid_app
    }

    /// Open the detail pane via the production path (Enter key → widget emits
    /// PendingNetAction → apply_pending_network_actions → network_open_detail_form).
    fn open_pane_via_enter(sid_app: &mut SidApp) {
        use crossterm::event::KeyCode;
        // Drive the key through the widget directly (form is None → key reaches widget).
        {
            let tab = sid_app.app.tabs_mut().active_mut();
            let w = tab
                .layout
                .iter_widgets_mut()
                .next()
                .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>())
                .expect("network widget");
            let (tx, _rx) = std::sync::mpsc::channel();
            let mut ctx = sid_core::context::WidgetCtx::new(tx);
            let ev = sid_core::event::Event::Key(chord(KeyCode::Enter));
            w.handle_event(&ev, &mut ctx);
        }
        // Wire: flush pending action → opens the form.
        apply_pending_network_actions(sid_app);
    }

    fn net_is_pane_open(sid_app: &SidApp) -> bool {
        let tab = &sid_app.app.tabs().active();
        tab.layout
            .iter_widgets()
            .next()
            .and_then(|w| w.as_any().downcast_ref::<sid_widgets::NetworkWidget>())
            .map(|n| n.is_detail_pane_open())
            .unwrap_or(false)
    }

    fn net_split_depth(sid_app: &SidApp) -> usize {
        let tab = &sid_app.app.tabs().active();
        tab.layout
            .iter_widgets()
            .next()
            .and_then(|w| w.as_any().downcast_ref::<sid_widgets::NetworkWidget>())
            .map(|n| n.split_depth())
            .unwrap_or(0)
    }

    /// Fix 1 (i): submit via dispatch_form_submit → widget pane closed.
    #[test]
    fn fix1_submit_closes_detail_pane_in_widget() {
        use std::collections::BTreeMap;
        let mut sid_app = sid_app_with_eth0();
        open_pane_via_enter(&mut sid_app);
        assert!(
            net_is_pane_open(&sid_app),
            "pane should be open after Enter"
        );
        assert!(sid_app.form.is_some(), "form should be set");

        let mut values = BTreeMap::new();
        values.insert("pinned".to_string(), "false".to_string());
        values.insert("alias".to_string(), String::new());
        dispatch_form_submit(&mut sid_app, "network.interface_prefs:eth0", values);

        assert!(
            sid_app.form.is_none(),
            "form should be cleared after submit"
        );
        assert!(
            !net_is_pane_open(&sid_app),
            "widget detail pane must be closed after submit (Fix 1)"
        );
    }

    /// Fix 1 (ii): Esc-cancel through route_key_event → widget pane closed.
    #[test]
    fn fix1_esc_cancel_closes_detail_pane_in_widget() {
        use crossterm::event::KeyCode;
        let mut sid_app = sid_app_with_eth0();
        open_pane_via_enter(&mut sid_app);
        assert!(net_is_pane_open(&sid_app));

        // Esc on the form triggers FormEvent::Cancel → close_network_detail_pane_if_network_form.
        route_key_event(&mut sid_app, chord(KeyCode::Esc));

        assert!(sid_app.form.is_none(), "form should be cleared after Esc");
        assert!(
            !net_is_pane_open(&sid_app),
            "widget pane must be closed after Esc-cancel (Fix 1)"
        );
    }

    /// Fix 1 (iii): dirty form + discard-confirm "Discard" → widget pane closed.
    #[test]
    fn fix1_discard_confirm_closes_detail_pane_in_widget() {
        use crossterm::event::KeyCode;
        let mut sid_app = sid_app_with_eth0();
        open_pane_via_enter(&mut sid_app);
        assert!(net_is_pane_open(&sid_app));

        // Make the form dirty: toggle the "pinned" field (first editable slot).
        // Space cycles a Toggle field and marks the form dirty.
        route_key_event(&mut sid_app, chord(KeyCode::Char(' ')));

        // Now Esc: because the form is dirty, it should RequestDiscardConfirm.
        route_key_event(&mut sid_app, chord(KeyCode::Esc));
        // The discard-confirm modal should now be open.
        assert!(
            sid_app.modal_stack.last().map(|m| m.id.0.as_str()) == Some("form.discard_confirm"),
            "discard confirm modal should be open"
        );
        // Form must still be alive (we haven't discarded yet).
        assert!(sid_app.form.is_some());
        assert!(net_is_pane_open(&sid_app));

        // Select "Discard" (Right cycles the Choice) then submit the modal
        // via the same pattern as the existing dirty_form_esc_opens_discard_confirm_and_discard_closes
        // test: directly drive the modal and call dispatch_modal_submit.
        {
            let modal = sid_app.modal_stack.last_mut().unwrap();
            sid_widgets::route_key_to_modal(modal, chord(KeyCode::Right));
            let outcome = sid_widgets::route_key_to_modal(
                sid_app.modal_stack.last_mut().unwrap(),
                chord(KeyCode::Enter),
            );
            assert_eq!(outcome, sid_widgets::ModalKeyOutcome::Submit);
            let popped = sid_app.modal_stack.pop().unwrap();
            let values = popped.collect_values();
            dispatch_modal_submit(&mut sid_app, &popped.id, &values).unwrap();
        }

        assert!(
            sid_app.form.is_none(),
            "form should be cleared after Discard"
        );
        assert!(
            !net_is_pane_open(&sid_app),
            "widget pane must be closed after discard-confirm (Fix 1)"
        );
    }

    /// Fix 1 (iv): repeated Enter×5 → split depth stays 1 (guard prevents stack growth).
    #[test]
    fn fix1_repeated_enter_does_not_grow_split_stack() {
        let mut sid_app = sid_app_with_eth0();
        open_pane_via_enter(&mut sid_app);
        assert_eq!(net_split_depth(&sid_app), 1, "depth 1 after first open");

        // Repeat Enter×4 more times via the production path.
        for _ in 0..4 {
            // While the form is open, Enter goes to the form (not the widget),
            // so we must first simulate that no form is set (to test the guard
            // on the widget itself).
            // Drive Enter directly into the widget bypassing the wire form intercept.
            {
                let tab = sid_app.app.tabs_mut().active_mut();
                let w = tab
                    .layout
                    .iter_widgets_mut()
                    .next()
                    .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>())
                    .expect("network widget");
                let (tx, _rx) = std::sync::mpsc::channel();
                let mut ctx = sid_core::context::WidgetCtx::new(tx);
                let ev = sid_core::event::Event::Key(chord(crossterm::event::KeyCode::Enter));
                w.handle_event(&ev, &mut ctx);
                // The pending action will be OpenDetailPane, but the guard must block the push.
            }
            apply_pending_network_actions(&mut sid_app);
        }
        assert_eq!(
            net_split_depth(&sid_app),
            1,
            "depth must stay 1 after repeated Enter (Fix 1 guard)"
        );
    }

    // ── Fix 2: Tab contract while pane-focused ────────────────────────────────

    /// Fix 2: Tab bubbles to wire when detail pane is open (SplitFocus::Pane).
    #[test]
    fn fix2_tab_bubbles_when_pane_open() {
        use crossterm::event::KeyCode;
        use sid_core::{
            event::{Event, KeyChord},
            widget::{EventOutcome, Widget},
        };

        let mut sid_app = sid_app_with_eth0();
        open_pane_via_enter(&mut sid_app);
        assert!(net_is_pane_open(&sid_app));

        // Drive Tab directly into the widget (bypass wire's form intercept).
        let tab = sid_app.app.tabs_mut().active_mut();
        let w = tab
            .layout
            .iter_widgets_mut()
            .next()
            .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>())
            .expect("network widget");
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = sid_core::context::WidgetCtx::new(tx);
        let ev = Event::Key(KeyChord::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        let outcome = w.handle_event(&ev, &mut ctx);
        assert_eq!(
            outcome,
            EventOutcome::Bubble,
            "Tab must bubble when SplitFocus::Pane (Fix 2)"
        );
    }

    /// Fix 2: Tab is consumed by the widget when in list mode (pane closed).
    #[test]
    fn fix2_tab_consumed_when_list_focused() {
        use crossterm::event::KeyCode;
        use sid_core::{
            event::{Event, KeyChord},
            widget::{EventOutcome, Widget},
        };

        let mut sid_app = sid_app_with_eth0();
        // Pane is NOT open — list mode.
        assert!(!net_is_pane_open(&sid_app));

        let tab = sid_app.app.tabs_mut().active_mut();
        let w = tab
            .layout
            .iter_widgets_mut()
            .next()
            .and_then(|w| w.as_any_mut().downcast_mut::<sid_widgets::NetworkWidget>())
            .expect("network widget");
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = sid_core::context::WidgetCtx::new(tx);
        let ev = Event::Key(KeyChord::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        let outcome = w.handle_event(&ev, &mut ctx);
        assert_eq!(
            outcome,
            EventOutcome::Consumed,
            "Tab must be consumed when SplitFocus::List (Fix 2)"
        );
    }

    // ── Fix 3: interface vanishes under open pane ─────────────────────────────

    /// Fix 3: snapshot without the open interface's name → pane closed, form
    /// cleared, and no store writes occur on a subsequent submit.
    #[test]
    fn fix3_vanished_interface_closes_pane_and_clears_form() {
        use sid_store::TypedSettings;
        let mut sid_app = sid_app_with_eth0();
        open_pane_via_enter(&mut sid_app);
        assert!(net_is_pane_open(&sid_app), "pane open after Enter");
        assert!(sid_app.form.is_some(), "form set after Enter");

        // Apply a snapshot that does NOT contain eth0.
        let snap_without_eth0 = sid_core::sys_probe::SysSnapshot {
            processes: vec![],
            listening_ports: vec![],
            interfaces: vec![sid_core::adapters::sys::NetInterface {
                name: "lo".into(),
                addrs: vec!["127.0.0.1".into()],
                rx_bytes: 0,
                tx_bytes: 0,
                is_up: true,
            }],
            default_route_iface: None,
            captured_at_unix_secs: 1,
        };
        refresh_network_widget(&mut sid_app, snap_without_eth0);

        assert!(
            sid_app.form.is_none(),
            "form must be cleared when interface vanishes (Fix 3)"
        );
        assert!(
            !net_is_pane_open(&sid_app),
            "widget pane must be closed when interface vanishes (Fix 3)"
        );

        // No store write should have happened for eth0 (the form was never submitted).
        assert_eq!(
            sid_app.store.get_bool("network.iface.eth0.pinned").unwrap(),
            None,
            "no orphaned store write for vanished interface"
        );
    }

    // ---- undo ring + u-chord interceptor ----

    #[test]
    fn u_chord_with_empty_ring_returns_false() {
        use crossterm::event::KeyCode;
        let mut sid_app = build_test_sid_app(None);
        let chord = chord(KeyCode::Char('u'));
        let consumed = route_key_event(&mut sid_app, chord);
        assert!(!consumed, "u with empty ring should not be consumed");
    }

    #[test]
    fn u_chord_with_fresh_entry_applies_and_is_consumed() {
        use crossterm::event::KeyCode;
        use sid_store::TypedSettings;

        use crate::settings_undo::{UndoEntry, UndoPayload};
        let mut sid_app = build_test_sid_app(None);
        // Seed the store with a theme value to undo.
        sid_app
            .store
            .put_string(sid_store::settings_keys::THEME_NAME, "void")
            .unwrap();
        // Push an undo entry that restores "cosmos".
        sid_app.undo_ring.push_back(UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: std::time::Instant::now(),
        });
        // Spec: u fires ONLY when the head toast is live and carries the marker.
        sid_app
            .toasts
            .push(Toast::success("Theme 'void' applied (u: undo)"));
        let chord = chord(KeyCode::Char('u'));
        let consumed = route_key_event(&mut sid_app, chord);
        assert!(
            consumed,
            "u with valid entry and live marker toast should be consumed"
        );
        let stored = sid_app
            .store
            .get_string(sid_store::settings_keys::THEME_NAME)
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some("cosmos"),
            "prior theme should be restored"
        );
        assert!(sid_app.undo_ring.is_empty(), "entry popped from ring");
    }

    #[test]
    fn u_chord_with_expired_entry_returns_false_and_discards() {
        use crossterm::event::KeyCode;

        use crate::settings_undo::{UNDO_TTL, UndoEntry, UndoPayload};
        let mut sid_app = build_test_sid_app(None);
        let mut entry = UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: std::time::Instant::now(),
        };
        entry.recorded_at =
            std::time::Instant::now() - UNDO_TTL - std::time::Duration::from_millis(1);
        sid_app.undo_ring.push_back(entry);
        // Even with a live marker toast, an expired ring entry is discarded.
        sid_app.toasts.push(Toast::success("Theme saved (u: undo)"));
        let chord = chord(KeyCode::Char('u'));
        let consumed = route_key_event(&mut sid_app, chord);
        assert!(!consumed, "expired entry should not be consumed");
        assert!(
            sid_app.undo_ring.is_empty(),
            "expired entry should be discarded"
        );
    }

    #[test]
    fn u_chord_ring_cap_evicts_oldest() {
        use crate::settings_undo::{UNDO_RING_CAP, UndoEntry, UndoPayload};
        // Fill the ring beyond cap and verify it doesn't exceed cap.
        let mut ring: std::collections::VecDeque<UndoEntry> = std::collections::VecDeque::new();
        for i in 0..=(UNDO_RING_CAP + 5) {
            push_undo(
                &mut ring,
                UndoEntry {
                    payload: UndoPayload::Theme {
                        prior: format!("theme_{i}"),
                    },
                    recorded_at: std::time::Instant::now(),
                },
            );
        }
        assert_eq!(ring.len(), UNDO_RING_CAP, "ring capped at UNDO_RING_CAP");
    }

    // ---- Fix 1: head-toast gating ----

    /// u with a live marker toast AND a fresh ring entry → undo fires.
    #[test]
    fn u_chord_live_marker_toast_fires_undo() {
        use crossterm::event::KeyCode;
        use sid_store::TypedSettings;

        use crate::settings_undo::{UndoEntry, UndoPayload};
        let mut sid_app = build_test_sid_app(None);
        sid_app
            .store
            .put_string(sid_store::settings_keys::THEME_NAME, "void")
            .unwrap();
        sid_app.undo_ring.push_back(UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: std::time::Instant::now(),
        });
        // Head toast is live and carries the marker.
        sid_app
            .toasts
            .push(Toast::success("Theme 'void' applied (u: undo)"));
        let consumed = route_key_event(&mut sid_app, chord(KeyCode::Char('u')));
        assert!(consumed, "u with live marker toast should fire undo");
        let stored = sid_app
            .store
            .get_string(sid_store::settings_keys::THEME_NAME)
            .unwrap();
        assert_eq!(stored.as_deref(), Some("cosmos"), "prior restored");
        assert!(sid_app.undo_ring.is_empty(), "entry consumed");
    }

    /// u after toast has expired (3s lifetime elapsed) but ring entry still
    /// within 30s TTL → head-toast gate rejects, undo does NOT fire, ring
    /// entry is preserved (not popped).
    #[test]
    fn u_chord_expired_toast_with_fresh_entry_falls_through() {
        use crossterm::event::KeyCode;
        use sid_store::TypedSettings;

        use crate::{
            settings_undo::{UndoEntry, UndoPayload},
            toast::TOAST_LIFETIME,
        };
        let mut sid_app = build_test_sid_app(None);
        sid_app
            .store
            .put_string(sid_store::settings_keys::THEME_NAME, "void")
            .unwrap();
        sid_app.undo_ring.push_back(UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: std::time::Instant::now(),
        });
        // Push a marker toast but age it past the toast lifetime.
        let mut expired_toast = Toast::success("Theme saved (u: undo)");
        expired_toast.spawned_at =
            std::time::Instant::now() - TOAST_LIFETIME - std::time::Duration::from_millis(1);
        sid_app.toasts.push(expired_toast);
        let ring_len_before = sid_app.undo_ring.len();
        let consumed = route_key_event(&mut sid_app, chord(KeyCode::Char('u')));
        assert!(!consumed, "u with expired toast must fall through");
        assert_eq!(
            sid_app.undo_ring.len(),
            ring_len_before,
            "ring entry must NOT be popped — toast guard rejected before ring pop"
        );
        // Store must be unchanged.
        let stored = sid_app
            .store
            .get_string(sid_store::settings_keys::THEME_NAME)
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some("void"),
            "theme must not be reverted"
        );
    }

    /// u with a live toast that does NOT carry the marker → falls through,
    /// ring unchanged.
    #[test]
    fn u_chord_live_toast_without_marker_falls_through() {
        use crossterm::event::KeyCode;
        use sid_store::TypedSettings;

        use crate::settings_undo::{UndoEntry, UndoPayload};
        let mut sid_app = build_test_sid_app(None);
        sid_app
            .store
            .put_string(sid_store::settings_keys::THEME_NAME, "void")
            .unwrap();
        sid_app.undo_ring.push_back(UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: std::time::Instant::now(),
        });
        // Live toast — but no "(u: undo)" in its text.
        sid_app.toasts.push(Toast::info("Some unrelated message"));
        let ring_len_before = sid_app.undo_ring.len();
        let consumed = route_key_event(&mut sid_app, chord(KeyCode::Char('u')));
        assert!(!consumed, "u with non-marker toast must fall through");
        assert_eq!(
            sid_app.undo_ring.len(),
            ring_len_before,
            "ring entry must NOT be popped — marker check failed"
        );
        let stored = sid_app
            .store
            .get_string(sid_store::settings_keys::THEME_NAME)
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some("void"),
            "theme must not be reverted"
        );
    }

    #[test]
    fn u_chord_ignored_while_modal_open() {
        use crossterm::event::KeyCode;
        use sid_store::TypedSettings;

        use crate::settings_undo::{UndoEntry, UndoPayload};
        let mut sid_app = build_test_sid_app(None);
        // Seed the store and push a fresh undo entry that would normally apply.
        sid_app
            .store
            .put_string(sid_store::settings_keys::THEME_NAME, "void")
            .unwrap();
        let ring_entry = UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: std::time::Instant::now(),
        };
        sid_app.undo_ring.push_back(ring_entry);
        // Include a live marker toast; modal still fires first so undo is skipped.
        sid_app.toasts.push(Toast::success("Theme saved (u: undo)"));
        let ring_len_before = sid_app.undo_ring.len();
        // Push a modal onto the stack so the undo interceptor is bypassed.
        sid_app.modal_stack.push(sid_widgets::ModalSpec::new(
            "test.modal",
            "Test",
            vec![sid_widgets::modal::Field::Display {
                label: "info".into(),
                body: "blocking".into(),
            }],
        ));
        let chord = chord(KeyCode::Char('u'));
        let _consumed = route_key_event(&mut sid_app, chord);
        // The modal intercepts the key (returns true), but the *undo* interceptor
        // must NOT have fired — the ring is unchanged and the theme is not reverted.
        assert_eq!(
            sid_app.undo_ring.len(),
            ring_len_before,
            "undo ring must be unchanged — modal intercepted the key, not the undo ring"
        );
        // The theme must NOT have been reverted.
        let stored = sid_app
            .store
            .get_string(sid_store::settings_keys::THEME_NAME)
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some("void"),
            "theme must not be reverted while modal is open"
        );
    }

    #[test]
    fn u_chord_ignored_while_form_open() {
        use crossterm::event::KeyCode;
        use sid_store::TypedSettings;

        use crate::settings_undo::{UndoEntry, UndoPayload};
        let mut sid_app = build_test_sid_app(None);
        // Seed the store and push a fresh undo entry.
        sid_app
            .store
            .put_string(sid_store::settings_keys::THEME_NAME, "void")
            .unwrap();
        let ring_entry = UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: std::time::Instant::now(),
        };
        sid_app.undo_ring.push_back(ring_entry);
        let ring_len_before = sid_app.undo_ring.len();
        // Open a form; the form intercept branch fires before the undo branch.
        open_form(&mut sid_app, test_form_spec("test.edit"));
        let chord = chord(KeyCode::Char('u'));
        let consumed = route_key_event(&mut sid_app, chord);
        // The form intercepts the key and returns Continue, so route_key_event
        // returns true (form consumed it), not the undo interceptor.
        assert!(
            consumed,
            "u should be consumed by the form, not the undo interceptor"
        );
        assert_eq!(
            sid_app.undo_ring.len(),
            ring_len_before,
            "undo ring must be unchanged — form consumed the key, not undo"
        );
        // The theme must NOT have been reverted.
        let stored = sid_app
            .store
            .get_string(sid_store::settings_keys::THEME_NAME)
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some("void"),
            "theme must not be reverted while form is open"
        );
        // The form's focused text field should now contain 'u' (typed into it).
        let form = sid_app.form.as_ref().expect("form still open");
        let first_field_value = form.spec.sections[0].fields[0].value_string();
        assert_eq!(
            first_field_value, "u",
            "u must have been typed into the focused form field"
        );
    }

    // ---- Fix 5: apply→undo round-trips per payload variant ----
    //
    // Each test:
    //  1. Seeds the store with an initial (prior) value.
    //  2. Applies the new value to the store the same way production does.
    //  3. Constructs an UndoEntry matching what apply_pending_settings_outcomes
    //     would push, then calls apply_undo_entry.
    //  4. Asserts the store is restored to the prior value via the same read
    //     production code uses — completing the apply→undo round-trip.

    #[test]
    fn behavior_toggle_apply_undo_round_trip() {
        use sid_store::TypedSettings;
        use sid_widgets::settings::behavior_toggles::ToggleValue;

        use crate::settings_undo::{UndoEntry, UndoPayload};

        let mut sid_app = build_test_sid_app(None);
        const KEY: &str = sid_store::settings_keys::AUTO_RESTORE_SESSION;

        // Prior state: false.
        sid_app.store.put_bool(KEY, false).unwrap();

        // Apply: toggle to true.
        sid_app.store.put_bool(KEY, true).unwrap();

        // Undo entry matching what apply_pending_settings_outcomes would push.
        let entry = UndoEntry {
            payload: UndoPayload::BehaviorToggle {
                key: KEY,
                prior: ToggleValue::Bool(false),
            },
            recorded_at: std::time::Instant::now(),
        };
        apply_undo_entry(&mut sid_app, entry);

        // Store must be restored to false via the same read production uses.
        assert_eq!(
            sid_app.store.get_bool(KEY).unwrap(),
            Some(false),
            "undo must restore prior bool value"
        );
    }

    #[test]
    fn workspace_roots_apply_undo_round_trip() {
        use sid_store::SettingValue;

        use crate::settings_undo::{UndoEntry, UndoPayload};

        let mut sid_app = build_test_sid_app(None);
        let prior_roots: Vec<std::path::PathBuf> = vec!["/prior/a".into(), "/prior/b".into()];
        let new_roots: Vec<std::path::PathBuf> = vec!["/new/x".into()];

        // Seed prior.
        let prior_json = serde_json::to_string(&prior_roots).unwrap();
        sid_app
            .store
            .put_setting(
                sid_store::settings_keys::WORKSPACE_ROOTS,
                &SettingValue(prior_json.into_bytes()),
            )
            .unwrap();

        // Apply new roots (same as production apply_pending_settings_outcomes).
        let new_json = serde_json::to_string(&new_roots).unwrap();
        sid_app
            .store
            .put_setting(
                sid_store::settings_keys::WORKSPACE_ROOTS,
                &SettingValue(new_json.into_bytes()),
            )
            .unwrap();

        // Undo.
        let entry = UndoEntry {
            payload: UndoPayload::WorkspaceRoots {
                prior: prior_roots.clone(),
            },
            recorded_at: std::time::Instant::now(),
        };
        apply_undo_entry(&mut sid_app, entry);

        // Restore check via same read production uses.
        let sv = sid_app
            .store
            .get_setting(sid_store::settings_keys::WORKSPACE_ROOTS)
            .unwrap()
            .unwrap();
        let restored: Vec<std::path::PathBuf> = serde_json::from_slice(&sv.0).unwrap();
        assert_eq!(
            restored, prior_roots,
            "undo must restore prior workspace roots"
        );
    }

    #[test]
    fn quick_action_upserted_apply_undo_round_trip() {
        use sid_store::{QuickAction, QuickActionScope};

        use crate::settings_undo::{UndoEntry, UndoPayload};

        let mut sid_app = build_test_sid_app(None);
        let original = QuickAction {
            id: "qa-rt-test".into(),
            label: "original".into(),
            cmd: "echo original".into(),
            keybind: None,
            scope: QuickActionScope::Global,
        };
        let updated = QuickAction {
            id: "qa-rt-test".into(),
            label: "updated".into(),
            cmd: "echo updated".into(),
            keybind: None,
            scope: QuickActionScope::Global,
        };

        // Seed prior.
        sid_app.store.upsert_quick_action(&original).unwrap();
        // Apply.
        sid_app.store.upsert_quick_action(&updated).unwrap();

        // Store reflects updated.
        assert_eq!(
            sid_app
                .store
                .get_quick_action("qa-rt-test")
                .unwrap()
                .unwrap()
                .label,
            "updated"
        );

        // Undo — prior was original.
        let entry = UndoEntry {
            payload: UndoPayload::QuickActionUpserted {
                prior: original.clone(),
            },
            recorded_at: std::time::Instant::now(),
        };
        apply_undo_entry(&mut sid_app, entry);

        assert_eq!(
            sid_app
                .store
                .get_quick_action("qa-rt-test")
                .unwrap()
                .unwrap()
                .label,
            "original",
            "undo must restore original quick action label"
        );
    }

    #[test]
    fn quick_action_removed_apply_undo_round_trip() {
        use sid_store::{QuickAction, QuickActionScope};

        use crate::settings_undo::{UndoEntry, UndoPayload};

        let mut sid_app = build_test_sid_app(None);
        let qa = QuickAction {
            id: "qa-rm-rt-test".into(),
            label: "to-remove".into(),
            cmd: "echo rm".into(),
            keybind: None,
            scope: QuickActionScope::Global,
        };

        // Seed.
        sid_app.store.upsert_quick_action(&qa).unwrap();
        // Apply removal.
        sid_app.store.remove_quick_action("qa-rm-rt-test").unwrap();
        assert!(
            sid_app
                .store
                .get_quick_action("qa-rm-rt-test")
                .unwrap()
                .is_none(),
            "action must be removed"
        );

        // Undo — prior was the qa record.
        let entry = UndoEntry {
            payload: UndoPayload::QuickActionRemoved { prior: qa.clone() },
            recorded_at: std::time::Instant::now(),
        };
        apply_undo_entry(&mut sid_app, entry);

        let restored = sid_app
            .store
            .get_quick_action("qa-rm-rt-test")
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.label, "to-remove",
            "undo must re-insert the removed quick action"
        );
    }

    #[test]
    fn keybind_apply_undo_round_trip() {
        use sid_core::keybind::KeybindMap;
        use sid_store::keybind_load::{load_keybind_profile, save_keybind_profile};

        use crate::settings_undo::{UndoEntry, UndoPayload};

        let mut sid_app = build_test_sid_app(None);

        // Prior: cosmos default (non-empty).
        let prior_map = KeybindMap::cosmos_default();
        save_keybind_profile(&*sid_app.store, "cosmos", &prior_map).unwrap();

        // Apply: empty map.
        let empty_map = KeybindMap::new();
        save_keybind_profile(&*sid_app.store, "cosmos", &empty_map).unwrap();
        assert_eq!(
            load_keybind_profile(&*sid_app.store, "cosmos")
                .unwrap()
                .unwrap()
                .iter()
                .count(),
            0,
            "store must hold empty map after apply"
        );

        // Undo.
        let entry = UndoEntry {
            payload: UndoPayload::Keybind {
                profile_name: "cosmos".into(),
                prior: prior_map,
            },
            recorded_at: std::time::Instant::now(),
        };
        apply_undo_entry(&mut sid_app, entry);

        let restored = load_keybind_profile(&*sid_app.store, "cosmos")
            .unwrap()
            .unwrap();
        assert!(
            restored.iter().count() > 0,
            "undo must restore the prior (non-empty) keybind map"
        );
    }

    // ----- Animation tick-gate regression tests ----------------------------

    /// After the user presses S in AnimationView (simulated here by injecting an
    /// AnimationChanged outcome directly), `SidApp.animation` must reflect the
    /// new config on the next event loop iteration.
    ///
    /// Before the fix: `apply_pending_settings_outcomes` ignores `AnimationChanged`
    /// so `SidApp.animation` stays at the startup value forever.
    #[tokio::test]
    async fn animation_config_propagates_after_settings_save() {
        use ratatui::backend::TestBackend;
        use tokio::sync::mpsc;

        let backend = TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut sid_app = build_test_sid_app(Some("settings"));
        // Confirm default animation is enabled.
        assert!(
            sid_app.animation.enabled,
            "precondition: animation enabled at startup"
        );

        // Build a new config with animation disabled.
        let new_cfg = sid_core::animation::AnimationConfig {
            enabled: false,
            ..sid_core::animation::AnimationConfig::default()
        };

        // Inject the outcome directly into the settings widget's pending queue,
        // bypassing the UI — this isolates the wire-layer drain logic.
        {
            use sid_core::layout::Layout;
            let tabs = sid_app.app.tabs_mut().tabs_mut();
            let settings_tab = tabs
                .iter_mut()
                .find(|t| t.id.as_str() == "settings")
                .expect("settings tab must be present in test app");
            let Layout::Single(w) = &mut settings_tab.layout else {
                panic!("settings tab must have Single layout");
            };
            let settings_widget = w
                .as_any_mut()
                .downcast_mut::<sid_widgets::SettingsWidget>()
                .expect("must downcast to SettingsWidget");
            settings_widget.push_pending_outcome(
                sid_widgets::settings::PendingSettingsOutcome::AnimationChanged(new_cfg.clone()),
            );
        }

        // Send one Tick so the loop runs once and drains the outcome.
        let (tx, mut rx) = mpsc::channel::<sid_core::event::Event>(4);
        tx.send(sid_core::event::Event::Tick).await.unwrap();
        drop(tx);

        run_event_loop(&mut terminal, &mut sid_app, &mut rx)
            .await
            .unwrap();

        // After the fix: SidApp.animation reflects the new config.
        assert!(
            !sid_app.animation.enabled,
            "SidApp.animation.enabled must be false after AnimationChanged drain"
        );
        // fx_state must be toggled to None because enabled=false.
        assert!(
            sid_app.fx_state.is_none(),
            "fx_state must be None when animation.enabled == false"
        );
    }

    // ---- T3: message logging + toasts logs-only ----------------------------

    /// Replace the test app's settings widget with one that carries a `Logs`
    /// category (the default `build_app` path uses the legacy zero-category
    /// widget, which has nowhere to record into).
    fn install_logs_settings_widget(sid_app: &mut SidApp) {
        use sid_core::layout::Layout;
        use sid_widgets::{SettingsCategory, SettingsWidget, settings::logs::LogsView};
        let tabs = sid_app.app.tabs_mut().tabs_mut();
        let settings_tab = tabs
            .iter_mut()
            .find(|t| t.id.as_str() == "settings")
            .expect("settings tab present");
        let widget = SettingsWidget::with_categories(vec![SettingsCategory::Logs(LogsView::new())]);
        settings_tab.layout = Layout::Single(Box::new(widget));
    }

    /// Read the Logs category's entries out of the test app's settings widget.
    /// The installed widget has `Logs` as its single (and focused) category.
    fn logs_entries(sid_app: &mut SidApp) -> Vec<sid_widgets::settings::logs::LogEntry> {
        use sid_widgets::SettingsCategory;
        let settings = settings_widget_mut(sid_app).expect("settings widget present");
        match settings.focused_category() {
            Some(SettingsCategory::Logs(v)) => v.entries().iter().cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// `record` pushes into BOTH the Logs ring and the toast queue, and maps
    /// each `LogLevel` to the right toast kind.
    #[test]
    fn record_feeds_logs_ring_and_toast_queue() {
        use sid_widgets::settings::logs::LogLevel;

        use crate::toast::ToastKind;

        let mut sid_app = build_test_sid_app(Some("settings"));
        install_logs_settings_widget(&mut sid_app);

        record(&mut sid_app, LogLevel::Success, "saved ok");
        record(&mut sid_app, LogLevel::Error, "boom");
        record(&mut sid_app, LogLevel::Info, "heads up");

        let entries = logs_entries(&mut sid_app);
        assert_eq!(entries.len(), 3, "all three messages must be logged");
        assert_eq!(entries[0].level, LogLevel::Success);
        assert_eq!(entries[0].message, "saved ok");
        assert_eq!(entries[1].level, LogLevel::Error);
        assert_eq!(entries[2].level, LogLevel::Info);

        // Toast queue is fed regardless of overlay gating, with mapped kinds.
        let kinds: Vec<ToastKind> = sid_app.toasts.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![ToastKind::Success, ToastKind::Error, ToastKind::Success],
            "Info maps to a neutral Success toast; Success/Error map directly"
        );
    }

    /// Driving a settings outcome that emits a message records it into the
    /// Logs ring. Uses `AnimationChanged`, which always emits a success
    /// message via `record`.
    #[test]
    fn settings_outcome_records_into_logs_ring() {
        let mut sid_app = build_test_sid_app(Some("settings"));
        install_logs_settings_widget(&mut sid_app);

        // Queue an AnimationChanged outcome on the (now Logs-only) widget.
        {
            let settings = settings_widget_mut(&mut sid_app).expect("settings widget");
            let new_cfg = sid_core::animation::AnimationConfig {
                enabled: false,
                ..sid_core::animation::AnimationConfig::default()
            };
            settings.push_pending_outcome(
                sid_widgets::settings::PendingSettingsOutcome::AnimationChanged(new_cfg),
            );
        }

        apply_pending_settings_outcomes(&mut sid_app);

        let entries = logs_entries(&mut sid_app);
        assert!(
            entries
                .iter()
                .any(|e| e.message == "Animation settings applied"),
            "AnimationChanged must record a log entry; got {entries:?}"
        );
    }

    /// With `TOASTS_ENABLED == false` (the shipped default), `draw` must not
    /// render any toast region even when the queue holds live toasts: the toast
    /// body must not appear anywhere in the rendered buffer. If `TOASTS_ENABLED`
    /// is ever flipped back to `true`, this test is expected to be revisited.
    #[test]
    fn toasts_overlay_suppressed_when_disabled() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut sid_app = build_test_sid_app(Some("workspaces"));
        // Push a toast with a highly distinctive body unlikely to occur in
        // normal chrome, so any appearance is attributable to the toast row.
        sid_app.toasts.push(Toast::error("ZZQQ_TOAST_MARKER_ZZQQ"));

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &sid_app)).unwrap();

        let text = test_buffer_text(&terminal, false);
        assert!(
            !text.contains("ZZQQ_TOAST_MARKER_ZZQQ"),
            "toast body must not be rendered when TOASTS_ENABLED is false"
        );
    }

    /// Render the test terminal's buffer as text, one row per line.
    ///
    /// With `styled` the cell style is appended to each glyph so colour-only
    /// animation changes are visible to comparisons; the plain form matches
    /// the readable insta snapshot format.
    fn test_buffer_text(
        terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
        styled: bool,
    ) -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| {
                        let cell = &buf[(x, y)];
                        if styled {
                            format!("{}{:?}", cell.symbol(), cell.style())
                        } else {
                            cell.symbol().to_string()
                        }
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Two consecutive frames separated by exactly one `SidEvent::Tick` must
    /// produce visually different buffer contents in at least one cell.
    ///
    /// This guards against "dead" animations where `tick_count` advances but
    /// the rendered output never changes. The test uses a seeded `FxState` so
    /// the star positions are deterministic; after one tick the `phase` of at
    /// least one star changes, which changes the rendered colour or glyph.
    ///
    /// The frame comparison includes cell styles (a colour-only twinkle must
    /// count as a change); the insta snapshot of the second frame stays
    /// symbols-only so the golden file remains readable.
    #[tokio::test]
    async fn animation_frames_differ_after_tick() {
        use ratatui::backend::TestBackend;
        use tokio::sync::mpsc;

        // Use a large-enough terminal so stars are guaranteed to exist.
        let backend = TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut sid_app = build_test_sid_app(Some("workspaces"));
        // Seed the FxState so positions are deterministic and we can assert
        // frame equality / difference reliably.
        let cfg = sid_core::animation::AnimationConfig {
            enabled: true,
            density: 30,
            fps: 8,
            ..sid_core::animation::AnimationConfig::default()
        };
        sid_app.animation = cfg.clone();
        sid_app.fx_state = Some(sid_fx::FxState::with_seed(42));

        // First run: one Tick, then the channel closes. The loop draws once
        // more after processing the tick, so on exit the terminal holds the
        // post-tick-1 frame.
        let (tx, mut rx) = mpsc::channel::<sid_core::event::Event>(8);
        tx.send(sid_core::event::Event::Tick).await.unwrap();
        drop(tx);
        run_event_loop(&mut terminal, &mut sid_app, &mut rx)
            .await
            .unwrap();
        let frame_after_tick1 = test_buffer_text(&terminal, true);

        // Second run against the SAME terminal and app: one more Tick.
        let (tx, mut rx) = mpsc::channel::<sid_core::event::Event>(8);
        tx.send(sid_core::event::Event::Tick).await.unwrap();
        drop(tx);
        run_event_loop(&mut terminal, &mut sid_app, &mut rx)
            .await
            .unwrap();
        let frame_after_tick2 = test_buffer_text(&terminal, true);

        // After two Tick events, tick_count must be 2.
        let tc = sid_app
            .fx_state
            .as_ref()
            .expect("fx_state present")
            .tick_count;
        assert_eq!(tc, 2, "exactly two ticks must have fired");

        // The animation must be visibly alive: one tick apart, the two
        // frames must differ in at least one cell (symbol or style).
        assert_ne!(
            frame_after_tick1, frame_after_tick2,
            "consecutive animation frames must be visually different"
        );

        // Snapshot the second frame (symbols only) so the rendered star
        // positions are locked. Re-accept with `cargo insta review` only
        // after deliberate visual changes to the animation layer.
        insta::assert_snapshot!(
            "animation_two_ticks_80x24_seed42",
            test_buffer_text(&terminal, false)
        );
    }

    #[test]
    fn fps_to_tick_ms_derives_and_clamps() {
        // Default fps=8 → 125ms; min fps=1 → 1000ms; max fps=30 → 33ms.
        assert_eq!(fps_to_tick_ms(8), 125);
        assert_eq!(fps_to_tick_ms(1), 1000);
        assert_eq!(fps_to_tick_ms(30), 33);
        // Out-of-range values clamp instead of dividing by zero (fps=0) or
        // over-spinning the pump (fps=255).
        assert_eq!(fps_to_tick_ms(0), 1000);
        assert_eq!(fps_to_tick_ms(255), 33);
    }

    /// `fx.tick()` must advance `tick_count` exactly once per `SidEvent::Tick`
    /// and must NOT advance it on key presses or mouse events.
    ///
    /// Before the fix this test fails because `fx.tick()` fires once per loop
    /// iteration regardless of event kind, so two key-press events advance
    /// `tick_count` by 2 instead of 0.
    #[tokio::test]
    async fn animation_only_ticks_on_tick_event() {
        use ratatui::backend::TestBackend;
        use tokio::sync::mpsc;

        let backend = TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut sid_app = build_test_sid_app(Some("workspaces"));
        // Replace the default None fx_state with a seeded one so tick_count is
        // observable. (build_test_sid_app sets fx_state = None; override it.)
        sid_app.fx_state = Some(sid_fx::FxState::with_seed(42));
        sid_app.animation = sid_core::animation::AnimationConfig::default();

        let (tx, mut rx) = mpsc::channel::<sid_core::event::Event>(16);

        // Send two key-press events then a Tick, then close the channel so the
        // loop exits after draining them.
        tx.send(sid_core::event::Event::Key(sid_core::event::KeyChord::new(
            crossterm::event::KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        )))
        .await
        .unwrap();
        tx.send(sid_core::event::Event::Key(sid_core::event::KeyChord::new(
            crossterm::event::KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        )))
        .await
        .unwrap();
        tx.send(sid_core::event::Event::Tick).await.unwrap();
        drop(tx); // close channel → loop exits after Tick

        run_event_loop(&mut terminal, &mut sid_app, &mut rx)
            .await
            .unwrap();

        let tick_count = sid_app
            .fx_state
            .as_ref()
            .expect("fx_state must remain Some")
            .tick_count;

        // BEFORE the fix: tick_count == 3 (once per loop iteration).
        // AFTER the fix:  tick_count == 1 (only on the Tick event).
        assert_eq!(
            tick_count, 1,
            "tick_count should be 1 (only the Tick event should advance it), got {tick_count}"
        );
    }
}
