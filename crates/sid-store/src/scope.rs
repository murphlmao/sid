//! Scope, workspace identity, and provenance — the attributive core.
//!
//! Nothing here overrides anything: [`Scope`] tags where an item lives, and
//! [`Attributed`] tags where a read found it. Duplicate handling is a *view* concern
//! (see the composer), never a storage rule.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// A stable identifier for a workspace, derived from its absolute root path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub String);

impl WorkspaceId {
    /// Derive an id from a workspace's absolute root path.
    pub fn from_root(root: &Path) -> Self {
        WorkspaceId(root.to_string_lossy().into_owned())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which layer an item lives in, or which layer a read is scoped to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    /// The machine-local global layer (redb) — always present.
    Global,
    /// A specific workspace's committed layer (`.sid/config.toml`).
    Workspace(WorkspaceId),
}

/// An item plus where it came from — what an attributive read returns.
///
/// `duplicate` is `true` when another layer also holds an item with the same
/// [`Identity`](crate::entities::Identity); both records are still returned (the store is
/// lossless), and the view decides whether to collapse them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attributed<T> {
    /// The item itself.
    pub item: T,
    /// The layer this item was read from.
    pub origin: Scope,
    /// Whether another layer holds a same-identity item.
    pub duplicate: bool,
}
