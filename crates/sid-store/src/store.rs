//! `Store` — the single facade the tabs consume.
//!
//! Wraps the global (redb) and per-workspace (TOML) layers and exposes scoped operations:
//! - [`Store::read_hosts`] composes both layers for a scope (via the [`compose`] rules);
//! - [`Store::write_host`] saves into the named layer;
//! - [`Store::promote_host`] / [`Store::demote_host`] move an item *between* layers.
//!
//! Hosts are the exemplar entity; connections and quick-actions follow the identical
//! shape and are added as their tabs come online.

use std::path::Path;

use crate::composer::{ViewFilters, compose};
use crate::entities::Host;
use crate::error::{Result, StoreError};
use crate::global::GlobalStore;
use crate::scope::{Attributed, Scope, WorkspaceId, WorkspaceMeta};
use crate::workspace::WorkspaceStore;

/// The store facade over the global + workspace layers.
pub struct Store {
    global: GlobalStore,
}

impl Store {
    /// Open the store, backed by the redb global DB at `db_path`.
    pub fn open(db_path: &Path) -> Result<Self> {
        Ok(Self {
            global: GlobalStore::open(db_path)?,
        })
    }

    /// Direct access to the global layer (registry + global-only operations).
    pub fn global(&self) -> &GlobalStore {
        &self.global
    }

    /// Register (or update) a workspace so scoped reads/writes can resolve its file.
    pub fn register_workspace(&self, meta: &WorkspaceMeta) -> Result<()> {
        self.global.upsert_workspace(meta)
    }

    /// Resolve a workspace id to its file store via the registry.
    fn workspace_store(&self, id: &WorkspaceId) -> Result<WorkspaceStore> {
        let meta = self
            .global
            .get_workspace(id.as_str())?
            .ok_or_else(|| StoreError::Storage(format!("unknown workspace: {}", id.as_str())))?;
        Ok(WorkspaceStore::new(meta.root))
    }

    /// Read hosts composed for `scope`, applying the view `filters`.
    pub fn read_hosts(&self, scope: &Scope, filters: ViewFilters) -> Result<Vec<Attributed<Host>>> {
        let global = self.global.list_hosts()?;
        match scope {
            Scope::Global => Ok(compose(&global, None, filters)),
            Scope::Workspace(id) => {
                let items = self.workspace_store(id)?.load()?.ssh.host;
                Ok(compose(&global, Some((id, &items)), filters))
            }
        }
    }

    /// Write a host into the named layer.
    pub fn write_host(&self, host: &Host, scope: &Scope) -> Result<()> {
        match scope {
            Scope::Global => self.global.upsert_host(host),
            Scope::Workspace(id) => self.workspace_store(id)?.upsert_host(host),
        }
    }

    /// Move a host from a workspace up to global (removing it from the workspace).
    pub fn promote_host(&self, alias: &str, from: &WorkspaceId) -> Result<()> {
        let ws = self.workspace_store(from)?;
        let item = ws
            .load()?
            .ssh
            .host
            .into_iter()
            .find(|h| h.alias == alias)
            .ok_or_else(|| {
                StoreError::Storage(format!("host {alias} not in workspace {}", from.as_str()))
            })?;
        if self.global.get_host(alias)?.is_some() {
            return Err(StoreError::Conflict(format!(
                "global already has a host {alias:?}; resolve before promoting"
            )));
        }
        self.global.upsert_host(&item)?;
        ws.remove_host(alias)?;
        Ok(())
    }

    /// Move a host from global down into a workspace (removing it from global).
    pub fn demote_host(&self, alias: &str, to: &WorkspaceId) -> Result<()> {
        let item = self
            .global
            .get_host(alias)?
            .ok_or_else(|| StoreError::Storage(format!("host {alias} not in global")))?;
        let ws = self.workspace_store(to)?;
        if ws.load()?.ssh.host.iter().any(|h| h.alias == alias) {
            return Err(StoreError::Conflict(format!(
                "workspace already has a host {alias:?}; resolve before demoting"
            )));
        }
        ws.upsert_host(&item)?;
        self.global.remove_host(alias)?;
        Ok(())
    }
}
