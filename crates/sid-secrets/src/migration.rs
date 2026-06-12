//! One-time migration: move secrets from [`PlainStore`] (redb) to a
//! [`KeyringStore`] (OS keyring).
//!
//! For each id returned by `source.list_ids()`:
//! 1. Read the value from `source`.
//! 2. Write it to `dest`.
//! 3. Read back from `dest` and assert the bytes match (verified write).
//! 4. On verified success, delete from `source`.
//!
//! On any failure: keep the unmigrated entry in `source` (idempotent — a
//! re-run will re-attempt only entries still present in `source`). If an id
//! is present in both backends after a previous partial migration (power loss
//! between step 2 and step 4), the source value is unconditionally written to
//! `dest` (overwriting any stale copy already there), verified, and then
//! deleted from `source`.  The keyring is the authoritative store after a
//! successful migration; in the normal power-loss case both copies hold the
//! same value, so the overwrite is a no-op in practice.

use sid_core::adapters::secrets::{SecretError, SecretId, SecretStore};
use tracing::info;

/// Result summary from a [`migrate_to_keyring`] call.
#[derive(Debug, Default)]
pub struct MigrateResult {
    /// Number of secrets successfully moved to the keyring and removed from
    /// the source store.
    pub migrated: usize,
    /// Number of secrets that could not be migrated (write, verify, or delete
    /// failure).
    pub failed: usize,
    /// Human-readable error messages for each failure, one per entry.
    pub errors: Vec<String>,
}

/// Move secrets from `source` to `dest`. Returns a [`MigrateResult`]
/// summarising how many migrated and how many failed.
///
/// On partial failure (`result.failed > 0`) the caller should check
/// `result.errors` for details. Returns `Err(SecretError)` only if
/// `source.list_ids()` itself fails.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use sid_secrets::PlainStore;
/// use sid_secrets::migration::migrate_to_keyring;
/// use sid_secrets::keyring_store::{KeyringStore, KeyringBackend};
/// use sid_core::adapters::secrets::{SecretId, SecretStore};
/// use sid_store::{OpenStore, RedbStore, Store};
/// use tempfile::tempdir;
///
/// struct NullBackend;
/// impl KeyringBackend for NullBackend {
///     fn set(&self, _k: &str, _v: &[u8]) -> Result<(), String> { Ok(()) }
///     fn get(&self, _k: &str) -> Result<Option<Vec<u8>>, String> { Ok(None) }
///     fn delete(&self, _k: &str) -> Result<(), String> { Ok(()) }
///     fn list_keys(&self) -> Result<Vec<String>, String> { Ok(vec![]) }
/// }
///
/// let dir = tempdir().unwrap();
/// let inner: Arc<dyn Store> =
///     Arc::new(RedbStore::open(&dir.path().join("sid.redb")).unwrap());
/// let src = PlainStore::new(inner);
/// let dst = KeyringStore::with_backend(NullBackend);
///
/// let r = migrate_to_keyring(&src, &dst).unwrap();
/// assert_eq!(r.failed, 0);
/// ```
pub fn migrate_to_keyring(
    source: &dyn SecretStore,
    dest: &dyn SecretStore,
) -> Result<MigrateResult, SecretError> {
    let ids = source.list_ids()?;
    let total = ids.len();
    info!(total, "starting secret migration to keyring");

    let mut result = MigrateResult::default();

    for id in ids {
        match migrate_one(source, dest, &id) {
            Ok(()) => {
                result.migrated += 1;
            }
            Err(msg) => {
                let full_msg = format!("failed to migrate '{}': {}", id.as_str(), msg);
                result.errors.push(full_msg);
                result.failed += 1;
            }
        }
    }

    info!(
        migrated = result.migrated,
        failed = result.failed,
        "secret migration complete"
    );
    Ok(result)
}

