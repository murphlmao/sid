//! State machine for the two-stage kill confirmation modal.
//!
//! The modal is intentionally pure: it never actually sends a signal. The
//! widget assembly drives transitions and the `run_kill_job` future (added
//! in Plan 5, Task 18) handles the real signal delivery. Decoupling means
//! tests can drive every transition deterministically without touching
//! the OS.
//!
//! ```text
//! Closed
//!   --(k)-->         ConfirmSigterm { pid }
//!                       --(y)-->     AwaitingTerm { pid, deadline }
//!                                       --(tick >= deadline, alive)-->
//!                                                                  ConfirmSigkill { pid }
//!                                                                  --(y)--> Done(EscalatedToSigkill)
//!                                                                  --(n)--> Done(GaveUp)
//!                                       --(tick >= deadline, dead)-->  Done(Killed)
//!                       --(n/Esc)--> Closed
//! ```

use std::time::{Duration, Instant};

use sid_core::adapters::sys::Pid;

/// Terminal-stage outcome the widget surfaces as a toast.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::kill_modal::KillResult;
/// // Variants are distinct.
/// let _a = KillResult::Killed;
/// let _b = KillResult::EscalatedToSigkill;
/// let _c = KillResult::GaveUp;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KillResult {
    /// SIGTERM delivered and process exited within the grace period.
    Killed,
    /// SIGTERM ignored; user confirmed SIGKILL escalation.
    EscalatedToSigkill,
    /// SIGTERM ignored; user declined SIGKILL.
    GaveUp,
}

/// Modal state machine.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, Instant};
/// use sid_core::adapters::sys::Pid;
/// use sid_widgets::network::kill_modal::{KillConfirmModalState, KillResult};
///
/// let mut m = KillConfirmModalState::new();
/// assert!(m.is_closed());
/// m.open(Pid::from_u32(42));
/// assert!(m.is_confirm_sigterm());
/// m.confirm();
/// assert!(m.is_awaiting_term());
/// // Process dies before the deadline.
/// let later = Instant::now() + Duration::from_secs(10);
/// m.tick(later, /* still_alive = */ false);
/// assert_eq!(m.result(), Some(KillResult::Killed));
/// ```
#[derive(Debug)]
pub struct KillConfirmModalState {
    inner: ModalState,
}

#[derive(Debug)]
enum ModalState {
    Closed,
    ConfirmSigterm { pid: Pid },
    AwaitingTerm { pid: Pid, deadline: Instant },
    ConfirmSigkill { pid: Pid },
    Done { result: KillResult, pid: Pid },
}

impl Default for KillConfirmModalState {
    fn default() -> Self {
        Self::new()
    }
}

impl KillConfirmModalState {
    /// Default grace period between SIGTERM and the SIGKILL prompt.
    pub const DEFAULT_GRACE: Duration = Duration::from_secs(5);

    /// Construct a fresh, closed modal.
    pub fn new() -> Self {
        Self {
            inner: ModalState::Closed,
        }
    }

    /// Open the modal targeting `pid`. No-op if already open.
    pub fn open(&mut self, pid: Pid) {
        if matches!(self.inner, ModalState::Closed) {
            self.inner = ModalState::ConfirmSigterm { pid };
        }
    }

    /// Close the modal regardless of state (Esc / n).
    pub fn close(&mut self) {
        self.inner = ModalState::Closed;
    }

    /// Affirm the current stage.
    ///
    /// - `ConfirmSigterm` → moves to `AwaitingTerm` with the default grace
    ///   period (use [`Self::confirm_with_grace`] to specify a custom one).
    /// - `ConfirmSigkill` → moves to `Done(EscalatedToSigkill)`.
    /// - All other states are no-ops, so a stuttered double-`y` keypress
    ///   cannot accidentally advance two stages at once.
    pub fn confirm(&mut self) {
        self.confirm_with_grace(Self::DEFAULT_GRACE, Instant::now());
    }

