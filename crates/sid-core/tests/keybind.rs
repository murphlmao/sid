//! Integration tests for `KeyBinding` and `KeybindMap`.

use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::action::ActionId;
use sid_core::event::KeyChord;
use sid_core::keybind::{KeyBinding, KeybindMap};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn ctrl(code: KeyCode) -> KeyChord {
    KeyChord::new(code, KeyModifiers::CONTROL)
}

fn bare(code: KeyCode) -> KeyChord {
    KeyChord::new(code, KeyModifiers::NONE)
}

fn bind_chord(map: &mut KeybindMap, chord: KeyChord, action: &str) {
    map.bind(KeyBinding { chord, action: ActionId::new(action) });
}

// ---------------------------------------------------------------------------
// Basic bind + lookup
// ---------------------------------------------------------------------------

#[test]
fn bind_then_lookup_returns_action() {
    let mut map = KeybindMap::new();
    let chord = ctrl(KeyCode::Char('q'));
    bind_chord(&mut map, chord, "app.quit");
    assert_eq!(map.lookup(&chord).map(|a| a.as_str()), Some("app.quit"));
}

#[test]
fn lookup_unbound_chord_returns_none() {
    let map = KeybindMap::new();
    let chord = ctrl(KeyCode::Char('q'));
    assert!(map.lookup(&chord).is_none());
}

#[test]
fn rebind_overwrites_previous_action() {
    let mut map = KeybindMap::new();
    let chord = ctrl(KeyCode::Char('q'));
    bind_chord(&mut map, chord, "first.action");
    bind_chord(&mut map, chord, "second.action");
    assert_eq!(map.lookup(&chord).map(|a| a.as_str()), Some("second.action"));
}

// ---------------------------------------------------------------------------
// Modifier differentiation
// ---------------------------------------------------------------------------

#[test]
fn different_modifier_is_different_binding() {
    let mut map = KeybindMap::new();
    let ctrl_q = ctrl(KeyCode::Char('q'));
    let bare_q = bare(KeyCode::Char('q'));
    bind_chord(&mut map, ctrl_q, "app.quit");
    // bare 'q' was not bound
    assert!(map.lookup(&bare_q).is_none());
}

#[test]
fn shift_modifier_distinguished_from_control() {
    let mut map = KeybindMap::new();
    let ctrl_f = ctrl(KeyCode::Char('f'));
    let shift_f = KeyChord::new(KeyCode::Char('f'), KeyModifiers::SHIFT);
    bind_chord(&mut map, ctrl_f, "palette.open");
    assert!(map.lookup(&shift_f).is_none());
}

#[test]
fn no_modifier_chord_not_confused_with_ctrl_chord() {
    let mut map = KeybindMap::new();
    let bare_esc = bare(KeyCode::Esc);
    let ctrl_esc = ctrl(KeyCode::Esc);
    bind_chord(&mut map, bare_esc, "palette.close");
    assert!(map.lookup(&ctrl_esc).is_none());
}

// ---------------------------------------------------------------------------
// cosmos_default profile
// ---------------------------------------------------------------------------

#[test]
fn cosmos_quit_bound() {
    let map = KeybindMap::cosmos_default();
    let chord = ctrl(KeyCode::Char('q'));
    assert_eq!(map.lookup(&chord).map(|a| a.as_str()), Some("app.quit"));
}

#[test]
fn cosmos_tab_next_and_prev_bound() {
    let map = KeybindMap::cosmos_default();
    assert_eq!(
        map.lookup(&ctrl(KeyCode::Right)).map(|a| a.as_str()),
        Some("tabs.next")
    );
    assert_eq!(
        map.lookup(&ctrl(KeyCode::Left)).map(|a| a.as_str()),
        Some("tabs.prev")
    );
}

#[test]
fn cosmos_jump_tabs_1_through_6_bound() {
    let map = KeybindMap::cosmos_default();
    for i in 1u32..=6 {
        let c = char::from_digit(i, 10).unwrap();
        let chord = ctrl(KeyCode::Char(c));
        let expected = format!("tabs.jump.{i}");
        assert_eq!(
            map.lookup(&chord).map(|a| a.as_str()),
            Some(expected.as_str()),
            "tabs.jump.{i} not bound"
        );
    }
}

