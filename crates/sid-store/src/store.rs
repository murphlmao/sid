//! `Store` — the single facade the tabs consume.
//!
//! Wraps the global (redb) and per-workspace (TOML) layers and exposes scoped operations:
//! - [`Store::read_hosts`] composes both layers for a scope (via the [`compose`] rules);
//! - [`Store::write_host`] saves into the named layer;
//! - [`Store::promote_host`] / [`Store::demote_host`] move an item *between* layers.
//!
//! Hosts are the exemplar entity; [`Store::read_connections`] / [`Store::write_connection`]
//! / [`Store::delete_connection`] / [`Store::promote_connection`] / [`Store::demote_connection`]
//! are a structural copy for `DbConnection`. Quick-actions follow the identical shape and
//! are added as their tab comes online.

use std::path::Path;

use crate::composer::{ViewFilters, compose};
use crate::entities::{DbConnection, Host, Settings};
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

    /// Read the machine-local [`Settings`] (identity-level; always global, never layered).
    pub fn settings(&self) -> Result<Settings> {
        self.global.get_settings()
    }

    /// Persist the machine-local [`Settings`].
    pub fn set_settings(&self, s: &Settings) -> Result<()> {
        self.global.set_settings(s)
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

    /// Read a host from **exactly** `scope` (no composition with the other layer) — the
    /// seam rename/folder mutators use to find the record they will write back in place.
    fn host_in_scope(&self, scope: &Scope, alias: &str) -> Result<Option<Host>> {
        match scope {
            Scope::Global => self.global.get_host(alias),
            Scope::Workspace(id) => Ok(self
                .workspace_store(id)?
                .load()?
                .ssh
                .host
                .into_iter()
                .find(|h| h.alias == alias)),
        }
    }

    /// Read a connection from **exactly** `scope` (see [`Store::host_in_scope`]).
    fn connection_in_scope(&self, scope: &Scope, id: &str) -> Result<Option<DbConnection>> {
        match scope {
            Scope::Global => self.global.get_connection(id),
            Scope::Workspace(ws_id) => Ok(self
                .workspace_store(ws_id)?
                .load()?
                .db
                .connection
                .into_iter()
                .find(|c| c.id == id)),
        }
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

    /// Delete a host from **exactly** the named layer, returning whether one was present.
    ///
    /// This removes only the record in `scope`; a same-alias copy in the other layer is
    /// untouched. Deleting a workspace copy therefore un-shadows the global copy in the
    /// collapsed view — that is attributive behaviour, not loss.
    pub fn delete_host(&self, alias: &str, scope: &Scope) -> Result<bool> {
        match scope {
            Scope::Global => self.global.remove_host(alias),
            Scope::Workspace(id) => self.workspace_store(id)?.remove_host(alias),
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

    /// Rename a host **within its own layer**: `alias` is the identity, so this is a
    /// write-new + delete-old under the hood, preserving every other field (auth,
    /// secret_ref, folder). Refuses a no-op rename, and refuses (via
    /// [`StoreError::Conflict`]) if `new_alias` already exists in `scope` — the other
    /// layer is never consulted or touched, matching [`Store::delete_host`]'s
    /// attributive, single-layer semantics.
    pub fn rename_host(&self, scope: &Scope, old_alias: &str, new_alias: &str) -> Result<()> {
        if old_alias == new_alias {
            return Err(StoreError::Conflict(format!(
                "host {old_alias:?} is already named {new_alias:?}"
            )));
        }
        let mut host = self
            .host_in_scope(scope, old_alias)?
            .ok_or_else(|| StoreError::Storage(format!("host {old_alias} not in {scope:?}")))?;
        if self.host_in_scope(scope, new_alias)?.is_some() {
            return Err(StoreError::Conflict(format!(
                "{scope:?} already has a host {new_alias:?}; resolve before renaming"
            )));
        }
        host.alias = new_alias.to_string();
        self.write_host(&host, scope)?;
        self.delete_host(old_alias, scope)?;
        Ok(())
    }

    /// Set (or clear) a host's `folder` in place, within its own layer. Errors if
    /// `alias` does not exist in `scope`.
    pub fn set_host_folder(
        &self,
        scope: &Scope,
        alias: &str,
        folder: Option<String>,
    ) -> Result<()> {
        let mut host = self
            .host_in_scope(scope, alias)?
            .ok_or_else(|| StoreError::Storage(format!("host {alias} not in {scope:?}")))?;
        host.folder = folder;
        self.write_host(&host, scope)
    }

    /// Read connections composed for `scope`, applying the view `filters`.
    pub fn read_connections(
        &self,
        scope: &Scope,
        filters: ViewFilters,
    ) -> Result<Vec<Attributed<DbConnection>>> {
        let global = self.global.list_connections()?;
        match scope {
            Scope::Global => Ok(compose(&global, None, filters)),
            Scope::Workspace(id) => {
                let items = self.workspace_store(id)?.load()?.db.connection;
                Ok(compose(&global, Some((id, &items)), filters))
            }
        }
    }

    /// Write a connection into the named layer.
    pub fn write_connection(&self, c: &DbConnection, scope: &Scope) -> Result<()> {
        match scope {
            Scope::Global => self.global.upsert_connection(c),
            Scope::Workspace(id) => self.workspace_store(id)?.upsert_connection(c),
        }
    }

    /// Delete a connection from **exactly** the named layer, returning whether one was
    /// present.
    ///
    /// This removes only the record in `scope`; a same-id copy in the other layer is
    /// untouched. Deleting a workspace copy therefore un-shadows the global copy in the
    /// collapsed view — that is attributive behaviour, not loss.
    pub fn delete_connection(&self, id: &str, scope: &Scope) -> Result<bool> {
        match scope {
            Scope::Global => self.global.remove_connection(id),
            Scope::Workspace(ws_id) => self.workspace_store(ws_id)?.remove_connection(id),
        }
    }

    /// Move a connection from a workspace up to global (removing it from the workspace).
    pub fn promote_connection(&self, id: &str, from: &WorkspaceId) -> Result<()> {
        let ws = self.workspace_store(from)?;
        let item = ws
            .load()?
            .db
            .connection
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| {
                StoreError::Storage(format!(
                    "connection {id} not in workspace {}",
                    from.as_str()
                ))
            })?;
        if self.global.get_connection(id)?.is_some() {
            return Err(StoreError::Conflict(format!(
                "global already has a connection {id:?}; resolve before promoting"
            )));
        }
        self.global.upsert_connection(&item)?;
        ws.remove_connection(id)?;
        Ok(())
    }

    /// Move a connection from global down into a workspace (removing it from global).
    pub fn demote_connection(&self, id: &str, to: &WorkspaceId) -> Result<()> {
        let item = self
            .global
            .get_connection(id)?
            .ok_or_else(|| StoreError::Storage(format!("connection {id} not in global")))?;
        let ws = self.workspace_store(to)?;
        if ws.load()?.db.connection.iter().any(|c| c.id == id) {
            return Err(StoreError::Conflict(format!(
                "workspace already has a connection {id:?}; resolve before demoting"
            )));
        }
        ws.upsert_connection(&item)?;
        self.global.remove_connection(id)?;
        Ok(())
    }

    /// Rename a connection's display `name` **in place, within its own layer**. Unlike
    /// [`Store::rename_host`], the identity is `id` (not `name`), so this is a plain
    /// read-modify-write — no delete/insert, no cross-layer conflict to guard against.
    /// Errors if `id` does not exist in `scope`.
    pub fn rename_connection(&self, scope: &Scope, id: &str, new_name: &str) -> Result<()> {
        let mut c = self
            .connection_in_scope(scope, id)?
            .ok_or_else(|| StoreError::Storage(format!("connection {id} not in {scope:?}")))?;
        c.name = new_name.to_string();
        self.write_connection(&c, scope)
    }

    /// Set (or clear) a connection's `folder` in place, within its own layer. Errors if
    /// `id` does not exist in `scope`.
    pub fn set_connection_folder(
        &self,
        scope: &Scope,
        id: &str,
        folder: Option<String>,
    ) -> Result<()> {
        let mut c = self
            .connection_in_scope(scope, id)?
            .ok_or_else(|| StoreError::Storage(format!("connection {id} not in {scope:?}")))?;
        c.folder = folder;
        self.write_connection(&c, scope)
    }
}
