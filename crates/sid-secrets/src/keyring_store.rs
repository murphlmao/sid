//! OS keyring-backed [`SecretStore`] implementation.
//!
//! `KeyringStore` maps every [`SecretId`] to a `keyring_core::Entry` under the
//! service name `"sid"`. keyring v4 selects the concrete keystore at RUNTIME
//! (the v3 compile-time-feature model is gone): a backend store must be
//! registered with keyring-core via [`install_default_backend`] before any
//! operation. sid registers the pure-Rust zbus Secret Service store on Linux
//! and the legacy Keychain store on macOS (see the per-target deps in this
//! crate's `Cargo.toml`). If no store is registered, every keyring op fails
//! loudly and the binary falls back to the (logged) plaintext `PlainStore` —
//! there is no silent in-memory fallback. The one residual data-loss trap —
//! registering an ephemeral (`ProcessOnly`) store such as the mock — is
//! rejected by [`default_backend_is_durable`].
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
// Runtime backend registration (keyring v4)
// ---------------------------------------------------------------------------

/// Register the platform OS keyring as keyring-core's process-global default
/// credential store.
///
/// keyring v4 selects the backend at runtime, so this MUST be called once at
/// startup before any [`KeyringStore`] operation. On Linux it registers the
/// pure-Rust zbus Secret Service store; on macOS the legacy Keychain store.
/// Returns `Err` if the platform store cannot be constructed (e.g. no Secret
/// Service daemon is reachable) or the platform is unsupported — the binary
/// treats that as "keyring unavailable" and falls back to the plaintext store.
///
/// # Examples
///
/// ```no_run
/// // Call once at startup, before constructing a KeyringStore.
/// sid_secrets::install_default_backend().expect("register OS keyring backend");
/// ```
#[cfg(target_os = "linux")]
pub fn install_default_backend() -> Result<(), String> {
    let store = zbus_secret_service_keyring_store::Store::new().map_err(|e| e.to_string())?;
    keyring_core::set_default_store(store);
    Ok(())
}

/// macOS: register the legacy Keychain store (available to unsandboxed apps).
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub fn install_default_backend() -> Result<(), String> {
    let store = apple_native_keyring_store::keychain::Store::new().map_err(|e| e.to_string())?;
    keyring_core::set_default_store(store);
    Ok(())
}

/// Unsupported platform: no OS keyring backend is compiled in.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
pub fn install_default_backend() -> Result<(), String> {
    Err("no OS keyring backend is compiled for this platform".into())
}

/// Pure decision: does this persistence class survive a process restart?
///
/// `ProcessOnly` (the in-memory mock) and `EntryOnly` storage evaporate when
/// the process exits, so a store reporting either would silently lose every
/// secret — the keyring-v4 analogue of the v3 mock-backend trap. Everything
/// else (the real Secret Service / Keychain backends report `UntilDelete`)
/// persists across restarts and is acceptable.
fn persistence_is_durable(p: keyring_core::CredentialPersistence) -> bool {
    use keyring_core::CredentialPersistence::*;
    !matches!(p, ProcessOnly | EntryOnly)
}

