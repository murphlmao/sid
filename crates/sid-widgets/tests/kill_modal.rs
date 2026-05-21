//! State-machine tests for `KillConfirmModalState`. Every documented
//! transition gets its own test; adversarial cases cover stuttered
//! keypresses, premature ticks, and the "Esc returns to Closed" guarantee.

use std::time::{Duration, Instant};

use sid_core::adapters::sys::Pid;
use sid_widgets::network::kill_modal::{KillConfirmModalState, KillResult};

const GRACE: Duration = Duration::from_secs(5);

fn pid() -> Pid {
    Pid::from_u32(42)
}

fn fresh() -> KillConfirmModalState {
    KillConfirmModalState::new()
}

#[test]
fn starts_closed() {
    let m = fresh();
    assert!(m.is_closed());
    assert!(m.target_pid().is_none());
    assert!(m.result().is_none());
}

#[test]
fn open_transitions_to_confirm_sigterm() {
    let mut m = fresh();
    m.open(pid());
    assert!(m.is_confirm_sigterm());
    assert_eq!(m.target_pid(), Some(pid()));
}

#[test]
fn open_when_already_open_is_noop() {
    let mut m = fresh();
    m.open(pid());
    m.open(Pid::from_u32(100)); // ignored
    assert_eq!(m.target_pid(), Some(pid()));
}

#[test]
fn confirm_from_confirm_sigterm_enters_awaiting_term() {
    let mut m = fresh();
    m.open(pid());
    m.confirm_with_grace(GRACE, Instant::now());
    assert!(m.is_awaiting_term());
}

#[test]
fn decline_from_confirm_sigterm_closes() {
    let mut m = fresh();
    m.open(pid());
    m.decline();
    assert!(m.is_closed());
}

#[test]
fn close_from_any_state_returns_to_closed() {
    for setup in [
        |m: &mut KillConfirmModalState| m.open(pid()),
        |m: &mut KillConfirmModalState| {
            m.open(pid());
            m.confirm_with_grace(GRACE, Instant::now());
        },
    ] {
        let mut m = fresh();
        setup(&mut m);
        m.close();
        assert!(m.is_closed());
    }
}

#[test]
fn tick_before_deadline_keeps_state_in_awaiting_term() {
    let mut m = fresh();
    let t0 = Instant::now();
    m.open(pid());
    m.confirm_with_grace(GRACE, t0);
    m.tick(t0 + Duration::from_millis(100), true);
    assert!(m.is_awaiting_term());
}

#[test]
fn tick_after_deadline_dead_transitions_to_killed() {
    let mut m = fresh();
    let t0 = Instant::now();
    m.open(pid());
    m.confirm_with_grace(GRACE, t0);
    m.tick(t0 + GRACE, false);
    assert!(m.is_done());
    assert_eq!(m.result(), Some(KillResult::Killed));
}

#[test]
fn tick_after_deadline_alive_transitions_to_confirm_sigkill() {
    let mut m = fresh();
    let t0 = Instant::now();
    m.open(pid());
    m.confirm_with_grace(GRACE, t0);
    m.tick(t0 + GRACE, true);
    assert!(m.is_confirm_sigkill());
}

#[test]
fn confirm_from_confirm_sigkill_escalates() {
    let mut m = fresh();
    let t0 = Instant::now();
    m.open(pid());
    m.confirm_with_grace(GRACE, t0);
    m.tick(t0 + GRACE, true);
    m.confirm_with_grace(GRACE, t0 + GRACE);
    assert!(m.is_done());
    assert_eq!(m.result(), Some(KillResult::EscalatedToSigkill));
}

#[test]
fn decline_from_confirm_sigkill_records_gave_up() {
    let mut m = fresh();
    let t0 = Instant::now();
    m.open(pid());
    m.confirm_with_grace(GRACE, t0);
    m.tick(t0 + GRACE, true);
    m.decline();
    assert!(m.is_done());
    assert_eq!(m.result(), Some(KillResult::GaveUp));
}

#[test]
fn acknowledge_clears_done_to_closed() {
    let mut m = fresh();
    let t0 = Instant::now();
    m.open(pid());
    m.confirm_with_grace(GRACE, t0);
    m.tick(t0 + GRACE, false);
    assert!(m.is_done());
    m.acknowledge();
    assert!(m.is_closed());
}

// ---- adversarial ----

#[test]
fn double_confirm_from_confirm_sigterm_does_not_skip_to_done() {
    let mut m = fresh();
    let t0 = Instant::now();
    m.open(pid());
    m.confirm_with_grace(GRACE, t0);
    // Second `confirm` while in AwaitingTerm must be a no-op.
    m.confirm_with_grace(GRACE, t0);
    assert!(m.is_awaiting_term());
}

#[test]
fn confirm_in_closed_is_noop() {
    let mut m = fresh();
    m.confirm_with_grace(GRACE, Instant::now());
    assert!(m.is_closed());
}

#[test]
fn decline_in_closed_is_noop() {
    let mut m = fresh();
    m.decline();
    assert!(m.is_closed());
}

#[test]
fn tick_in_closed_is_noop() {
    let mut m = fresh();
    m.tick(Instant::now(), true);
    assert!(m.is_closed());
}

#[test]
fn tick_in_confirm_sigterm_does_not_advance() {
    let mut m = fresh();
    m.open(pid());
    m.tick(Instant::now() + GRACE, true);
    assert!(m.is_confirm_sigterm());
}
