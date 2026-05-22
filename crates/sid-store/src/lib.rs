//! Domain-shaped storage trait. `RedbStore` is the v1 implementation.
//! Domain types here; impl details in `redb_impl.rs`.
//!
//! # Examples
//!
//! Opening and using the store (requires a filesystem path — see the
//! integration tests in `crates/sid-store/tests/` for runnable examples).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sid_core::SidError;
use sid_core::tab::TabId;
use sid_core::widget::WidgetId;

pub mod codec;
pub mod keybind_load;
pub mod redb_impl;
pub mod schema;
pub mod sid_toml;

pub use redb_impl::RedbStore;

/// Canonical keys for settings persisted in the `settings` table.
///
/// Centralised so the Settings widget, the `sid settings get/set` CLI, and the
/// reset-to-defaults flow agree on the names byte-for-byte.
///
/// # Examples
///
/// ```
/// use sid_store::settings_keys;
/// assert_eq!(settings_keys::THEME_NAME, "theme_name");
/// assert_eq!(settings_keys::KEYBIND_PROFILE_NAME, "keybind_profile_name");
/// assert_eq!(settings_keys::WORKSPACE_ROOTS, "workspace_roots");
/// assert_eq!(settings_keys::PERSIST_DEBOUNCE_MS, "persist_debounce_ms");
/// assert_eq!(settings_keys::HEARTBEAT_INTERVAL_SECS, "heartbeat_interval_secs");
/// assert_eq!(settings_keys::AUTO_RESTORE_SESSION, "auto_restore_session");
/// assert_eq!(settings_keys::AUTO_SCAN_WORKSPACES, "auto_scan_workspaces");
/// assert_eq!(settings_keys::DEFAULT_TAB, "default_tab");
/// assert_eq!(settings_keys::SETTINGS_FOCUSED_CATEGORY, "settings_focused_category");
/// ```
pub mod settings_keys {
    /// Canonical name of the active theme.
    pub const THEME_NAME: &str = "theme_name";
    /// Canonical name of the active keybind profile (postcard blob lives in the
    /// `keybinds` table; this setting names which profile is loaded).
    pub const KEYBIND_PROFILE_NAME: &str = "keybind_profile_name";
    /// JSON-encoded list of absolute workspace roots that the discovery walker
    /// should consider as scan origins.
    pub const WORKSPACE_ROOTS: &str = "workspace_roots";
    /// Debounce window (milliseconds) for `StatePersister` flushes.
    pub const PERSIST_DEBOUNCE_MS: &str = "persist_debounce_ms";
    /// Heartbeat interval (seconds) for the detached session writer.
    pub const HEARTBEAT_INTERVAL_SECS: &str = "heartbeat_interval_secs";
    /// Whether the previous session should be restored on startup.
    pub const AUTO_RESTORE_SESSION: &str = "auto_restore_session";
    /// Whether workspace discovery runs automatically on startup.
    pub const AUTO_SCAN_WORKSPACES: &str = "auto_scan_workspaces";
    /// Tab id (string) to land on when sid starts with no prior session.
    pub const DEFAULT_TAB: &str = "default_tab";
    /// Settings widget — last-focused sub-category (string id).
    pub const SETTINGS_FOCUSED_CATEGORY: &str = "settings_focused_category";
}

/// String-typed setting helpers. Default impls call [`Store::get_setting`] /
/// [`Store::put_setting`] and store the value as UTF-8 bytes so that
/// `sid settings get` can dump them without invoking the codec.
///
/// All accessors return `Ok(None)` when a key is unset — they never fabricate
/// defaults; defaulting is a widget-level concern.
///
/// # Examples
///
/// ```
/// use sid_store::{settings_keys, OpenStore, RedbStore, TypedSettings};
/// use tempfile::tempdir;
///
/// let dir = tempdir().unwrap();
/// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
/// store.put_string(settings_keys::THEME_NAME, "cosmos").unwrap();
/// assert_eq!(
///     store.get_string(settings_keys::THEME_NAME).unwrap().as_deref(),
///     Some("cosmos"),
/// );
/// ```
pub trait TypedSettings: Store {
    /// Read a UTF-8 string-typed setting. `Ok(None)` if unset.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, TypedSettings};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.get_string("theme_name").unwrap().is_none());
    /// store.put_string("theme_name", "cosmos").unwrap();
    /// assert_eq!(store.get_string("theme_name").unwrap().as_deref(), Some("cosmos"));
    /// ```
    fn get_string(&self, key: &str) -> Result<Option<String>, SidError> {
        match self.get_setting(key)? {
            None => Ok(None),
            Some(v) => Ok(Some(String::from_utf8(v.0).map_err(|e| {
                SidError::Storage(format!("non-utf8 setting '{key}': {e}"))
            })?)),
        }
    }

    /// Write a UTF-8 string-typed setting.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, TypedSettings};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.put_string("k", "v").unwrap();
    /// assert_eq!(store.get_string("k").unwrap().as_deref(), Some("v"));
    /// ```
    fn put_string(&self, key: &str, val: &str) -> Result<(), SidError> {
        self.put_setting(key, &SettingValue(val.as_bytes().to_vec()))
    }

    /// Read a `u64`-typed setting (UTF-8 ASCII decimal). `Ok(None)` if unset.
    ///
    /// # Errors
    ///
    /// Returns `SidError::Storage` if the bytes are not valid UTF-8 or do not
    /// parse as a `u64`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, TypedSettings};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.put_u64("persist_debounce_ms", 250).unwrap();
    /// assert_eq!(store.get_u64("persist_debounce_ms").unwrap(), Some(250));
    /// ```
    fn get_u64(&self, key: &str) -> Result<Option<u64>, SidError> {
        match self.get_setting(key)? {
            None => Ok(None),
            Some(v) => {
                let s = std::str::from_utf8(&v.0)
                    .map_err(|e| SidError::Storage(format!("non-utf8 u64 '{key}': {e}")))?;
                let parsed = s.parse::<u64>().map_err(|e| {
                    SidError::Storage(format!("invalid u64 '{s}' for '{key}': {e}"))
                })?;
                Ok(Some(parsed))
            }
        }
    }

    /// Write a `u64`-typed setting, encoded as UTF-8 decimal bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, TypedSettings};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.put_u64("k", 42).unwrap();
    /// assert_eq!(store.get_u64("k").unwrap(), Some(42));
    /// ```
    fn put_u64(&self, key: &str, val: u64) -> Result<(), SidError> {
        self.put_string(key, &val.to_string())
    }

    /// Read a `bool`-typed setting (either `"true"` or `"false"`). `Ok(None)`
    /// if unset.
    ///
    /// # Errors
    ///
    /// Returns `SidError::Storage` if the bytes are not exactly `"true"` or
    /// `"false"`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, TypedSettings};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.put_bool("auto_restore_session", true).unwrap();
    /// assert_eq!(store.get_bool("auto_restore_session").unwrap(), Some(true));
    /// ```
    fn get_bool(&self, key: &str) -> Result<Option<bool>, SidError> {
        match self.get_string(key)? {
            None => Ok(None),
            Some(s) => match s.as_str() {
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                other => Err(SidError::Storage(format!(
                    "invalid bool '{other}' for key '{key}'"
                ))),
            },
        }
    }

