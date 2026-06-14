//! One-time migration: move secrets from [`PlainStore`](crate::PlainStore)
//! (redb) to a [`KeyringStore`](crate::KeyringStore) (OS keyring).
//!
//! For each id returned by `source.list_ids()`:
//! 1. Read the value from `source`.
//! 2. Write it to `dest`.
//! 3. Read back from `dest` and assert the bytes match (verified write).
//! 4. On verified success, delete from `source`.
//!
//! On any failure: keep the unmigrated entry in `source` (idempotent — a
//! re-run will re-attempt only entries still present in `source`).
//!
//! ## Double-presence (an id already in `dest`)
//!
//! If an id is present in BOTH backends after a previous partial migration
//! (power loss between step 2 and step 4), `migrate_one` reads the existing
//! `dest` value first and compares:
//!
//! - **Equal** (the normal power-loss case): the copies already agree, so the
//!   re-write is skipped and `source` is simply deleted. This is the cheap,
//!   silent path.
//! - **Divergent**: `source` and `dest` hold *different* values for the same
//!   id. The source value wins (spec-aligned: a fresh migration treats the
//!   redb plaintext store as the source of truth), so `dest` is overwritten,
//!   re-verified, and `source` deleted — but the overwrite is **not silent**:
//!   it emits a `tracing::warn!` naming the id and increments
//!   [`MigrateResult::conflicts`] so the caller can surface it in a toast.

use sid_core::adapters::secrets::{SecretError, SecretId, SecretStore};
use tracing::{info, warn};

/// Result summary from a [`migrate_to_keyring`] call.
#[derive(Debug, Default)]
pub struct MigrateResult {
    /// Number of secrets successfully moved to the keyring and removed from
    /// the source store.
    pub migrated: usize,
    /// Number of secrets that could not be migrated (write, verify, or delete
    /// failure).
    pub failed: usize,
    /// Number of secrets that existed in BOTH stores with *different* values
    /// and were resolved source-wins (the keyring copy was overwritten with
    /// the redb value). These are counted within `migrated` as well; this
    /// field exists so the caller can warn the user that a divergence was
    /// silently resolved. Equal-value double-presence does NOT count here.
    pub conflicts: usize,
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
            Ok(outcome) => {
                result.migrated += 1;
                if outcome == MigrateOne::Conflict {
                    result.conflicts += 1;
                }
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
        conflicts = result.conflicts,
        "secret migration complete"
    );
    Ok(result)
}

/// Outcome of migrating a single id, distinguishing a clean move from a
/// source-wins overwrite of a divergent `dest` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrateOne {
    /// Normal path: written (or already-equal) and source deleted.
    Clean,
    /// `dest` already held a *different* value; source-wins overwrite applied.
    Conflict,
}

fn migrate_one(
    source: &dyn SecretStore,
    dest: &dyn SecretStore,
    id: &SecretId,
) -> Result<MigrateOne, String> {
    let value = source
        .get(id)
        .map_err(|e| format!("source read error: {e}"))?
        .ok_or_else(|| "entry absent in source (already migrated?)".to_string())?;

    // Inspect dest first so equal double-presence is a cheap delete-only path
    // and divergence is detected (and warned about) rather than silently
    // overwriting the keyring with a stale redb value.
    let existing = dest
        .get(id)
        .map_err(|e| format!("dest pre-read error: {e}"))?;

    let outcome = match existing {
        // Already present and identical: nothing to write, just clean source.
        Some(ref d) if *d == value => MigrateOne::Clean,
        // Present but different: source-wins, but this is a real divergence —
        // make it loud (warn + conflict count) so it is never silent.
        Some(_) => {
            warn!(
                id = id.as_str(),
                "secret diverged between redb and keyring; source-wins, \
                 overwriting keyring copy"
            );
            write_and_verify(dest, id, &value)?;
            MigrateOne::Conflict
        }
        // Not present: ordinary write + verify.
        None => {
            write_and_verify(dest, id, &value)?;
            MigrateOne::Clean
        }
    };

    source
        .delete(id)
        .map_err(|e| format!("source delete error: {e}"))?;

    Ok(outcome)
}

