//! Backend selection chain: [`resolve_secret_store`] walks **keyring (if enabled & the
//! startup probe passes) → encrypted-file (if enabled) → memory** according to the
//! user's toggles, and returns an honest account of what ended up live and why. Memory
//! is the unconditional final fallback — this function never fails to hand back a
//! usable store.

use std::path::PathBuf;
use std::sync::Arc;

use crate::file::EncryptedFileStore;
use crate::keyring::{self, KeyringStore};
use crate::{MemorySecretStore, SecretStore};

/// The user's persisted backend toggles. Mirrors `sid_store::Settings`'
/// `secret_keyring_enabled` / `secret_file_enabled` fields — `sid-secrets` does not
/// depend on `sid-store` (that would invert the dependency direction the store already
/// has on this crate), so the caller maps its own settings into this shape.
#[derive(Debug, Clone, Copy)]
pub struct SecretBackendToggles {
    /// Whether the OS keyring is a candidate backend at all. Still gated by the startup
    /// durability probe — enabling this doesn't guarantee the keyring is what's used.
    pub keyring_enabled: bool,
    /// Whether the encrypted-file vault is a candidate backend.
    pub file_enabled: bool,
}

/// Outcome of the startup keyring durability probe.
pub enum KeyringProbe {
    /// A full put/get/delete round-trip against the OS keyring succeeded.
    Available,
    /// No working keyring: either the platform backend could not be registered, or the
    /// round-trip failed. Carries a human-readable reason.
    Unavailable(String),
}

/// Run the **real** startup keyring probe against the OS: register the platform
/// backend, then round-trip a canary secret (see [`keyring`]'s module docs). A free
/// function — rather than inlined into [`resolve_secret_store`] — so tests can
/// substitute a canned [`KeyringProbe`] instead of touching a real Secret Service
/// daemon (or lack thereof) in the test environment.
pub fn probe_keyring() -> KeyringProbe {
    if let Err(e) = keyring::install_default_backend() {
        return KeyringProbe::Unavailable(e);
    }
    let candidate = KeyringStore::new();
    match keyring::probe(&candidate) {
        Ok(()) => KeyringProbe::Available,
        Err(e) => KeyringProbe::Unavailable(e),
    }
}

/// Which backend ended up serving secret requests.
///
/// `EncryptedFile` carries a handle to the *concrete* store, not just the
/// [`SecretStore`] trait object — the trait itself stays a plain `&self`-sync
/// `put`/`get`/`delete`/`list_ids` surface with no notion of passphrases, so the app
/// needs this handle to drive the unlock/create UI (`EncryptedFileStore::unlock` /
/// `::create`). `Resolved::store` is the trait object every other call site uses.
#[derive(Clone)]
pub enum BackendKind {
    /// The OS keyring, having passed the startup probe.
    Keyring,
    /// The encrypted-file vault. May or may not be unlocked yet — check
    /// [`EncryptedFileStore::is_unlocked`]/[`EncryptedFileStore::exists`].
    EncryptedFile(Arc<EncryptedFileStore>),
    /// The non-persistent in-memory fallback.
    Memory,
}

/// [`resolve_secret_store`]'s full result.
pub struct Resolved {
    /// The store every secret call site should use.
    pub store: Box<dyn SecretStore>,
    /// Which backend is actually live.
    pub effective: BackendKind,
    /// Set exactly when the effective backend is weaker than what the user's toggles
    /// asked for (memory-only when a persistent backend was wanted, or a keyring the
    /// user enabled but couldn't get).
    pub warning: Option<String>,
    /// Set alongside a keyring-unavailable warning, or when both persistent backends
    /// are disabled: how to fix it.
    pub recommendation: Option<String>,
}

const KEYRING_RECOMMENDATION: &str = "install a Secret Service provider (e.g. `sudo pacman -S gnome-keyring`) or enable \
     the encrypted-file backend in settings";