    /// Write a `bool`-typed setting as `"true"` or `"false"` bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, TypedSettings};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.put_bool("k", false).unwrap();
    /// assert_eq!(store.get_bool("k").unwrap(), Some(false));
    /// ```
    fn put_bool(&self, key: &str, val: bool) -> Result<(), SidError> {
        self.put_string(key, if val { "true" } else { "false" })
    }
}

impl<S: Store + ?Sized> TypedSettings for S {}

/// Wall-clock instant as nanoseconds since UNIX epoch. Used for ordering.
///
/// # Examples
///
/// ```
/// use sid_store::now_epoch;
/// let t = now_epoch();
/// // Epoch is always a positive value in normal conditions.
/// assert!(t > 0);
/// ```
pub type Epoch = u64;

/// Returns the current wall-clock time as nanoseconds since UNIX epoch.
///
/// Returns `0` only if the system clock is before the UNIX epoch (unlikely in
/// practice; treated as a safe fallback).
///
/// # Examples
///
/// ```
/// use sid_store::now_epoch;
/// let t1 = now_epoch();
/// let t2 = now_epoch();
/// // Time is monotonically non-decreasing.
/// assert!(t2 >= t1);
/// ```
pub fn now_epoch() -> Epoch {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// An opaque settings value stored as raw bytes.
///
/// # Examples
///
/// ```
/// use sid_store::SettingValue;
/// let v = SettingValue(b"cosmos".to_vec());
/// assert_eq!(v.0, b"cosmos");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingValue(pub Vec<u8>);

/// A persisted session record.
///
/// # Examples
///
/// ```
/// use sid_store::{now_epoch, SessionRecord};
///
/// let r = SessionRecord {
///     id: "sess-1".into(),
///     started_at: now_epoch(),
///     last_active: now_epoch(),
///     ended_at: None,
///     active_tab: None,
///     open_tabs: vec![],
/// };
/// assert_eq!(r.id, "sess-1");
/// assert!(r.ended_at.is_none());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub started_at: Epoch,
    pub last_active: Epoch,
    pub ended_at: Option<Epoch>,
    pub active_tab: Option<TabId>,
    pub open_tabs: Vec<TabId>,
}

/// Widget UI state blob keyed by `(tab_id, widget_id)`.
///
/// # Examples
///
/// ```
/// use sid_core::tab::TabId;
/// use sid_core::widget::WidgetId;
/// use sid_store::WidgetState;
///
/// let ws = WidgetState {
///     tab_id: TabId::new("workspaces"),
///     widget_id: WidgetId::new("workspaces.root"),
///     blob: vec![1, 2, 3],
/// };
/// assert_eq!(ws.blob.len(), 3);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetState {
    pub tab_id: TabId,
    pub widget_id: WidgetId,
    pub blob: Vec<u8>,
}

// ─── Workspace domain types ──────────────────────────────────────────────────

/// Re-export `WorkspaceKind` from `sid-core` for consumers of `sid-store`
/// who need the type without a direct `sid-core` dep.
///
/// # Examples
///
/// ```
/// use sid_store::WorkspaceKind;
///
/// let kind = WorkspaceKind::Repo;
/// assert_eq!(kind, WorkspaceKind::Repo);
/// assert_ne!(kind, WorkspaceKind::Umbrella);
/// ```
pub use sid_core::workspace_metadata::WorkspaceKind;

/// A workspace registered in the sid registry.
///
/// Workspaces are keyed by their absolute filesystem path. The `kind` field
/// classifies whether this is a plain git repo, an umbrella, or other.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_store::{Workspace, WorkspaceKind, now_epoch};
///
/// let w = Workspace {
///     path: PathBuf::from("/home/user/vcs/myproject"),
///     name: "myproject".into(),
///     kind: WorkspaceKind::Repo,
///     manifest_hash: 0,
///     last_seen: now_epoch(),
///     parent: None,
/// };
/// assert_eq!(w.name, "myproject");
/// assert!(w.parent.is_none());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Absolute path. Acts as the primary key.
    pub path: PathBuf,
    /// Human-readable name (usually the directory basename).
    pub name: String,
    /// Classification of this workspace.
    pub kind: WorkspaceKind,
    /// Fast cache-invalidation hint for manifest files (computed via xxhash3).
    /// `0` means "not computed / unknown".
    pub manifest_hash: u64,
    /// Wall-clock nanoseconds when this workspace was last observed on disk.
    pub last_seen: Epoch,
    /// For child workspaces of an umbrella, the parent's absolute path.
    pub parent: Option<PathBuf>,
}

// ─── Theme / keybind / quick-action domain types ─────────────────────────────

/// A theme stored in the `themes` table.
///
/// The palette + glyphs are the same shape `sid_ui::theme::Theme` carries; we
/// redeclare here to avoid making `sid-store` depend on `sid-ui` (adapter
/// pattern: `sid-store` owns the on-disk shape only).
///
/// # Examples
///
/// ```
/// use sid_store::{ThemeGlyphs, ThemePalette, ThemeSpec};
/// let spec = ThemeSpec {
///     name: "cosmos".into(),
///     palette: ThemePalette {
///         background: 0x0F1020, surface: 0x1A1B2E, foreground: 0xE3E4F1,
///         muted: 0x6E7090, accent_primary: 0x8F9CFF, accent_success: 0x6FCF97,
///         accent_warning: 0xE0C46C, accent_error: 0xE07A7A, border: 0x2D2E4A,
///     },
///     glyphs: ThemeGlyphs { star: '★', small_star: '·', dot: '•' },
/// };
/// assert_eq!(spec.name, "cosmos");
/// assert_eq!(spec.glyphs.star, '★');
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThemeSpec {
    /// Theme name (also acts as primary key in the `themes` table).
    pub name: String,
    /// RGB palette as packed `u32`s (`0x00RRGGBB`).
    pub palette: ThemePalette,
    /// Decorative glyphs (stars, dots) used in the cosmos aesthetic.
    pub glyphs: ThemeGlyphs,
}

/// RGB palette for a theme. Each colour is a packed `0x00RRGGBB` `u32`.
///
/// # Examples
///
/// ```
/// use sid_store::ThemePalette;
/// let p = ThemePalette {
///     background: 0x0F1020, surface: 0x1A1B2E, foreground: 0xE3E4F1,
///     muted: 0x6E7090, accent_primary: 0x8F9CFF, accent_success: 0x6FCF97,
///     accent_warning: 0xE0C46C, accent_error: 0xE07A7A, border: 0x2D2E4A,
/// };
/// assert_eq!(p.accent_primary, 0x8F9CFF);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThemePalette {
    /// Window background.
    pub background: u32,
    /// Surface / panel fill.
    pub surface: u32,
    /// Default foreground text colour.
    pub foreground: u32,
    /// Muted / secondary text colour.
    pub muted: u32,
    /// Primary accent (titles, focused borders).
    pub accent_primary: u32,
    /// Success accent (green-ish).
    pub accent_success: u32,
    /// Warning accent (amber).
    pub accent_warning: u32,
    /// Error accent (red).
    pub accent_error: u32,
    /// Border colour.
    pub border: u32,
}

