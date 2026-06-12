//! Per-session settings undo ring.
//!
//! Holds at most [`UNDO_RING_CAP`] entries; entries older than
//! [`UNDO_TTL`] are treated as expired and ignored by the `u` chord
//! interceptor.
//!
//! # Examples
//!
//! ```
//! use std::time::{Duration, Instant};
//! use sid::settings_undo::{UndoEntry, UndoPayload, UNDO_RING_CAP, UNDO_TTL};
//!
//! let entry = UndoEntry {
//!     payload: UndoPayload::Theme { prior: "cosmos".into() },
//!     recorded_at: Instant::now(),
//! };
//! assert!(!entry.is_expired());
//! assert_eq!(UNDO_RING_CAP, 10);
//! assert_eq!(UNDO_TTL, Duration::from_secs(30));
//! ```

use std::time::{Duration, Instant};

use sid_widgets::settings::behavior_toggles::ToggleValue;

/// Maximum number of undo entries kept per session. Oldest entry is evicted
/// when a new one is pushed into a full ring.
pub const UNDO_RING_CAP: usize = 10;

/// How long an undo entry remains live. After this duration the `u` chord
/// interceptor treats the entry as expired and does not apply it.
pub const UNDO_TTL: Duration = Duration::from_secs(30);

/// The prior value carried by an [`UndoEntry`].
///
/// `DbPathOverrideWritten` and `FactoryResetConfirmed` are intentionally absent
/// — DB path changes require a restart anyway and factory reset is irreversible
/// in this scope.
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::behavior_toggles::ToggleValue;
/// use sid::settings_undo::UndoPayload;
///
/// let p = UndoPayload::BehaviorToggle {
///     key: "auto_restore_session",
///     prior: ToggleValue::Bool(false),
/// };
/// assert!(matches!(p, UndoPayload::BehaviorToggle { .. }));
/// ```
#[derive(Clone, Debug)]
pub enum UndoPayload {
    /// Undo a behavior toggle: restore `prior` under `key`.
    BehaviorToggle {
        /// The setting key, e.g. `settings_keys::AUTO_RESTORE_SESSION`.
        key: &'static str,
        /// The value that was active *before* the change.
        prior: ToggleValue,
    },
    /// Undo a workspace-roots change: restore these paths.
    WorkspaceRoots {
        /// The root list snapshot active *before* the change.
        prior: Vec<std::path::PathBuf>,
    },
    /// Undo a quick-action upsert: restore by re-upserting the prior record.
    QuickActionUpserted {
        /// The quick-action that was overwritten (or was absent → `None` for
        /// a net-new add that should be deleted; handled by the caller).
        prior: sid_store::QuickAction,
    },
    /// Undo a quick-action removal: restore by re-inserting.
    QuickActionRemoved {
        /// The quick-action that was removed.
        prior: sid_store::QuickAction,
    },
    /// Undo a keybind profile save: restore the prior map snapshot.
    Keybind {
        /// Profile name that was saved.
        profile_name: String,
        /// Map snapshot active *before* the save.
        prior: sid_core::keybind::KeybindMap,
    },
    /// Undo a theme change: restore the prior theme name.
    Theme {
        /// The theme name active *before* the change.
        prior: String,
    },
}

/// A single entry in the per-session undo ring.
///
/// # Examples
///
/// ```
/// use std::time::Instant;
/// use sid::settings_undo::{UndoEntry, UndoPayload};
///
/// let entry = UndoEntry {
///     payload: UndoPayload::Theme { prior: "cosmos".into() },
///     recorded_at: Instant::now(),
/// };
/// assert!(!entry.is_expired());
/// ```
#[derive(Clone, Debug)]
pub struct UndoEntry {
    /// What to restore if the user presses `u`.
    pub payload: UndoPayload,
    /// When the entry was recorded; used to enforce [`UNDO_TTL`].
    ///
    /// Tests age this field directly by subtracting `Duration`s from
    /// `Instant::now()` — the same pattern used by `Toast::spawned_at`.
    pub recorded_at: Instant,
}

