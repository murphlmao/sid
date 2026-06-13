//! `sid-secrets`: concrete secret-store adapters.
//!
//! This crate provides [`PlainStore`], the always-available file-backed
//! [`SecretStore`] implementation. It wraps any `Arc<dyn Store>` from
//! `sid-store` and routes secret operations to the `secrets` table. There is
//! no encryption at rest; that responsibility is left to the operating
//! system's file permissions on the redb file.
//!
//! [`KeyringStore`] is the OS-keyring-backed impl (keyring v4 / keyring-core):
//! call [`install_default_backend`] once at startup to register the platform
//! store (zbus Secret Service on Linux, legacy Keychain on macOS), then use
//! [`default_backend_is_durable`] to confirm a real (non-ephemeral) store was
//! registered before trusting the keyring with secrets.

use std::sync::Arc;

use sid_core::SidError;
use sid_core::adapters::secrets::{SecretError, SecretId, SecretStore};
use sid_store::Store;

pub mod keyring_store;
pub mod migration;

pub use keyring_store::{KeyringStore, default_backend_is_durable, install_default_backend};

/// File-backed [`SecretStore`] using the `secrets` table of a `sid-store`
/// [`Store`].
///
/// `PlainStore` does not encrypt values; the on-disk redb file's filesystem
/// permissions are the only protection. Callers that need stronger guarantees
/// should layer a keychain-backed implementation on top.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use sid_core::adapters::secrets::{SecretId, SecretStore};
/// use sid_secrets::PlainStore;
/// use sid_store::{OpenStore, RedbStore, Store};
/// use tempfile::tempdir;
///
/// let dir = tempdir().unwrap();
/// let inner: Arc<dyn Store> =
///     Arc::new(RedbStore::open(&dir.path().join("sid.redb")).unwrap());
/// let secrets = PlainStore::new(inner);
///
/// let id = SecretId::new("ssh.key.id_ed25519");
/// secrets.put(&id, b"passphrase").unwrap();
/// assert_eq!(secrets.get(&id).unwrap().unwrap(), b"passphrase".to_vec());
/// ```
pub struct PlainStore {
    inner: Arc<dyn Store>,
}

impl PlainStore {
    /// Wrap an `Arc<dyn Store>` as a `SecretStore`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use sid_secrets::PlainStore;
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let inner: Arc<dyn Store> =
    ///     Arc::new(RedbStore::open(&dir.path().join("sid.redb")).unwrap());
    /// let _ = PlainStore::new(inner);
    /// ```
    pub fn new(inner: Arc<dyn Store>) -> Self {
        Self { inner }
    }
}

fn sid_to_secret_err(e: SidError) -> SecretError {
    SecretError::Storage(format!("{e}"))
}

impl SecretStore for PlainStore {
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
        self.inner
            .secret_put(id.as_str(), value)
            .map_err(sid_to_secret_err)
    }

    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
        self.inner
            .secret_get(id.as_str())
            .map_err(sid_to_secret_err)
    }

    fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
        self.inner
            .secret_delete(id.as_str())
            .map_err(sid_to_secret_err)
    }

    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
        let ids = self.inner.list_secret_ids().map_err(sid_to_secret_err)?;
        Ok(ids.into_iter().map(SecretId::new).collect())
    }
}

/// Test-only fake keyring backend. Never compiled outside `#[cfg(test)]`.
#[cfg(test)]
pub(crate) mod tests_support {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::keyring_store::KeyringBackend;

    /// In-memory fake that satisfies [`KeyringBackend`] without touching the
    /// OS keyring daemon. All operations are guarded by a `Mutex` so tests
    /// can construct `KeyringStore<FakeKeyring>` and verify `Send + Sync`.
    #[derive(Default)]
    pub struct FakeKeyring {
        inner: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl KeyringBackend for FakeKeyring {
        fn set(&self, key: &str, secret: &[u8]) -> Result<(), String> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), secret.to_vec());
            Ok(())
        }

        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }

        fn delete(&self, key: &str) -> Result<(), String> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }

        fn list_keys(&self) -> Result<Vec<String>, String> {
            Ok(self.inner.lock().unwrap().keys().cloned().collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_store::{OpenStore, RedbStore};
    use tempfile::tempdir;

    fn assert_send_sync<T: Send + Sync>() {}

    fn fresh() -> (tempfile::TempDir, PlainStore) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sid.redb");
        let inner: Arc<dyn Store> = Arc::new(RedbStore::open(&path).unwrap());
        (dir, PlainStore::new(inner))
    }

    #[test]
    fn plain_store_put_then_get() {
        let (_d, s) = fresh();
        let id = SecretId::new("a");
        s.put(&id, b"hello").unwrap();
        assert_eq!(s.get(&id).unwrap().unwrap(), b"hello".to_vec());
    }

    #[test]
    fn plain_store_get_missing_returns_none() {
        let (_d, s) = fresh();
        assert!(s.get(&SecretId::new("missing")).unwrap().is_none());
    }

    #[test]
    fn plain_store_delete_then_get_returns_none() {
        let (_d, s) = fresh();
        let id = SecretId::new("doomed");
        s.put(&id, b"value").unwrap();
        s.delete(&id).unwrap();
        assert!(s.get(&id).unwrap().is_none());
    }

    #[test]
    fn plain_store_delete_missing_is_noop() {
        let (_d, s) = fresh();
        s.delete(&SecretId::new("never.was")).unwrap();
    }

    #[test]
    fn plain_store_is_send_sync() {
        assert_send_sync::<PlainStore>();
        assert_send_sync::<&dyn SecretStore>();
    }
}
