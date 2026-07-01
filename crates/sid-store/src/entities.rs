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
