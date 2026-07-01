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

/// The value used to decide whether two entries (in different layers) are the same thing.
pub trait Identity {
    /// The stable identity key (e.g. a host alias, a connection id).
    fn identity(&self) -> &str;
}

/// An SSH host / SFTP target.
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
}

impl Identity for Host {
    fn identity(&self) -> &str {
        &self.alias
    }
}

/// A saved database connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbConnection {
    /// Stable id; the identity used for dedup.
    pub id: String,
    /// Data-source name / connection string (no embedded secret).
    pub dsn: String,
    /// Opaque keyring reference for the password.
    #[serde(default)]
    pub secret_ref: Option<String>,
}

impl Identity for DbConnection {
    fn identity(&self) -> &str {
        &self.id
    }
}

/// Machine-local, identity-level preferences. Always global — never layered per workspace.
///
/// Stored in its own single-key redb table; a missing value reads as [`Settings::default`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Which layer the "save to" dialog preselects for new items.
    #[serde(default)]
    pub default_scope: DefaultScope,
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