/// Select the effective secret backend and open it.
///
/// `vault_path` is where the encrypted-file backend's vault lives; constructing an
/// [`EncryptedFileStore`] handle does no I/O (see its docs), so this is cheap to call
/// even when that backend doesn't end up effective.
///
/// `keyring_probe` runs only when `toggles.keyring_enabled` is true. Production callers
/// pass [`probe_keyring`]; tests inject a canned [`KeyringProbe`] so the full selection
/// matrix is exercised without a real OS keyring daemon.
///
/// Toggle semantics: keyring off + file on ⇒ file only. Both off ⇒ memory only, with a
/// warning explaining that's an explicit consequence of the toggles, not a failure.
pub fn resolve_secret_store(
    toggles: SecretBackendToggles,
    vault_path: PathBuf,
    keyring_probe: impl FnOnce() -> KeyringProbe,
) -> Resolved {
    if toggles.keyring_enabled {
        match keyring_probe() {
            KeyringProbe::Available => {
                return Resolved {
                    store: Box::new(KeyringStore::new()),
                    effective: BackendKind::Keyring,
                    warning: None,
                    recommendation: None,
                };
            }
            KeyringProbe::Unavailable(reason) => {
                if toggles.file_enabled {
                    let efs = Arc::new(EncryptedFileStore::at(vault_path));
                    return Resolved {
                        store: Box::new(Arc::clone(&efs)),
                        effective: BackendKind::EncryptedFile(efs),
                        warning: Some(format!(
                            "OS keyring unavailable ({reason}); using the encrypted-file \
                             backend instead"
                        )),
                        recommendation: Some(KEYRING_RECOMMENDATION.to_string()),
                    };
                }
                return Resolved {
                    store: Box::new(MemorySecretStore::new()),
                    effective: BackendKind::Memory,
                    warning: Some(format!(
                        "OS keyring unavailable ({reason}); secrets will not persist \
                         across restarts"
                    )),
                    recommendation: Some(KEYRING_RECOMMENDATION.to_string()),
                };
            }
        }
    }

    if toggles.file_enabled {
        let efs = Arc::new(EncryptedFileStore::at(vault_path));
        return Resolved {
            store: Box::new(Arc::clone(&efs)),
            effective: BackendKind::EncryptedFile(efs),
            warning: None,
            recommendation: None,
        };
    }

    Resolved {
        store: Box::new(MemorySecretStore::new()),
        effective: BackendKind::Memory,
        warning: Some(
            "the keyring and encrypted-file backends are both disabled; secrets will \
             not persist across restarts"
                .to_string(),
        ),
        recommendation: Some(
            "enable the encrypted-file backend in settings for persistence without an \
             OS keyring"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toggles(keyring_enabled: bool, file_enabled: bool) -> SecretBackendToggles {
        SecretBackendToggles {
            keyring_enabled,
            file_enabled,
        }
    }

    fn vault_path() -> PathBuf {
        // Never actually touched unless the encrypted-file backend becomes effective
        // and something writes to it — these tests only inspect `effective`/`warning`/
        // `recommendation`, not vault I/O (that's `file.rs`'s job).
        std::env::temp_dir().join(format!("sid-resolve-test-{}.vault", std::process::id()))
    }

    // ---- the full (keyring × file × probe) selection matrix --------------------

    #[test]
    fn keyring_on_probe_pass_wins_regardless_of_file_toggle() {
        for file_enabled in [true, false] {
            let r = resolve_secret_store(toggles(true, file_enabled), vault_path(), || {
                KeyringProbe::Available
            });
            assert!(matches!(r.effective, BackendKind::Keyring));
            assert!(r.warning.is_none());
            assert!(r.recommendation.is_none());
        }
    }

    #[test]
    fn keyring_on_probe_fail_file_on_falls_back_to_encrypted_file() {
        let r = resolve_secret_store(toggles(true, true), vault_path(), || {
            KeyringProbe::Unavailable("no Secret Service".into())
        });
        assert!(matches!(r.effective, BackendKind::EncryptedFile(_)));
        let warning = r.warning.expect("must warn about the keyring fallback");
        assert!(warning.contains("no Secret Service"), "{warning}");
        assert!(r.recommendation.is_some());
    }

    #[test]
    fn keyring_on_probe_fail_file_off_falls_back_to_memory() {
        let r = resolve_secret_store(toggles(true, false), vault_path(), || {
            KeyringProbe::Unavailable("no Secret Service".into())
        });
        assert!(matches!(r.effective, BackendKind::Memory));
        let warning = r.warning.expect("must warn about memory-only persistence");
        assert!(warning.contains("will not persist"), "{warning}");
        assert!(r.recommendation.is_some());
    }

    #[test]
    fn keyring_off_file_on_uses_encrypted_file_with_no_warning() {
        let r = resolve_secret_store(toggles(false, true), vault_path(), || {
            panic!("probe must not run when the keyring is disabled")
        });
        assert!(matches!(r.effective, BackendKind::EncryptedFile(_)));
        assert!(r.warning.is_none());
        assert!(r.recommendation.is_none());
    }

    #[test]
    fn both_disabled_uses_memory_with_a_warning_and_recommendation() {
        let r = resolve_secret_store(toggles(false, false), vault_path(), || {
            panic!("probe must not run when the keyring is disabled")
        });
        assert!(matches!(r.effective, BackendKind::Memory));
        let warning = r.warning.expect("both-disabled must warn");
        assert!(warning.contains("both disabled"), "{warning}");
        assert!(r.recommendation.is_some());
    }

    #[test]
    fn keyring_disabled_never_invokes_the_probe() {
        // Regression guard for the toggle semantics: a disabled keyring must short
        // circuit before the probe closure runs at all (Murphy: "keyring off + file on
        // ⇒ file only" — the probe closure panicking proves it was never called).
        let _ = resolve_secret_store(toggles(false, true), vault_path(), || {
            panic!("probe ran despite keyring_enabled == false")
        });
    }

    // ---- resolve_secret_store against the real probe never panics --------------

    #[test]
    fn real_probe_end_to_end_never_panics_and_always_yields_a_usable_store() {
        let toggles = SecretBackendToggles {
            keyring_enabled: true,
            file_enabled: true,
        };
        let resolved = resolve_secret_store(toggles, vault_path(), probe_keyring);
        // Whatever backend won, the trait object itself must be minimally sane: an
        // empty `list_ids()` (or, for the keyring, at least not panicking) — we avoid
        // touching a REAL keyring's persistent storage here (see `file.rs`/`keyring.rs`
        // for the backends' own dedicated round-trip tests), so this only asserts we
        // got *something* back without a panic.
        match resolved.effective {
            BackendKind::Keyring | BackendKind::Memory => {}
            BackendKind::EncryptedFile(handle) => assert!(!handle.is_unlocked()),
        }
    }
}
