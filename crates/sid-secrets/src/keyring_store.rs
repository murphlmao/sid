//! OS keyring-backed [`SecretStore`] implementation.
//!
//! `KeyringStore` maps every [`SecretId`] to a `keyring::Entry` under the
//! service name `"sid"`. On Linux this talks to the secret-service D-Bus
//! API; on macOS it talks to the Keychain API.
//!
//! `KeyringStore` is generic over `B: KeyringBackend` so tests can inject a
//! [`FakeKeyring`](crate::tests_support::FakeKeyring) without touching the
//! real OS keyring daemon. Use [`KeyringStore::new`] for the production path
//! or [`KeyringStore::with_backend`] in tests.
//!
//! ## `list_ids` limitation
//!
//! The OS keyring API does not expose enumeration. `KeyringStore::list_ids`
//! returns `Err(SecretError::Storage(...))` with a clear message. Callers
//! that need enumeration (migration helpers) use `PlainStore::list_ids` to
//! obtain the set of ids, then drive reads from `KeyringStore`.

use sid_core::adapters::secrets::{SecretError, SecretId, SecretStore};

/// Service name used as the keyring "service" label.
const SERVICE: &str = "sid";

// ---------------------------------------------------------------------------
// Backend abstraction (enables injection of fake in tests)
// ---------------------------------------------------------------------------

/// Abstracts over the OS keyring so tests can inject a fake without touching
/// the real keyring daemon.
///
/// All byte values are hex-encoded before being stored; implementations must
/// accept arbitrary hex strings as the `secret` parameter.
pub trait KeyringBackend: Send + Sync {
    /// Store `secret` under `key`. Returns `Err(String)` on failure.
    fn set(&self, key: &str, secret: &[u8]) -> Result<(), String>;

    /// Retrieve the value for `key`. Returns `Ok(None)` if absent.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;

    /// Remove the entry for `key`. Must succeed even if the key was absent.
    fn delete(&self, key: &str) -> Result<(), String>;

    /// List all known keys. The OS keyring does not support this; return
    /// `Err(...)` if enumeration is impossible.
    fn list_keys(&self) -> Result<Vec<String>, String>;
}

// ---------------------------------------------------------------------------
// OsKeyringBackend — production path using `keyring` crate
// ---------------------------------------------------------------------------

/// Production backend that delegates to the OS keyring via the `keyring` crate.
pub struct OsKeyringBackend;