/// Decorative glyphs for a theme.
///
/// # Examples
///
/// ```
/// use sid_store::ThemeGlyphs;
/// let g = ThemeGlyphs { star: '★', small_star: '·', dot: '•' };
/// assert_eq!(g.star, '★');
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThemeGlyphs {
    /// Large decorative star.
    pub star: char,
    /// Small star / dust speck.
    pub small_star: char,
    /// Bullet-point dot.
    pub dot: char,
}

/// A keybind profile stored in the `keybinds` table.
///
/// A profile is a vector of (chord-string, action-id) pairs. The chord string
/// format mirrors the `KeyChord` debug shape from `sid-core` so that any
/// crate can stringify/parse a chord without depending on a richer type here.
///
/// # Examples
///
/// ```
/// use sid_store::{KeybindEntry, KeybindProfile};
/// let p = KeybindProfile {
///     name: "default".into(),
///     bindings: vec![KeybindEntry { chord: "Char('q')|0".into(), action: "app.quit".into() }],
/// };
/// assert_eq!(p.bindings.len(), 1);
/// assert_eq!(p.bindings[0].action, "app.quit");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KeybindProfile {
    /// Profile name (primary key in the `keybinds` table).
    pub name: String,
    /// Chord → action bindings, in user-presentation order.
    pub bindings: Vec<KeybindEntry>,
}

/// One row in a [`KeybindProfile`]: a chord string and the action id it fires.
///
/// # Examples
///
/// ```
/// use sid_store::KeybindEntry;
/// let e = KeybindEntry { chord: "Char('?')|0".into(), action: "app.help".into() };
/// assert_eq!(e.action, "app.help");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KeybindEntry {
    /// Stringified chord (e.g. `"Char('q')|0"`).
    pub chord: String,
    /// Action id (e.g. `"app.quit"`).
    pub action: String,
}

/// A user-pinned configuration file path with display label and optional
/// custom opener command. The default opener is the external terminal spawner
/// cd'd into the parent dir running `$EDITOR <file>`.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_store::{now_epoch, PinnedConfig};
/// let p = PinnedConfig {
///     path: PathBuf::from("/etc/nginx/nginx.conf"),
///     label: "nginx config".into(),
///     opener_cmd: None,
///     created_at: now_epoch(),
/// };
/// assert_eq!(p.label, "nginx config");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PinnedConfig {
    /// Absolute path. Acts as the primary key in [`schema::PINNED_CONFIGS`].
    pub path: PathBuf,
    /// Display label for the System tab list.
    pub label: String,
    /// Override default opener. `None` = use the binary's [`TerminalSpawner`] default.
    ///
    /// [`TerminalSpawner`]: sid_core::adapters::terminal_spawner::TerminalSpawner
    pub opener_cmd: Option<String>,
    /// Creation timestamp (sortable display).
    pub created_at: Epoch,
}

/// A global quick-action. Shared between Plan 6 (System tab) and Plan 7
/// (Settings tab editor).
///
/// # Examples
///
/// ```
/// use sid_store::{QuickAction, QuickActionScope};
/// let a = QuickAction {
///     id: "qa.reload".into(),
///     label: "Reload".into(),
///     cmd: "sid reload".into(),
///     keybind: Some("Char('r')|2".into()),
///     scope: QuickActionScope::Global,
/// };
/// assert_eq!(a.scope, QuickActionScope::Global);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QuickAction {
    /// Stable id (primary key).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Shell command (or `sid` subcommand) to run.
    pub cmd: String,
    /// Optional chord (string format matches [`KeybindEntry::chord`]).
    pub keybind: Option<String>,
    /// Scope of this action.
    pub scope: QuickActionScope,
}

impl QuickAction {
    /// Generate a fresh action id of the form `qa-<14 lowercase hex>`.
    ///
    /// Mixes the wall-clock nanoseconds with the current process id to
    /// reduce the chance of collision across rapid parallel calls. The
    /// result is **not** cryptographically random — it is a primary-key
    /// hint, suitable for redb keys.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::QuickAction;
    /// let a = QuickAction::new_id();
    /// assert!(a.starts_with("qa-"));
    /// assert_eq!(a.len(), 3 + 14);
    /// ```
    pub fn new_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let mixed = nanos ^ (pid.wrapping_mul(2_654_435_761));
        let masked = (mixed & 0x00FF_FFFF_FFFF_FFFF) as u64;
        format!("qa-{masked:014x}")
    }
}

/// Scope a [`QuickAction`] applies to.
///
/// # Examples
///
/// ```
/// use sid_store::QuickActionScope;
/// assert_ne!(QuickActionScope::Global, QuickActionScope::Workspace);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum QuickActionScope {
    /// Visible everywhere.
    Global,
    /// Visible only when a workspace is active.
    Workspace,
}

// ─────────────────────────────────────────────────────────────────────────────

/// A saved database connection (Plan 4 / Database tab).
///
/// The DSN excludes the password; the password (if any) lives in the secrets
/// table via `secret_ref`.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::DbKind;
/// use sid_core::adapters::secrets::SecretId;
/// use sid_store::{DbConnection, now_epoch};
/// let c = DbConnection {
///     id: "local-pg".into(),
///     kind: DbKind::Postgres,
///     name: "local postgres".into(),
///     dsn: "postgres://user@localhost/db".into(),
///     secret_ref: Some(SecretId::new("db.local-pg.password")),
///     created_at: now_epoch(),
/// };
/// assert_eq!(c.kind, DbKind::Postgres);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbConnection {
    /// Stable id; used as the redb key and as the CLI selector.
    pub id: String,
    /// Database kind discriminator (Postgres vs SQLite).
    pub kind: sid_core::adapters::db_client::DbKind,
    /// User-facing label.
    pub name: String,
    /// DSN minus password. Password lives behind `secret_ref`.
    pub dsn: String,
    /// Optional pointer into the secrets table for the password.
    pub secret_ref: Option<sid_core::adapters::secrets::SecretId>,
    /// Epoch timestamp at creation.
    pub created_at: Epoch,
}

/// One row of the per-connection query history.
///
/// # Examples
///
/// ```
/// use sid_store::QueryRecord;
/// let r = QueryRecord {
///     conn_id: "local-pg".into(),
///     sql: "SELECT 1".into(),
///     duration_ms: 12,
///     row_count: 1,
///     ts_ns: 1,
/// };
/// assert_eq!(r.row_count, 1);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryRecord {
    /// Connection id this query ran against.
    pub conn_id: String,
    /// Verbatim SQL text.
    pub sql: String,
    /// Wall-clock duration of the query in milliseconds.
    pub duration_ms: u64,
    /// Approximate row count (0 for DDL/DML rather than SELECT).
    pub row_count: u64,
    /// Wall-clock nanoseconds since epoch — also the first half of the key.
    pub ts_ns: u128,
}

/// A plaintext secret (Plan 4). Stored versioned in the `secrets` table when
/// using [`crate::Store::put_secret`].
///
/// # Examples
///
/// ```
/// use sid_store::PlainSecret;
/// let s = PlainSecret { value: "shh".into() };
/// assert_eq!(s.value, "shh");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlainSecret {
    /// Secret value as a UTF-8 string.
    pub value: String,
}

/// Source of an SSH host entry.
///
/// # Examples
///
/// ```
/// use sid_store::SshHostSource;
/// let _ = SshHostSource::Manual;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SshHostSource {
    /// Sourced from `~/.ssh/config` (read-only).
    SshConfig,
    /// Added by the user via `sid ssh add` or the SSH tab UI.
    Manual,
}

