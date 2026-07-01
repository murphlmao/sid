//! `GlobalStore` — the machine-local redb layer (always present).
//!
//! Generic versioned-postcard CRUD, lifted from the sid-poc store
//! (`crates/sid-store/src/redb_impl.rs`) and adapted to the local [`StoreError`]. Each
//! entity type gets its own table, keyed by the entity's [`Identity`]; values are the
//! versioned-postcard codec. Also holds the workspace registry (metadata, not config).

use std::path::Path;

use redb::{
    Database, Key, ReadOnlyTable, ReadTransaction, ReadableDatabase, ReadableTable, Table,
    TableDefinition, Value, WriteTransaction,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::codec::{decode_versioned, encode_versioned};
use crate::entities::{DbConnection, Host, Identity, QuickAction, Settings};
use crate::error::{Result, StoreError};
use crate::scope::WorkspaceMeta;

const HOSTS: TableDefinition<&str, &[u8]> = TableDefinition::new("hosts");
const CONNECTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("connections");
const QUICK_ACTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("quick_actions");
const WORKSPACES: TableDefinition<&str, &[u8]> = TableDefinition::new("workspaces");
const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");

/// The single key under which [`Settings`] lives in the [`SETTINGS`] table.
const SETTINGS_KEY: &str = "settings";

/// Codec version for all global entities (bump per-entity when a layout changes).
const V1: u8 = 1;

/// The machine-local global layer.
pub struct GlobalStore {
    db: Database,
}

impl GlobalStore {
    /// Open (or create) the redb database at `path`, ensuring every table exists.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path)
            .map_err(|e| StoreError::Storage(format!("redb open {path:?}: {e}")))?;
        let txn = db
            .begin_write()
            .map_err(|e| StoreError::Storage(format!("begin_write: {e}")))?;
        write_table(&txn, HOSTS, "open hosts")?;
        write_table(&txn, CONNECTIONS, "open connections")?;
        write_table(&txn, QUICK_ACTIONS, "open quick_actions")?;
        write_table(&txn, WORKSPACES, "open workspaces")?;
        write_table(&txn, SETTINGS, "open settings")?;
        txn.commit()
            .map_err(|e| StoreError::Storage(format!("commit: {e}")))?;
        Ok(Self { db })
    }

    fn with_read<T>(&self, f: impl FnOnce(&ReadTransaction) -> Result<T>) -> Result<T> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| StoreError::Storage(format!("read txn: {e}")))?;
        f(&txn)
    }

    fn with_write<T>(
        &self,
        label: &str,
        f: impl FnOnce(&WriteTransaction) -> Result<T>,
    ) -> Result<T> {
        let txn = self
            .db
            .begin_write()
            .map_err(|e| StoreError::Storage(format!("write txn: {e}")))?;
        let out = f(&txn)?;
        txn.commit()
            .map_err(|e| StoreError::Storage(format!("{label}: {e}")))?;
        Ok(out)
    }

    fn list<T: DeserializeOwned>(&self, def: TableDefinition<'_, &str, &[u8]>) -> Result<Vec<T>> {
        self.with_read(|txn| {
            let tbl = read_table(txn, def, "open table")?;
            let mut out = Vec::new();
            let iter = tbl
                .iter()
                .map_err(|e| StoreError::Storage(format!("iter: {e}")))?;
            for entry in iter {
                let (_k, v) = entry.map_err(|e| StoreError::Storage(format!("iter step: {e}")))?;
                let (_ver, item) = decode_versioned::<T>(v.value())?;
                out.push(item);
            }
            Ok(out)
        })
    }

    fn get<T: DeserializeOwned>(
        &self,
        def: TableDefinition<'_, &str, &[u8]>,
        key: &str,
    ) -> Result<Option<T>> {
        self.with_read(|txn| {
            let tbl = read_table(txn, def, "open table")?;
            match tbl
                .get(key)
                .map_err(|e| StoreError::Storage(format!("get: {e}")))?
            {
                None => Ok(None),
                Some(guard) => Ok(Some(decode_versioned::<T>(guard.value())?.1)),
            }
        })
    }

    fn upsert<T: Serialize>(
        &self,
        def: TableDefinition<'_, &str, &[u8]>,
        key: &str,
        value: &T,
    ) -> Result<()> {
        let bytes = encode_versioned(V1, value)?;
        self.with_write("commit", |txn| {
            let mut tbl = write_table(txn, def, "open table")?;
            tbl.insert(key, &bytes[..])
                .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
            Ok(())
        })
    }

    /// Remove `key` from `def`. Returns whether a value was present.
    fn remove(&self, def: TableDefinition<'_, &str, &[u8]>, key: &str) -> Result<bool> {
        self.with_write("commit", |txn| {
            let mut tbl = write_table(txn, def, "open table")?;
            let prior = tbl
                .remove(key)
                .map_err(|e| StoreError::Storage(format!("remove: {e}")))?;
            Ok(prior.is_some())
        })
    }

    // ---- hosts ----
    pub fn list_hosts(&self) -> Result<Vec<Host>> {
        self.list(HOSTS)
    }
    pub fn get_host(&self, alias: &str) -> Result<Option<Host>> {
        self.get(HOSTS, alias)
    }
    pub fn upsert_host(&self, h: &Host) -> Result<()> {
        self.upsert(HOSTS, h.identity(), h)
    }
    pub fn remove_host(&self, alias: &str) -> Result<bool> {
        self.remove(HOSTS, alias)
    }

    // ---- connections ----
    pub fn list_connections(&self) -> Result<Vec<DbConnection>> {
        self.list(CONNECTIONS)
    }
    pub fn get_connection(&self, id: &str) -> Result<Option<DbConnection>> {
        self.get(CONNECTIONS, id)
    }
    pub fn upsert_connection(&self, c: &DbConnection) -> Result<()> {
        self.upsert(CONNECTIONS, c.identity(), c)
    }
    pub fn remove_connection(&self, id: &str) -> Result<bool> {
        self.remove(CONNECTIONS, id)
    }

    // ---- quick actions ----
    pub fn list_quick_actions(&self) -> Result<Vec<QuickAction>> {
        self.list(QUICK_ACTIONS)
    }
    pub fn upsert_quick_action(&self, q: &QuickAction) -> Result<()> {
        self.upsert(QUICK_ACTIONS, q.identity(), q)
    }
    pub fn remove_quick_action(&self, label: &str) -> Result<bool> {
        self.remove(QUICK_ACTIONS, label)
    }

    // ---- workspace registry (metadata; the config lives in each repo's file) ----
    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceMeta>> {
        self.list(WORKSPACES)
    }
    pub fn get_workspace(&self, id: &str) -> Result<Option<WorkspaceMeta>> {
        self.get(WORKSPACES, id)
    }
    pub fn upsert_workspace(&self, w: &WorkspaceMeta) -> Result<()> {
        self.upsert(WORKSPACES, w.id.as_str(), w)
    }
    pub fn remove_workspace(&self, id: &str) -> Result<bool> {
        self.remove(WORKSPACES, id)
    }

    // ---- settings (single-key, identity-level; a missing value is the default) ----

    /// Read the machine-local [`Settings`]; a never-written table yields the default.
    pub fn get_settings(&self) -> Result<Settings> {
        Ok(self.get(SETTINGS, SETTINGS_KEY)?.unwrap_or_default())
    }

    /// Persist the machine-local [`Settings`].
    pub fn set_settings(&self, s: &Settings) -> Result<()> {
        self.upsert(SETTINGS, SETTINGS_KEY, s)
    }
}

/// Open a table from a read transaction, labelling any open error.
fn read_table<K: Key + 'static, V: Value + 'static>(
    txn: &ReadTransaction,
    def: TableDefinition<'_, K, V>,
    label: &str,
) -> Result<ReadOnlyTable<K, V>> {
    txn.open_table(def)
        .map_err(|e| StoreError::Storage(format!("{label}: {e}")))
}

/// Open a table from a write transaction, labelling any open error.
fn write_table<'t, K: Key + 'static, V: Value + 'static>(
    txn: &'t WriteTransaction,
    def: TableDefinition<'_, K, V>,
    label: &str,
) -> Result<Table<'t, K, V>> {
    txn.open_table(def)
        .map_err(|e| StoreError::Storage(format!("{label}: {e}")))
}