impl KeyringBackend for OsKeyringBackend {
    fn set(&self, key: &str, secret: &[u8]) -> Result<(), String> {
        let hex = hex_encode(secret);
        let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        entry.set_password(&hex).map_err(|e| e.to_string())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(hex) => {
                let bytes = hex_decode(&hex)?;
                Ok(Some(bytes))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            // Deleting a non-existent entry is fine
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn list_keys(&self) -> Result<Vec<String>, String> {
        Err("list_ids is not supported by the OS keyring backend; \
             migration helpers which need ids should call PlainStore::list_ids \
             and drive reads from KeyringStore separately"
            .into())
    }
}

// ---------------------------------------------------------------------------
// Hex helpers (bytes ↔ hex string)
// ---------------------------------------------------------------------------

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err(format!("odd hex length {}", s.len()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| format!("invalid hex at offset {i}: {e}"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// KeyringStore
// ---------------------------------------------------------------------------

/// OS keyring-backed [`SecretStore`].
///
/// Generic over `B: KeyringBackend` so tests can inject a
/// [`FakeKeyring`](crate::tests_support::FakeKeyring) without touching the
/// real OS keyring daemon. Use [`KeyringStore::new`] for the production path
/// or [`KeyringStore::with_backend`] in tests.
///
/// ## `list_ids` limitation
///
/// OS keyring does not expose enumeration. `KeyringStore::list_ids` returns
/// `Err(SecretError::Storage(...))` with a clear message. Callers
/// (`PlainStore::list_ids`) that need the full list should ask the plain store
/// and then drive the keyring for individual gets.
pub struct KeyringStore<B = OsKeyringBackend> {
    backend: B,
}

impl KeyringStore<OsKeyringBackend> {
    /// Construct a production `KeyringStore` backed by the real OS keyring.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use sid_secrets::keyring_store::KeyringStore;
    /// let store = KeyringStore::new();
    /// ```
    pub fn new() -> Self {
        Self {
            backend: OsKeyringBackend,
        }
    }
}

impl Default for KeyringStore<OsKeyringBackend> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: KeyringBackend> KeyringStore<B> {
    /// Construct a `KeyringStore` with an injected backend (useful in tests).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_secrets::keyring_store::{KeyringStore, KeyringBackend};
    /// use sid_core::adapters::secrets::{SecretId, SecretStore};
    ///
    /// struct NullBackend;
    /// impl KeyringBackend for NullBackend {
    ///     fn set(&self, _k: &str, _v: &[u8]) -> Result<(), String> { Ok(()) }
    ///     fn get(&self, _k: &str) -> Result<Option<Vec<u8>>, String> { Ok(None) }
    ///     fn delete(&self, _k: &str) -> Result<(), String> { Ok(()) }
    ///     fn list_keys(&self) -> Result<Vec<String>, String> { Ok(vec![]) }
    /// }
    ///
    /// let store = KeyringStore::with_backend(NullBackend);
    /// assert!(store.get(&SecretId::new("a")).unwrap().is_none());
    /// ```
    pub fn with_backend(backend: B) -> Self {
        Self { backend }
    }
}

impl<B: KeyringBackend> SecretStore for KeyringStore<B> {
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
        self.backend
            .set(id.as_str(), value)
            .map_err(SecretError::Storage)
    }

    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
        self.backend.get(id.as_str()).map_err(SecretError::Storage)
    }

    fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
        self.backend
            .delete(id.as_str())
            .map_err(SecretError::Storage)
    }

    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
        self.backend
            .list_keys()
            .map(|ks| ks.into_iter().map(SecretId::new).collect())
            .map_err(SecretError::Storage)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_support::FakeKeyring;
    use sid_core::adapters::secrets::{SecretId, SecretStore};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn keyring_store_put_then_get() {
        let store = KeyringStore::with_backend(FakeKeyring::default());
        let id = SecretId::new("test.key");
        store.put(&id, b"hunter2").unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap(), b"hunter2".to_vec());
    }

    #[test]
    fn keyring_store_get_missing_returns_none() {
        let store = KeyringStore::with_backend(FakeKeyring::default());
        let id = SecretId::new("missing");
        assert!(store.get(&id).unwrap().is_none());
    }

    #[test]
    fn keyring_store_delete_then_get_none() {
        let store = KeyringStore::with_backend(FakeKeyring::default());
        let id = SecretId::new("to.delete");
        store.put(&id, b"secret").unwrap();
        store.delete(&id).unwrap();
        assert!(store.get(&id).unwrap().is_none());
    }

    #[test]
    fn keyring_store_put_empty_bytes() {
        let store = KeyringStore::with_backend(FakeKeyring::default());
        let id = SecretId::new("empty");
        store.put(&id, b"").unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap(), b"".to_vec());
    }

    #[test]
    fn keyring_store_list_ids_round_trips_via_fake() {
        let store = KeyringStore::with_backend(FakeKeyring::default());
        store.put(&SecretId::new("a"), b"1").unwrap();
        store.put(&SecretId::new("b"), b"2").unwrap();
        let mut ids = store.list_ids().unwrap();
        ids.sort_by(|x, y| x.as_str().cmp(y.as_str()));
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].as_str(), "a");
        assert_eq!(ids[1].as_str(), "b");
    }

    #[test]
    fn keyring_store_is_send_sync() {
        assert_send_sync::<KeyringStore<FakeKeyring>>();
    }

    #[test]
    fn hex_encode_decode_roundtrip() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        let hex = hex_encode(&bytes);
        assert_eq!(hex_decode(&hex).unwrap(), bytes);
    }

    #[test]
    fn hex_decode_rejects_odd_length() {
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn hex_decode_rejects_non_hex() {
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn backend_error_surfaces_as_secret_error_storage() {
        struct AlwaysFail;
        impl KeyringBackend for AlwaysFail {
            fn set(&self, _k: &str, _v: &[u8]) -> Result<(), String> {
                Err("boom".into())
            }
            fn get(&self, _k: &str) -> Result<Option<Vec<u8>>, String> {
                Err("boom".into())
            }
            fn delete(&self, _k: &str) -> Result<(), String> {
                Err("boom".into())
            }
            fn list_keys(&self) -> Result<Vec<String>, String> {
                Err("boom".into())
            }
        }

        let store = KeyringStore::with_backend(AlwaysFail);
        let id = SecretId::new("x");
        assert!(matches!(store.put(&id, b"v"), Err(SecretError::Storage(_))));
        assert!(matches!(store.get(&id), Err(SecretError::Storage(_))));
        assert!(matches!(store.delete(&id), Err(SecretError::Storage(_))));
        assert!(matches!(store.list_ids(), Err(SecretError::Storage(_))));
    }
}