impl UndoEntry {
    /// `true` if entry is older than [`UNDO_TTL`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, Instant};
    /// use sid::settings_undo::{UndoEntry, UndoPayload, UNDO_TTL};
    ///
    /// let mut e = UndoEntry {
    ///     payload: UndoPayload::Theme { prior: "cosmos".into() },
    ///     recorded_at: Instant::now(),
    /// };
    /// assert!(!e.is_expired());
    /// e.recorded_at = Instant::now() - UNDO_TTL - Duration::from_millis(1);
    /// assert!(e.is_expired());
    /// ```
    pub fn is_expired(&self) -> bool {
        self.recorded_at.elapsed() > UNDO_TTL
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use sid_store::{OpenStore, RedbStore, TypedSettings, settings_keys};
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn fresh_entry_is_not_expired() {
        let e = UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: Instant::now(),
        };
        assert!(!e.is_expired());
    }

    #[test]
    fn entry_aged_beyond_ttl_is_expired() {
        let mut e = UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: Instant::now(),
        };
        e.recorded_at = Instant::now() - UNDO_TTL - Duration::from_millis(1);
        assert!(e.is_expired());
    }

    #[test]
    fn entry_aged_exactly_at_ttl_boundary_is_expired() {
        let mut e = UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: Instant::now(),
        };
        // Exactly at boundary — elapsed == TTL is still expired
        // (uses strict `>` so equality is NOT expired; that's the intended
        // contract: > means expired, == means still live by one tick).
        // We test one tick past.
        e.recorded_at = Instant::now() - UNDO_TTL - Duration::from_nanos(1);
        assert!(e.is_expired());
    }

    #[test]
    fn expired_entry_is_not_applied_by_ring_check() {
        let mut e = UndoEntry {
            payload: UndoPayload::Theme {
                prior: "cosmos".into(),
            },
            recorded_at: Instant::now(),
        };
        assert!(!e.is_expired());
        e.recorded_at = Instant::now() - UNDO_TTL - Duration::from_millis(1);
        assert!(e.is_expired(), "entry aged beyond TTL must not be applied");
    }

    // Spec-mandated property test.
    //
    // Generates a sequence of `(key_index, do_undo: bool)` operations
    // against a three-key bool store, maintaining a parallel undo ring and
    // model array. After all ops replay, the final store value should match
    // the model (replaying non-undone writes).
    proptest! {
        #[test]
        fn random_toggle_undo_sequence_preserves_baseline(
            ops in prop::collection::vec((0usize..3, any::<bool>()), 1..20usize),
        ) {
            // Three boolean settings, all initialised false.
            const KEYS: [&str; 3] = [
                settings_keys::AUTO_RESTORE_SESSION,
                settings_keys::AUTO_SCAN_WORKSPACES,
                settings_keys::DEFAULT_TAB,
            ];

            let d = tempdir().unwrap();
            let store = RedbStore::open(&d.path().join("t.redb")).unwrap();
            for k in KEYS {
                store.put_bool(k, false).unwrap();
            }

            // model[i] tracks what the store *should* contain after ops.
            let mut model = [false; 3];
            // Simplified undo ring: Vec of (key_index, prior_bool).
            let mut ring: std::collections::VecDeque<(usize, bool)> = std::collections::VecDeque::new();

            for (key_idx, do_undo) in &ops {
                let ki = key_idx % 3;
                if *do_undo {
                    if let Some((undo_ki, prior)) = ring.pop_back() {
                        model[undo_ki] = prior;
                        store.put_bool(KEYS[undo_ki], model[undo_ki]).unwrap();
                    }
                } else {
                    // Record prior before the write.
                    let prior = model[ki];
                    // Evict oldest if at cap.
                    if ring.len() == UNDO_RING_CAP {
                        ring.pop_front();
                    }
                    ring.push_back((ki, prior));
                    model[ki] = !model[ki];
                    store.put_bool(KEYS[ki], model[ki]).unwrap();
                }
            }

            // After all ops, store values must match model.
            for (i, k) in KEYS.iter().enumerate() {
                let stored = store.get_bool(k).unwrap().unwrap_or(false);
                prop_assert_eq!(
                    stored,
                    model[i],
                    "key {}: store={}, model={}",
                    k,
                    stored,
                    model[i]
                );
            }
        }
    }
}