/// Preferred authentication method for an SSH host.
///
/// Used by the SSH widget's connect flow to pick which auth method to try
/// first. Stored on disk as part of [`SshHost`] (v3+). Older records (v1/v2)
/// migrate to [`SshAuthKind::Agent`] which works without any explicit
/// configuration on most modern setups.
///
/// # Examples
///
/// ```
/// use sid_store::SshAuthKind;
/// assert_eq!(SshAuthKind::default(), SshAuthKind::Agent);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum SshAuthKind {
    /// Public-key auth using the host's `identity_file`.
    Key,
    /// Password (prompted at connect time, never stored on disk via this
    /// field — passwords live in the secrets table).
    Password,
    /// SSH agent forwarding. Default for backward-compatible records that
    /// were written before this field existed.
    #[default]
    Agent,
}

/// A registered SSH host. The `alias` is the primary key.
///
/// # Versioning
///
/// On-disk format is `[version: u8][postcard-encoded payload]`. Older
/// versions are migrated forward on read by [`decode_ssh_host`]:
/// - v1 records lack `last_sftp_path` (added in v2) and `auth_kind`
///   (added in v3). Both default to `None` / [`SshAuthKind::Agent`].
/// - v2 records lack `auth_kind` only; same default.
///
/// New records are always written at the current version.
///
/// # Examples
///
/// ```
/// use sid_store::{SshAuthKind, SshHost, SshHostSource};
/// let h = SshHost {
///     alias: "dev".into(),
///     host: "10.0.0.1".into(),
///     port: 22,
///     user: "pi".into(),
///     identity_file: None,
///     source: SshHostSource::Manual,
///     last_connected: 0,
///     command_history: Vec::new(),
///     last_sftp_path: None,
///     auth_kind: SshAuthKind::Agent,
/// };
/// assert_eq!(h.alias, "dev");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshHost {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
    pub source: SshHostSource,
    pub last_connected: Epoch,
    pub command_history: Vec<String>,
    /// Last remote path browsed via the SFTP panel for this host (Phase 5,
    /// `F` / SFTP persist). `None` for new hosts and for records written by
    /// pre-v2 versions of sid.
    pub last_sftp_path: Option<String>,
    /// Preferred authentication method. Persisted from the Add/Edit modal's
    /// `auth` choice. Records from pre-v3 versions migrate to
    /// [`SshAuthKind::Agent`].
    pub auth_kind: SshAuthKind,
}

/// Current on-disk version for [`SshHost`] records.
/// - v1 → v2: added `last_sftp_path`.
/// - v2 → v3: added `auth_kind`.
pub const SSH_HOST_VERSION: u8 = 3;

/// V1 wire layout — pre-`last_sftp_path`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SshHostV1 {
    alias: String,
    host: String,
    port: u16,
    user: String,
    identity_file: Option<String>,
    source: SshHostSource,
    last_connected: Epoch,
    command_history: Vec<String>,
}

impl From<SshHostV1> for SshHost {
    fn from(v: SshHostV1) -> Self {
        SshHost {
            alias: v.alias,
            host: v.host,
            port: v.port,
            user: v.user,
            identity_file: v.identity_file,
            source: v.source,
            last_connected: v.last_connected,
            command_history: v.command_history,
            last_sftp_path: None,
            auth_kind: SshAuthKind::Agent,
        }
    }
}

/// V2 wire layout — pre-`auth_kind`, but with `last_sftp_path`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SshHostV2 {
    alias: String,
    host: String,
    port: u16,
    user: String,
    identity_file: Option<String>,
    source: SshHostSource,
    last_connected: Epoch,
    command_history: Vec<String>,
    last_sftp_path: Option<String>,
}

impl From<SshHostV2> for SshHost {
    fn from(v: SshHostV2) -> Self {
        SshHost {
            alias: v.alias,
            host: v.host,
            port: v.port,
            user: v.user,
            identity_file: v.identity_file,
            source: v.source,
            last_connected: v.last_connected,
            command_history: v.command_history,
            last_sftp_path: v.last_sftp_path,
            auth_kind: SshAuthKind::Agent,
        }
    }
}

/// Decode an on-disk [`SshHost`] record, transparently migrating version-1
/// blobs (no `last_sftp_path` field) into the current shape with that field
/// set to `None`. Unknown versions are reported as a storage error.
///
/// # Errors
///
/// - `SidError::Storage` for empty payloads.
/// - `SidError::Storage` for unknown version bytes.
/// - `SidError::Storage` for malformed payloads at known versions.
///
/// # Examples
///
/// Round-tripping a current (v2) record:
/// ```
/// use sid_store::{SshAuthKind, SshHost, SshHostSource, SSH_HOST_VERSION, decode_ssh_host};
/// use sid_store::codec::encode_versioned;
/// let h = SshHost {
///     alias: "a".into(),
///     host: "h".into(),
///     port: 22,
///     user: "u".into(),
///     identity_file: None,
///     source: SshHostSource::Manual,
///     last_connected: 0,
///     command_history: vec![],
///     last_sftp_path: Some("/tmp".into()),
///     auth_kind: SshAuthKind::Agent,
/// };
/// let bytes = encode_versioned(SSH_HOST_VERSION, &h).unwrap();
/// let back = decode_ssh_host(&bytes).unwrap();
/// assert_eq!(back, h);
/// ```
pub fn decode_ssh_host(bytes: &[u8]) -> Result<SshHost, SidError> {
    let (&v, rest) = bytes
        .split_first()
        .ok_or_else(|| SidError::Storage("empty ssh_host payload".into()))?;
    match v {
        1 => {
            let old: SshHostV1 = postcard::from_bytes(rest)
                .map_err(|e| SidError::Storage(format!("ssh_host v1 decode: {e}")))?;
            Ok(old.into())
        }
        2 => {
            let mid: SshHostV2 = postcard::from_bytes(rest)
                .map_err(|e| SidError::Storage(format!("ssh_host v2 decode: {e}")))?;
            Ok(mid.into())
        }
        3 => {
            let cur: SshHost = postcard::from_bytes(rest)
                .map_err(|e| SidError::Storage(format!("ssh_host v3 decode: {e}")))?;
            Ok(cur)
        }
        other => Err(SidError::Storage(format!(
            "unknown ssh_host version: {other}"
        ))),
    }
}

