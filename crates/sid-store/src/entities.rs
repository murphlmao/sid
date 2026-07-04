//! Persisted domain entities.
//!
//! Field shapes are cribbed from the sid-poc store, minus the single-global-scope
//! assumption: an entity does not know its scope — the store tags it ([`Scope`] /
//! [`Attributed`]). Each entity exposes an [`Identity`] used to detect true duplicates
//! across layers.
//!
//! [`Scope`]: crate::scope::Scope
//! [`Attributed`]: crate::scope::Attributed

use serde::{Deserialize, Serialize};
use sid_core::db::DbKind;

/// The value used to decide whether two entries (in different layers) are the same thing.
pub trait Identity {
    /// The stable identity key (e.g. a host alias, a connection id).
    fn identity(&self) -> &str;
}

/// How a host authenticates. Secrets themselves never live here — a password or key
/// passphrase is stored in the OS keyring and referenced by [`Host::secret_ref`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuthMethod {
    /// Use the running SSH agent (no stored secret).
    #[default]
    Agent,
    /// Password auth; the password bytes live in the keyring under `secret_ref`.
    Password,
    /// Public-key auth; `path` (committed) points at the private key, an optional
    /// passphrase lives in the keyring under `secret_ref`.
    Key {
        /// Filesystem path to the private key.
        path: String,
    },
}

/// An SSH host / SFTP target.
///
/// Stored in redb under codec version 3 (see [`HOST_VERSION`]). Version-1 values (the
/// pre-`auth` shape) migrate on read via [`HostV1`] → [`HostV2`] (`auth: Agent`), and
/// version-2 values (the pre-`folder` shape) migrate via [`HostV2`] → `folder: None`.
///
/// [`HOST_VERSION`]: crate::global::HOST_VERSION
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// Short name; the identity used for dedup.
    pub alias: String,
    /// Login user.
    pub user: String,
    /// Hostname or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Opaque keyring reference for the key/password (never the secret itself).
    #[serde(default)]
    pub secret_ref: Option<String>,
    /// How this host authenticates. Absent in v1 stores / older TOML — defaults to
    /// [`AuthMethod::Agent`].
    #[serde(default)]
    pub auth: AuthMethod,
    /// Optional flat grouping label (one level; deeper nesting, if ever wanted, would be
    /// a `/`-separated convention on this same string — not a new field). Absent in
    /// v1/v2 stores / older TOML — defaults to `None`, and stays absent from the
    /// committed TOML when unset (mirrors `secret_ref`/`auth`).
    #[serde(default)]
    pub folder: Option<String>,
}

impl Identity for Host {
    fn identity(&self) -> &str {
        &self.alias
    }
}

/// The version-1 on-disk shape of [`Host`] (before `auth`). Retained only to decode
/// legacy redb values; `From<HostV1> for HostV2` migrates it forward with `auth: Agent`.
///
/// postcard is positional, so a v1 value must be decoded against this exact 5-field
/// layout — decoding it as a later [`Host`] shape would misread the trailing bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HostV1 {
    pub alias: String,
    pub user: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub secret_ref: Option<String>,
}

impl From<HostV1> for HostV2 {
    fn from(v: HostV1) -> Self {
        HostV2 {
            alias: v.alias,
            user: v.user,
            host: v.host,
            port: v.port,
            secret_ref: v.secret_ref,
            auth: AuthMethod::Agent,
        }
    }
}

/// The version-2 on-disk shape of [`Host`] (after `auth`, before `folder`). Retained
/// only to decode legacy redb values; `From<HostV2> for Host` migrates it forward with
/// `folder: None`.
///
/// postcard is positional, so a v2 value must be decoded against this exact 6-field
/// layout — decoding it as the current [`Host`] would misread the trailing bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HostV2 {
    pub alias: String,
    pub user: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub auth: AuthMethod,
}

impl From<HostV2> for Host {
    fn from(v: HostV2) -> Self {
        Host {
            alias: v.alias,
            user: v.user,
            host: v.host,
            port: v.port,
            secret_ref: v.secret_ref,
            auth: v.auth,
            folder: None,
        }
    }
}

