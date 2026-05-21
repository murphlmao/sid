//! `RedbStore`: the redb-backed implementation of the `Store` trait.
//!
//! All write operations open a write transaction, mutate, and commit before
//! returning. This is intentionally simple — no batching or connection pooling
//! for Plan 1.

use std::path::Path;

use redb::{Database, ReadableDatabase, ReadableTable};
use sid_core::tab::TabId;
use sid_core::widget::WidgetId;
use sid_core::SidError;

use crate::schema::{SECRETS, SESSION_META, SESSIONS, SETTINGS, WIDGET_STATE, WORKSPACES};
use crate::{OpenStore, SessionRecord, SettingValue, Store, Workspace, WidgetState};

/// redb-backed implementation of [`crate::Store`].
///
/// Open with [`OpenStore::open`]; all methods are thread-safe via redb's
/// internal transaction model.
pub struct RedbStore {
    db: Database,
}

impl OpenStore for RedbStore {
    /// Open (or create) the redb database at `path`, creating all four tables
    /// if they do not already exist.
    ///
    /// # Errors
    ///
    /// Returns `SidError::Storage` if the file cannot be created, the
    /// directory does not exist, or permissions are insufficient.
    fn open(path: &Path) -> Result<Self, SidError> {
        let db = Database::create(path)
            .map_err(|e| SidError::Storage(format!("redb open {path:?}: {e}")))?;
        // Ensure tables exist by opening each in a write transaction.
        let txn = db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("begin_write: {e}")))?;
        {
            let _ = txn
                .open_table(SETTINGS)
                .map_err(|e| SidError::Storage(format!("open settings: {e}")))?;
            let _ = txn
                .open_table(SESSIONS)
                .map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
            let _ = txn
                .open_table(SESSION_META)
                .map_err(|e| SidError::Storage(format!("open session_meta: {e}")))?;
            let _ = txn
                .open_table(WIDGET_STATE)
                .map_err(|e| SidError::Storage(format!("open widget_state: {e}")))?;
            let _ = txn
                .open_table(WORKSPACES)
                .map_err(|e| SidError::Storage(format!("open workspaces: {e}")))?;
            let _ = txn
                .open_table(SECRETS)
                .map_err(|e| SidError::Storage(format!("open secrets: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit: {e}")))?;
        Ok(Self { db })
    }
}

impl RedbStore {
    /// Expose the underlying `Database` for internal use.
    #[allow(dead_code)]
    pub(crate) fn raw(&self) -> &Database {
        &self.db
    }
}

