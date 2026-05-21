//! Secret storage adapter trait + supporting types.
//!
//! `SecretStore` is the trait used by tabs that need to persist sensitive
//! material — SSH key passphrases (Plan 3), DB connection passwords (Plan 4),
//! and credentials used by future remote integrations (Plan 6). Concrete
//! implementations live in their own crates: `sid-secrets` provides the
//! always-available `PlainStore` (file-backed, no OS keyring), and future
//! OS-keychain impls (e.g., libsecret, Keychain) will land alongside it.
//!
//! The trait is intentionally minimal — put/get/delete/list — so adapters with
//! different backing stores share the same shape.

use serde::{Deserialize, Serialize};

/// Stable identifier for a stored secret.
///
/// `SecretId`s are opaque strings owned by the caller. By convention they are
/// dotted, prefixed by the consumer crate (e.g., `ssh.key.id_ed25519`,
/// `db.connection.local-pg.password`). The store does not interpret the
/// contents; any non-empty string is accepted.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::secrets::SecretId;
///
/// let id = SecretId::new("ssh.key.id_ed25519");
/// assert_eq!(id.as_str(), "ssh.key.id_ed25519");
/// ```
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SecretId(String);

impl SecretId {
    /// Build a new [`SecretId`] from any string-like value.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::secrets::SecretId;
    ///
    /// let from_str = SecretId::new("a.b");
    /// let from_string = SecretId::new(String::from("a.b"));
    /// assert_eq!(from_str, from_string);
    /// ```
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// View the secret id as a `&str`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::secrets::SecretId;
    ///
    /// let id = SecretId::new("ssh.passphrase");
    /// assert_eq!(id.as_str(), "ssh.passphrase");
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Domain-shaped error for secret store operations.
///
/// Concrete implementations map their backing-store errors into this enum.
/// The variants are intentionally narrow: the secret either is not there, or
/// the backing store failed (with a free-form message).
///
/// # Examples
///
/// ```
/// use sid_core::adapters::secrets::SecretError;
///
/// let not_found = SecretError::NotFound("ssh.key".into());
/// assert!(format!("{not_found}").contains("ssh.key"));
///
/// let storage = SecretError::Storage("disk full".into());
/// assert!(format!("{storage}").contains("disk full"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No secret was stored under the given id.
    #[error("secret '{0}' not found")]
    NotFound(String),
    /// The backing store reported a failure (I/O, corruption, etc.).
    #[error("storage error: {0}")]
    Storage(String),
}

/// Persistent secret storage abstraction.
///
/// `SecretStore` is the trait used by adapters that need to read or write
/// sensitive material (SSH key passphrases, DB passwords, API tokens). The
/// trait is `Send + Sync` so impls can be shared across async tasks via
/// `Arc<dyn SecretStore>`.
///
/// # Backing-store guarantees
///
/// Implementations must:
/// - Survive process restart (i.e., persist to durable storage).
/// - Return `Err(SecretError::NotFound)` only from [`SecretStore::delete`]
///   when the id is known to be absent at the application level; `get` on a
///   missing id returns `Ok(None)`. (Mirrors `HashMap::get`.)
/// - Treat any byte sequence as a valid value, including empty `[]`.
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use std::sync::Mutex;
/// use sid_core::adapters::secrets::{SecretError, SecretId, SecretStore};
///
/// struct MemStore(Mutex<HashMap<String, Vec<u8>>>);
///
/// impl SecretStore for MemStore {
///     fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
///         self.0.lock().unwrap().insert(id.as_str().to_string(), value.to_vec());
///         Ok(())
///     }
///     fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
///         Ok(self.0.lock().unwrap().get(id.as_str()).cloned())
///     }
///     fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
///         self.0.lock().unwrap().remove(id.as_str());
///         Ok(())
///     }
///     fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
///         Ok(self.0.lock().unwrap().keys().cloned().map(SecretId::new).collect())
///     }
/// }
///
/// let store = MemStore(Mutex::new(HashMap::new()));
/// let id = SecretId::new("test");
/// store.put(&id, b"secret").unwrap();
/// assert_eq!(store.get(&id).unwrap().unwrap(), b"secret".to_vec());
/// ```
pub trait SecretStore: Send + Sync {
    /// Insert or replace the secret at `id` with `value`.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError::Storage`] if the backing store cannot persist
    /// the write.
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError>;

    /// Retrieve the secret stored at `id`, or `None` if no such secret exists.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError::Storage`] if the backing store cannot be read.
    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError>;

    /// Remove the secret at `id`. Idempotent — deleting a missing id is `Ok(())`.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError::Storage`] if the backing store cannot apply the
    /// removal.
    fn delete(&self, id: &SecretId) -> Result<(), SecretError>;

    /// List every secret id currently held by the store.
    ///
    /// Order is implementation-defined. Returns an empty `Vec` if the store
    /// is empty.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError::Storage`] if the backing store cannot be
    /// enumerated.
    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn secret_id_new_and_as_str_roundtrip() {
        let id = SecretId::new("ssh.key.foo");
        assert_eq!(id.as_str(), "ssh.key.foo");
    }

    #[test]
    fn secret_id_equality() {
        let a = SecretId::new("x");
        let b = SecretId::new(String::from("x"));
        let c = SecretId::new("y");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn secret_id_serde_roundtrip() {
        let id = SecretId::new("api.token");
        let s = serde_json::to_string(&id).unwrap();
        let back: SecretId = serde_json::from_str(&s).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn secret_error_not_found_message_includes_id() {
        let e = SecretError::NotFound("ssh.key".into());
        let msg = format!("{e}");
        assert!(msg.contains("ssh.key"));
        assert!(msg.contains("not found"));
    }

    #[test]
    fn secret_error_storage_message_includes_detail() {
        let e = SecretError::Storage("disk full".into());
        let msg = format!("{e}");
        assert!(msg.contains("storage error"));
        assert!(msg.contains("disk full"));
    }

    #[test]
    fn secret_store_trait_object_is_send_sync() {
        assert_send_sync::<&dyn SecretStore>();
    }
}
