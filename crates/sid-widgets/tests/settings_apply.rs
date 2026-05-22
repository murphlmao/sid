//! Unit tests for the Settings sub-view Outcome plumbing.

use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::event::{Event, KeyChord};
use sid_widgets::settings::behavior_toggles::{
    BehaviorTogglesOutcome, BehaviorTogglesView, ToggleValue,
};

fn key(code: KeyCode) -> Event {
    Event::Key(KeyChord::new(code, KeyModifiers::NONE))
}

#[test]
fn right_arrow_emits_toggled_outcome() {
    let mut v = BehaviorTogglesView::defaults();
    let out = v.handle_event(&key(KeyCode::Right));
    match out {
        BehaviorTogglesOutcome::Toggled { key, value } => {
            assert_eq!(key, "auto_restore_session");
            assert!(matches!(value, ToggleValue::Choice { .. }));
        }
        BehaviorTogglesOutcome::None => panic!("expected Toggled, got None"),
    }
}

#[test]
fn up_down_only_moves_focus_no_outcome() {
    let mut v = BehaviorTogglesView::defaults();
    assert_eq!(v.handle_event(&key(KeyCode::Down)), BehaviorTogglesOutcome::None);
    assert_eq!(v.handle_event(&key(KeyCode::Up)), BehaviorTogglesOutcome::None);
}

#[test]
fn left_arrow_cycles_backward_and_emits() {
    let mut v = BehaviorTogglesView::defaults();
    let _ = v.handle_event(&key(KeyCode::Right));
    let out = v.handle_event(&key(KeyCode::Left));
    assert!(matches!(out, BehaviorTogglesOutcome::Toggled { .. }));
}

#[test]
fn unrecognised_key_is_none() {
    let mut v = BehaviorTogglesView::defaults();
    assert_eq!(
        v.handle_event(&key(KeyCode::Char('z'))),
        BehaviorTogglesOutcome::None,
    );
}

#[test]
fn h_l_vim_keys_also_cycle() {
    let mut v = BehaviorTogglesView::defaults();
    let out = v.handle_event(&key(KeyCode::Char('l')));
    assert!(matches!(out, BehaviorTogglesOutcome::Toggled { .. }));
    let out = v.handle_event(&key(KeyCode::Char('h')));
    assert!(matches!(out, BehaviorTogglesOutcome::Toggled { .. }));
}

#[test]
fn j_k_vim_keys_move_focus_silently() {
    let mut v = BehaviorTogglesView::defaults();
    assert_eq!(
        v.handle_event(&key(KeyCode::Char('j'))),
        BehaviorTogglesOutcome::None,
    );
    assert_eq!(
        v.handle_event(&key(KeyCode::Char('k'))),
        BehaviorTogglesOutcome::None,
    );
}