/// A saved database connection.
///
/// Stored in redb under codec version 3 (see [`CONNECTION_VERSION`]). Version-1 values
/// (the pre-`kind`/`name` shape) migrate on read via [`DbConnectionV1`] → [`DbConnectionV2`]
/// (`kind: Postgres`, `name: id.clone()`), and version-2 values (the pre-`folder` shape)
/// migrate via [`DbConnectionV2`] → `folder: None`.
///
/// [`CONNECTION_VERSION`]: crate::global::CONNECTION_VERSION
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbConnection {
    /// Stable id; the identity used for dedup.
    pub id: String,
    /// Data-source name / connection string (no embedded secret).
    pub dsn: String,
    /// Opaque keyring reference for the password.
    #[serde(default)]
    pub secret_ref: Option<String>,
    /// Which database engine this connection targets. Absent in v1 stores / older
    /// TOML — defaults to [`DbKind::Postgres`].
    #[serde(default)]
    pub kind: DbKind,
    /// Display label. Absent in v1 stores / older TOML — defaults to `id`.
    #[serde(default)]
    pub name: String,
    /// Optional flat grouping label (one level; see [`Host::folder`] for the nesting
    /// convention). Absent in v1/v2 stores / older TOML — defaults to `None`, and stays
    /// absent from the committed TOML when unset.
    #[serde(default)]
    pub folder: Option<String>,
}

impl Identity for DbConnection {
    fn identity(&self) -> &str {
        &self.id
    }
}

/// The version-1 on-disk shape of [`DbConnection`] (before `kind`/`name`). Retained only
/// to decode legacy redb values; `From<DbConnectionV1> for DbConnectionV2` migrates it
/// forward with `kind: Postgres`, `name: id.clone()`.
///
/// postcard is positional, so a v1 value must be decoded against this exact 3-field
/// layout — decoding it as a later [`DbConnection`] shape would misread the trailing
/// bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DbConnectionV1 {
    pub id: String,
    pub dsn: String,
    #[serde(default)]
    pub secret_ref: Option<String>,
}

impl From<DbConnectionV1> for DbConnectionV2 {
    fn from(v: DbConnectionV1) -> Self {
        DbConnectionV2 {
            name: v.id.clone(),
            id: v.id,
            dsn: v.dsn,
            secret_ref: v.secret_ref,
            kind: DbKind::Postgres,
        }
    }
}

/// The version-2 on-disk shape of [`DbConnection`] (after `kind`/`name`, before
/// `folder`). Retained only to decode legacy redb values; `From<DbConnectionV2> for
/// DbConnection` migrates it forward with `folder: None`.
///
/// postcard is positional, so a v2 value must be decoded against this exact 5-field
/// layout — decoding it as the current [`DbConnection`] would misread the trailing bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DbConnectionV2 {
    pub id: String,
    pub dsn: String,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub kind: DbKind,
    #[serde(default)]
    pub name: String,
}

impl From<DbConnectionV2> for DbConnection {
    fn from(v: DbConnectionV2) -> Self {
        DbConnection {
            id: v.id,
            dsn: v.dsn,
            secret_ref: v.secret_ref,
            kind: v.kind,
            name: v.name,
            folder: None,
        }
    }
}

/// Machine-local, identity-level preferences. Always global — never layered per workspace.
///
/// Stored in its own single-key redb table under codec version 4 (see
/// [`SETTINGS_VERSION`]); a missing value reads as [`Settings::default`]. Legacy values
/// migrate on read through the version chain [`SettingsV1`] → [`SettingsV2`] →
/// [`SettingsV3`] → the current shape, each hop filling its added fields with defaults
/// (`file_browser_side: Left`, secret toggles `true`, `theme: "cosmos"`).
///
/// [`SETTINGS_VERSION`]: crate::global::SETTINGS_VERSION
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Which layer the "save to" dialog preselects for new items.
    #[serde(default)]
    pub default_scope: DefaultScope,
    /// Which side of the SSH/SFTP tab the file browser docks to. Absent in v1 stores —
    /// defaults to [`PanelSide::Left`].
    #[serde(default)]
    pub file_browser_side: PanelSide,
    /// Whether the OS keyring is a candidate secret backend
    /// (`sid_secrets::resolve_secret_store`'s selection chain). Still gated by a
    /// startup durability probe — enabling this doesn't guarantee the keyring wins.
    /// Absent in v1/v2 stores — defaults to `true`.
    #[serde(default = "default_secret_backend_enabled")]
    pub secret_keyring_enabled: bool,
    /// **Ignored/dormant since round D §A (2026-07-03).** Used to gate the
    /// dependency-less encrypted-file vault as a candidate secret backend;
    /// `sid_secrets::resolve_secret_store`'s chain no longer reads it (keyring-or-memory
    /// only — see that function's module doc). Kept in the struct rather than removed:
    /// `Settings` is postcard-encoded positionally, so deleting a field would misread
    /// every later field in every already-persisted value. Absent in v1/v2 stores —
    /// defaults to `true`.
    #[serde(default = "default_secret_backend_enabled")]
    pub secret_file_enabled: bool,
    /// Name of the active UI theme — one of `crates/sid`'s built-in palettes
    /// (`cosmos`, `void`, `dusk`, `cosmos-light`; ported from the POC's theme system).
    /// Stored as a free string so future user-defined themes need no schema change;
    /// an unknown name falls back to the default at resolve time, never at decode
    /// time. Absent in v1..v3 stores — defaults to `"cosmos"`.
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_scope: DefaultScope::default(),
            file_browser_side: PanelSide::default(),
            secret_keyring_enabled: true,
            secret_file_enabled: true,
            theme: default_theme(),
        }
    }
}

