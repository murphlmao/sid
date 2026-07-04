//! Backend selection chain: [`resolve_secret_store`] walks **keyring (if enabled & the
//! startup probe passes) → memory** according to the user's toggle, and returns an
//! honest account of what ended up live and why. Memory is the unconditional final
//! fallback — this function never fails to hand back a usable store.
//!
//! **2026-07-03 (round D, §A):** the encrypted-file vault (`crate::file`) was dropped
//! from this chain — Murphy: the passphrase-on-every-launch vault UX is dead. The
//! model is now **keyring → memory**: no working keyring means sid does not persist
//! credentials at all; callers fall back to asking for the password at connect time
//! instead (`crates/sid/src/ui/password_prompt.rs`). `file.rs` itself stays in this
//! crate (its crypto is reviewed and tested) but is wired into nothing — see that
//! module's doc comment.

use crate::keyring::{self, KeyringStore};
use crate::{MemorySecretStore, SecretStore};

/// The user's persisted backend toggle. Mirrors `sid_store::Settings::secret_keyring_enabled`
/// — `sid-secrets` does not depend on `sid-store` (that would invert the dependency
/// direction the store already has on this crate), so the caller maps its own settings
/// into this shape.
///
/// `Settings` still carries a `secret_file_enabled` field (postcard positional encoding
/// means it can't be removed — see that field's doc comment), but it is never read into
/// this shape: the encrypted-file backend is no longer a candidate at all.
#[derive(Debug, Clone, Copy)]
pub struct SecretBackendToggles {
    /// Whether the OS keyring is a candidate backend at all. Still gated by the startup
    /// durability probe — enabling this doesn't guarantee the keyring is what's used.
    pub keyring_enabled: bool,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// The OS keyring, having passed the startup probe.
    Keyring,
    /// The non-persistent in-memory fallback — either the keyring is disabled, or it
    /// failed the startup probe. Secrets asked for while this is effective are
    /// requested fresh every session (the connect-time password prompt).
    Memory,
}

/// [`resolve_secret_store`]'s full result.
pub struct Resolved {
    /// The store every secret call site should use.
    pub store: Box<dyn SecretStore>,
    /// Which backend is actually live.
    pub effective: BackendKind,
    /// Set exactly when the effective backend is weaker than what the user's toggle
    /// asked for: memory-only when a keyring was wanted (disabled, or enabled but
    /// unavailable).
    pub warning: Option<String>,
    /// Set alongside `warning`: how to get a persistent backend back.
    pub recommendation: Option<String>,
}

const KEYRING_RECOMMENDATION: &str = "install a Secret Service provider (e.g. `sudo pacman -S gnome-keyring`) so secrets \
     persist across restarts";

/// Select the effective secret backend and open it.
///
/// `keyring_probe` runs only when `toggles.keyring_enabled` is true. Production callers
/// pass [`probe_keyring`]; tests inject a canned [`KeyringProbe`] so the full selection
/// matrix is exercised without a real OS keyring daemon.
pub fn resolve_secret_store(
    toggles: SecretBackendToggles,
    keyring_probe: impl FnOnce() -> KeyringProbe,
) -> Resolved {
    if !toggles.keyring_enabled {
        return Resolved {
            store: Box::new(MemorySecretStore::new()),
            effective: BackendKind::Memory,
            warning: Some(
                "the OS keyring is disabled; secrets will not persist across restarts".to_string(),
            ),
            recommendation: Some("enable the OS keyring in settings".to_string()),
        };
    }

    match keyring_probe() {
        KeyringProbe::Available => Resolved {
            store: Box::new(KeyringStore::new()),
            effective: BackendKind::Keyring,
            warning: None,
            recommendation: None,
        },
        KeyringProbe::Unavailable(reason) => Resolved {
            store: Box::new(MemorySecretStore::new()),
            effective: BackendKind::Memory,
            warning: Some(format!(
                "OS keyring unavailable ({reason}); secrets will not persist across restarts"
            )),
            recommendation: Some(KEYRING_RECOMMENDATION.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toggles(keyring_enabled: bool) -> SecretBackendToggles {
        SecretBackendToggles { keyring_enabled }
    }

    // ---- the (keyring enabled x probe outcome) selection matrix -----------------

    #[test]
    fn keyring_on_probe_pass_wins() {
        let r = resolve_secret_store(toggles(true), || KeyringProbe::Available);
        assert!(matches!(r.effective, BackendKind::Keyring));
        assert!(r.warning.is_none());
        assert!(r.recommendation.is_none());
    }

    #[test]
    fn keyring_on_probe_fail_falls_back_to_memory() {
        let r = resolve_secret_store(toggles(true), || {
            KeyringProbe::Unavailable("no Secret Service".into())
        });
        assert!(matches!(r.effective, BackendKind::Memory));
        let warning = r.warning.expect("must warn about memory-only persistence");
        assert!(warning.contains("no Secret Service"), "{warning}");
        assert!(warning.contains("will not persist"), "{warning}");
        assert!(r.recommendation.is_some());
    }

    #[test]
    fn keyring_off_uses_memory_with_a_warning_and_recommendation() {
        let r = resolve_secret_store(toggles(false), || {
            panic!("probe must not run when the keyring is disabled")
        });
        assert!(matches!(r.effective, BackendKind::Memory));
        let warning = r.warning.expect("keyring-disabled must warn");
        assert!(warning.contains("disabled"), "{warning}");
        assert!(r.recommendation.is_some());
    }

    #[test]
    fn keyring_disabled_never_invokes_the_probe() {
        // Regression guard for the toggle semantics: a disabled keyring must short
        // circuit before the probe closure runs at all.
        let _ = resolve_secret_store(toggles(false), || {
            panic!("probe ran despite keyring_enabled == false")
        });
    }

    // ---- resolve_secret_store against the real probe never panics --------------

    #[test]
    fn real_probe_end_to_end_never_panics_and_always_yields_a_usable_store() {
        let toggles = SecretBackendToggles {
            keyring_enabled: true,
        };
        let resolved = resolve_secret_store(toggles, probe_keyring);
        // Whatever backend won, the trait object itself must be minimally sane —
        // we avoid touching a REAL keyring's persistent storage here (see
        // `keyring.rs` for its own dedicated round-trip tests), so this only asserts
        // we got *something* back without a panic.
        match resolved.effective {
            BackendKind::Keyring | BackendKind::Memory => {}
        }
    }
}