/// The domain storage trait. `sid-store` is the only crate that provides an
/// implementation (`RedbStore`); other crates depend on this trait only.
///
/// All methods take `&self` (interior mutability lives inside the impl via
/// `redb`'s transaction model).
///
/// # Examples
///
/// Implementing the trait for a test double:
///
/// ```
/// use std::collections::HashMap;
/// use std::path::Path;
/// use std::sync::Mutex;
/// use sid_core::SidError;
/// use sid_core::tab::TabId;
/// use sid_core::widget::WidgetId;
/// use sid_store::{
///     Epoch, KeybindProfile, QuickAction, SessionRecord, SettingValue, Store, ThemeSpec,
///     Workspace, WidgetState,
/// };
///
/// struct MemStore {
///     settings: Mutex<HashMap<String, SettingValue>>,
/// }
///
/// impl Store for MemStore {
///     fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError> {
///         Ok(self.settings.lock().unwrap().get(key).cloned())
///     }
///     fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError> {
///         self.settings.lock().unwrap().insert(key.to_string(), val.clone());
///         Ok(())
///     }
///     fn delete_setting(&self, key: &str) -> Result<bool, SidError> {
///         Ok(self.settings.lock().unwrap().remove(key).is_some())
///     }
///     fn list_setting_keys(&self) -> Result<Vec<String>, SidError> {
///         Ok(self.settings.lock().unwrap().keys().cloned().collect())
///     }
///     fn current_session(&self) -> Result<Option<SessionRecord>, SidError> { Ok(None) }
///     fn upsert_session(&self, _: &SessionRecord) -> Result<(), SidError> { Ok(()) }
///     fn end_session(&self, _: &str, _: Epoch) -> Result<(), SidError> { Ok(()) }
///     fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError> { Ok(vec![]) }
///     fn save_widget_state(&self, _: &WidgetState) -> Result<(), SidError> { Ok(()) }
///     fn load_widget_state(&self, _: &TabId, _: &WidgetId) -> Result<Option<Vec<u8>>, SidError> { Ok(None) }
///     fn list_workspaces(&self) -> Result<Vec<Workspace>, SidError> { Ok(vec![]) }
///     fn upsert_workspace(&self, _: &Workspace) -> Result<(), SidError> { Ok(()) }
///     fn get_workspace(&self, _: &Path) -> Result<Option<Workspace>, SidError> { Ok(None) }
///     fn remove_workspace(&self, _: &Path) -> Result<(), SidError> { Ok(()) }
///     fn secret_put(&self, _: &str, _: &[u8]) -> Result<(), SidError> { Ok(()) }
///     fn secret_get(&self, _: &str) -> Result<Option<Vec<u8>>, SidError> { Ok(None) }
///     fn secret_delete(&self, _: &str) -> Result<(), SidError> { Ok(()) }
///     fn list_secret_ids(&self) -> Result<Vec<String>, SidError> { Ok(vec![]) }
///     fn list_themes(&self) -> Result<Vec<ThemeSpec>, SidError> { Ok(vec![]) }
///     fn get_theme(&self, _: &str) -> Result<Option<ThemeSpec>, SidError> { Ok(None) }
///     fn upsert_theme(&self, _: &ThemeSpec) -> Result<(), SidError> { Ok(()) }
///     fn remove_theme(&self, _: &str) -> Result<(), SidError> { Ok(()) }
///     fn list_keybind_profiles(&self) -> Result<Vec<KeybindProfile>, SidError> { Ok(vec![]) }
///     fn get_keybind_profile(&self, _: &str) -> Result<Option<KeybindProfile>, SidError> { Ok(None) }
///     fn upsert_keybind_profile(&self, _: &KeybindProfile) -> Result<(), SidError> { Ok(()) }
///     fn remove_keybind_profile(&self, _: &str) -> Result<(), SidError> { Ok(()) }
///     fn list_quick_actions(&self) -> Result<Vec<QuickAction>, SidError> { Ok(vec![]) }
///     fn get_quick_action(&self, _: &str) -> Result<Option<QuickAction>, SidError> { Ok(None) }
///     fn upsert_quick_action(&self, _: &QuickAction) -> Result<(), SidError> { Ok(()) }
///     fn remove_quick_action(&self, _: &str) -> Result<(), SidError> { Ok(()) }
///     fn list_pinned_configs(&self) -> Result<Vec<sid_store::PinnedConfig>, SidError> { Ok(vec![]) }
///     fn upsert_pinned_config(&self, _: &sid_store::PinnedConfig) -> Result<(), SidError> { Ok(()) }
///     fn get_pinned_config(&self, _: &Path) -> Result<Option<sid_store::PinnedConfig>, SidError> { Ok(None) }
///     fn remove_pinned_config(&self, _: &Path) -> Result<(), SidError> { Ok(()) }
///     fn list_db_connections(&self) -> Result<Vec<sid_store::DbConnection>, SidError> { Ok(vec![]) }
///     fn upsert_db_connection(&self, _: &sid_store::DbConnection) -> Result<(), SidError> { Ok(()) }
///     fn get_db_connection(&self, _: &str) -> Result<Option<sid_store::DbConnection>, SidError> { Ok(None) }
///     fn remove_db_connection(&self, _: &str) -> Result<(), SidError> { Ok(()) }
///     fn append_query_record(&self, _: &sid_store::QueryRecord) -> Result<(), SidError> { Ok(()) }
///     fn recent_queries(&self, _: &str, _: usize) -> Result<Vec<sid_store::QueryRecord>, SidError> { Ok(vec![]) }
///     fn list_ssh_hosts(&self) -> Result<Vec<sid_store::SshHost>, SidError> { Ok(vec![]) }
///     fn upsert_ssh_host(&self, _: &sid_store::SshHost) -> Result<(), SidError> { Ok(()) }
///     fn get_ssh_host(&self, _: &str) -> Result<Option<sid_store::SshHost>, SidError> { Ok(None) }
///     fn remove_ssh_host(&self, _: &str) -> Result<(), SidError> { Ok(()) }
/// }
/// ```
pub trait Store: Send + Sync {
    /// Retrieve a setting value by key. Returns `None` if not set.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, SettingValue, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// // Missing key returns None.
    /// assert!(store.get_setting("theme").unwrap().is_none());
    /// // After a put, the value is returned.
    /// store.put_setting("theme", &SettingValue(b"cosmos".to_vec())).unwrap();
    /// assert_eq!(store.get_setting("theme").unwrap().unwrap().0, b"cosmos");
    /// ```
    fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError>;

    /// Persist a setting value. Overwrites any existing value for the key.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, SettingValue, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.put_setting("key", &SettingValue(b"v1".to_vec())).unwrap();
    /// // Overwrite with a new value.
    /// store.put_setting("key", &SettingValue(b"v2".to_vec())).unwrap();
    /// assert_eq!(store.get_setting("key").unwrap().unwrap().0, b"v2");
    /// ```
    fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError>;

    /// List every setting key currently present in the `settings` table.
    ///
    /// Order is implementation-defined (redb returns keys in lexicographic
    /// order).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, SettingValue, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_setting_keys().unwrap().is_empty());
    /// store.put_setting("k", &SettingValue(b"v".to_vec())).unwrap();
    /// assert_eq!(store.list_setting_keys().unwrap(), vec!["k".to_string()]);
    /// ```
    fn list_setting_keys(&self) -> Result<Vec<String>, SidError>;

    /// Delete a setting by key. Returns `Ok(true)` if a value was removed and
    /// `Ok(false)` if the key did not exist. Idempotent.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, SettingValue, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.put_setting("k", &SettingValue(b"v".to_vec())).unwrap();
    /// assert!(store.delete_setting("k").unwrap());
    /// assert!(store.get_setting("k").unwrap().is_none());
    /// // Idempotent.
    /// assert!(!store.delete_setting("k").unwrap());
    /// ```
    fn delete_setting(&self, key: &str) -> Result<bool, SidError>;

