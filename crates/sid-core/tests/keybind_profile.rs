//! Integration tests for `keybind_profile` round-trip through `KeybindMap`.

use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::{
    action::ActionId,
    event::KeyChord,
    keybind::{KeyBinding, KeybindMap},
    keybind_profile::{ProfileEntry, chord_from_string, chord_to_string, from_map, to_map},
};

#[test]
fn empty_map_round_trips() {
    let m = KeybindMap::new();
    let entries = from_map(&m);
    assert!(entries.is_empty());
    let m2 = to_map(&entries);
    assert!(
        m2.lookup(&KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL))
            .is_none()
    );
}

#[test]
fn cosmos_default_full_round_trip() {
    let m = KeybindMap::cosmos_default();
    let entries = from_map(&m);
    let m2 = to_map(&entries);
    // Every entry survives.
    for (chord, action) in m.iter() {
        assert_eq!(
            m2.lookup(chord).map(|a| a.as_str()),
            Some(action.as_str()),
            "missing chord {chord:?}"
        );
    }
}

#[test]
fn single_custom_binding_round_trips() {
    let mut m = KeybindMap::new();
    let chord = KeyChord::new(KeyCode::Char('z'), KeyModifiers::ALT);
    m.bind(KeyBinding {
        chord,
        action: ActionId::new("custom.action"),
    });
    let entries = from_map(&m);
    let m2 = to_map(&entries);
    assert_eq!(m2.lookup(&chord).map(|a| a.as_str()), Some("custom.action"));
}

#[test]
fn chord_string_format_is_stable() {
    let c = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(chord_to_string(&c), "Char('q')|2");
}

#[test]
fn parse_round_trip_preserves_all_modifier_combinations() {
    for bits in 0u8..=15u8 {
        let mods = KeyModifiers::from_bits(bits).unwrap();
        let c = KeyChord::new(KeyCode::Char('a'), mods);
        let s = chord_to_string(&c);
        let c2 = chord_from_string(&s).unwrap();
        assert_eq!(c2.mods, mods, "failed for bits {bits}");
    }
}

#[test]
fn unknown_action_id_still_round_trips_as_opaque_string() {
    let entries = vec![ProfileEntry {
        chord: "Char('x')|0".into(),
        action: "anything.unknown".into(),
    }];
    let m = to_map(&entries);
    let chord = KeyChord::new(KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(
        m.lookup(&chord).map(|a| a.as_str()),
        Some("anything.unknown")
    );
}
