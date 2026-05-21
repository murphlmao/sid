//! Tests for `decide()` — pure session-restore decision helper.

use sid_core::restore::{decide, RestoreDecision, SessionView};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn s(last_active_secs_ago: u64, cleanly_ended: bool) -> SessionView {
    SessionView { last_active_secs_ago, cleanly_ended }
}

// ---------------------------------------------------------------------------
// Plan-specified tests
// ---------------------------------------------------------------------------

#[test]
fn no_prior_session_means_new() {
    let d = decide(None, 60);
    assert_eq!(d, RestoreDecision::StartNew);
}

#[test]
fn clean_shutdown_means_new_session() {
    let d = decide(Some(s(10, true)), 60);
    assert_eq!(d, RestoreDecision::StartNew);
}

#[test]
fn fresh_dirty_session_offers_resume() {
    let d = decide(Some(s(30, false)), 60);
    assert_eq!(d, RestoreDecision::OfferResume);
}

#[test]
fn stale_dirty_session_is_treated_as_history() {
    let d = decide(Some(s(120, false)), 60);
    assert_eq!(d, RestoreDecision::StartNew);
}

// ---------------------------------------------------------------------------
// Adversarial tests
// ---------------------------------------------------------------------------

/// Boundary: last_active_secs_ago == fresh_threshold_secs (exactly at threshold).
/// The boundary case: > threshold means StartNew; == threshold means OfferResume.
#[test]
fn boundary_at_threshold_offers_resume() {
    // last_active == threshold: s > threshold is false, so OfferResume.
    let d = decide(Some(s(60, false)), 60);
    assert_eq!(d, RestoreDecision::OfferResume);
}

/// Boundary: last_active == threshold + 1 means StartNew.
#[test]
fn one_second_past_threshold_is_start_new() {
    let d = decide(Some(s(61, false)), 60);
    assert_eq!(d, RestoreDecision::StartNew);
}

/// threshold of 0: any session that is dirty should immediately be OfferResume
/// only if last_active_secs_ago == 0, otherwise StartNew.
#[test]
fn threshold_zero_dirty_session_secs_0_offers_resume() {
    // last_active=0 (just happened) and threshold=0 → 0 > 0 is false → OfferResume.
    let d = decide(Some(s(0, false)), 0);
    assert_eq!(d, RestoreDecision::OfferResume);
}

#[test]
fn threshold_zero_dirty_session_secs_1_is_start_new() {
    // last_active=1 and threshold=0 → 1 > 0 → StartNew.
    let d = decide(Some(s(1, false)), 0);
    assert_eq!(d, RestoreDecision::StartNew);
}

/// Clean shutdown always yields StartNew regardless of recency or threshold.
#[test]
fn clean_very_recent_session_is_still_start_new() {
    let d = decide(Some(s(0, true)), 1_000_000);
    assert_eq!(d, RestoreDecision::StartNew);
}

// ---------------------------------------------------------------------------
// Property test: decide is total — never panics on arbitrary inputs
// ---------------------------------------------------------------------------

use proptest::prelude::*;

proptest! {
    /// For any combination of prev session and threshold, decide never panics
    /// and always returns a valid RestoreDecision.
    #[test]
    fn decide_is_total(
        last_active in 0u64..u64::MAX,
        cleanly_ended in any::<bool>(),
        threshold in 0u64..u64::MAX,
        has_prev in any::<bool>(),
    ) {
        let prev = if has_prev {
            Some(SessionView { last_active_secs_ago: last_active, cleanly_ended })
        } else {
            None
        };
        let d = decide(prev, threshold);
        // Any valid RestoreDecision is acceptable — just must not panic.
        prop_assert!(d == RestoreDecision::StartNew || d == RestoreDecision::OfferResume);
    }

    /// No prior session always yields StartNew regardless of threshold.
    #[test]
    fn no_prior_always_start_new(threshold in 0u64..u64::MAX) {
        let d = decide(None::<SessionView>, threshold);
        prop_assert_eq!(d, RestoreDecision::StartNew);
    }

    /// Clean session always yields StartNew regardless of recency or threshold.
    #[test]
    fn clean_session_always_start_new(
        last_active in 0u64..u64::MAX,
        threshold in 0u64..u64::MAX,
    ) {
        let d = decide(Some(SessionView { last_active_secs_ago: last_active, cleanly_ended: true }), threshold);
        prop_assert_eq!(d, RestoreDecision::StartNew);
    }
}