    /// Retrieve the most recently active session, if any.
    ///
    /// Returns `None` if no session has ever been upserted.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.current_session().unwrap().is_none());
    ///
    /// let s = SessionRecord {
    ///     id: "s1".into(),
    ///     started_at: now_epoch(),
    ///     last_active: now_epoch(),
    ///     ended_at: None,
    ///     active_tab: None,
    ///     open_tabs: vec![],
    /// };
    /// store.upsert_session(&s).unwrap();
    /// assert_eq!(store.current_session().unwrap().unwrap().id, "s1");
    /// ```
    fn current_session(&self) -> Result<Option<SessionRecord>, SidError>;

    /// Create or update a session record. Also updates the "current" pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let s = SessionRecord {
    ///     id: "sess".into(),
    ///     started_at: now_epoch(),
    ///     last_active: now_epoch(),
    ///     ended_at: None,
    ///     active_tab: None,
    ///     open_tabs: vec![],
    /// };
    /// store.upsert_session(&s).unwrap();
    /// assert_eq!(store.list_sessions().unwrap().len(), 1);
    /// ```
    fn upsert_session(&self, s: &SessionRecord) -> Result<(), SidError>;

    /// Mark a session as ended at the given epoch timestamp.
    ///
    /// No-op if the session id does not exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let s = SessionRecord {
    ///     id: "s".into(),
    ///     started_at: 1,
    ///     last_active: 2,
    ///     ended_at: None,
    ///     active_tab: None,
    ///     open_tabs: vec![],
    /// };
    /// store.upsert_session(&s).unwrap();
    /// store.end_session("s", 999).unwrap();
    /// let sessions = store.list_sessions().unwrap();
    /// assert_eq!(sessions[0].ended_at, Some(999));
    /// // Calling on a nonexistent id is a no-op.
    /// store.end_session("no-such-id", 0).unwrap();
    /// ```
    fn end_session(&self, id: &str, ended_at: Epoch) -> Result<(), SidError>;

    /// Return all stored sessions.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_sessions().unwrap().is_empty());
    ///
    /// for id in &["a", "b", "c"] {
    ///     store.upsert_session(&SessionRecord {
    ///         id: id.to_string(),
    ///         started_at: 0,
    ///         last_active: 0,
    ///         ended_at: None,
    ///         active_tab: None,
    ///         open_tabs: vec![],
    ///     }).unwrap();
    /// }
    /// assert_eq!(store.list_sessions().unwrap().len(), 3);
    /// ```
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError>;

    /// Persist widget UI state blob for the given `(tab_id, widget_id)` pair.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::tab::TabId;
    /// use sid_core::widget::WidgetId;
    /// use sid_store::{OpenStore, RedbStore, Store, WidgetState};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let state = WidgetState {
    ///     tab_id: TabId::new("workspaces"),
    ///     widget_id: WidgetId::new("workspaces.root"),
    ///     blob: vec![1, 2, 3],
    /// };
    /// store.save_widget_state(&state).unwrap();
    /// ```
    fn save_widget_state(&self, s: &WidgetState) -> Result<(), SidError>;

    /// Load widget UI state blob. Returns `None` if never saved.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::tab::TabId;
    /// use sid_core::widget::WidgetId;
    /// use sid_store::{OpenStore, RedbStore, Store, WidgetState};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let tab = TabId::new("ssh");
    /// let widget = WidgetId::new("ssh.root");
    /// // No state saved yet.
    /// assert!(store.load_widget_state(&tab, &widget).unwrap().is_none());
    ///
    /// store.save_widget_state(&WidgetState {
    ///     tab_id: tab.clone(),
    ///     widget_id: widget.clone(),
    ///     blob: vec![42, 43],
    /// }).unwrap();
    /// assert_eq!(
    ///     store.load_widget_state(&tab, &widget).unwrap().unwrap(),
    ///     vec![42, 43]
    /// );
    /// ```
    fn load_widget_state(
        &self,
        tab: &TabId,
        widget: &WidgetId,
    ) -> Result<Option<Vec<u8>>, SidError>;

    /// Return all registered workspaces.
    ///
    /// Order is implementation-defined (redb returns keys in lexicographic order).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_workspaces().unwrap().is_empty());
    /// ```
    fn list_workspaces(&self) -> Result<Vec<Workspace>, SidError>;

    /// Insert or replace the workspace record keyed by `w.path`.
    ///
    /// If a workspace with the same path already exists it is fully replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_store::{now_epoch, OpenStore, RedbStore, Store, Workspace, WorkspaceKind};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let w = Workspace {
    ///     path: PathBuf::from("/tmp/myrepo"),
    ///     name: "myrepo".into(),
    ///     kind: WorkspaceKind::Repo,
    ///     manifest_hash: 0,
    ///     last_seen: now_epoch(),
    ///     parent: None,
    /// };
    /// store.upsert_workspace(&w).unwrap();
    /// assert_eq!(store.list_workspaces().unwrap().len(), 1);
    /// ```
    fn upsert_workspace(&self, w: &Workspace) -> Result<(), SidError>;

    /// Retrieve a workspace by its absolute path. Returns `None` if not registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_store::{now_epoch, OpenStore, RedbStore, Store, Workspace, WorkspaceKind};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.get_workspace(&PathBuf::from("/missing")).unwrap().is_none());
    /// ```
    fn get_workspace(&self, path: &Path) -> Result<Option<Workspace>, SidError>;

    /// Remove the workspace at `path`. No-op if not registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// // Removing a nonexistent path is always a no-op.
    /// store.remove_workspace(&PathBuf::from("/not-there")).unwrap();
    /// ```
    fn remove_workspace(&self, path: &Path) -> Result<(), SidError>;

    /// Insert or replace the secret bytes under `id`. Empty values are valid.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.secret_put("ssh.key.foo", b"passphrase").unwrap();
    /// assert_eq!(
    ///     store.secret_get("ssh.key.foo").unwrap().unwrap(),
    ///     b"passphrase".to_vec()
    /// );
    /// ```
    fn secret_put(&self, id: &str, value: &[u8]) -> Result<(), SidError>;

    /// Retrieve the secret bytes stored under `id`. Returns `None` if absent.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.secret_get("missing.id").unwrap().is_none());
    /// ```
    fn secret_get(&self, id: &str) -> Result<Option<Vec<u8>>, SidError>;

    /// Remove the secret stored under `id`. Idempotent — removing a missing
    /// id returns `Ok(())`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.secret_put("api.token", b"hunter2").unwrap();
    /// store.secret_delete("api.token").unwrap();
    /// assert!(store.secret_get("api.token").unwrap().is_none());
    /// // Removing a nonexistent id is always a no-op.
    /// store.secret_delete("never.was").unwrap();
    /// ```
    fn secret_delete(&self, id: &str) -> Result<(), SidError>;

    /// List every secret id currently stored.
    ///
    /// Order is implementation-defined (redb returns keys in lexicographic
    /// order).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_secret_ids().unwrap().is_empty());
    /// store.secret_put("a", b"1").unwrap();
    /// store.secret_put("b", b"2").unwrap();
    /// let ids = store.list_secret_ids().unwrap();
    /// assert_eq!(ids.len(), 2);
    /// ```
    fn list_secret_ids(&self) -> Result<Vec<String>, SidError>;

    /// Return all stored themes.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_themes().unwrap().is_empty());
    /// ```
    fn list_themes(&self) -> Result<Vec<ThemeSpec>, SidError>;