impl Store for RedbStore {
    /// Retrieve a setting value by key. Returns `None` if not set.
    fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn
            .open_table(SETTINGS)
            .map_err(|e| SidError::Storage(format!("open settings: {e}")))?;
        let got = tbl
            .get(key)
            .map_err(|e| SidError::Storage(format!("get setting: {e}")))?;
        match got {
            None => Ok(None),
            Some(guard) => Ok(Some(SettingValue(guard.value().to_vec()))),
        }
    }

    /// Persist a setting value. Overwrites any existing value for the key.
    fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError> {
        let txn = self
            .db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut tbl = txn
                .open_table(SETTINGS)
                .map_err(|e| SidError::Storage(format!("open settings: {e}")))?;
            tbl.insert(key, &val.0[..])
                .map_err(|e| SidError::Storage(format!("insert setting: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit settings: {e}")))?;
        Ok(())
    }

    fn current_session(&self) -> Result<Option<SessionRecord>, SidError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let meta = txn
            .open_table(SESSION_META)
            .map_err(|e| SidError::Storage(format!("open session_meta: {e}")))?;
        let current_guard = meta
            .get("current")
            .map_err(|e| SidError::Storage(format!("get current: {e}")))?;
        let Some(current_guard) = current_guard else {
            return Ok(None);
        };
        let id_bytes: Vec<u8> = current_guard.value().to_vec();
        drop(current_guard);
        let id_str = std::str::from_utf8(&id_bytes)
            .map_err(|_| SidError::Storage("non-utf8 session id".into()))?;
        let tbl = txn
            .open_table(SESSIONS)
            .map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
        let blob_guard = tbl
            .get(id_str)
            .map_err(|e| SidError::Storage(format!("get session: {e}")))?;
        let Some(blob_guard) = blob_guard else {
            return Ok(None);
        };
        let blob_bytes: Vec<u8> = blob_guard.value().to_vec();
        drop(blob_guard);
        let (_version, rec) =
            crate::codec::decode_versioned::<SessionRecord>(&blob_bytes)?;
        Ok(Some(rec))
    }

    fn upsert_session(&self, s: &SessionRecord) -> Result<(), SidError> {
        let bytes = crate::codec::encode_versioned(1, s)?;
        let txn = self
            .db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut sess = txn
                .open_table(SESSIONS)
                .map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
            sess.insert(s.id.as_str(), &bytes[..])
                .map_err(|e| SidError::Storage(format!("insert session: {e}")))?;
            let mut meta = txn
                .open_table(SESSION_META)
                .map_err(|e| SidError::Storage(format!("open session_meta: {e}")))?;
            meta.insert("current", s.id.as_bytes())
                .map_err(|e| SidError::Storage(format!("set current: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit session: {e}")))?;
        Ok(())
    }

    fn end_session(&self, id: &str, ended_at: crate::Epoch) -> Result<(), SidError> {
        let txn = self
            .db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut sess = txn
                .open_table(SESSIONS)
                .map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
            // Read existing bytes, then drop the guard before mutating.
            let existing_bytes: Option<Vec<u8>> = {
                let guard = sess
                    .get(id)
                    .map_err(|e| SidError::Storage(format!("get session: {e}")))?;
                guard.map(|g| g.value().to_vec())
            };
            let Some(existing_bytes) = existing_bytes else {
                return Ok(()); // no-op: session not found
            };
            let (_v, mut rec) =
                crate::codec::decode_versioned::<SessionRecord>(&existing_bytes)?;
            rec.ended_at = Some(ended_at);
            let bytes = crate::codec::encode_versioned(1, &rec)?;
            sess.insert(id, &bytes[..])
                .map_err(|e| SidError::Storage(format!("update session: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit end_session: {e}")))?;
        Ok(())
    }

    fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn
            .open_table(SESSIONS)
            .map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
        let mut out = Vec::new();
        let iter = tbl
            .iter()
            .map_err(|e| SidError::Storage(format!("iter sessions: {e}")))?;
        for entry in iter {
            let (_k, v) = entry
                .map_err(|e| SidError::Storage(format!("iter step: {e}")))?;
            let blob: Vec<u8> = v.value().to_vec();
            let (_ver, rec) = crate::codec::decode_versioned::<SessionRecord>(&blob)?;
            out.push(rec);
        }
        Ok(out)
    }

    fn save_widget_state(&self, s: &WidgetState) -> Result<(), SidError> {
        let key = format!("{}\0{}", s.tab_id.as_str(), s.widget_id.as_str());
        let txn = self
            .db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut tbl = txn
                .open_table(WIDGET_STATE)
                .map_err(|e| SidError::Storage(format!("open widget_state: {e}")))?;
            tbl.insert(key.as_str(), &s.blob[..])
                .map_err(|e| SidError::Storage(format!("insert widget_state: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit widget_state: {e}")))?;
        Ok(())
    }

    fn load_widget_state(
        &self,
        tab: &TabId,
        widget: &WidgetId,
    ) -> Result<Option<Vec<u8>>, SidError> {
        let key = format!("{}\0{}", tab.as_str(), widget.as_str());
        let txn = self
            .db
            .begin_read()
            .map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn
            .open_table(WIDGET_STATE)
            .map_err(|e| SidError::Storage(format!("open widget_state: {e}")))?;
        let got = tbl
            .get(key.as_str())
            .map_err(|e| SidError::Storage(format!("get widget_state: {e}")))?;
        match got {
            None => Ok(None),
            Some(guard) => Ok(Some(guard.value().to_vec())),
        }
    }

    fn list_workspaces(&self) -> Result<Vec<Workspace>, SidError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn
            .open_table(WORKSPACES)
            .map_err(|e| SidError::Storage(format!("open workspaces: {e}")))?;
        let mut out = Vec::new();
        let iter = tbl
            .iter()
            .map_err(|e| SidError::Storage(format!("iter workspaces: {e}")))?;
        for entry in iter {
            let (_k, v) =
                entry.map_err(|e| SidError::Storage(format!("iter step: {e}")))?;
            let (_ver, w) = crate::codec::decode_versioned::<Workspace>(v.value())?;
            out.push(w);
        }
        Ok(out)
    }

    fn upsert_workspace(&self, w: &Workspace) -> Result<(), SidError> {
        let bytes = crate::codec::encode_versioned(1, w)?;
        let key = w.path.to_string_lossy().to_string();
        let txn = self
            .db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut tbl = txn
                .open_table(WORKSPACES)
                .map_err(|e| SidError::Storage(format!("open workspaces: {e}")))?;
            tbl.insert(key.as_str(), &bytes[..])
                .map_err(|e| SidError::Storage(format!("insert workspace: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit workspace: {e}")))?;
        Ok(())
    }

    fn get_workspace(&self, path: &std::path::Path) -> Result<Option<Workspace>, SidError> {
        let key = path.to_string_lossy().to_string();
        let txn = self
            .db
            .begin_read()
            .map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn
            .open_table(WORKSPACES)
            .map_err(|e| SidError::Storage(format!("open workspaces: {e}")))?;
        let got = tbl
            .get(key.as_str())
            .map_err(|e| SidError::Storage(format!("get workspace: {e}")))?;
        match got {
            Some(v) => {
                let (_ver, w) = crate::codec::decode_versioned::<Workspace>(v.value())?;
                Ok(Some(w))
            }
            None => Ok(None),
        }
    }

    fn remove_workspace(&self, path: &std::path::Path) -> Result<(), SidError> {
        let key = path.to_string_lossy().to_string();
        let txn = self
            .db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut tbl = txn
                .open_table(WORKSPACES)
                .map_err(|e| SidError::Storage(format!("open workspaces: {e}")))?;
            tbl.remove(key.as_str())
                .map_err(|e| SidError::Storage(format!("remove workspace: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit remove: {e}")))?;
        Ok(())
    }

    fn secret_put(&self, id: &str, value: &[u8]) -> Result<(), SidError> {
        let txn = self
            .db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut tbl = txn
                .open_table(SECRETS)
                .map_err(|e| SidError::Storage(format!("open secrets: {e}")))?;
            tbl.insert(id, value)
                .map_err(|e| SidError::Storage(format!("insert secret: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit secret: {e}")))?;
        Ok(())
    }

    fn secret_get(&self, id: &str) -> Result<Option<Vec<u8>>, SidError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn
            .open_table(SECRETS)
            .map_err(|e| SidError::Storage(format!("open secrets: {e}")))?;
        let got = tbl
            .get(id)
            .map_err(|e| SidError::Storage(format!("get secret: {e}")))?;
        match got {
            None => Ok(None),
            Some(guard) => Ok(Some(guard.value().to_vec())),
        }
    }

    fn secret_delete(&self, id: &str) -> Result<(), SidError> {
        let txn = self
            .db
            .begin_write()
            .map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut tbl = txn
                .open_table(SECRETS)
                .map_err(|e| SidError::Storage(format!("open secrets: {e}")))?;
            tbl.remove(id)
                .map_err(|e| SidError::Storage(format!("remove secret: {e}")))?;
        }
        txn.commit()
            .map_err(|e| SidError::Storage(format!("commit secret remove: {e}")))?;
        Ok(())
    }

    fn list_secret_ids(&self) -> Result<Vec<String>, SidError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn
            .open_table(SECRETS)
            .map_err(|e| SidError::Storage(format!("open secrets: {e}")))?;
        let mut out = Vec::new();
        let iter = tbl
            .iter()
            .map_err(|e| SidError::Storage(format!("iter secrets: {e}")))?;
        for entry in iter {
            let (k, _v) =
                entry.map_err(|e| SidError::Storage(format!("iter step: {e}")))?;
            out.push(k.value().to_string());
        }
        Ok(out)
    }
}
