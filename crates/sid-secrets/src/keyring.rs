//! OS keyring-backed [`SecretStore`] implementation.
//!
//! `KeyringStore` maps every [`SecretId`] to a `keyring_core::Entry` under the service
//! name `"sid"`. keyring-core selects the concrete keystore at **runtime**: a backend
//! store must be registered via [`install_default_backend`] before any operation, or
//! every op fails. sid registers the pure-Rust zbus Secret Service store on Linux (see
//! this crate's `Cargo.toml` for the load-bearing feature notes); other platforms are
//! empty slots for now (`CLAUDE.md`: cross-platform is accommodated, not solved).
//!
//! `KeyringStore` is generic over `B: KeyringBackend` so tests can inject a fake
//! (`FakeKeyring`) without touching the real OS keyring daemon. Use [`KeyringStore::new`]
//! for the production path or [`KeyringStore::with_backend`] in tests.
//!
//! ## Startup durability probe
//!
//! Registering a backend store can succeed even when the platform's Secret Service
//! daemon is unreachable or misconfigured (e.g. no `org.freedesktop.secrets` on the
//! session bus) — the failure only surfaces on the first real operation. [`open_default_secrets`]
//! runs a put/get/delete round-trip against a canary id at startup and only hands back a
//! `KeyringStore` if that probe fully succeeds; otherwise it falls back to
//! [`MemorySecretStore`] plus a human-readable warning the app can surface later.
//!
//! ## `list_ids` limitation
//!
//! The OS keyring API does not expose enumeration. `KeyringStore::list_ids` returns
//! `Err(SecretError::Backend(...))` with a clear message. Callers that need enumeration
//! must track ids themselves (e.g. via the committed config's `secret_ref` fields).

use crate::{MemorySecretStore, SecretError, SecretId, SecretStore};

/// Service name used as the keyring "service" label.
const SERVICE: &str = "sid";

/// Id used for the startup durability probe canary. Never a real secret.
const PROBE_ID: &str = "sid.startup-probe";

// ---------------------------------------------------------------------------
// Runtime backend registration
// ---------------------------------------------------------------------------

/// Register the platform OS keyring as keyring-core's process-global default credential
/// store.
///
/// keyring-core selects the backend at runtime, so this MUST be called once at startup
/// before any [`KeyringStore`] operation — otherwise keyring-core has no default store
/// and every secret op fails silently. On Linux this registers the pure-Rust zbus Secret
/// Service store. Returns `Err` if the platform store cannot be constructed (e.g. no
/// Secret Service daemon is reachable) or the platform is unsupported — the caller treats
/// that as "keyring unavailable" and falls back to an in-memory store.
#[cfg(target_os = "linux")]
pub fn install_default_backend() -> Result<(), String> {
    let store = zbus_secret_service_keyring_store::Store::new().map_err(|e| e.to_string())?;
    keyring_core::set_default_store(store);
    Ok(())
}

/// Unsupported platform: no OS keyring backend is compiled in. This is an intentional
/// empty slot (`CLAUDE.md`: keep the seams; don't write Mac/Windows code yet).
#[cfg(not(target_os = "linux"))]
pub fn install_default_backend() -> Result<(), String> {
    Err("no OS keyring backend is compiled for this platform".into())
}

// ---------------------------------------------------------------------------
// Backend abstraction (enables injection of a fake in tests)
// ---------------------------------------------------------------------------

/// Abstracts over the OS keyring so tests can inject a fake without touching the real
/// keyring daemon.
///
/// The contract is byte-level: implementations store and return raw `&[u8]` values
/// verbatim, with no encoding layer. The production [`OsKeyringBackend`] satisfies this
/// directly via keyring-core's byte-native `set_secret`/`get_secret` API.
pub trait KeyringBackend: Send + Sync {
    /// Store `secret` under `key`. Returns `Err(String)` on failure.
    fn set(&self, key: &str, secret: &[u8]) -> Result<(), String>;

    /// Retrieve the value for `key`. Returns `Ok(None)` if absent.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;

    /// Remove the entry for `key`. Must succeed even if the key was absent.
    fn delete(&self, key: &str) -> Result<(), String>;

    /// List all known keys. The OS keyring does not support this; return `Err(...)` if
    /// enumeration is impossible.
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
            // Deleting a non-existent entry is fine.
            Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn list_keys(&self) -> Result<Vec<String>, String> {
        Err(
            "list_ids is not supported by the OS keyring backend; callers must track ids \
             themselves (e.g. via the committed config's secret_ref fields)"
                .into(),
        )
    }
}

