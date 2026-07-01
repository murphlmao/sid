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
use crate::entities::{DbConnection, Host, HostV1, Identity, QuickAction, Settings};
use crate::error::{Result, StoreError};
use crate::scope::WorkspaceMeta;

const HOSTS: TableDefinition<&str, &[u8]> = TableDefinition::new("hosts");
const CONNECTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("connections");
const QUICK_ACTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("quick_actions");
const WORKSPACES: TableDefinition<&str, &[u8]> = TableDefinition::new("workspaces");
const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");

/// The single key under which [`Settings`] lives in the [`SETTINGS`] table.
const SETTINGS_KEY: &str = "settings";

/// Codec version for global entities that have not changed layout.
const V1: u8 = 1;

/// Current codec version for [`Host`] values. Bumped to 2 when `auth` was added; reads
/// branch on the leading version byte and migrate v1 values forward (see [`decode_host`]).
pub(crate) const HOST_VERSION: u8 = 2;

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
        self.list_with(def, |b| Ok(decode_versioned::<T>(b)?.1))
    }

    /// Like [`list`], but decoding each value with `decode` — the seam a versioned
    /// entity (e.g. hosts) uses to branch on the leading version byte.
    fn list_with<T>(
        &self,
        def: TableDefinition<'_, &str, &[u8]>,
        decode: impl Fn(&[u8]) -> Result<T>,
    ) -> Result<Vec<T>> {
        self.with_read(|txn| {
            let tbl = read_table(txn, def, "open table")?;
            let mut out = Vec::new();
            let iter = tbl
                .iter()
                .map_err(|e| StoreError::Storage(format!("iter: {e}")))?;
            for entry in iter {
                let (_k, v) = entry.map_err(|e| StoreError::Storage(format!("iter step: {e}")))?;
                out.push(decode(v.value())?);
            }
            Ok(out)
        })
    }

    fn get<T: DeserializeOwned>(
        &self,
        def: TableDefinition<'_, &str, &[u8]>,
        key: &str,
    ) -> Result<Option<T>> {
        self.get_with(def, key, |b| Ok(decode_versioned::<T>(b)?.1))
    }

    /// Like [`get`], but decoding the value with `decode` (see [`list_with`]).
    fn get_with<T>(
        &self,
        def: TableDefinition<'_, &str, &[u8]>,
        key: &str,
        decode: impl Fn(&[u8]) -> Result<T>,
    ) -> Result<Option<T>> {
        self.with_read(|txn| {
            let tbl = read_table(txn, def, "open table")?;
            match tbl
                .get(key)
                .map_err(|e| StoreError::Storage(format!("get: {e}")))?
            {
                None => Ok(None),
                Some(guard) => Ok(Some(decode(guard.value())?)),
            }
        })
    }

    fn upsert<T: Serialize>(
        &self,
        def: TableDefinition<'_, &str, &[u8]>,
        key: &str,
        value: &T,
    ) -> Result<()> {
        self.upsert_versioned(def, key, V1, value)
    }

    /// Like [`upsert`], but stamping an explicit codec `version` (hosts write V2).
    fn upsert_versioned<T: Serialize>(
        &self,
        def: TableDefinition<'_, &str, &[u8]>,
        key: &str,
        version: u8,
        value: &T,
    ) -> Result<()> {
        let bytes = encode_versioned(version, value)?;
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

    // ---- hosts (versioned: writes V2, reads migrate V1 forward) ----
    pub fn list_hosts(&self) -> Result<Vec<Host>> {
        self.list_with(HOSTS, decode_host)
    }
    pub fn get_host(&self, alias: &str) -> Result<Option<Host>> {
        self.get_with(HOSTS, alias, decode_host)
    }
    pub fn upsert_host(&self, h: &Host) -> Result<()> {
        self.upsert_versioned(HOSTS, h.identity(), HOST_VERSION, h)
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

/// Decode a stored [`Host`] value, branching on the leading codec version byte:
/// `1` → the pre-`auth` [`HostV1`] shape, migrated forward (`auth: Agent`);
/// `2` → the current [`Host`] shape. Any other version is rejected.
fn decode_host(bytes: &[u8]) -> Result<Host> {
    let &version = bytes.first().ok_or_else(|| StoreError::Decode {
        version: 0,
        msg: "empty host payload".into(),
    })?;
    match version {
        1 => Ok(decode_versioned::<HostV1>(bytes)?.1.into()),
        HOST_VERSION => Ok(decode_versioned::<Host>(bytes)?.1),
        other => Err(StoreError::UnsupportedVersion(other)),
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

#[cfg(test)]
mod tests {
    //! A3 migration: these exercise the version-branching host decode against the
    //! *private* `HostV1` shape, so they live in-crate rather than in `tests/`.
    use super::*;
    use crate::entities::AuthMethod;

    fn host_v1(alias: &str) -> HostV1 {
        HostV1 {
            alias: alias.into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: None,
        }
    }

    /// A crafted v1 payload decodes into a `Host` with `auth == Agent`.
    #[test]
    fn v1_payload_migrates_to_agent() {
        let bytes = encode_versioned(1, &host_v1("legacy")).unwrap();
        assert_eq!(bytes[0], 1, "leading byte is the v1 version");
        let host = decode_host(&bytes).unwrap();
        assert_eq!(host.alias, "legacy");
        assert_eq!(host.auth, AuthMethod::Agent);
    }

    /// A store seeded with raw v1 bytes reopens and lists cleanly, migrating on read.
    #[test]
    fn seeded_v1_store_reopens_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        // Write a v1 host straight into the HOSTS table (simulating a pre-`auth` store).
        {
            let store = GlobalStore::open(&path).unwrap();
            let bytes = encode_versioned(1, &host_v1("legacy")).unwrap();
            store
                .with_write("seed v1", |txn| {
                    let mut tbl = write_table(txn, HOSTS, "open hosts")?;
                    tbl.insert("legacy", &bytes[..])
                        .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                    Ok(())
                })
                .unwrap();
        }
        let reopened = GlobalStore::open(&path).unwrap();
        let hosts = reopened.list_hosts().unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "legacy");
        assert_eq!(hosts[0].auth, AuthMethod::Agent);
        // get_host takes the same migrating path.
        assert_eq!(
            reopened.get_host("legacy").unwrap().unwrap().auth,
            AuthMethod::Agent
        );
    }

    /// Hosts written now carry version 2 (the migration only fires for old values).
    #[test]
    fn hosts_are_written_at_version_2() {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        store
            .upsert_host(&Host {
                alias: "new".into(),
                user: "u".into(),
                host: "h".into(),
                port: 22,
                secret_ref: None,
                auth: AuthMethod::Key { path: "/k".into() },
            })
            .unwrap();
        let raw = store.get_with(HOSTS, "new", |b| Ok(b[0])).unwrap().unwrap();
        assert_eq!(raw, HOST_VERSION, "new hosts are stamped V2");
        assert_eq!(
            store.get_host("new").unwrap().unwrap().auth,
            AuthMethod::Key { path: "/k".into() }
        );
    }
}