#[test]
fn cosmos_palette_open_bound() {
    let map = KeybindMap::cosmos_default();
    assert_eq!(
        map.lookup(&ctrl(KeyCode::Char('f'))).map(|a| a.as_str()),
        Some("palette.open")
    );
}

#[test]
fn cosmos_settings_bound() {
    let map = KeybindMap::cosmos_default();
    assert_eq!(
        map.lookup(&ctrl(KeyCode::Char(','))).map(|a| a.as_str()),
        Some("app.open_settings")
    );
}

#[test]
fn cosmos_detach_attach_reload_bound() {
    let map = KeybindMap::cosmos_default();
    assert_eq!(
        map.lookup(&ctrl(KeyCode::Char('d'))).map(|a| a.as_str()),
        Some("tab.detach")
    );
    assert_eq!(
        map.lookup(&ctrl(KeyCode::Char('a'))).map(|a| a.as_str()),
        Some("tab.attach")
    );
    assert_eq!(
        map.lookup(&ctrl(KeyCode::Char('r'))).map(|a| a.as_str()),
        Some("tab.reload")
    );
}

// ---------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------

#[test]
fn unbound_chord_variants_return_none() {
    let map = KeybindMap::cosmos_default();
    // Modifiers that weren't used in cosmos_default
    let alt_q = KeyChord::new(KeyCode::Char('q'), KeyModifiers::ALT);
    assert!(map.lookup(&alt_q).is_none());

    // A completely unmapped key
    let ctrl_z = ctrl(KeyCode::Char('z'));
    assert!(map.lookup(&ctrl_z).is_none());

    // Function keys
    let f1 = bare(KeyCode::F(1));
    assert!(map.lookup(&f1).is_none());
}

#[test]
fn lookup_on_empty_map_always_none() {
    let map = KeybindMap::new();
    for code in [
        KeyCode::Char('a'),
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Left,
        KeyCode::F(1),
    ] {
        assert!(map.lookup(&bare(code)).is_none());
        assert!(map.lookup(&ctrl(code)).is_none());
    }
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

use proptest::prelude::*;

// Restrict to a subset of KeyCode variants that are stable and constructable.
fn arb_key_code() -> impl Strategy<Value = KeyCode> {
    prop_oneof![
        any::<u8>().prop_map(|c| KeyCode::Char(char::from(c.clamp(b'a', b'z')))),
        Just(KeyCode::Enter),
        Just(KeyCode::Esc),
        Just(KeyCode::Left),
        Just(KeyCode::Right),
        Just(KeyCode::Up),
        Just(KeyCode::Down),
        (1u8..=12u8).prop_map(KeyCode::F),
    ]
}

fn arb_mods() -> impl Strategy<Value = KeyModifiers> {
    prop_oneof![
        Just(KeyModifiers::NONE),
        Just(KeyModifiers::CONTROL),
        Just(KeyModifiers::SHIFT),
        Just(KeyModifiers::ALT),
    ]
}

proptest! {
    /// bind(chord, action) then lookup(chord) always returns Some(action).
    #[test]
    fn bind_then_lookup_returns_some(
        code in arb_key_code(),
        mods in arb_mods(),
        action_str in "[a-z]{1,10}(\\.[a-z]{1,10})?",
    ) {
        let mut map = KeybindMap::new();
        let chord = KeyChord::new(code, mods);
        let action = ActionId::new(action_str.clone());
        map.bind(KeyBinding { chord, action });
        let got = map.lookup(&chord);
        prop_assert!(got.is_some());
        prop_assert_eq!(got.unwrap().as_str(), &action_str);
    }

    /// Two distinct chords (different code) are never confused.
    #[test]
    fn distinct_chords_do_not_collide(
        code_a in arb_key_code(),
        code_b in arb_key_code(),
        mods in arb_mods(),
    ) {
        // Only test when codes differ to ensure the chords are actually distinct.
        prop_assume!(code_a != code_b);
        let mut map = KeybindMap::new();
        let chord_a = KeyChord::new(code_a, mods);
        map.bind(KeyBinding { chord: chord_a, action: ActionId::new("a") });
        let chord_b = KeyChord::new(code_b, mods);
        // chord_b should still be unbound
        prop_assert!(map.lookup(&chord_b).is_none());
    }
}