// ---------------------------------------------------------------------------
// KeyringStore
// ---------------------------------------------------------------------------

/// OS keyring-backed [`SecretStore`].
///
/// Generic over `B: KeyringBackend` so tests can inject a fake (`FakeKeyring`) without
/// touching the real OS keyring daemon. Use [`KeyringStore::new`] for the production path
/// or [`KeyringStore::with_backend`] in tests.
pub struct KeyringStore<B = OsKeyringBackend> {
    backend: B,
}

impl KeyringStore<OsKeyringBackend> {
    /// Construct a production `KeyringStore` backed by the real OS keyring.
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
    pub fn with_backend(backend: B) -> Self {
        Self { backend }
    }
}

impl<B: KeyringBackend> SecretStore for KeyringStore<B> {
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
        self.backend
            .set(id.as_str(), value)
            .map_err(SecretError::Backend)
    }

    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
        self.backend.get(id.as_str()).map_err(SecretError::Backend)
    }

    fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
        self.backend
            .delete(id.as_str())
            .map_err(SecretError::Backend)
    }

    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
        self.backend
            .list_keys()
            .map(|ks| ks.into_iter().map(SecretId::new).collect())
            .map_err(SecretError::Backend)
    }
}

// ---------------------------------------------------------------------------
// Startup durability probe + bootstrap
// ---------------------------------------------------------------------------

/// Round-trip a canary secret through `store` to prove the backend actually works
/// end-to-end (put, then get back the same bytes, then delete). Registering a keyring
/// backend store can succeed even when the daemon behind it is unreachable — the failure
/// only shows up on the first real operation — so this probe is what actually gates
/// whether we trust the keyring.
fn probe(store: &dyn SecretStore) -> Result<(), String> {
    let id = SecretId::new(PROBE_ID);
    let payload = b"sid-startup-probe";
    store.put(&id, payload).map_err(|e| e.to_string())?;
    let got = store.get(&id).map_err(|e| e.to_string())?;
    // Clean up regardless of whether the read-back matched, so a partial failure never
    // leaves the canary behind.
    let delete_result = store.delete(&id).map_err(|e| e.to_string());
    match got {
        Some(bytes) if bytes == payload => delete_result,
        Some(_) => Err("startup probe: read-back bytes did not match".into()),
        None => Err("startup probe: value vanished immediately after put".into()),
    }
}

/// Open the default secret backend: the OS keyring if it passes a startup durability
/// probe, otherwise an in-memory fallback plus a human-readable warning.
///
/// The warning is `Some(..)` exactly when the fallback is in use, so the app can surface
/// it (secrets entered this session will not survive a restart).
pub fn open_default_secrets() -> (Box<dyn SecretStore>, Option<String>) {
    if let Err(e) = install_default_backend() {
        return (
            Box::new(MemorySecretStore::new()),
            Some(format!(
                "OS keyring unavailable ({e}); secrets will not persist across restarts"
            )),
        );
    }

    let candidate = KeyringStore::new();
    match probe(&candidate) {
        Ok(()) => (Box::new(candidate), None),
        Err(e) => (
            Box::new(MemorySecretStore::new()),
            Some(format!(
                "OS keyring probe failed ({e}); secrets will not persist across restarts"
            )),
        ),
    }
}

// ---------------------------------------------------------------------------
// Test support
// ---------------------------------------------------------------------------

/// In-memory fake that satisfies [`KeyringBackend`] without touching a real OS keyring
/// daemon. Test-only: exercises `KeyringStore`'s trait plumbing in isolation.
#[derive(Default)]
pub struct FakeKeyring {
    map: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl KeyringBackend for FakeKeyring {
    fn set(&self, key: &str, secret: &[u8]) -> Result<(), String> {
        self.map
            .lock()
            .expect("fake keyring poisoned")
            .insert(key.to_string(), secret.to_vec());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self
            .map
            .lock()
            .expect("fake keyring poisoned")
            .get(key)
            .cloned())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.map.lock().expect("fake keyring poisoned").remove(key);
        Ok(())
    }

