//! Integration tests for `CommandPalette`.

use sid_core::{
    action::{Action, ActionRegistry},
    palette::CommandPalette,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn populated_registry() -> ActionRegistry {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("app.quit", "Quit"));
    reg.register(Action::new("palette.open", "Open Palette"));
    reg.register(Action::new("tabs.next", "Next Tab"));
    reg.register(Action::new("tabs.prev", "Previous Tab"));
    reg
}

fn empty_registry() -> ActionRegistry {
    ActionRegistry::new()
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn new_palette_is_closed() {
    let p = CommandPalette::new();
    assert!(!p.is_open());
}

#[test]
fn new_palette_has_empty_query() {
    let p = CommandPalette::new();
    assert_eq!(p.query(), "");
}

#[test]
fn new_palette_selected_is_zero() {
    let p = CommandPalette::new();
    assert_eq!(p.selected_index(), 0);
}

#[test]
fn default_equals_new() {
    let d = CommandPalette::default();
    assert!(!d.is_open());
    assert_eq!(d.query(), "");
    assert_eq!(d.selected_index(), 0);
}

// ---------------------------------------------------------------------------
// open / close
// ---------------------------------------------------------------------------

#[test]
fn open_sets_open_flag() {
    let mut p = CommandPalette::new();
    p.open();
    assert!(p.is_open());
}

#[test]
fn open_clears_existing_query() {
    let mut p = CommandPalette::new();
    p.input("stale");
    p.open();
    assert_eq!(p.query(), "");
}

#[test]
fn open_resets_selection() {
    let mut p = CommandPalette::new();
    // manually advance selection would require cursor movement, so just open twice
    p.open();
    p.open();
    assert_eq!(p.selected_index(), 0);
}

#[test]
fn close_clears_open_flag() {
    let mut p = CommandPalette::new();
    p.open();
    p.close();
    assert!(!p.is_open());
}

#[test]
fn close_clears_query() {
    let mut p = CommandPalette::new();
    p.open();
    p.input("something");
    p.close();
    assert_eq!(p.query(), "");
}

#[test]
fn close_resets_selection() {
    let mut p = CommandPalette::new();
    p.open();
    p.close();
    assert_eq!(p.selected_index(), 0);
}

#[test]
fn open_then_close_state_matches_new() {
    let mut p = CommandPalette::new();
    p.open();
    p.input("x");
    p.close();
    // should be equivalent to a freshly constructed palette
    assert!(!p.is_open());
    assert_eq!(p.query(), "");
    assert_eq!(p.selected_index(), 0);
}

// ---------------------------------------------------------------------------
// input / backspace
// ---------------------------------------------------------------------------

#[test]
fn input_appends_to_query() {
    let mut p = CommandPalette::new();
    p.input("qu");
    p.input("it");
    assert_eq!(p.query(), "quit");
}

#[test]
fn input_resets_selected_to_zero() {
    let reg = populated_registry();
    let mut p = CommandPalette::new();
    p.input("q");
    p.cursor_down(&reg); // move to 1
    p.input("u"); // should reset to 0
    assert_eq!(p.selected_index(), 0);
}

#[test]
fn backspace_removes_last_char() {
    let mut p = CommandPalette::new();
    p.input("qui");
    p.backspace();
    assert_eq!(p.query(), "qu");
}

#[test]
fn backspace_on_empty_is_noop() {
    let mut p = CommandPalette::new();
    p.backspace(); // should not panic
    assert_eq!(p.query(), "");
}

#[test]
fn backspace_multiple_times_stays_at_empty() {
    let mut p = CommandPalette::new();
    for _ in 0..10 {
        p.backspace();
    }
    assert_eq!(p.query(), "");
}

// ---------------------------------------------------------------------------
// matches / current
// ---------------------------------------------------------------------------

#[test]
fn matches_returns_filtered_actions() {
    let reg = populated_registry();
    let mut p = CommandPalette::new();
    p.input("quit");
    let m = p.matches(&reg);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].id.as_str(), "app.quit");
}

