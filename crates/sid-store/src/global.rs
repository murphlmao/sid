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
use crate::entities::{
    DbConnection, DbConnectionV1, DbConnectionV2, HistoryEntry, Host, HostHistory, HostV1, HostV2,
    Identity, KeyBinding, PinnedFile, QuickAction, SecretBinding, Settings, SettingsV1, SettingsV2,
    SettingsV3, SettingsV4,
};
use crate::error::{Result, StoreError};
use crate::scope::WorkspaceMeta;

const HOSTS: TableDefinition<&str, &[u8]> = TableDefinition::new("hosts");
const CONNECTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("connections");
const QUICK_ACTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("quick_actions");
const WORKSPACES: TableDefinition<&str, &[u8]> = TableDefinition::new("workspaces");
const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
/// Pinned config-file paths (Round E §D) — global-only (see [`PinnedFile`]'s doc
/// comment). Follows the [`QUICK_ACTIONS`] table's shape exactly: no versioning, keyed
/// by the entity's own identity.
const PINNED_FILES: TableDefinition<&str, &[u8]> = TableDefinition::new("pinned_files");
/// User keybinding overrides (Settings → Keymap) — global-only, keyed by the frontend's
/// stable action id. Follows the [`QUICK_ACTIONS`] table's shape exactly: no versioning
/// yet, keyed by the entity's own identity. A separate table rather than fields on
/// [`Settings`] — see [`KeyBinding`]'s doc comment for why.
const KEYBINDINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("keybindings");

/// Per-host SSH command history, keyed by [`Host::alias`]; one [`HostHistory`] row per
/// host, global-only (command history is machine-local, never committed).
const HOST_HISTORY: TableDefinition<&str, &[u8]> = TableDefinition::new("host_history");

/// Codec version of [`HostHistory`] rows.
pub(crate) const HOST_HISTORY_VERSION: u8 = 1;

/// What each stored secret was saved for, keyed by its keyring key — see
/// [`SecretBinding`]. Global-only: a binding is this machine's memory of what the user
/// typed a secret for, which is exactly what a committed file must not be able to edit.
const SECRET_BINDINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("secret_bindings");

/// Codec version of [`SecretBinding`] rows.
pub(crate) const SECRET_BINDING_VERSION: u8 = 1;

/// The single key under which [`Settings`] lives in the [`SETTINGS`] table.
const SETTINGS_KEY: &str = "settings";

/// Codec version for global entities that have not changed layout.
const V1: u8 = 1;

/// Current codec version for [`Host`] values. Bumped to 2 when `auth` was added and to 3
/// when `folder` was added; reads branch on the leading version byte and migrate older
/// values forward (see [`decode_host`]).
pub(crate) const HOST_VERSION: u8 = 3;

/// Current codec version for [`DbConnection`] values. Bumped to 2 when `kind`/`name`
/// were added and to 3 when `folder` was added; reads branch on the leading version byte
/// and migrate older values forward (see [`decode_connection`]).
pub(crate) const CONNECTION_VERSION: u8 = 3;