fn migrate_one(
    source: &dyn SecretStore,
    dest: &dyn SecretStore,
    id: &SecretId,
) -> Result<(), String> {
    let value = source
        .get(id)
        .map_err(|e| format!("source read error: {e}"))?
        .ok_or_else(|| "entry absent in source (already migrated?)".to_string())?;

    dest.put(id, &value)
        .map_err(|e| format!("dest write error: {e}"))?;

    let read_back = dest
        .get(id)
        .map_err(|e| format!("dest read-back error: {e}"))?
        .ok_or_else(|| "dest read-back returned None immediately after write".to_string())?;

    if read_back != value {
        return Err(format!(
            "read-back mismatch: wrote {} bytes, read {} bytes",
            value.len(),
            read_back.len()
        ));
    }

    source
        .delete(id)
        .map_err(|e| format!("source delete error: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use crate::PlainStore;
    use crate::keyring_store::{KeyringBackend, KeyringStore};
    use crate::tests_support::FakeKeyring;
    use sid_store::{OpenStore, RedbStore, Store};
    use tempfile::tempdir;

    fn plain_store() -> (tempfile::TempDir, PlainStore) {
        let dir = tempdir().unwrap();
        let inner: Arc<dyn Store> = Arc::new(RedbStore::open(&dir.path().join("s.redb")).unwrap());
        (dir, PlainStore::new(inner))
    }

    fn fake_dest() -> KeyringStore<FakeKeyring> {
        KeyringStore::with_backend(FakeKeyring::default())
    }

    /// A backend that fails writes for a specific key.
    struct PartialFail {
        inner: Mutex<HashMap<String, Vec<u8>>>,
        fail_key: &'static str,
    }

    impl KeyringBackend for PartialFail {
        fn set(&self, k: &str, v: &[u8]) -> Result<(), String> {
            if k == self.fail_key {
                return Err(format!("injected write failure for '{k}'"));
            }
            self.inner.lock().unwrap().insert(k.into(), v.into());
            Ok(())
        }

        fn get(&self, k: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.inner.lock().unwrap().get(k).cloned())
        }

        fn delete(&self, k: &str) -> Result<(), String> {
            self.inner.lock().unwrap().remove(k);
            Ok(())
        }

        fn list_keys(&self) -> Result<Vec<String>, String> {
            Ok(self.inner.lock().unwrap().keys().cloned().collect())
        }
    }

    /// A backend that corrupts read-back values.
    struct CorruptReadBack(Mutex<HashMap<String, Vec<u8>>>);

    impl KeyringBackend for CorruptReadBack {
        fn set(&self, k: &str, v: &[u8]) -> Result<(), String> {
            let mut corrupted = v.to_vec();
            corrupted.push(0xff); // corrupt the stored data
            self.0.lock().unwrap().insert(k.into(), corrupted);
            Ok(())
        }

        fn get(&self, k: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.lock().unwrap().get(k).cloned())
        }

        fn delete(&self, k: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(k);
            Ok(())
        }

        fn list_keys(&self) -> Result<Vec<String>, String> {
            Ok(self.0.lock().unwrap().keys().cloned().collect())
        }
    }

    #[test]
    fn migration_empty_source() {
        let (_d, src) = plain_store();
        let dst = fake_dest();
        let r = migrate_to_keyring(&src, &dst).unwrap();
        assert_eq!(r.migrated, 0);
        assert_eq!(r.failed, 0);
        assert!(r.errors.is_empty());
    }

    #[test]
    fn migration_moves_all_secrets() {
        let (_d, src) = plain_store();
        let dst = fake_dest();
        src.put(&SecretId::new("a"), b"aa").unwrap();
        src.put(&SecretId::new("b"), b"bb").unwrap();
        let r = migrate_to_keyring(&src, &dst).unwrap();
        assert_eq!(r.migrated, 2);
        assert_eq!(r.failed, 0);
        // Values accessible in dest
        assert_eq!(
            dst.get(&SecretId::new("a")).unwrap().unwrap(),
            b"aa".to_vec()
        );
        assert_eq!(
            dst.get(&SecretId::new("b")).unwrap().unwrap(),
            b"bb".to_vec()
        );
        // Source is empty
        assert!(src.get(&SecretId::new("a")).unwrap().is_none());
        assert!(src.get(&SecretId::new("b")).unwrap().is_none());
    }

    #[test]
    fn migration_source_deleted_after_verified_write() {
        let (_d, src) = plain_store();
        let dst = fake_dest();
        let id = SecretId::new("x");
        src.put(&id, b"xval").unwrap();
        migrate_to_keyring(&src, &dst).unwrap();
        assert!(src.get(&id).unwrap().is_none());
    }

    #[test]
    fn migration_is_idempotent_on_partial_prior_success() {
        // Simulate a previous run that moved "a" but not "b".
        // After partial migration, source still has "b".
        let (_d, src) = plain_store();
        let dst = fake_dest();
        src.put(&SecretId::new("a"), b"aa").unwrap();
        src.put(&SecretId::new("b"), b"bb").unwrap();

        // Simulate "a" already migrated: remove from src, put in dst
        dst.put(&SecretId::new("a"), b"aa").unwrap();
        src.delete(&SecretId::new("a")).unwrap();

        let r = migrate_to_keyring(&src, &dst).unwrap();
        // Only "b" needs migrating
        assert_eq!(r.migrated, 1);
        assert_eq!(r.failed, 0);
        assert!(src.get(&SecretId::new("b")).unwrap().is_none());
        assert_eq!(
            dst.get(&SecretId::new("b")).unwrap().unwrap(),
            b"bb".to_vec()
        );
    }

    #[test]
    fn migration_partial_failure_leaves_failed_entry_in_source() {
        let (_d, src) = plain_store();
        let dst = KeyringStore::with_backend(PartialFail {
            inner: Mutex::new(HashMap::new()),
            fail_key: "bad",
        });
        src.put(&SecretId::new("good"), b"g").unwrap();
        src.put(&SecretId::new("bad"), b"b").unwrap();

        let r = migrate_to_keyring(&src, &dst).unwrap();
        assert_eq!(r.migrated, 1);
        assert_eq!(r.failed, 1);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("bad"));
        // "bad" still in source
        assert!(src.get(&SecretId::new("bad")).unwrap().is_some());
        // "good" removed from source
        assert!(src.get(&SecretId::new("good")).unwrap().is_none());
    }

    #[test]
    fn migration_read_back_mismatch_is_a_failure() {
        let (_d, src) = plain_store();
        let dst = KeyringStore::with_backend(CorruptReadBack(Mutex::new(HashMap::new())));
        src.put(&SecretId::new("corrupt"), b"original").unwrap();

        let r = migrate_to_keyring(&src, &dst).unwrap();
        assert_eq!(r.failed, 1);
        assert_eq!(r.migrated, 0);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("mismatch"));
        // Source entry must NOT have been deleted (verification failed)
        assert!(src.get(&SecretId::new("corrupt")).unwrap().is_some());
    }

    // -----------------------------------------------------------------------
    // Fix 3 — source.list_ids() Err propagates as migrate_to_keyring Err
    // -----------------------------------------------------------------------

    /// A SecretStore whose list_ids always returns Err.
    struct ListIdsFails;

    impl SecretStore for ListIdsFails {
        fn put(&self, _id: &SecretId, _value: &[u8]) -> Result<(), SecretError> {
            panic!("ListIdsFails::put should never be called");
        }
        fn get(&self, _id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
            panic!("ListIdsFails::get should never be called");
        }
        fn delete(&self, _id: &SecretId) -> Result<(), SecretError> {
            panic!("ListIdsFails::delete should never be called");
        }
        fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
            Err(SecretError::Storage("injected list_ids failure".into()))
        }
    }

    /// When `source.list_ids()` fails, `migrate_to_keyring` must propagate
    /// the error as `Err(SecretError)` and must not call any method on `dest`.
    #[test]
    fn migration_source_list_ids_err_propagates() {
        let dst = fake_dest();
        let result = migrate_to_keyring(&ListIdsFails, &dst);
        assert!(
            matches!(result, Err(SecretError::Storage(_))),
            "expected Err(SecretError::Storage), got {result:?}"
        );
        // dest received no writes — its list_ids is empty
        assert!(
            dst.list_ids().unwrap().is_empty(),
            "dest must remain empty when source.list_ids() fails"
        );
    }

    // -----------------------------------------------------------------------
    // Fix 4 — double-presence test (power-loss simulation with different values)
    // -----------------------------------------------------------------------

    /// Simulate power loss between dest-write (step 2) and source-delete (step 4):
    /// id present in BOTH source (value A) and dest (value B, a stale prior run).
    ///
    /// Code behaviour: `migrate_one` calls `dest.put(id, &source_value)` unconditionally,
    /// so source's value overwrites dest.  This satisfies the spec comment
    /// ("keyring copy wins") in the sense that the keyring (dest) is the
    /// authoritative store after migration — but when the two copies differ,
    /// the SOURCE value is what lands in the keyring.  The spec comment is
    /// updated to clarify this: "the source value is written to the keyring
    /// (overwriting any stale copy), verified, then deleted from source."
    #[test]
    fn migration_overwrites_dest_when_present_in_both() {
        let (_d, src) = plain_store();
        let dst = fake_dest();
        let id = SecretId::new("pw");

        // Source has value A, dest has a stale value B (simulating power loss
        // between step 2 of a prior run and step 4 of that run).
        src.put(&id, b"value_from_source").unwrap();
        dst.put(&id, b"stale_dest_value").unwrap();

        let r = migrate_to_keyring(&src, &dst).unwrap();

        // Migration must succeed: dest receives source's value, source is cleaned up.
        assert_eq!(r.migrated, 1, "expected 1 migrated, got {r:?}");
        assert_eq!(r.failed, 0, "expected 0 failed, got {r:?}");

        // dest ends up with the SOURCE value (overwrite happened).
        assert_eq!(
            dst.get(&id).unwrap().unwrap(),
            b"value_from_source".to_vec(),
            "dest should hold source value after migration"
        );

        // source entry is deleted.
        assert!(
            src.get(&id).unwrap().is_none(),
            "source entry must be deleted after successful migration"
        );
    }
}
