//! OS keyring-backed [`SecretStore`] implementation.
//!
//! `KeyringStore` maps every [`SecretId`] to a `keyring::Entry` under the
//! service name `"sid"`. The concrete keystore is selected at compile time by
//! the `keyring` crate's Cargo features (see the workspace `Cargo.toml`): on
//! Linux the `sync-secret-service` feature talks to the Secret Service D-Bus
//! API, and on macOS the `apple-native` feature talks to the Keychain. With
//! NO keyring features the crate silently compiles its in-memory `mock`
//! backend — values written through one `Entry` are unreadable through a
//! fresh one — so those features are load-bearing, not optional.
//!
//! `KeyringStore` is generic over `B: KeyringBackend` so tests can inject a
//! fake (`FakeKeyring`) without touching the real OS keyring daemon. Use
//! [`KeyringStore::new`] for the production path or
//! [`KeyringStore::with_backend`] in tests.
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
/// The contract is byte-level: implementations store and return raw `&[u8]`
/// values verbatim, with no encoding layer. The production
/// [`OsKeyringBackend`] satisfies this directly via keyring v3's byte-native
/// `set_secret`/`get_secret` API.
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
        // keyring v3 is byte-native: store the raw bytes verbatim, no encoding.
        let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        entry.set_secret(secret).map_err(|e| e.to_string())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        match entry.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
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
// KeyringStore
// ---------------------------------------------------------------------------

/// OS keyring-backed [`SecretStore`].
///
/// Generic over `B: KeyringBackend` so tests can inject a fake (`FakeKeyring`)
/// without touching the real OS keyring daemon. Use [`KeyringStore::new`] for
/// the production path or [`KeyringStore::with_backend`] in tests.
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

    /// Always-on regression guard for Fix C1 (the mock-backend near-miss).
    ///
    /// keyring v3 compiles NO platform keystore by default; with no Cargo
    /// features it resolves `keyring::default` to `pub use mock as default`,
    /// a process-local in-memory store. If the `sync-secret-service` /
    /// `apple-native` features were ever dropped from the workspace
    /// `Cargo.toml`, this test fails at the next `cargo test` — long before a
    /// user silently loses every secret to the plaintext fallback.
    ///
    /// keyring exposes the resolution at runtime: `keyring::default` is a
    /// re-export of whichever backend module the active features selected, and
    /// its `default_credential_builder()` returns a `MockCredentialBuilder`
    /// only when the mock was chosen. We downcast and assert it is NOT the
    /// mock, which is exactly the feasible runtime detection the fix called for.
    #[test]
    fn compiled_keyring_default_is_not_the_mock_backend() {
        let builder = keyring::default::default_credential_builder();
        let is_mock = builder
            .as_any()
            .downcast_ref::<keyring::mock::MockCredentialBuilder>()
            .is_some();
        assert!(
            !is_mock,
            "keyring compiled to its in-memory MOCK backend — the real \
             platform keystore features (sync-secret-service / apple-native) \
             are missing from the workspace Cargo.toml. Every secret would be \
             written to a process-local store, fail the startup probe, and \
             fall back to plaintext. Restore the keyring features."
        );
    }

    /// Opt-in round-trip against the REAL OS keyring daemon (Fix C1).
    ///
    /// Off by default: CI and headless boxes have no Secret Service / Keychain.
    /// Run locally where a daemon exists:
    ///
    /// ```text
    /// cargo test -p sid-secrets --features os-keyring-smoke
    /// ```
    ///
    /// Crucially this performs put → get through TWO SEPARATE `Entry`
    /// constructions (a fresh `OsKeyringBackend` for the read), which is the
    /// exact pattern the mock backend cannot satisfy: the mock keeps state per
    /// process but not per fresh `Entry`, so a real keystore is required for
    /// the read-back to succeed. Cleans up after itself with a delete.
    #[cfg(feature = "os-keyring-smoke")]
    #[test]
    fn os_keyring_real_daemon_round_trip() {
        // Unique key so concurrent test runs / leftover state never collide.
        let key = format!("sid.smoke.{}", std::process::id());
        let id = SecretId::new(key);
        let payload = b"smoke-test-secret-\x00\xff-bytes";

        // Write through one backend instance.
        let writer = KeyringStore::new();
        writer
            .put(&id, payload)
            .expect("put to real OS keyring should succeed");

        // Read through a SEPARATE backend / Entry construction. This is what
        // proves we hit a persistent OS keystore and not a per-Entry mock.
        let reader = KeyringStore::new();
        let got = reader
            .get(&id)
            .expect("get from real OS keyring should succeed")
            .expect("secret written through a separate Entry must be readable");
        assert_eq!(got, payload.to_vec(), "round-trip bytes must match exactly");

        // Cleanup so repeated local runs stay clean.
        reader.delete(&id).expect("delete should succeed");
        assert!(
            reader
                .get(&id)
                .expect("post-delete get should succeed")
                .is_none(),
            "secret must be gone after delete"
        );
    }
}
