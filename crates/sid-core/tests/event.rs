use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use proptest::prelude::*;
use sid_core::event::{Event, KeyChord};

// ── existing tests ────────────────────────────────────────────────────────────

#[test]
fn from_crossterm_key_extracts_chord() {
    let crossterm_ev =
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    let ev = Event::from_crossterm(crossterm_ev);
    match ev {
        Event::Key(chord) => {
            assert_eq!(chord, KeyChord::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        }
        other => panic!("expected Key, got {other:?}"),
    }
}

#[test]
fn tick_event_constructs() {
    let _ = Event::Tick;
}

// ── exhaustive from_crossterm mapping ────────────────────────────────────────

#[test]
fn from_crossterm_mouse_passes_through() {
    let mouse = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };
    let ev = Event::from_crossterm(crossterm::event::Event::Mouse(mouse));
    match ev {
        Event::Mouse(m) => {
            assert_eq!(m.column, 10);
            assert_eq!(m.row, 5);
            assert_eq!(m.kind, MouseEventKind::Down(MouseButton::Left));
        }
        other => panic!("expected Mouse, got {other:?}"),
    }
}

#[test]
fn from_crossterm_resize_maps_width_and_height() {
    let ev = Event::from_crossterm(crossterm::event::Event::Resize(120, 40));
    match ev {
        Event::Resize { width, height } => {
            assert_eq!(width, 120);
            assert_eq!(height, 40);
        }
        other => panic!("expected Resize, got {other:?}"),
    }
}

#[test]
fn from_crossterm_resize_zero_dimensions() {
    let ev = Event::from_crossterm(crossterm::event::Event::Resize(0, 0));
    assert_eq!(ev, Event::Resize { width: 0, height: 0 });
}

#[test]
fn from_crossterm_resize_max_dimensions() {
    let ev = Event::from_crossterm(crossterm::event::Event::Resize(u16::MAX, u16::MAX));
    assert_eq!(ev, Event::Resize { width: u16::MAX, height: u16::MAX });
}

#[test]
fn from_crossterm_focus_gained_is_focus_true() {
    let ev = Event::from_crossterm(crossterm::event::Event::FocusGained);
    assert_eq!(ev, Event::Focus(true));
}

#[test]
fn from_crossterm_focus_lost_is_focus_false() {
    let ev = Event::from_crossterm(crossterm::event::Event::FocusLost);
    assert_eq!(ev, Event::Focus(false));
}

#[test]
fn from_crossterm_paste_becomes_custom_paste() {
    let ev = Event::from_crossterm(crossterm::event::Event::Paste("hello world".into()));
    assert_eq!(ev, Event::Custom("paste".into()));
}

#[test]
fn from_crossterm_paste_with_empty_string_still_maps_to_paste() {
    let ev = Event::from_crossterm(crossterm::event::Event::Paste(String::new()));
    assert_eq!(ev, Event::Custom("paste".into()));
}

#[test]
fn from_crossterm_paste_with_long_string_still_maps_to_paste() {
    let long_paste = "x".repeat(100_000);
    let ev = Event::from_crossterm(crossterm::event::Event::Paste(long_paste));
    assert_eq!(ev, Event::Custom("paste".into()));
}

// ── KeyChord: equality properties ────────────────────────────────────────────

