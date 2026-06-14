//! State machine for the keybind editor's capture mode.
//!
//! The editor enters capture mode when the user presses `Enter` on an action
//! row. From there the user can press a key chord, which may either be
//! unbound (immediate apply) or already in use (conflict — confirm overwrite
//! before applying). `Esc` cancels at any point.
//!
//! State diagram:
//!
//! ```text
//!             Enter
//!    Idle ─────────────▶ Waiting
//!     ▲                    │
//!     │ Esc                │ KeyChord captured
//!     │                    ▼
//!     │                Captured(chord)
//!     │                    │
//!     │                    │ conflict? ──Yes──▶ ConfirmOverwrite
//!     │                    │ No                       │ y → Apply
//!     │                    ▼                          │ n → Waiting
//!     └──────────────── Apply ◀───────────────────────┘
//! ```

use crate::{action::ActionId, event::KeyChord};

/// Current position in the capture-mode state machine.
///
/// # Examples
///
/// ```
/// use sid_core::keybind_capture::{CaptureInput, CaptureState};
/// use sid_core::action::ActionId;
///
/// let s = CaptureState::new();
/// let s = s.step(CaptureInput::EnterCaptureFor(ActionId::new("app.quit")));
/// assert!(matches!(s, CaptureState::Waiting { .. }));
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureState {
    /// Not capturing.
    Idle,
    /// Waiting for the user to press a chord for `for_action`.
    Waiting {
        /// Action being rebound.
        for_action: ActionId,
    },
    /// Chord captured; conflict detection pending.
    Captured {
        /// Action being rebound.
        for_action: ActionId,
        /// Chord the user pressed.
        chord: KeyChord,
    },
    /// Chord conflicts with `conflicting_action`; awaiting user confirmation.
    ConfirmOverwrite {
        /// Action being rebound.
        for_action: ActionId,
        /// Chord the user pressed.
        chord: KeyChord,
        /// Action that currently owns this chord.
        conflicting_action: ActionId,
    },
    /// Terminal state — the (chord, action) pair should be persisted by the
    /// owning widget, then [`CaptureInput::Reset`] (or any input) returns to
    /// [`CaptureState::Idle`].
    Apply {
        /// Action that was rebound.
        for_action: ActionId,
        /// Chord that was applied.
        chord: KeyChord,
    },
}

/// External inputs driving the [`CaptureState`] machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureInput {
    /// Press Enter on an action row to begin capturing a new chord for it.
    EnterCaptureFor(ActionId),
    /// The user pressed a chord during capture.
    ChordPressed(KeyChord),
    /// The owning widget detected a conflict between the captured chord and
    /// an existing binding.
    ConflictResolved {
        /// Existing action that this chord is already bound to.
        conflicting_action: ActionId,
    },
    /// The owning widget detected no conflict.
    NoConflict,
    /// User confirmed the overwrite.
    ConfirmYes,
    /// User rejected the overwrite.
    ConfirmNo,
    /// User pressed Esc to cancel.
    Cancel,
    /// External reset after `Apply` has been observed.
    Reset,
}

impl CaptureState {
    /// Initial state — [`CaptureState::Idle`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::keybind_capture::CaptureState;
    /// assert_eq!(CaptureState::new(), CaptureState::Idle);
    /// ```
    pub fn new() -> Self {
        Self::Idle
    }

    /// Apply `input` to `self` and return the resulting state.
    ///
    /// This is a *total* function: every (state, input) pair maps to some
    /// state without panicking. Invalid pairs map to the same state (no-op).
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::action::ActionId;
    /// use sid_core::event::KeyChord;
    /// use sid_core::keybind_capture::{CaptureInput, CaptureState};
    ///
    /// let s = CaptureState::Idle
    ///     .step(CaptureInput::EnterCaptureFor(ActionId::new("a")))
    ///     .step(CaptureInput::ChordPressed(KeyChord::new(
    ///         KeyCode::Char('x'), KeyModifiers::CONTROL,
    ///     )))
    ///     .step(CaptureInput::NoConflict);
    /// assert!(matches!(s, CaptureState::Apply { .. }));
    /// ```
    pub fn step(self, input: CaptureInput) -> Self {
        use CaptureInput::*;
        use CaptureState::*;
        match (self, input) {
            (Idle, EnterCaptureFor(a)) => Waiting { for_action: a },

            (Waiting { for_action }, ChordPressed(c)) => Captured {
                for_action,
                chord: c,
            },
            (Waiting { .. }, Cancel) => Idle,

            (Captured { for_action, chord }, NoConflict) => Apply { for_action, chord },
            (Captured { for_action, chord }, ConflictResolved { conflicting_action }) => {
                ConfirmOverwrite {
                    for_action,
                    chord,
                    conflicting_action,
                }
            }
            (Captured { .. }, Cancel) => Idle,

            (
                ConfirmOverwrite {
                    for_action, chord, ..
                },
                ConfirmYes,
            ) => Apply { for_action, chord },
            (ConfirmOverwrite { for_action, .. }, ConfirmNo) => Waiting { for_action },
            (ConfirmOverwrite { .. }, Cancel) => Idle,

            // After Apply, ANY input resets to Idle. This keeps the contract
            // simple for the owning widget: observe Apply, persist, then send
            // any input (Reset is the canonical one).
            (Apply { .. }, _) => Idle,

            // Anything else is a no-op (invalid transition).
            (state, _) => state,
        }
    }
}