    /// Like [`Self::confirm`] but with a caller-supplied grace and
    /// reference time (useful in deterministic tests).
    pub fn confirm_with_grace(&mut self, grace: Duration, now: Instant) {
        match self.inner {
            ModalState::ConfirmSigterm { pid } => {
                self.inner = ModalState::AwaitingTerm {
                    pid,
                    deadline: now + grace,
                };
            }
            ModalState::ConfirmSigkill { pid } => {
                self.inner = ModalState::Done {
                    result: KillResult::EscalatedToSigkill,
                    pid,
                };
            }
            _ => {}
        }
    }

    /// Negative response.
    ///
    /// - `ConfirmSigterm` / `ConfirmSigkill` → closes the modal (with
    ///   `Done(GaveUp)` recorded in the SIGKILL case so the toast can
    ///   distinguish "user aborted before any signal" from "user gave up
    ///   after SIGTERM").
    pub fn decline(&mut self) {
        match self.inner {
            ModalState::ConfirmSigterm { .. } => {
                self.inner = ModalState::Closed;
            }
            ModalState::ConfirmSigkill { pid } => {
                self.inner = ModalState::Done {
                    result: KillResult::GaveUp,
                    pid,
                };
            }
            _ => {}
        }
    }

    /// Drive the timer transition from `AwaitingTerm`.
    ///
    /// - If `now >= deadline` and `still_alive` is `false`, transitions to
    ///   `Done(Killed)`.
    /// - If `now >= deadline` and `still_alive` is `true`, transitions to
    ///   `ConfirmSigkill`.
    /// - Otherwise, no transition.
    ///
    /// Other states ignore `tick`.
    pub fn tick(&mut self, now: Instant, still_alive: bool) {
        if let ModalState::AwaitingTerm { pid, deadline } = self.inner
            && now >= deadline
        {
            if still_alive {
                self.inner = ModalState::ConfirmSigkill { pid };
            } else {
                self.inner = ModalState::Done {
                    result: KillResult::Killed,
                    pid,
                };
            }
        }
    }

    /// Acknowledge a `Done` state, returning to `Closed`. No-op otherwise.
    pub fn acknowledge(&mut self) {
        if let ModalState::Done { .. } = self.inner {
            self.inner = ModalState::Closed;
        }
    }

    /// True iff the modal is in the `Closed` state.
    pub fn is_closed(&self) -> bool {
        matches!(self.inner, ModalState::Closed)
    }

    /// True iff the modal is in the `ConfirmSigterm` state.
    pub fn is_confirm_sigterm(&self) -> bool {
        matches!(self.inner, ModalState::ConfirmSigterm { .. })
    }

    /// True iff the modal is in the `AwaitingTerm` state.
    pub fn is_awaiting_term(&self) -> bool {
        matches!(self.inner, ModalState::AwaitingTerm { .. })
    }

    /// True iff the modal is in the `ConfirmSigkill` state.
    pub fn is_confirm_sigkill(&self) -> bool {
        matches!(self.inner, ModalState::ConfirmSigkill { .. })
    }

    /// True iff the modal has reached a terminal state.
    pub fn is_done(&self) -> bool {
        matches!(self.inner, ModalState::Done { .. })
    }

    /// Target PID, if the modal is open in a state that has one.
    pub fn target_pid(&self) -> Option<Pid> {
        match self.inner {
            ModalState::Closed => None,
            ModalState::ConfirmSigterm { pid }
            | ModalState::AwaitingTerm { pid, .. }
            | ModalState::ConfirmSigkill { pid }
            | ModalState::Done { pid, .. } => Some(pid),
        }
    }

    /// Terminal outcome if the modal is `Done`, otherwise `None`.
    pub fn result(&self) -> Option<KillResult> {
        match self.inner {
            ModalState::Done { result, .. } => Some(result),
            _ => None,
        }
    }
}