#[test]
fn matches_empty_query_returns_all() {
    let reg = populated_registry();
    let p = CommandPalette::new();
    assert_eq!(p.matches(&reg).len(), 4);
}

#[test]
fn current_returns_selected_action() {
    let reg = populated_registry();
    let mut p = CommandPalette::new();
    p.input("quit");
    let hit = p.current(&reg);
    assert!(hit.is_some());
    assert_eq!(hit.unwrap().id.as_str(), "app.quit");
}

#[test]
fn current_returns_none_on_no_match() {
    let reg = populated_registry();
    let mut p = CommandPalette::new();
    p.input("zzzzz");
    assert!(p.current(&reg).is_none());
}

#[test]
fn current_returns_none_on_empty_registry() {
    let reg = empty_registry();
    let p = CommandPalette::new();
    assert!(p.current(&reg).is_none());
}

// ---------------------------------------------------------------------------
// cursor_down / cursor_up
// ---------------------------------------------------------------------------

#[test]
fn cursor_down_advances_selection() {
    let reg = populated_registry();
    let mut p = CommandPalette::new();
    // empty query → all 4 actions visible
    p.cursor_down(&reg);
    assert_eq!(p.selected_index(), 1);
}

#[test]
fn cursor_down_wraps_around() {
    let reg = populated_registry();
    let mut p = CommandPalette::new();
    // 4 actions: cycle all the way around
    for _ in 0..4 {
        p.cursor_down(&reg);
    }
    assert_eq!(p.selected_index(), 0);
}

#[test]
fn cursor_up_wraps_from_zero_to_last() {
    let reg = populated_registry();
    let mut p = CommandPalette::new();
    p.cursor_up(&reg); // 0 → 3 (4 matches)
    assert_eq!(p.selected_index(), 3);
}

#[test]
fn cursor_down_on_empty_matches_does_not_panic() {
    let reg = empty_registry();
    let mut p = CommandPalette::new();
    // empty registry → no matches; should not panic or divide by zero
    p.cursor_down(&reg);
    p.cursor_down(&reg);
    assert_eq!(p.selected_index(), 0);
}

#[test]
fn cursor_up_on_empty_matches_does_not_panic() {
    let reg = empty_registry();
    let mut p = CommandPalette::new();
    p.cursor_up(&reg);
    p.cursor_up(&reg);
    assert_eq!(p.selected_index(), 0);
}

#[test]
fn cursor_down_on_no_match_query_does_not_panic() {
    let reg = populated_registry();
    let mut p = CommandPalette::new();
    p.input("zzzzz"); // no matches
    p.cursor_down(&reg); // should not panic
    assert_eq!(p.selected_index(), 0); // clamped to 0 with max(1)
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

use proptest::prelude::*;

proptest! {
    /// After open() then close(), state is identical to a fresh new().
    #[test]
    fn open_close_resets_to_fresh_state(
        query_chars in "[a-z]{0,20}",
    ) {
        let mut p = CommandPalette::new();
        p.open();
        p.input(&query_chars);
        p.close();
        prop_assert!(!p.is_open());
        prop_assert_eq!(p.query(), "");
        prop_assert_eq!(p.selected_index(), 0);
    }

    /// After input, selected_index is always 0.
    #[test]
    fn input_always_resets_selected(s in "[a-z]{1,20}") {
        let reg = populated_registry();
        let mut p = CommandPalette::new();
        // Move selection away first
        p.cursor_down(&reg);
        p.input(&s);
        prop_assert_eq!(p.selected_index(), 0);
    }

    /// cursor_down then cursor_up returns to the original selection when
    /// there is at least one match.
    #[test]
    fn cursor_down_then_up_is_identity(prefix in "[a-zA-Z]{0,5}") {
        let reg = populated_registry();
        let mut p = CommandPalette::new();
        p.input(&prefix);
        // Only test when there are matches; fuzzy can return 0 for weird prefixes
        let n = p.matches(&reg).len();
        if n > 1 {
            let before = p.selected_index();
            p.cursor_down(&reg);
            p.cursor_up(&reg);
            prop_assert_eq!(p.selected_index(), before);
        }
    }
}