/// Current codec version for [`Settings`]. Bumped to 2 when `file_browser_side` was
/// added, to 3 when `secret_keyring_enabled`/`secret_file_enabled` were added, to 4 when
/// `theme` was, and to 5 when `ui_scale_percent` was; reads branch on the leading version
/// byte and migrate older values forward (see [`decode_settings`]).
pub(crate) const SETTINGS_VERSION: u8 = 5;

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
        write_table(&txn, PINNED_FILES, "open pinned_files")?;
        write_table(&txn, KEYBINDINGS, "open keybindings")?;
        write_table(&txn, HOST_HISTORY, "open host_history")?;
        write_table(&txn, SECRET_BINDINGS, "open secret_bindings")?;
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

    // ---- connections (versioned: writes V2, reads migrate V1 forward) ----
    pub fn list_connections(&self) -> Result<Vec<DbConnection>> {
        self.list_with(CONNECTIONS, decode_connection)
    }
    pub fn get_connection(&self, id: &str) -> Result<Option<DbConnection>> {
        self.get_with(CONNECTIONS, id, decode_connection)
    }
    pub fn upsert_connection(&self, c: &DbConnection) -> Result<()> {
        self.upsert_versioned(CONNECTIONS, c.identity(), CONNECTION_VERSION, c)
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

    // ---- pinned config files (Round E §D; global-only — see `PinnedFile`'s doc
    // comment) ----
    pub fn list_pinned_files(&self) -> Result<Vec<PinnedFile>> {
        self.list(PINNED_FILES)
    }
    /// Pin `p`. Idempotent — re-pinning an already-pinned path is a harmless overwrite
    /// (the identity is the path itself, so there is nothing else to update).
    pub fn pin_file(&self, p: &PinnedFile) -> Result<()> {
        self.upsert(PINNED_FILES, p.identity(), p)
    }
    pub fn unpin_file(&self, path: &str) -> Result<bool> {
        self.remove(PINNED_FILES, path)
    }

    // ---- per-host command history (global-only; see `HostHistory` for the contract) ----

    /// Record `command` for the host `alias` (atomic read-modify-write). Returns whether a
    /// new entry was appended (`false`: blank, or collapsed into an identical latest
    /// entry). The caller must not pass password-prompt input - see [`HostHistory`].
    ///
    /// **Not wired yet, and must never be wired from raw terminal keystrokes.** A remote
    /// echo-off prompt (`sudo`, `su`, `mysql -p`, an `ssh` hop, a passphrase read) is
    /// invisible to sid — the PTY simply stops echoing — so "the line typed before
    /// Enter" is sometimes a password, and this table is plaintext on disk. Feed it only
    /// from a source that *knows* it ran a command: a sid-issued command (quick action,
    /// palette), or shell integration that reports the executed command line.
    pub fn record_host_command(&self, alias: &str, command: &str, at_unix: i64) -> Result<bool> {
        self.with_write("commit", |txn| {
            let mut tbl = write_table(txn, HOST_HISTORY, "open host_history")?;
            let mut hist = match tbl
                .get(alias)
                .map_err(|e| StoreError::Storage(format!("get: {e}")))?
            {
                Some(g) => decode_history(g.value())?,
                None => HostHistory::default(),
            };
            let appended = hist.push(command, at_unix);
            let bytes = encode_versioned(HOST_HISTORY_VERSION, &hist)?;
            tbl.insert(alias, &bytes[..])
                .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
            Ok(appended)
        })
    }

    /// Newest-first history of `alias`, filtered to commands starting with `prefix`
    /// (`None` = all), at most `limit` entries.
    pub fn host_history(
        &self,
        alias: &str,
        prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>> {
        Ok(self
            .get_with(HOST_HISTORY, alias, decode_history)?
            .map(|h| h.search(prefix, limit))
            .unwrap_or_default())
    }

    /// Forget `alias`'s history; returns whether any existed.
    pub fn clear_host_history(&self, alias: &str) -> Result<bool> {
        self.remove(HOST_HISTORY, alias)
    }

    /// Move `old`'s history under `new` (call when a host is renamed). Refuses with
    /// [`StoreError::Conflict`] if `new` already has history; a no-op if `old` has none.
    pub fn rename_host_history(&self, old: &str, new: &str) -> Result<()> {
        if old == new {
            return Ok(());
        }
        self.with_write("commit", |txn| {
            let mut tbl = write_table(txn, HOST_HISTORY, "open host_history")?;
            let exists_new = tbl
                .get(new)
                .map_err(|e| StoreError::Storage(format!("get: {e}")))?
                .is_some();
            let moved = tbl
                .remove(old)
                .map_err(|e| StoreError::Storage(format!("remove: {e}")))?
                .map(|g| g.value().to_vec());
            let Some(bytes) = moved else { return Ok(()) };
            if exists_new {
                // put it back untouched, then refuse
                tbl.insert(old, &bytes[..])
                    .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                return Err(StoreError::Conflict(format!(
                    "history for `{new}` already exists"
                )));
            }
            tbl.insert(new, &bytes[..])
                .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
            Ok(())
        })
    }

    // ---- secret bindings (global-only; see `SecretBinding` for the contract) ----

    /// The binding recorded for the keyring key `key`, if any.
    pub fn secret_binding(&self, key: &str) -> Result<Option<SecretBinding>> {
        self.get_with(SECRET_BINDINGS, key, decode_binding)
    }

    /// Record `b` only if `b.key` has no binding yet — trust-on-first-use for secrets
    /// saved before bindings existed. Check and insert run in one write transaction, so
    /// two first uses cannot both win; the loser gets [`StoreError::Conflict`] and must
    /// re-read rather than assume its endpoint was recorded.
    pub fn bind_secret_if_unbound(&self, b: &SecretBinding) -> Result<()> {
        let bytes = encode_versioned(SECRET_BINDING_VERSION, b)?;
        self.with_write("commit", |txn| {
            let mut tbl = write_table(txn, SECRET_BINDINGS, "open secret_bindings")?;
            let exists = tbl
                .get(b.identity())
                .map_err(|e| StoreError::Storage(format!("get: {e}")))?
                .is_some();
            if exists {
                return Err(StoreError::Conflict(format!(
                    "secret `{}` is already bound",
                    b.key
                )));
            }
            tbl.insert(b.identity(), &bytes[..])
                .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
            Ok(())
        })
    }

    /// Record `b`, replacing any earlier binding for `b.key`. Only for a secret the user
    /// has just typed (a form save, a password prompt) — that act is what makes the new
    /// endpoint trusted. Never call it to "repair" a mismatch the user did not confirm.
    pub fn rebind_secret(&self, b: &SecretBinding) -> Result<()> {
        self.upsert_versioned(SECRET_BINDINGS, b.identity(), SECRET_BINDING_VERSION, b)
    }

    /// Forget the binding for `key` (its secret was deleted); returns whether one existed.
    pub fn unbind_secret(&self, key: &str) -> Result<bool> {
        self.remove(SECRET_BINDINGS, key)
    }

    // ---- keybinding overrides (Settings -> Keymap; global-only) ----

    /// Every user override. Order is redb's key order (the action id) — the frontend
    /// composes them against its own defaults, so it never depends on this order.
    pub fn list_keybindings(&self) -> Result<Vec<KeyBinding>> {
        self.list(KEYBINDINGS)
    }

    /// Bind (or re-bind) one action. Keyed by [`KeyBinding::action`], so a second rebind
    /// of the same action overwrites the first rather than adding an ambiguous row.
    pub fn set_keybinding(&self, b: &KeyBinding) -> Result<()> {
        self.upsert(KEYBINDINGS, b.identity(), b)
    }

    /// Drop one action's override (Settings → Keymap's per-row "reset"), returning
    /// whether one was set. The action falls back to the frontend's default.
    pub fn remove_keybinding(&self, action: &str) -> Result<bool> {
        self.remove(KEYBINDINGS, action)
    }

    /// Drop every override ("reset all"), returning how many were dropped. Idempotent:
    /// resetting an already-default keymap is `Ok(0)`, not an error.
    pub fn clear_keybindings(&self) -> Result<usize> {
        self.with_write("commit", |txn| {
            let mut tbl = write_table(txn, KEYBINDINGS, "open keybindings")?;
            // Collected first: the iterator borrows the table immutably and `remove`
            // needs it mutably.
            let keys: Vec<String> = {
                let iter = tbl
                    .iter()
                    .map_err(|e| StoreError::Storage(format!("iter: {e}")))?;
                let mut keys = Vec::new();
                for entry in iter {
                    let (k, _v) =
                        entry.map_err(|e| StoreError::Storage(format!("iter step: {e}")))?;
                    keys.push(k.value().to_string());
                }
                keys
            };
            let removed = keys.len();
            for key in keys {
                tbl.remove(key.as_str())
                    .map_err(|e| StoreError::Storage(format!("remove: {e}")))?;
            }
            Ok(removed)
        })
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
        Ok(self
            .get_with(SETTINGS, SETTINGS_KEY, decode_settings)?
            .unwrap_or_default())
    }

    /// Persist the machine-local [`Settings`].
    pub fn set_settings(&self, s: &Settings) -> Result<()> {
        self.upsert_versioned(SETTINGS, SETTINGS_KEY, SETTINGS_VERSION, s)
    }
}

/// Decode a stored [`Host`] value, branching on the leading codec version byte:
/// `1` → the pre-`auth` [`HostV1`] shape, migrated forward through [`HostV2`]
/// (`auth: Agent`, `folder: None`); `2` → the pre-`folder` [`HostV2`] shape, migrated
/// forward (`folder: None`); `3` → the current [`Host`] shape. Any other version is
/// rejected.
fn decode_host(bytes: &[u8]) -> Result<Host> {
    let &version = bytes.first().ok_or_else(|| StoreError::Decode {
        version: 0,
        msg: "empty host payload".into(),
    })?;
    match version {
        1 => Ok(Host::from(HostV2::from(
            decode_versioned::<HostV1>(bytes)?.1,
        ))),
        2 => Ok(decode_versioned::<HostV2>(bytes)?.1.into()),
        HOST_VERSION => Ok(decode_versioned::<Host>(bytes)?.1),
        other => Err(StoreError::UnsupportedVersion(other)),
    }
}

/// Decode a stored [`DbConnection`] value, branching on the leading codec version byte:
/// `1` → the pre-`kind`/`name` [`DbConnectionV1`] shape, migrated forward through
/// [`DbConnectionV2`] (`kind: Postgres`, `name: id.clone()`, `folder: None`); `2` → the
/// pre-`folder` [`DbConnectionV2`] shape, migrated forward (`folder: None`); `3` → the
/// current [`DbConnection`] shape. Any other version is rejected.
fn decode_connection(bytes: &[u8]) -> Result<DbConnection> {
    let &version = bytes.first().ok_or_else(|| StoreError::Decode {
        version: 0,
        msg: "empty connection payload".into(),
    })?;
    match version {
        1 => Ok(DbConnection::from(DbConnectionV2::from(
            decode_versioned::<DbConnectionV1>(bytes)?.1,
        ))),
        2 => Ok(decode_versioned::<DbConnectionV2>(bytes)?.1.into()),
        CONNECTION_VERSION => Ok(decode_versioned::<DbConnection>(bytes)?.1),
        other => Err(StoreError::UnsupportedVersion(other)),
    }
}

/// Decode a stored [`Settings`] value, branching on the leading codec version byte:
/// `1` → the pre-`file_browser_side` [`SettingsV1`] shape; `2` → the pre-secret-toggle
/// [`SettingsV2`] shape; `3` → the pre-`theme` [`SettingsV3`] shape; `4` → the
/// pre-`ui_scale_percent` [`SettingsV4`] shape; each migrates forward through the version
/// chain, filling added fields with defaults; `5` → the current [`Settings`] shape. Any
/// other version is rejected.
fn decode_settings(bytes: &[u8]) -> Result<Settings> {
    let &version = bytes.first().ok_or_else(|| StoreError::Decode {
        version: 0,
        msg: "empty settings payload".into(),
    })?;
    match version {
        1 => Ok(Settings::from(SettingsV4::from(SettingsV3::from(
            SettingsV2::from(decode_versioned::<SettingsV1>(bytes)?.1),
        )))),
        2 => Ok(Settings::from(SettingsV4::from(SettingsV3::from(
            decode_versioned::<SettingsV2>(bytes)?.1,
        )))),
        3 => Ok(Settings::from(SettingsV4::from(
            decode_versioned::<SettingsV3>(bytes)?.1,
        ))),
        4 => Ok(decode_versioned::<SettingsV4>(bytes)?.1.into()),
        SETTINGS_VERSION => Ok(decode_versioned::<Settings>(bytes)?.1),
        other => Err(StoreError::UnsupportedVersion(other)),
    }
}

/// Decode a [`HostHistory`] row, branching on its version byte.
fn decode_history(bytes: &[u8]) -> Result<HostHistory> {
    match bytes.first().copied() {
        Some(HOST_HISTORY_VERSION) => Ok(decode_versioned::<HostHistory>(bytes)?.1),
        Some(other) => Err(StoreError::UnsupportedVersion(other)),
        None => Err(StoreError::Decode {
            version: 0,
            msg: "empty payload".into(),
        }),
    }
}

/// Decode a [`SecretBinding`] row, branching on its version byte.
fn decode_binding(bytes: &[u8]) -> Result<SecretBinding> {
    match bytes.first().copied() {
        Some(SECRET_BINDING_VERSION) => Ok(decode_versioned::<SecretBinding>(bytes)?.1),
        Some(other) => Err(StoreError::UnsupportedVersion(other)),
        None => Err(StoreError::Decode {
            version: 0,
            msg: "empty payload".into(),
        }),
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
    //! A3 migration: these exercise the version-branching host/connection/settings
    //! decode against the *private* V1/V2 shapes, so they live in-crate rather than in
    //! `tests/`.
    use super::*;
    use crate::entities::{AuthMethod, PanelSide};

    fn host_v1(alias: &str) -> HostV1 {
        HostV1 {
            alias: alias.into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: None,
        }
    }

    fn host_v2(alias: &str, auth: AuthMethod) -> HostV2 {
        HostV2 {
            alias: alias.into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: None,
            auth,
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

    /// A crafted v2 (pre-`folder`) payload decodes into a `Host` with `folder == None`.
    #[test]
    fn v2_payload_migrates_to_no_folder() {
        let bytes = encode_versioned(2, &host_v2("legacy2", AuthMethod::Password)).unwrap();
        assert_eq!(bytes[0], 2, "leading byte is the v2 version");
        let host = decode_host(&bytes).unwrap();
        assert_eq!(host.alias, "legacy2");
        assert_eq!(host.auth, AuthMethod::Password, "v2 auth is preserved");
        assert_eq!(host.folder, None);
    }

    /// A store seeded with raw v2 bytes reopens and lists cleanly, migrating on read.
    #[test]
    fn seeded_v2_store_reopens_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        {
            let store = GlobalStore::open(&path).unwrap();
            let bytes = encode_versioned(2, &host_v2("legacy2", AuthMethod::Agent)).unwrap();
            store
                .with_write("seed v2", |txn| {
                    let mut tbl = write_table(txn, HOSTS, "open hosts")?;
                    tbl.insert("legacy2", &bytes[..])
                        .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                    Ok(())
                })
                .unwrap();
        }
        let reopened = GlobalStore::open(&path).unwrap();
        let hosts = reopened.list_hosts().unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "legacy2");
        assert_eq!(hosts[0].folder, None);
    }

    /// Hosts written now carry version 3 (the migration only fires for old values).
    #[test]
    fn hosts_are_written_at_version_3() {
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
                folder: Some("prod".into()),
            })
            .unwrap();
        let raw = store.get_with(HOSTS, "new", |b| Ok(b[0])).unwrap().unwrap();
        assert_eq!(raw, HOST_VERSION, "new hosts are stamped V3");
        let got = store.get_host("new").unwrap().unwrap();
        assert_eq!(got.auth, AuthMethod::Key { path: "/k".into() });
        assert_eq!(got.folder.as_deref(), Some("prod"));
    }

    fn connection_v1(id: &str) -> DbConnectionV1 {
        DbConnectionV1 {
            id: id.into(),
            dsn: "postgres://x".into(),
            secret_ref: None,
        }
    }

    fn connection_v2(id: &str) -> DbConnectionV2 {
        DbConnectionV2 {
            id: id.into(),
            dsn: "postgres://x".into(),
            secret_ref: None,
            kind: sid_core::db::DbKind::Sqlite,
            name: "Legacy".into(),
        }
    }

    /// A crafted v1 connection payload decodes into a `DbConnection` with `kind ==
    /// Postgres` and `name == id`.
    #[test]
    fn v1_connection_payload_migrates_to_postgres_and_name() {
        let bytes = encode_versioned(1, &connection_v1("legacy-pg")).unwrap();
        assert_eq!(bytes[0], 1, "leading byte is the v1 version");
        let conn = decode_connection(&bytes).unwrap();
        assert_eq!(conn.id, "legacy-pg");
        assert_eq!(conn.name, "legacy-pg");
        assert_eq!(conn.kind, sid_core::db::DbKind::Postgres);
    }

    /// A store seeded with raw v1 connection bytes reopens and lists cleanly, migrating
    /// on read.
    #[test]
    fn seeded_v1_connection_store_reopens_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        {
            let store = GlobalStore::open(&path).unwrap();
            let bytes = encode_versioned(1, &connection_v1("legacy-pg")).unwrap();
            store
                .with_write("seed v1", |txn| {
                    let mut tbl = write_table(txn, CONNECTIONS, "open connections")?;
                    tbl.insert("legacy-pg", &bytes[..])
                        .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                    Ok(())
                })
                .unwrap();
        }
        let reopened = GlobalStore::open(&path).unwrap();
        let conns = reopened.list_connections().unwrap();
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].id, "legacy-pg");
        assert_eq!(conns[0].name, "legacy-pg");
        assert_eq!(conns[0].kind, sid_core::db::DbKind::Postgres);
        // get_connection takes the same migrating path.
        assert_eq!(
            reopened.get_connection("legacy-pg").unwrap().unwrap().kind,
            sid_core::db::DbKind::Postgres
        );
    }

    /// A crafted v2 (pre-`folder`) connection payload decodes with `folder == None`.
    #[test]
    fn v2_connection_payload_migrates_to_no_folder() {
        let bytes = encode_versioned(2, &connection_v2("legacy-pg2")).unwrap();
        assert_eq!(bytes[0], 2, "leading byte is the v2 version");
        let conn = decode_connection(&bytes).unwrap();
        assert_eq!(conn.id, "legacy-pg2");
        assert_eq!(conn.name, "Legacy", "v2 name is preserved");
        assert_eq!(
            conn.kind,
            sid_core::db::DbKind::Sqlite,
            "v2 kind is preserved"
        );
        assert_eq!(conn.folder, None);
    }

    /// A store seeded with raw v2 connection bytes reopens and lists cleanly, migrating
    /// on read.
    #[test]
    fn seeded_v2_connection_store_reopens_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        {
            let store = GlobalStore::open(&path).unwrap();
            let bytes = encode_versioned(2, &connection_v2("legacy-pg2")).unwrap();
            store
                .with_write("seed v2", |txn| {
                    let mut tbl = write_table(txn, CONNECTIONS, "open connections")?;
                    tbl.insert("legacy-pg2", &bytes[..])
                        .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                    Ok(())
                })
                .unwrap();
        }
        let reopened = GlobalStore::open(&path).unwrap();
        let conns = reopened.list_connections().unwrap();
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].id, "legacy-pg2");
        assert_eq!(conns[0].folder, None);
    }

    /// Connections written now carry version 3 (the migration only fires for old values).
    #[test]
    fn connections_are_written_at_version_3() {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        store
            .upsert_connection(&DbConnection {
                id: "new-pg".into(),
                dsn: "postgres://x".into(),
                secret_ref: None,
                kind: sid_core::db::DbKind::Sqlite,
                name: "New PG".into(),
                folder: Some("analytics".into()),
            })
            .unwrap();
        let raw = store
            .get_with(CONNECTIONS, "new-pg", |b| Ok(b[0]))
            .unwrap()
            .unwrap();
        assert_eq!(raw, CONNECTION_VERSION, "new connections are stamped V3");
        let got = store.get_connection("new-pg").unwrap().unwrap();
        assert_eq!(got.kind, sid_core::db::DbKind::Sqlite);
        assert_eq!(got.name, "New PG");
        assert_eq!(got.folder.as_deref(), Some("analytics"));
    }

    fn settings_v1(default_scope: crate::entities::DefaultScope) -> SettingsV1 {
        SettingsV1 { default_scope }
    }

    fn settings_v2(
        default_scope: crate::entities::DefaultScope,
        file_browser_side: PanelSide,
    ) -> SettingsV2 {
        SettingsV2 {
            default_scope,
            file_browser_side,
        }
    }

    /// A crafted v4 (pre-`ui_scale_percent`) settings payload decodes as unzoomed, with
    /// every pre-existing field preserved — including a non-default theme and a `false`
    /// toggle, proving the hop copies rather than re-defaults them.
    ///
    /// The failure this pins is not cosmetic: postcard is positional and not
    /// self-describing, so decoding a 5-field v4 payload against the 6-field current
    /// shape runs off the end of the buffer. Without the hop, every existing user's
    /// settings become an unreadable blob.
    #[test]
    fn settings_v4_payload_migrates_to_unzoomed() {
        use crate::entities::DefaultScope;
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        let v4 = SettingsV4 {
            default_scope: DefaultScope::Workspace,
            file_browser_side: PanelSide::Right,
            secret_keyring_enabled: false,
            secret_file_enabled: true,
            theme: "void".into(),
        };
        let bytes = encode_versioned(4, &v4).unwrap();
        store
            .with_write("seed v4", |txn| {
                let mut tbl = write_table(txn, SETTINGS, "open settings")?;
                tbl.insert(SETTINGS_KEY, &bytes[..])
                    .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                Ok(())
            })
            .unwrap();
        let settings = store.get_settings().unwrap();
        assert_eq!(
            settings.ui_scale_percent, 100,
            "a v4 store was never zoomed"
        );
        assert_eq!(settings.theme, "void", "the v4 theme is carried forward");
        assert_eq!(settings.default_scope, DefaultScope::Workspace);
        assert_eq!(settings.file_browser_side, PanelSide::Right);
        assert!(!settings.secret_keyring_enabled);
        assert!(settings.secret_file_enabled);
    }

    /// The oldest payload still has to reach the newest shape: v1 predates four hops, and
    /// the chain is only as good as its longest walk.
    #[test]
    fn a_v1_payload_walks_every_hop_to_an_unzoomed_current_shape() {
        use crate::entities::DefaultScope;
        let bytes = encode_versioned(1, &settings_v1(DefaultScope::Workspace)).unwrap();
        let settings = decode_settings(&bytes).unwrap();
        assert_eq!(settings.ui_scale_percent, 100);
        assert_eq!(settings.theme, "cosmos");
        assert_eq!(settings.default_scope, DefaultScope::Workspace);
    }

    /// A zoom level survives the write/read round-trip verbatim — the store must not snap
    /// or clamp it, because the ladder lives in the frontend and the store is a byte
    /// carrier. (`sid_ui::UiScale::from_percent` is what makes an odd value usable.)
    #[test]
    fn a_zoom_level_round_trips_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        let mut settings = Settings {
            ui_scale_percent: 150,
            ..Default::default()
        };
        store.set_settings(&settings).unwrap();
        assert_eq!(store.get_settings().unwrap().ui_scale_percent, 150);
        // ...and a value off the frontend's ladder is stored as given, not rejected.
        settings.ui_scale_percent = 137;
        store.set_settings(&settings).unwrap();
        assert_eq!(store.get_settings().unwrap().ui_scale_percent, 137);
    }

    /// A crafted v3 (pre-`theme`) settings payload decodes with `theme == "cosmos"`
    /// and every pre-existing field preserved — including a non-default `false`
    /// toggle, proving the migration copies rather than re-defaults them.
    #[test]
    fn settings_v3_payload_migrates_to_cosmos_theme() {
        use crate::entities::DefaultScope;
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        let v3 = SettingsV3 {
            default_scope: DefaultScope::Workspace,
            file_browser_side: PanelSide::Right,
            secret_keyring_enabled: false,
            secret_file_enabled: true,
        };
        let bytes = encode_versioned(3, &v3).unwrap();
        store
            .with_write("seed v3", |txn| {
                let mut tbl = write_table(txn, SETTINGS, "open settings")?;
                tbl.insert(SETTINGS_KEY, &bytes[..])
                    .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                Ok(())
            })
            .unwrap();
        let settings = store.get_settings().unwrap();
        assert_eq!(settings.theme, "cosmos");
        assert_eq!(settings.default_scope, DefaultScope::Workspace);
        assert_eq!(settings.file_browser_side, PanelSide::Right);
        assert!(!settings.secret_keyring_enabled);
        assert!(settings.secret_file_enabled);
    }

    /// A crafted v1 (pre-`file_browser_side`) settings payload decodes with
    /// `file_browser_side == Left` and both secret-backend toggles defaulting to `true`.
    #[test]
    fn settings_v1_payload_migrates_to_left_panel_and_both_backends_enabled() {
        use crate::entities::DefaultScope;
        let bytes = encode_versioned(1, &settings_v1(DefaultScope::Workspace)).unwrap();
        assert_eq!(bytes[0], 1, "leading byte is the v1 version");
        let settings = decode_settings(&bytes).unwrap();
        assert_eq!(settings.default_scope, DefaultScope::Workspace);
        assert_eq!(settings.file_browser_side, PanelSide::Left);
        assert!(settings.secret_keyring_enabled);
        assert!(settings.secret_file_enabled);
    }

    /// A store seeded with raw v1 settings bytes reopens and reads cleanly, migrating on
    /// read.
    #[test]
    fn seeded_v1_settings_store_reopens_and_reads() {
        use crate::entities::DefaultScope;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        {
            let store = GlobalStore::open(&path).unwrap();
            let bytes = encode_versioned(1, &settings_v1(DefaultScope::Global)).unwrap();
            store
                .with_write("seed v1", |txn| {
                    let mut tbl = write_table(txn, SETTINGS, "open settings")?;
                    tbl.insert(SETTINGS_KEY, &bytes[..])
                        .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                    Ok(())
                })
                .unwrap();
        }
        let reopened = GlobalStore::open(&path).unwrap();
        let settings = reopened.get_settings().unwrap();
        assert_eq!(settings.default_scope, DefaultScope::Global);
        assert_eq!(settings.file_browser_side, PanelSide::Left);
        assert!(settings.secret_keyring_enabled);
        assert!(settings.secret_file_enabled);
    }

    /// A crafted v2 (pre-secret-toggle) settings payload decodes with both toggles
    /// defaulting to `true`, preserving the v2 fields untouched.
    #[test]
    fn settings_v2_payload_migrates_to_both_backends_enabled() {
        use crate::entities::DefaultScope;
        let bytes =
            encode_versioned(2, &settings_v2(DefaultScope::Global, PanelSide::Right)).unwrap();
        assert_eq!(bytes[0], 2, "leading byte is the v2 version");
        let settings = decode_settings(&bytes).unwrap();
        assert_eq!(settings.default_scope, DefaultScope::Global);
        assert_eq!(
            settings.file_browser_side,
            PanelSide::Right,
            "v2 field is preserved"
        );
        assert!(settings.secret_keyring_enabled);
        assert!(settings.secret_file_enabled);
    }

    /// A store seeded with raw v2 settings bytes reopens and reads cleanly, migrating on
    /// read.
    #[test]
    fn seeded_v2_settings_store_reopens_and_reads() {
        use crate::entities::DefaultScope;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        {
            let store = GlobalStore::open(&path).unwrap();
            let bytes = encode_versioned(2, &settings_v2(DefaultScope::Workspace, PanelSide::Left))
                .unwrap();
            store
                .with_write("seed v2", |txn| {
                    let mut tbl = write_table(txn, SETTINGS, "open settings")?;
                    tbl.insert(SETTINGS_KEY, &bytes[..])
                        .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                    Ok(())
                })
                .unwrap();
        }
        let reopened = GlobalStore::open(&path).unwrap();
        let settings = reopened.get_settings().unwrap();
        assert_eq!(settings.default_scope, DefaultScope::Workspace);
        assert!(settings.secret_keyring_enabled);
        assert!(settings.secret_file_enabled);
    }

    /// Settings written now carry [`SETTINGS_VERSION`] (the migration only fires for
    /// old values), and a false secret-backend toggle round-trips as `false` (not
    /// silently coerced back to the serde default).
    #[test]
    fn settings_are_written_at_current_version() {
        use crate::entities::DefaultScope;
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        store
            .set_settings(&Settings {
                default_scope: DefaultScope::Ask,
                file_browser_side: PanelSide::Right,
                secret_keyring_enabled: false,
                secret_file_enabled: true,
                theme: "cosmos".into(),
                ui_scale_percent: 125,
            })
            .unwrap();
        let raw = store
            .get_with(SETTINGS, SETTINGS_KEY, |b| Ok(b[0]))
            .unwrap()
            .unwrap();
        assert_eq!(
            raw, SETTINGS_VERSION,
            "new settings are stamped at the current version"
        );
        assert_eq!(
            store.get_settings().unwrap().file_browser_side,
            PanelSide::Right
        );
        assert!(!store.get_settings().unwrap().secret_keyring_enabled);
        assert!(store.get_settings().unwrap().secret_file_enabled);
        assert_eq!(store.get_settings().unwrap().ui_scale_percent, 125);
    }

    // ---- secret bindings (H2: a committed secret_ref must not be redirectable) ----

    use crate::scope::{Scope, WorkspaceId};

    fn binding(key: &str, scope: Scope, endpoint: &str) -> SecretBinding {
        SecretBinding {
            key: key.into(),
            scope,
            endpoint: endpoint.into(),
        }
    }

    fn ws(id: &str) -> Scope {
        Scope::Workspace(WorkspaceId(id.into()))
    }

    /// The migration that matters for a brand-new table: a store file written before
    /// `secret_bindings` existed (only older tables in it) reopens, gains the table, and
    /// reads "no binding" — which the app treats as trust-on-first-use, not as an error
    /// that would lock everyone out of their saved passwords.
    #[test]
    fn a_store_from_before_bindings_existed_reopens_with_none_bound() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        {
            // Hand-build the pre-binding file: old tables only, no `secret_bindings`.
            let db = Database::create(&path).unwrap();
            let txn = db.begin_write().unwrap();
            write_table(&txn, HOSTS, "seed").unwrap();
            write_table(&txn, SETTINGS, "seed").unwrap();
            txn.commit().unwrap();
        }
        let store = GlobalStore::open(&path).unwrap();
        assert_eq!(store.secret_binding("ssh-a-1").unwrap(), None);
        store
            .bind_secret_if_unbound(&binding("ssh-a-1", Scope::Global, "ssh:password:a@h:22"))
            .unwrap();
        assert!(store.secret_binding("ssh-a-1").unwrap().is_some());
    }

    #[test]
    fn a_binding_round_trips_with_its_workspace_scope_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        let b = binding("k", ws("/home/me/repo"), "ssh:password:alice@db1:2222");
        {
            let store = GlobalStore::open(&path).unwrap();
            store.rebind_secret(&b).unwrap();
        }
        let reopened = GlobalStore::open(&path).unwrap();
        assert_eq!(reopened.secret_binding("k").unwrap(), Some(b));
    }

    #[test]
    fn bindings_are_written_at_version_1() {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        store
            .rebind_secret(&binding("k", Scope::Global, "e"))
            .unwrap();
        let raw = store
            .get_with(SECRET_BINDINGS, "k", |b| Ok(b[0]))
            .unwrap()
            .unwrap();
        assert_eq!(raw, SECRET_BINDING_VERSION);
        assert_eq!(SECRET_BINDING_VERSION, 1);
    }

    /// A binding from a newer build must not be misread as this layout — an unknown
    /// version is an error the caller fails closed on, never a silent "unbound".
    #[test]
    fn a_binding_at_an_unknown_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        let bytes = encode_versioned(9, &binding("k", Scope::Global, "e")).unwrap();
        store
            .with_write("seed v9", |txn| {
                let mut tbl = write_table(txn, SECRET_BINDINGS, "open")?;
                tbl.insert("k", &bytes[..])
                    .map_err(|e| StoreError::Storage(format!("insert: {e}")))?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            store.secret_binding("k"),
            Err(StoreError::UnsupportedVersion(9))
        ));
    }

    /// The write guard: first-use binding may only ever claim an *unbound* key. If it
    /// could overwrite, a record whose endpoint was swapped would simply re-bind the
    /// stored password to the new endpoint on its first connect.
    #[test]
    fn first_use_binding_refuses_to_overwrite_an_existing_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        let original = binding("k", ws("/w"), "ssh:password:alice@good:22");
        store.bind_secret_if_unbound(&original).unwrap();
        let err = store
            .bind_secret_if_unbound(&binding("k", ws("/w"), "ssh:password:alice@evil:22"))
            .unwrap_err();
        assert!(matches!(err, StoreError::Conflict(_)), "{err:?}");
        assert_eq!(store.secret_binding("k").unwrap(), Some(original));
    }

    #[test]
    fn an_explicit_rebind_replaces_and_unbind_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        store
            .bind_secret_if_unbound(&binding("k", Scope::Global, "old"))
            .unwrap();
        let new = binding("k", Scope::Global, "new");
        store.rebind_secret(&new).unwrap();
        assert_eq!(store.secret_binding("k").unwrap(), Some(new));
        assert!(store.unbind_secret("k").unwrap());
        assert!(!store.unbind_secret("k").unwrap());
        assert_eq!(store.secret_binding("k").unwrap(), None);
    }
}
