//! Session-restore decision helper.
//!
//! A pure function that, given a previous session summary and a recency
//! threshold, decides whether to offer the user the option to resume or to
//! start fresh. No I/O, no UI — the binary crate acts on the result.

/// The outcome of a session-restore decision.
///
/// # Examples
///
/// ```
/// use sid_core::restore::{decide, RestoreDecision, SessionView};
///
/// // No prior session → always start new.
/// assert_eq!(decide(None, 60), RestoreDecision::StartNew);
///
/// // Dirty session within threshold → offer resume.
/// assert_eq!(
///     decide(Some(SessionView { last_active_secs_ago: 30, cleanly_ended: false }), 60),
///     RestoreDecision::OfferResume,
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreDecision {
    /// Start a fresh session. The default for clean shutdowns, no prior
    /// session, or when the previous session is too old.
    StartNew,
    /// Prompt the user to resume. Only offered when the previous session was
    /// recent and did not end cleanly (e.g., process killed or crashed).
    OfferResume,
}

/// A minimal view of a previous session used to make the restore decision.
///
/// Constructed by the binary from the `Store` before the UI starts. Using
/// a flat struct keeps `sid-core` free of storage dependencies.
///
/// # Examples
///
/// ```
/// use sid_core::restore::SessionView;
///
/// let v = SessionView { last_active_secs_ago: 45, cleanly_ended: false };
/// assert!(!v.cleanly_ended);
/// assert_eq!(v.last_active_secs_ago, 45);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionView {
    /// Seconds elapsed since the session was last active (based on the stored
    /// heartbeat / last-active timestamp).
    pub last_active_secs_ago: u64,
    /// `true` if the session ended cleanly (the store recorded an `ended_at`
    /// timestamp before the process exited).
    pub cleanly_ended: bool,
}

/// Decide whether to offer resuming a previous session.
///
/// ## Decision rules
///
/// | Condition | Result |
/// |---|---|
/// | No prior session (`prev` is `None`) | [`RestoreDecision::StartNew`] |
/// | Prior session ended cleanly | [`RestoreDecision::StartNew`] |
/// | Prior session is stale (`last_active > threshold`) | [`RestoreDecision::StartNew`] |
/// | Prior session is recent and dirty | [`RestoreDecision::OfferResume`] |
///
/// ## Boundary
///
/// `last_active_secs_ago == fresh_threshold_secs` is treated as "within the
/// threshold" (i.e., returns `OfferResume` if all other conditions hold).
/// Only `last_active_secs_ago > fresh_threshold_secs` triggers `StartNew`.
///
/// ## Examples
///
/// ```
/// use sid_core::restore::{decide, RestoreDecision, SessionView};
///
/// // No prior session.
/// assert_eq!(decide(None, 60), RestoreDecision::StartNew);
///
/// // Clean shutdown — always start new even if very recent.
/// assert_eq!(
///     decide(Some(SessionView { last_active_secs_ago: 5, cleanly_ended: true }), 60),
///     RestoreDecision::StartNew,
/// );
///
/// // Fresh dirty session — offer resume.
/// assert_eq!(
///     decide(Some(SessionView { last_active_secs_ago: 30, cleanly_ended: false }), 60),
///     RestoreDecision::OfferResume,
/// );
///
/// // Stale dirty session — treat as history, start new.
/// assert_eq!(
///     decide(Some(SessionView { last_active_secs_ago: 120, cleanly_ended: false }), 60),
///     RestoreDecision::StartNew,
/// );
/// ```
pub fn decide(prev: Option<SessionView>, fresh_threshold_secs: u64) -> RestoreDecision {
    match prev {
        None => RestoreDecision::StartNew,
        Some(s) if s.cleanly_ended => RestoreDecision::StartNew,
        Some(s) if s.last_active_secs_ago > fresh_threshold_secs => RestoreDecision::StartNew,
        Some(_) => RestoreDecision::OfferResume,
    }
}