    /// Get a stored theme by name. Returns `None` if not present.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.get_theme("missing").unwrap().is_none());
    /// ```
    fn get_theme(&self, name: &str) -> Result<Option<ThemeSpec>, SidError>;

    /// Insert or replace a theme keyed by its `name`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store, ThemeGlyphs, ThemePalette, ThemeSpec};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.upsert_theme(&ThemeSpec {
    ///     name: "t1".into(),
    ///     palette: ThemePalette {
    ///         background: 0, surface: 0, foreground: 0, muted: 0,
    ///         accent_primary: 0, accent_success: 0, accent_warning: 0,
    ///         accent_error: 0, border: 0,
    ///     },
    ///     glyphs: ThemeGlyphs { star: '*', small_star: '.', dot: '.' },
    /// }).unwrap();
    /// assert_eq!(store.list_themes().unwrap().len(), 1);
    /// ```
    fn upsert_theme(&self, t: &ThemeSpec) -> Result<(), SidError>;

    /// Remove a theme by name. Idempotent.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.remove_theme("never-existed").unwrap();
    /// ```
    fn remove_theme(&self, name: &str) -> Result<(), SidError>;

    /// Return all stored keybind profiles.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_keybind_profiles().unwrap().is_empty());
    /// ```
    fn list_keybind_profiles(&self) -> Result<Vec<KeybindProfile>, SidError>;

    /// Get a keybind profile by name. Returns `None` if not present.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.get_keybind_profile("default").unwrap().is_none());
    /// ```
    fn get_keybind_profile(&self, name: &str) -> Result<Option<KeybindProfile>, SidError>;

    /// Insert or replace a keybind profile keyed by its `name`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{KeybindEntry, KeybindProfile, OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.upsert_keybind_profile(&KeybindProfile {
    ///     name: "default".into(),
    ///     bindings: vec![KeybindEntry { chord: "Char('q')|0".into(), action: "app.quit".into() }],
    /// }).unwrap();
    /// assert_eq!(store.list_keybind_profiles().unwrap().len(), 1);
    /// ```
    fn upsert_keybind_profile(&self, p: &KeybindProfile) -> Result<(), SidError>;

    /// Remove a keybind profile by name. Idempotent.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.remove_keybind_profile("never").unwrap();
    /// ```
    fn remove_keybind_profile(&self, name: &str) -> Result<(), SidError>;

    /// Return all stored quick actions.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_quick_actions().unwrap().is_empty());
    /// ```
    fn list_quick_actions(&self) -> Result<Vec<QuickAction>, SidError>;

    /// Get a quick action by id. Returns `None` if not present.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.get_quick_action("qa.missing").unwrap().is_none());
    /// ```
    fn get_quick_action(&self, id: &str) -> Result<Option<QuickAction>, SidError>;

    /// Insert or replace a quick action keyed by its `id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, QuickAction, QuickActionScope, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.upsert_quick_action(&QuickAction {
    ///     id: "qa.x".into(), label: "X".into(), cmd: "echo x".into(),
    ///     keybind: None, scope: QuickActionScope::Global,
    /// }).unwrap();
    /// assert_eq!(store.list_quick_actions().unwrap().len(), 1);
    /// ```
    fn upsert_quick_action(&self, a: &QuickAction) -> Result<(), SidError>;

    /// Remove a quick action by id. Idempotent.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.remove_quick_action("never").unwrap();
    /// ```
    fn remove_quick_action(&self, id: &str) -> Result<(), SidError>;

    // ─── Pinned configs (Plan 6) ────────────────────────────────────────────

    /// List all pinned configs.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_pinned_configs().unwrap().is_empty());
    /// ```
    fn list_pinned_configs(&self) -> Result<Vec<PinnedConfig>, SidError>;

    /// Insert or replace a pinned config (keyed by `pc.path`).
    fn upsert_pinned_config(&self, pc: &PinnedConfig) -> Result<(), SidError>;

    /// Look up a pinned config by absolute path. Returns `None` if not present.
    fn get_pinned_config(&self, path: &Path) -> Result<Option<PinnedConfig>, SidError>;

    /// Remove a pinned config by absolute path. No-op if absent.
    fn remove_pinned_config(&self, path: &Path) -> Result<(), SidError>;

    /// List all saved DB connections (Plan 4 / Database tab).
    ///
    /// Returns an empty vector if none are present.
    fn list_db_connections(&self) -> Result<Vec<DbConnection>, SidError>;

    /// Insert or replace a saved DB connection (keyed by `c.id`).
    fn upsert_db_connection(&self, c: &DbConnection) -> Result<(), SidError>;

    /// Look up a saved DB connection by id. Returns `None` if absent.
    fn get_db_connection(&self, id: &str) -> Result<Option<DbConnection>, SidError>;

    /// Remove a saved DB connection by id. No-op if absent.
    fn remove_db_connection(&self, id: &str) -> Result<(), SidError>;

    /// Append a row to the per-connection query history. (Plan 4.)
    fn append_query_record(&self, r: &QueryRecord) -> Result<(), SidError>;

    /// Return the most recent `limit` query records for the given connection,
    /// newest first. (Plan 4.)
    fn recent_queries(&self, conn_id: &str, limit: usize) -> Result<Vec<QueryRecord>, SidError>;

    /// List every SSH host stored in the registry. Order is implementation-defined.
    fn list_ssh_hosts(&self) -> Result<Vec<SshHost>, SidError>;

    /// Insert or replace an SSH host keyed on its `alias`.
    fn upsert_ssh_host(&self, h: &SshHost) -> Result<(), SidError>;

    /// Fetch an SSH host by alias. Returns `Ok(None)` if absent.
    fn get_ssh_host(&self, alias: &str) -> Result<Option<SshHost>, SidError>;

    /// Remove an SSH host by alias. No-op if absent.
    fn remove_ssh_host(&self, alias: &str) -> Result<(), SidError>;
}

/// Trait for opening a store from a filesystem path.
///
/// Separate from `Store` so the open path (which creates/migrates the DB) is
/// not confused with the read/write operations.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_store::{OpenStore, RedbStore};
///
/// // Open (or create) the store at a filesystem path.
/// let store = RedbStore::open(Path::new("/tmp/sid-example.redb")).unwrap();
/// ```
pub trait OpenStore {
    /// Open (or create) the store at the given path.
    ///
    /// # Errors
    ///
    /// Returns `SidError::Storage` if the path cannot be created or opened
    /// (e.g. the parent directory does not exist, permissions are denied, or
    /// the file is corrupted).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let path = dir.path().join("sid.redb");
    /// let store = RedbStore::open(&path).unwrap();
    /// // The file is created on disk.
    /// assert!(path.exists());
    /// ```
    fn open(path: &Path) -> Result<Self, SidError>
    where
        Self: Sized;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_epoch_is_positive() {
        assert!(now_epoch() > 0);
    }

    #[test]
    fn now_epoch_is_non_decreasing() {
        let t1 = now_epoch();
        let t2 = now_epoch();
        assert!(t2 >= t1);
    }

    #[test]
    fn setting_value_stores_bytes() {
        let v = SettingValue(b"test".to_vec());
        assert_eq!(v.0, b"test");
    }