impl Default for CaptureState {
    /// Defaults to [`CaptureState::Idle`].
    fn default() -> Self {
        Self::Idle
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};
    use proptest::prelude::*;

    use super::*;

    fn chord() -> KeyChord {
        KeyChord::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
    }
    fn action() -> ActionId {
        ActionId::new("test.action")
    }

    #[test]
    fn new_starts_idle() {
        assert_eq!(CaptureState::new(), CaptureState::Idle);
    }

    #[test]
    fn default_starts_idle() {
        assert_eq!(CaptureState::default(), CaptureState::Idle);
    }

    #[test]
    fn idle_enter_capture_goes_to_waiting() {
        let s = CaptureState::Idle.step(CaptureInput::EnterCaptureFor(action()));
        assert!(matches!(s, CaptureState::Waiting { .. }));
    }

    #[test]
    fn waiting_chord_pressed_goes_to_captured() {
        let s = CaptureState::Waiting {
            for_action: action(),
        }
        .step(CaptureInput::ChordPressed(chord()));
        assert!(matches!(s, CaptureState::Captured { .. }));
    }

    #[test]
    fn captured_no_conflict_applies() {
        let s = CaptureState::Captured {
            for_action: action(),
            chord: chord(),
        }
        .step(CaptureInput::NoConflict);
        assert!(matches!(s, CaptureState::Apply { .. }));
    }

    #[test]
    fn captured_with_conflict_goes_to_confirm() {
        let s = CaptureState::Captured {
            for_action: action(),
            chord: chord(),
        }
        .step(CaptureInput::ConflictResolved {
            conflicting_action: ActionId::new("other"),
        });
        assert!(matches!(s, CaptureState::ConfirmOverwrite { .. }));
    }

    #[test]
    fn confirm_yes_applies() {
        let s = CaptureState::ConfirmOverwrite {
            for_action: action(),
            chord: chord(),
            conflicting_action: ActionId::new("other"),
        }
        .step(CaptureInput::ConfirmYes);
        assert!(matches!(s, CaptureState::Apply { .. }));
    }

    #[test]
    fn confirm_no_returns_to_waiting() {
        let s = CaptureState::ConfirmOverwrite {
            for_action: action(),
            chord: chord(),
            conflicting_action: ActionId::new("other"),
        }
        .step(CaptureInput::ConfirmNo);
        assert!(matches!(s, CaptureState::Waiting { .. }));
    }

    #[test]
    fn cancel_from_any_state_returns_to_idle() {
        for s in [
            CaptureState::Waiting {
                for_action: action(),
            },
            CaptureState::Captured {
                for_action: action(),
                chord: chord(),
            },
            CaptureState::ConfirmOverwrite {
                for_action: action(),
                chord: chord(),
                conflicting_action: ActionId::new("o"),
            },
        ] {
            let s2 = s.step(CaptureInput::Cancel);
            assert!(matches!(s2, CaptureState::Idle), "got {s2:?}");
        }
    }

    #[test]
    fn idle_chord_pressed_is_noop() {
        let s = CaptureState::Idle;
        let s2 = s.clone().step(CaptureInput::ChordPressed(chord()));
        assert_eq!(s, s2);
    }

    #[test]
    fn idle_confirm_yes_is_noop() {
        let s = CaptureState::Idle;
        let s2 = s.clone().step(CaptureInput::ConfirmYes);
        assert_eq!(s, s2);
    }

    #[test]
    fn waiting_confirm_yes_is_noop() {
        let s = CaptureState::Waiting {
            for_action: action(),
        };
        let s2 = s.clone().step(CaptureInput::ConfirmYes);
        assert_eq!(s, s2);
    }

    #[test]
    fn captured_confirm_yes_is_noop() {
        let s = CaptureState::Captured {
            for_action: action(),
            chord: chord(),
        };
        let s2 = s.clone().step(CaptureInput::ConfirmYes);
        assert_eq!(s, s2);
    }

    #[test]
    fn apply_any_input_resets_to_idle() {
        let s = CaptureState::Apply {
            for_action: action(),
            chord: chord(),
        };
        assert_eq!(s.step(CaptureInput::Reset), CaptureState::Idle);
    }

    #[test]
    fn apply_chord_pressed_also_resets() {
        let s = CaptureState::Apply {
            for_action: action(),
            chord: chord(),
        };
        assert_eq!(
            s.step(CaptureInput::ChordPressed(chord())),
            CaptureState::Idle
        );
    }

    proptest! {
        #[test]
        fn prop_step_is_total(state_pick in 0u8..5, input_pick in 0u8..8) {
            let s = match state_pick {
                0 => CaptureState::Idle,
                1 => CaptureState::Waiting { for_action: action() },
                2 => CaptureState::Captured { for_action: action(), chord: chord() },
                3 => CaptureState::ConfirmOverwrite {
                    for_action: action(), chord: chord(),
                    conflicting_action: ActionId::new("o"),
                },
                _ => CaptureState::Apply { for_action: action(), chord: chord() },
            };
            let i = match input_pick {
                0 => CaptureInput::EnterCaptureFor(action()),
                1 => CaptureInput::ChordPressed(chord()),
                2 => CaptureInput::NoConflict,
                3 => CaptureInput::ConflictResolved { conflicting_action: ActionId::new("o") },
                4 => CaptureInput::ConfirmYes,
                5 => CaptureInput::ConfirmNo,
                6 => CaptureInput::Cancel,
                _ => CaptureInput::Reset,
            };
            // The transition must not panic for any (state, input) pair.
            let _ = s.step(i);
        }
    }
}