#[test]
fn keychord_equality_is_reflexive() {
    let c = KeyChord::new(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(c, c);
}

#[test]
fn keychord_equality_is_symmetric() {
    let a = KeyChord::new(KeyCode::Char('z'), KeyModifiers::ALT);
    let b = KeyChord::new(KeyCode::Char('z'), KeyModifiers::ALT);
    assert_eq!(a, b);
    assert_eq!(b, a);
}

#[test]
fn keychord_different_code_not_equal() {
    let a = KeyChord::new(KeyCode::Char('a'), KeyModifiers::NONE);
    let b = KeyChord::new(KeyCode::Char('b'), KeyModifiers::NONE);
    assert_ne!(a, b);
}

#[test]
fn keychord_different_mods_not_equal() {
    let a = KeyChord::new(KeyCode::Char('a'), KeyModifiers::NONE);
    let b = KeyChord::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_ne!(a, b);
}

// ── KeyChord: Hash consistency ────────────────────────────────────────────────

fn hash_chord(c: &KeyChord) -> u64 {
    let mut h = DefaultHasher::new();
    c.hash(&mut h);
    h.finish()
}

#[test]
fn keychord_hash_is_deterministic() {
    let c = KeyChord::new(KeyCode::F(5), KeyModifiers::SHIFT);
    assert_eq!(hash_chord(&c), hash_chord(&c));
}

#[test]
fn keychord_hash_consistent_across_clones() {
    let c = KeyChord::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
    let cl = c;  // Copy
    assert_eq!(hash_chord(&c), hash_chord(&cl));
}

#[test]
fn keychord_equal_values_have_equal_hashes() {
    let a = KeyChord::new(KeyCode::Esc, KeyModifiers::NONE);
    let b = KeyChord::new(KeyCode::Esc, KeyModifiers::NONE);
    // Per Hash contract: a == b → hash(a) == hash(b)
    assert_eq!(a, b);
    assert_eq!(hash_chord(&a), hash_chord(&b));
}

// ── adversarial: all modifiers set simultaneously ────────────────────────────

#[test]
fn keychord_all_modifiers_simultaneously() {
    // Ctrl+Shift+Alt+Super all set at once
    let all_mods = KeyModifiers::CONTROL
        | KeyModifiers::SHIFT
        | KeyModifiers::ALT
        | KeyModifiers::SUPER;
    let chord = KeyChord::new(KeyCode::Char('a'), all_mods);
    assert!(chord.mods.contains(KeyModifiers::CONTROL));
    assert!(chord.mods.contains(KeyModifiers::SHIFT));
    assert!(chord.mods.contains(KeyModifiers::ALT));
    assert!(chord.mods.contains(KeyModifiers::SUPER));
    // Hash and equality must still work
    let _ = hash_chord(&chord);
    let copy = chord;
    assert_eq!(chord, copy);
}

#[test]
fn keychord_no_modifiers_is_empty() {
    let chord = KeyChord::new(KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(chord.mods.is_empty());
}

#[test]
fn from_crossterm_key_with_all_modifiers() {
    let all_mods = KeyModifiers::CONTROL
        | KeyModifiers::SHIFT
        | KeyModifiers::ALT
        | KeyModifiers::SUPER;
    let ct = crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('a'), all_mods));
    let ev = Event::from_crossterm(ct);
    match ev {
        Event::Key(chord) => {
            assert!(chord.mods.contains(KeyModifiers::CONTROL));
            assert!(chord.mods.contains(KeyModifiers::SHIFT));
            assert!(chord.mods.contains(KeyModifiers::ALT));
            assert!(chord.mods.contains(KeyModifiers::SUPER));
        }
        other => panic!("expected Key, got {other:?}"),
    }
}

#[test]
fn keychord_function_keys_full_range() {
    // F1..F12 — all should hash and compare without panicking
    for n in 1u8..=12 {
        let c = KeyChord::new(KeyCode::F(n), KeyModifiers::NONE);
        let _ = hash_chord(&c);
        let copy = c;
        assert_eq!(c, copy);
    }
}

// ── proptest: KeyChord Hash and equality invariants ──────────────────────────

proptest! {
    #[test]
    fn prop_keychord_hash_consistent_across_copies(
        code_idx in 0usize..10usize,
        mods_bits in 0u8..=0x0f_u8,
    ) {
        let codes = [
            KeyCode::Char('a'), KeyCode::Char('z'), KeyCode::Enter,
            KeyCode::Esc, KeyCode::Tab, KeyCode::Backspace,
            KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right,
        ];
        let code = codes[code_idx];
        let mods = KeyModifiers::from_bits_truncate(mods_bits);
        let c = KeyChord::new(code, mods);
        let copy = c;
        // Equal values must have equal hashes
        prop_assert_eq!(c, copy);
        prop_assert_eq!(hash_chord(&c), hash_chord(&copy));
    }
}

proptest! {
    #[test]
    fn prop_keychord_equality_is_reflexive(
        code_idx in 0usize..5usize,
        mods_bits in 0u8..=7u8,
    ) {
        let codes = [
            KeyCode::Char('a'), KeyCode::Enter, KeyCode::Esc,
            KeyCode::F(1), KeyCode::Backspace,
        ];
        let code = codes[code_idx];
        let mods = KeyModifiers::from_bits_truncate(mods_bits);
        let c = KeyChord::new(code, mods);
        prop_assert_eq!(c, c);
    }
}