/// Write `value` to `dest` under `id` and verify the read-back matches.
fn write_and_verify(dest: &dyn SecretStore, id: &SecretId, value: &[u8]) -> Result<(), String> {
    dest.put(id, value)
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
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use sid_store::{OpenStore, RedbStore, Store};
    use tempfile::tempdir;

    use super::*;
    use crate::{
        PlainStore,
        keyring_store::{KeyringBackend, KeyringStore},
        tests_support::FakeKeyring,
    };

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

    /// Fix I2: a secret written to the plain store AFTER a completed migration
    /// (e.g. during a fallback or keyring-toggle-off session) is picked up by
    /// the next migration sweep. Because `main.rs` now runs the sweep on every
    /// keyring-active startup regardless of the done-flag, this models that
    /// second sweep: run once to drain, add a new secret to source, run again.
    #[test]
    fn migration_sweeps_secrets_added_after_a_completed_run() {
        let (_d, src) = plain_store();
        let dst = fake_dest();

        // First sweep drains the initial secret.
        src.put(&SecretId::new("first"), b"1").unwrap();
        let r1 = migrate_to_keyring(&src, &dst).unwrap();
        assert_eq!(r1.migrated, 1);
        assert!(src.get(&SecretId::new("first")).unwrap().is_none());

        // A later session (fallback / toggle-off) writes a NEW secret to the
        // plain store. The done-flag would have been set after r1, but the
        // sweep runs anyway.
        src.put(&SecretId::new("later"), b"2").unwrap();

        // Second sweep migrates the newly-added secret.
        let r2 = migrate_to_keyring(&src, &dst).unwrap();
        assert_eq!(r2.migrated, 1, "later-added secret must be swept up");
        assert_eq!(r2.failed, 0);
        assert!(
            src.get(&SecretId::new("later")).unwrap().is_none(),
            "later secret must be removed from source after migration"
        );
        assert_eq!(
            dst.get(&SecretId::new("later")).unwrap().unwrap(),
            b"2".to_vec(),
            "later secret must be readable from keyring"
        );
    }

    /// A sweep over an already-drained source is a clean no-op (the cheap path
    /// that justifies running it on every startup per Fix I2).
    #[test]
    fn migration_repeated_sweep_on_empty_source_is_noop() {
        let (_d, src) = plain_store();
        let dst = fake_dest();
        src.put(&SecretId::new("x"), b"v").unwrap();
        assert_eq!(migrate_to_keyring(&src, &dst).unwrap().migrated, 1);
        // Source now empty; a second sweep migrates nothing and fails nothing.
        let r = migrate_to_keyring(&src, &dst).unwrap();
        assert_eq!(r.migrated, 0);
        assert_eq!(r.failed, 0);
        assert_eq!(r.conflicts, 0);
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
        assert_eq!(r.conflicts, 0, "no divergence — must not be a conflict");
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
    // Fix I4 — double-presence: equal → delete-only, divergent → source-wins
    //           but counted as a conflict and warned about (never silent).
    // -----------------------------------------------------------------------

    /// A backend that counts `put` calls so a test can prove the equal-value
    /// double-presence path skips the write entirely (delete-only).
    ///
    /// The counter is behind a shared `Arc` so the test keeps a handle to it
    /// after the backend is moved into `KeyringStore::with_backend` — the
    /// post-migration count is the assertion that matters.
    struct CountingPut {
        inner: Mutex<HashMap<String, Vec<u8>>>,
        puts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl CountingPut {
        fn new() -> Self {
            Self {
                inner: Mutex::new(HashMap::new()),
                puts: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
        /// Clone the counter handle; survives moving the backend into a store.
        fn puts_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
            std::sync::Arc::clone(&self.puts)
        }
    }

    impl KeyringBackend for CountingPut {
        fn set(&self, k: &str, v: &[u8]) -> Result<(), String> {
            self.puts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

    /// Equal double-presence (the normal power-loss case): id present in both
    /// stores with the SAME value. Must NOT re-write dest (delete-only path,
    /// asserted via the put-counter), must NOT count as a conflict, and must
    /// delete the source entry.
    #[test]
    fn migration_equal_double_presence_skips_dest_write() {
        let (_d, src) = plain_store();
        let counting = CountingPut::new();
        let puts = counting.puts_handle();
        let id = SecretId::new("pw");
        src.put(&id, b"same_value").unwrap();
        // Seed dest directly through the backend (1 put) so both agree.
        counting.set(id.as_str(), b"same_value").unwrap();
        assert_eq!(
            puts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "seed put"
        );

        let dst = KeyringStore::with_backend(counting);
        let r = migrate_to_keyring(&src, &dst).unwrap();

        assert_eq!(r.migrated, 1);
        assert_eq!(r.conflicts, 0);
        // THE assertion this test exists for: no additional put happened
        // during migration — the equal double-presence path is delete-only.
        assert_eq!(
            puts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "equal-value double presence must skip the dest write"
        );
        assert_eq!(dst.get(&id).unwrap().unwrap(), b"same_value".to_vec());
        assert!(src.get(&id).unwrap().is_none());
    }

    /// Divergent double-presence: id present in both stores with DIFFERENT
    /// values (power loss between dest-write of a prior run and a later edit of
    /// the redb copy). Source-wins per spec, but it must be counted as a
    /// conflict (and a `tracing::warn!` is emitted — not asserted here).
    #[test]
    fn migration_divergent_double_presence_is_source_wins_conflict() {
        let (_d, src) = plain_store();
        let dst = fake_dest();
        let id = SecretId::new("pw");

        // Source has value A, dest has a DIFFERENT stale value B.
        src.put(&id, b"value_from_source").unwrap();
        dst.put(&id, b"stale_dest_value").unwrap();

        let r = migrate_to_keyring(&src, &dst).unwrap();

        assert_eq!(r.migrated, 1, "expected 1 migrated, got {r:?}");
        assert_eq!(r.failed, 0, "expected 0 failed, got {r:?}");
        assert_eq!(
            r.conflicts, 1,
            "divergence must be reported as exactly one conflict, got {r:?}"
        );

        // dest ends up with the SOURCE value (source-wins overwrite happened).
        assert_eq!(
            dst.get(&id).unwrap().unwrap(),
            b"value_from_source".to_vec(),
            "dest should hold source value after a divergent migration"
        );
        // source entry is deleted.
        assert!(
            src.get(&id).unwrap().is_none(),
            "source entry must be deleted after successful migration"
        );
    }
}