/// The `#[serde(default = ...)]` value for [`Settings::theme`]: the POC's signature
/// deep-space palette.
fn default_theme() -> String {
    "cosmos".into()
}

/// The `#[serde(default = ...)]` value for both secret-backend toggles: `true` — both
/// backends are candidates out of the box (per the resolution chain in
/// `sid_secrets::resolve_secret_store`; memory is the unconditional last resort and is
/// never toggleable). A plain function (rather than a `bool::default`-style literal) is
/// what `#[serde(default = "...")]` requires.
fn default_secret_backend_enabled() -> bool {
    true
}

/// The version-1 on-disk shape of [`Settings`] (before `file_browser_side`). Retained
/// only to decode legacy redb values; `From<SettingsV1> for SettingsV2` migrates it
/// forward with `file_browser_side: PanelSide::Left`.
///
/// postcard is positional, so a v1 value must be decoded against this exact 1-field
/// layout — decoding it as a later [`Settings`] shape would misread the trailing bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SettingsV1 {
    #[serde(default)]
    pub default_scope: DefaultScope,
}

impl From<SettingsV1> for SettingsV2 {
    fn from(v: SettingsV1) -> Self {
        SettingsV2 {
            default_scope: v.default_scope,
            file_browser_side: PanelSide::Left,
        }
    }
}

/// The version-2 on-disk shape of [`Settings`] (after `file_browser_side`, before the
/// secret-backend toggles). Retained only to decode legacy redb values;
/// `From<SettingsV2> for Settings` migrates it forward with both toggles defaulting to
/// `true`.
///
/// postcard is positional, so a v2 value must be decoded against this exact 2-field
/// layout — decoding it as the current [`Settings`] would misread the trailing bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SettingsV2 {
    #[serde(default)]
    pub default_scope: DefaultScope,
    #[serde(default)]
    pub file_browser_side: PanelSide,
}

impl From<SettingsV2> for SettingsV3 {
    fn from(v: SettingsV2) -> Self {
        SettingsV3 {
            default_scope: v.default_scope,
            file_browser_side: v.file_browser_side,
            secret_keyring_enabled: true,
            secret_file_enabled: true,
        }
    }
}

/// The version-3 on-disk shape of [`Settings`] (after the secret-backend toggles,
/// before `theme`). Retained only to decode legacy redb values;
/// `From<SettingsV3> for Settings` migrates it forward with `theme: "cosmos"`.
///
/// postcard is positional, so a v3 value must be decoded against this exact 4-field
/// layout — decoding it as the current [`Settings`] would misread the trailing bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SettingsV3 {
    #[serde(default)]
    pub default_scope: DefaultScope,
    #[serde(default)]
    pub file_browser_side: PanelSide,
    #[serde(default = "default_secret_backend_enabled")]
    pub secret_keyring_enabled: bool,
    #[serde(default = "default_secret_backend_enabled")]
    pub secret_file_enabled: bool,
}

impl From<SettingsV3> for Settings {
    fn from(v: SettingsV3) -> Self {
        Settings {
            default_scope: v.default_scope,
            file_browser_side: v.file_browser_side,
            secret_keyring_enabled: v.secret_keyring_enabled,
            secret_file_enabled: v.secret_file_enabled,
            theme: default_theme(),
        }
    }
}

/// Which side of the tab a docked panel (currently: the SFTP file browser) renders on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PanelSide {
    /// Docked to the left of the main view.
    #[default]
    Left,
    /// Docked to the right of the main view.
    Right,
}

/// The layer a new item is saved to by default (the "save to" dialog's preselection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DefaultScope {
    /// No preselection — always prompt.
    #[default]
    Ask,
    /// Preselect the active workspace layer.
    Workspace,
    /// Preselect the global layer.
    Global,
}

/// A pinned quick action (label + shell command).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickAction {
    /// Human label; the identity used for dedup.
    pub label: String,
    /// Shell command to run.
    pub cmd: String,
}

impl Identity for QuickAction {
    fn identity(&self) -> &str {
        &self.label
    }
}

/// A pinned config-file path (Round E §D, Systems tab). Machine-local and **global
/// only** — the path itself is the identity, so pinning is idempotent. Workspace-scoped
/// pins (e.g. a repo pinning its own `.env`) are future work; every pin lives in the
/// global layer regardless of the active workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedFile {
    /// Absolute filesystem path (already tilde-expanded by the caller).
    pub path: String,
}

impl Identity for PinnedFile {
    fn identity(&self) -> &str {
        &self.path
    }
}
