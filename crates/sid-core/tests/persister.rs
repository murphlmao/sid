//! Tests for `StatePersister` — debounced dirty-flush gating.

use std::time::Duration;

use sid_core::persister::StatePersister;

// ---------------------------------------------------------------------------
// Plan-specified tests
// ---------------------------------------------------------------------------

#[test]
fn mark_dirty_within_debounce_does_not_trigger() {
    let mut p = StatePersister::new(Duration::from_millis(50));
    p.mark_dirty();
    // Immediately after marking dirty the debounce window hasn't elapsed.
    assert!(!p.should_flush());
    assert!(p.is_dirty());
}

#[test]
fn after_debounce_returns_true() {
    let mut p = StatePersister::new(Duration::from_millis(5));
    p.mark_dirty();
    std::thread::sleep(Duration::from_millis(15));
    assert!(p.should_flush());
}

#[test]
fn clean_means_no_flush_needed() {
    let mut p = StatePersister::new(Duration::from_millis(1));
    assert!(!p.should_flush());
    std::thread::sleep(Duration::from_millis(5));
    // Still no flush because nothing was marked dirty.
    assert!(!p.should_flush());
}

// ---------------------------------------------------------------------------
// Adversarial tests
// ---------------------------------------------------------------------------

/// Duration::ZERO debounce — should_flush is true immediately after mark_dirty.
#[test]
fn zero_debounce_flushes_immediately() {
    let mut p = StatePersister::new(Duration::ZERO);
    assert!(!p.is_dirty());
    p.mark_dirty();
    // With zero debounce, elapsed time is always >= 0.
    assert!(p.should_flush());
    // After flushing, dirty marker is consumed.
    assert!(!p.is_dirty());
}

/// Never-dirty case: is_dirty and should_flush always false.
#[test]
fn never_marked_dirty_never_flushes() {
    let mut p = StatePersister::new(Duration::from_secs(1));
    assert!(!p.is_dirty());
    assert!(!p.should_flush());
    // Even after a long simulated wait (we can't actually sleep long in tests,
    // but we can verify the state machine is correct at construction).
    assert!(!p.is_dirty());
    assert!(!p.should_flush());
}

/// Repeated mark_dirty should NOT reset the timer — only the first call sets
/// dirty_since; subsequent calls are no-ops so the debounce is not extended.
#[test]
fn repeated_mark_dirty_does_not_reset_timer() {
    let mut p = StatePersister::new(Duration::from_millis(5));
    p.mark_dirty();
    std::thread::sleep(Duration::from_millis(3));
    // Second mark_dirty should NOT extend the debounce window.
    p.mark_dirty();
    std::thread::sleep(Duration::from_millis(4)); // 7ms total from first mark_dirty
    // 7ms > 5ms debounce: should flush.
    assert!(p.should_flush());
}

/// should_flush consumes the dirty marker — calling it twice doesn't flush twice.
#[test]
fn should_flush_consumes_dirty_marker() {
    let mut p = StatePersister::new(Duration::from_millis(5));
    p.mark_dirty();
    std::thread::sleep(Duration::from_millis(10));
    assert!(p.should_flush()); // first call: true, consumes marker
    assert!(!p.should_flush()); // second call: false — no longer dirty
    assert!(!p.is_dirty());
}

/// is_dirty returns true between mark_dirty and should_flush.
#[test]
fn is_dirty_reflects_pending_state() {
    let mut p = StatePersister::new(Duration::from_millis(50));
    assert!(!p.is_dirty());
    p.mark_dirty();
    assert!(p.is_dirty());
    // After should_flush returns true, is_dirty clears.
    // (We simulate elapsed time by using a zero debounce in a fresh instance.)
    let mut p2 = StatePersister::new(Duration::ZERO);
    p2.mark_dirty();
    assert!(p2.is_dirty());
    let _ = p2.should_flush();
    assert!(!p2.is_dirty());
}