    #[test]
    fn setting_value_equality() {
        let a = SettingValue(b"x".to_vec());
        let b = SettingValue(b"x".to_vec());
        let c = SettingValue(b"y".to_vec());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn session_record_constructor() {
        let r = SessionRecord {
            id: "s1".into(),
            started_at: 100,
            last_active: 200,
            ended_at: None,
            active_tab: None,
            open_tabs: vec![],
        };
        assert_eq!(r.id, "s1");
        assert_eq!(r.started_at, 100);
        assert!(r.ended_at.is_none());
        assert!(r.open_tabs.is_empty());
    }

    #[test]
    fn session_record_with_ended_at() {
        let r = SessionRecord {
            id: "s2".into(),
            started_at: 1,
            last_active: 2,
            ended_at: Some(3),
            active_tab: Some(TabId::new("workspaces")),
            open_tabs: vec![TabId::new("workspaces")],
        };
        assert_eq!(r.ended_at, Some(3));
        assert_eq!(r.active_tab.as_ref().unwrap().as_str(), "workspaces");
    }

    #[test]
    fn widget_state_constructor() {
        let ws = WidgetState {
            tab_id: TabId::new("ssh"),
            widget_id: WidgetId::new("ssh.root"),
            blob: vec![0xDE, 0xAD],
        };
        assert_eq!(ws.tab_id.as_str(), "ssh");
        assert_eq!(ws.widget_id.as_str(), "ssh.root");
        assert_eq!(ws.blob, vec![0xDE, 0xAD]);
    }

    /// Verify the Store trait can be object-safe and implemented by a MemStore.
    #[test]
    fn store_trait_can_be_implemented() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        struct MemStore {
            settings: Mutex<HashMap<String, SettingValue>>,
        }

        impl Store for MemStore {
            fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError> {
                Ok(self.settings.lock().unwrap().get(key).cloned())
            }
            fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError> {
                self.settings
                    .lock()
                    .unwrap()
                    .insert(key.to_string(), val.clone());
                Ok(())
            }
            fn delete_setting(&self, key: &str) -> Result<bool, SidError> {
                Ok(self.settings.lock().unwrap().remove(key).is_some())
            }
            fn list_setting_keys(&self) -> Result<Vec<String>, SidError> {
                Ok(self.settings.lock().unwrap().keys().cloned().collect())
            }
            fn current_session(&self) -> Result<Option<SessionRecord>, SidError> {
                Ok(None)
            }
            fn upsert_session(&self, _: &SessionRecord) -> Result<(), SidError> {
                Ok(())
            }
            fn end_session(&self, _: &str, _: Epoch) -> Result<(), SidError> {
                Ok(())
            }
            fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError> {
                Ok(vec![])
            }
            fn save_widget_state(&self, _: &WidgetState) -> Result<(), SidError> {
                Ok(())
            }
            fn load_widget_state(
                &self,
                _: &TabId,
                _: &WidgetId,
            ) -> Result<Option<Vec<u8>>, SidError> {
                Ok(None)
            }
            fn list_workspaces(&self) -> Result<Vec<Workspace>, SidError> {
                Ok(vec![])
            }
            fn upsert_workspace(&self, _: &Workspace) -> Result<(), SidError> {
                Ok(())
            }
            fn get_workspace(&self, _: &Path) -> Result<Option<Workspace>, SidError> {
                Ok(None)
            }
            fn remove_workspace(&self, _: &Path) -> Result<(), SidError> {
                Ok(())
            }
            fn secret_put(&self, _: &str, _: &[u8]) -> Result<(), SidError> {
                Ok(())
            }
            fn secret_get(&self, _: &str) -> Result<Option<Vec<u8>>, SidError> {
                Ok(None)
            }
            fn secret_delete(&self, _: &str) -> Result<(), SidError> {
                Ok(())
            }
            fn list_secret_ids(&self) -> Result<Vec<String>, SidError> {
                Ok(vec![])
            }
            fn list_themes(&self) -> Result<Vec<ThemeSpec>, SidError> {
                Ok(vec![])
            }
            fn get_theme(&self, _: &str) -> Result<Option<ThemeSpec>, SidError> {
                Ok(None)
            }
            fn upsert_theme(&self, _: &ThemeSpec) -> Result<(), SidError> {
                Ok(())
            }
            fn remove_theme(&self, _: &str) -> Result<(), SidError> {
                Ok(())
            }
            fn list_keybind_profiles(&self) -> Result<Vec<KeybindProfile>, SidError> {
                Ok(vec![])
            }
            fn get_keybind_profile(&self, _: &str) -> Result<Option<KeybindProfile>, SidError> {
                Ok(None)
            }
            fn upsert_keybind_profile(&self, _: &KeybindProfile) -> Result<(), SidError> {
                Ok(())
            }
            fn remove_keybind_profile(&self, _: &str) -> Result<(), SidError> {
                Ok(())
            }
            fn list_quick_actions(&self) -> Result<Vec<QuickAction>, SidError> {
                Ok(vec![])
            }
            fn get_quick_action(&self, _: &str) -> Result<Option<QuickAction>, SidError> {
                Ok(None)
            }
            fn upsert_quick_action(&self, _: &QuickAction) -> Result<(), SidError> {
                Ok(())
            }
            fn remove_quick_action(&self, _: &str) -> Result<(), SidError> {
                Ok(())
            }
            fn list_pinned_configs(&self) -> Result<Vec<PinnedConfig>, SidError> {
                Ok(vec![])
            }
            fn upsert_pinned_config(&self, _: &PinnedConfig) -> Result<(), SidError> {
                Ok(())
            }
            fn get_pinned_config(&self, _: &Path) -> Result<Option<PinnedConfig>, SidError> {
                Ok(None)
            }
            fn remove_pinned_config(&self, _: &Path) -> Result<(), SidError> {
                Ok(())
            }
            fn list_db_connections(&self) -> Result<Vec<DbConnection>, SidError> {
                Ok(vec![])
            }
            fn upsert_db_connection(&self, _: &DbConnection) -> Result<(), SidError> {
                Ok(())
            }
            fn get_db_connection(&self, _: &str) -> Result<Option<DbConnection>, SidError> {
                Ok(None)
            }
            fn remove_db_connection(&self, _: &str) -> Result<(), SidError> {
                Ok(())
            }
            fn append_query_record(&self, _: &QueryRecord) -> Result<(), SidError> {
                Ok(())
            }
            fn recent_queries(&self, _: &str, _: usize) -> Result<Vec<QueryRecord>, SidError> {
                Ok(vec![])
            }
            fn list_ssh_hosts(&self) -> Result<Vec<crate::SshHost>, SidError> {
                Ok(vec![])
            }
            fn upsert_ssh_host(&self, _: &crate::SshHost) -> Result<(), SidError> {
                Ok(())
            }
            fn get_ssh_host(&self, _: &str) -> Result<Option<crate::SshHost>, SidError> {
                Ok(None)
            }
            fn remove_ssh_host(&self, _: &str) -> Result<(), SidError> {
                Ok(())
            }
        }

        let store = MemStore {
            settings: Mutex::new(HashMap::new()),
        };
        let key = "foo";
        let val = SettingValue(b"bar".to_vec());
        store.put_setting(key, &val).unwrap();
        let got = store.get_setting(key).unwrap().unwrap();
        assert_eq!(got, val);

        // Verify trait object works
        let _dyn_store: &dyn Store = &store;
    }
}
