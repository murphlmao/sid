//! `sid-secrets` — the secret-storage adapter seam.
//!
//! Secrets never live in the committed config or the redb store. A config record holds
//! only an opaque [`SecretId`] (`secret_ref`); the actual bytes live behind a
//! [`SecretStore`]. This crate defines that trait plus three implementations:
//! [`MemorySecretStore`] (non-persistent, tests + final fallback), [`keyring::KeyringStore`]
//! (the OS Secret Service), and [`file::EncryptedFileStore`] (a dependency-less
//! passphrase-protected vault — a peer to the keyring, not a lesser fallback).
//! [`resolve::resolve_secret_store`] is the entry point that picks between them per the
//! user's toggles plus a startup keyring durability probe.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use thiserror::Error;

pub mod file;
pub mod keyring;
pub mod resolve;

pub use file::EncryptedFileStore;
pub use resolve::{
    BackendKind, KeyringProbe, Resolved, SecretBackendToggles, probe_keyring, resolve_secret_store,
};

/// An opaque reference to a secret, e.g. `"ssh.prod.key"`. This is all that ever appears
/// in committed config.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretId(pub String);

impl SecretId {
    /// Construct from anything string-like.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Errors from a secret backend.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The underlying backend failed.
    #[error("secret backend: {0}")]
    Backend(String),
    /// [`file::EncryptedFileStore`] exists but hasn't been unlocked this session (or
    /// hasn't been created yet). The app should show the unlock/create modal
    /// (`ui::secret_unlock` in the `sid` crate) and retry the operation after
    /// [`file::EncryptedFileStore::unlock`] or `::create` succeeds.
    #[error("secret vault is locked — unlock it first")]
    Locked,
}

/// A backend that stores opaque secret bytes keyed by [`SecretId`].
pub trait SecretStore: Send + Sync {
    /// Store (or overwrite) `value` under `id`.
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError>;
    /// Fetch the bytes for `id`, or `None` if absent.
    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError>;
    /// Delete `id` (no-op if absent).
    fn delete(&self, id: &SecretId) -> Result<(), SecretError>;
    /// List every stored id.
    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError>;
}

/// Delegate through an `Arc`, so a concrete store can be shared (e.g. the app keeps an
/// `Arc<EncryptedFileStore>` to drive the unlock modal's `unlock`/`create` calls) while
/// also being handed out as the common `Box<dyn SecretStore>` every other call site
/// uses — see [`resolve::resolve_secret_store`].
impl<T: SecretStore + ?Sized> SecretStore for Arc<T> {
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
        (**self).put(id, value)
    }
    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
        (**self).get(id)
    }
    fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
        (**self).delete(id)
    }
    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
        (**self).list_ids()
    }
}

/// An in-memory [`SecretStore`] — for tests and as a non-persistent fallback.
#[derive(Default)]
pub struct MemorySecretStore {
    map: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemorySecretStore {
    /// A fresh, empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
        self.map
            .lock()
            .expect("secret map poisoned")
            .insert(id.0.clone(), value.to_vec());
        Ok(())
    }

    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
        Ok(self
            .map
            .lock()
            .expect("secret map poisoned")
            .get(&id.0)
            .cloned())
    }

    fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
        self.map.lock().expect("secret map poisoned").remove(&id.0);
        Ok(())
    }

    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
        Ok(self
            .map
            .lock()
            .expect("secret map poisoned")
            .keys()
            .cloned()
            .map(SecretId)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_delete_roundtrip() {
        let s = MemorySecretStore::new();
        let id = SecretId::new("ssh.prod.key");
        assert_eq!(s.get(&id).unwrap(), None);
        s.put(&id, b"PRIVATE-KEY-BYTES").unwrap();
        assert_eq!(
            s.get(&id).unwrap().as_deref(),
            Some(&b"PRIVATE-KEY-BYTES"[..])
        );
        assert_eq!(s.list_ids().unwrap(), vec![id.clone()]);
        s.delete(&id).unwrap();
        assert_eq!(s.get(&id).unwrap(), None);
    }

    #[test]
    fn delete_absent_is_ok() {
        let s = MemorySecretStore::new();
        assert!(s.delete(&SecretId::new("nope")).is_ok());
    }
}