    fn list_keys(&self) -> Result<Vec<String>, String> {
        Ok(self
            .map
            .lock()
            .expect("fake keyring poisoned")
            .keys()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn keyring_store_delete_missing_is_noop() {
        let store = KeyringStore::with_backend(FakeKeyring::default());
        assert!(store.delete(&SecretId::new("never-was")).is_ok());
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
    fn backend_error_surfaces_as_secret_error_backend() {
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
        assert!(matches!(store.put(&id, b"v"), Err(SecretError::Backend(_))));
        assert!(matches!(store.get(&id), Err(SecretError::Backend(_))));
        assert!(matches!(store.delete(&id), Err(SecretError::Backend(_))));
        assert!(matches!(store.list_ids(), Err(SecretError::Backend(_))));
    }

    // ---- probe --------------------------------------------------------

    #[test]
    fn probe_passes_against_a_working_fake_backend() {
        let store = KeyringStore::with_backend(FakeKeyring::default());
        assert!(probe(&store).is_ok());
        // The canary must not be left behind.
        assert!(store.get(&SecretId::new(PROBE_ID)).unwrap().is_none());
    }

    #[test]
    fn probe_fails_when_backend_put_fails() {
        struct PutFails;
        impl KeyringBackend for PutFails {
            fn set(&self, _k: &str, _v: &[u8]) -> Result<(), String> {
                Err("no secret service".into())
            }
            fn get(&self, _k: &str) -> Result<Option<Vec<u8>>, String> {
                Ok(None)
            }
            fn delete(&self, _k: &str) -> Result<(), String> {
                Ok(())
            }
            fn list_keys(&self) -> Result<Vec<String>, String> {
                Ok(vec![])
            }
        }
        let store = KeyringStore::with_backend(PutFails);
        assert!(probe(&store).is_err());
    }

    #[test]
    fn probe_fails_when_read_back_does_not_match() {
        // A backend that reports success on put but returns something else on get —
        // e.g. a stale entry from a previous run under process isolation quirks.
        struct Deceptive;
        impl KeyringBackend for Deceptive {
            fn set(&self, _k: &str, _v: &[u8]) -> Result<(), String> {
                Ok(())
            }
            fn get(&self, _k: &str) -> Result<Option<Vec<u8>>, String> {
                Ok(Some(b"not-the-canary".to_vec()))
            }
            fn delete(&self, _k: &str) -> Result<(), String> {
                Ok(())
            }
            fn list_keys(&self) -> Result<Vec<String>, String> {
                Ok(vec![])
            }
        }
        let store = KeyringStore::with_backend(Deceptive);
        assert!(probe(&store).is_err());
    }

    #[test]
    fn probe_fails_when_get_returns_none() {
        struct Vanishes;
        impl KeyringBackend for Vanishes {
            fn set(&self, _k: &str, _v: &[u8]) -> Result<(), String> {
                Ok(())
            }
            fn get(&self, _k: &str) -> Result<Option<Vec<u8>>, String> {
                Ok(None)
            }
            fn delete(&self, _k: &str) -> Result<(), String> {
                Ok(())
            }
            fn list_keys(&self) -> Result<Vec<String>, String> {
                Ok(vec![])
            }
        }
        let store = KeyringStore::with_backend(Vanishes);
        assert!(probe(&store).is_err());
    }

    // ---- open_default_secrets -------------------------------------------

    /// On a platform/environment with no reachable Secret Service daemon,
    /// `install_default_backend` itself fails, so `open_default_secrets` must fall back
    /// to the in-memory store with a warning rather than panicking or silently losing
    /// secrets. CI/sandboxed dev boxes typically have no session bus, so this exercises
    /// the real fallback path without requiring a live daemon.
    #[test]
    fn open_default_secrets_falls_back_to_memory_without_a_daemon() {
        let (store, warning) = open_default_secrets();
        // Whichever path was taken, the returned store must be minimally functional.
        // Clean up afterwards: on a dev box this may be the REAL keyring, and leaving
        // the entry behind would deposit test residue in the user's Secret Service.
        let id = SecretId::new("sid.test-smoke");
        store.put(&id, b"v").unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap(), b"v".to_vec());
        store.delete(&id).unwrap();
        assert!(store.get(&id).unwrap().is_none());

        // If no real keyring daemon is reachable in this environment (the common case
        // for CI and sandboxes), a warning must be present so the app can surface it.
        // We can't assert which branch was taken (a dev box may have a real session
        // Secret Service running), but we can assert the contract: whenever a warning
        // is returned, it's non-empty and human-readable.
        if let Some(msg) = warning {
            assert!(!msg.is_empty());
        }
    }
}