/// True if the currently-registered default store persists across a process
/// restart (i.e. is a real OS keystore, not the in-memory mock).
///
/// The binary calls this after [`install_default_backend`] and refuses to use
/// the keyring — falling back to the logged plaintext store — when it returns
/// false, so an accidentally-registered ephemeral store can never silently
/// swallow secrets. Returns `false` when no store is registered.
///
/// # Examples
///
/// ```no_run
/// sid_secrets::install_default_backend().ok();
/// if sid_secrets::default_backend_is_durable() {
///     // safe to use the OS keyring
/// }
/// ```
pub fn default_backend_is_durable() -> bool {
    match keyring_core::get_default_store() {
        Some(store) => persistence_is_durable(store.persistence()),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Backend abstraction (enables injection of fake in tests)
// ---------------------------------------------------------------------------

/// Abstracts over the OS keyring so tests can inject a fake without touching
/// the real keyring daemon.
///
/// The contract is byte-level: implementations store and return raw `&[u8]`
/// values verbatim, with no encoding layer. The production
/// [`OsKeyringBackend`] satisfies this directly via keyring-core's byte-native
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
// OsKeyringBackend — production path using keyring-core
// ---------------------------------------------------------------------------

/// Production backend that delegates to the OS keyring via keyring-core's
/// runtime-registered default store (see [`install_default_backend`]).
pub struct OsKeyringBackend;

impl KeyringBackend for OsKeyringBackend {
    fn set(&self, key: &str, secret: &[u8]) -> Result<(), String> {
        // keyring-core is byte-native: store the raw bytes verbatim, no encoding.
        let entry = keyring_core::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        entry.set_secret(secret).map_err(|e| e.to_string())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let entry = keyring_core::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        match entry.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        let entry = keyring_core::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            // Deleting a non-existent entry is fine
            Err(keyring_core::Error::NoEntry) => Ok(()),
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

    /// Always-on regression guard for the keyring-v4 analogue of Fix C1.
    ///
    /// keyring v4 selects the backend at runtime, so there is no longer a
    /// compile-time "mock vs real" module to downcast. The residual data-loss
    /// trap is registering an EPHEMERAL store (`ProcessOnly`, like the
    /// in-memory mock, or `EntryOnly`) whose secrets evaporate at process exit.
    /// [`default_backend_is_durable`] guards production against that by reading
    /// the registered store's `persistence()`. This locks the decision logic:
    /// every ephemeral persistence class is rejected and every durable one is
    /// accepted, so the guard cannot silently start treating the mock as real.
    /// Pure and parallel-safe — it touches no process-global state.
    #[test]
    fn ephemeral_persistence_classes_are_rejected_as_non_durable() {
        use keyring_core::CredentialPersistence::*;
        // Ephemeral: do not survive a process restart → must be rejected.
        assert!(
            !persistence_is_durable(ProcessOnly),
            "ProcessOnly (the mock)"
        );
        assert!(!persistence_is_durable(EntryOnly), "EntryOnly");
        // Durable enough to survive a process restart → accepted.
        assert!(persistence_is_durable(UntilDelete), "UntilDelete (disk)");
        assert!(
            persistence_is_durable(UntilReboot),
            "UntilReboot (keyutils)"
        );
        assert!(persistence_is_durable(UntilLogout), "UntilLogout");
    }

    /// Always-on guard that the in-memory mock self-reports as ephemeral.
    ///
    /// Pairs with [`ephemeral_persistence_classes_are_rejected_as_non_durable`]:
    /// that test proves `ProcessOnly` is rejected; this proves the canonical
    /// ephemeral store (keyring-core's mock) actually reports `ProcessOnly`.
    /// Together they prove a mock backend can never pass the durability gate.
    /// Constructs the mock directly (no `set_default_store`) so it stays
    /// parallel-safe and needs no daemon.
    #[test]
    fn mock_store_self_reports_ephemeral() {
        // Coerce to the trait-object store type used in production so the
        // `persistence()` method resolves without importing the trait.
        let mock: std::sync::Arc<keyring_core::CredentialStore> =
            keyring_core::mock::Store::new().expect("mock store builds");
        assert!(
            !persistence_is_durable(mock.persistence()),
            "keyring-core mock must report an ephemeral persistence class so \
             default_backend_is_durable() rejects it"
        );
    }

    /// Opt-in round-trip against the REAL OS keyring daemon (Fix C1).
    ///
    /// Compiled behind `--features os-keyring-smoke`, but `--all-features`
    /// (CI, `/sid-gate --full`) compiles it on boxes with no Secret Service /
    /// Keychain daemon too. A missing daemon is missing INFRASTRUCTURE, not a
    /// bug: the test warns loudly and returns early in that case, asserting
    /// only where a daemon exists. Set `SID_OS_KEYRING_SMOKE=1` to make a
    /// missing daemon a hard failure (explicit operator runs):
    ///
    /// ```text
    /// SID_OS_KEYRING_SMOKE=1 cargo test -p sid-secrets --features os-keyring-smoke
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

        let strict = std::env::var_os("SID_OS_KEYRING_SMOKE").is_some();

        // keyring v4 selects the backend at runtime: the platform store must be
        // registered before any op. A missing Secret Service / Keychain daemon
        // surfaces here as a registration error.
        if let Err(e) = install_default_backend() {
            assert!(
                !strict,
                "install_default_backend should succeed (SID_OS_KEYRING_SMOKE is set): {e}"
            );
            eprintln!(
                "WARNING: os_keyring_real_daemon_round_trip skipped — OS keyring \
                 backend registration failed ({e}). Set SID_OS_KEYRING_SMOKE=1 to \
                 make this a hard failure."
            );
            return;
        }
        assert!(
            default_backend_is_durable(),
            "a real OS keyring daemon must register a durable (non-mock) store, \
             not an ephemeral ProcessOnly/EntryOnly one"
        );

        // Write through one backend instance.
        let writer = KeyringStore::new();
        if let Err(e) = writer.put(&id, payload) {
            assert!(
                !strict,
                "put to real OS keyring should succeed (SID_OS_KEYRING_SMOKE is set): {e}"
            );
            eprintln!(
                "WARNING: os_keyring_real_daemon_round_trip skipped — no usable \
                 OS keyring daemon on this box ({e}). Set SID_OS_KEYRING_SMOKE=1 \
                 to make this a hard failure."
            );
            return;
        }

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
