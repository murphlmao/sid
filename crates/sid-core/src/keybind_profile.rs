//! Convert between the in-memory [`KeybindMap`] and the persisted entry list
//! that lives in `sid-store`'s `keybinds` table.
//!
//! `sid-core` does not depend on `sid-store`; instead this module exposes
//! [`ProfileEntry`] (a chord-string / action-id pair) which the adapter side
//! converts to/from `sid_store::KeybindEntry`.
//!
//! The chord string format is `"{KeyCode:?}|{u8 mod bits}"` — e.g.
//! `"Char('q')|2"` for `Ctrl+Q`. The format is intentionally human-inspectable
//! so an operator can dump `sid settings get` and read the bindings.
//!
//! # Examples
//!
//! ```
//! use crossterm::event::{KeyCode, KeyModifiers};
//! use sid_core::event::KeyChord;
//! use sid_core::keybind::KeybindMap;
//! use sid_core::keybind_profile::{from_map, to_map};
//!
//! let m = KeybindMap::cosmos_default();
//! let entries = from_map(&m);
//! assert!(!entries.is_empty());
//! let back = to_map(&entries);
//! let quit = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
//! assert!(back.lookup(&quit).is_some());
//! ```

use crossterm::event::{KeyCode, KeyModifiers};

use crate::action::ActionId;
use crate::event::KeyChord;
use crate::keybind::{KeyBinding, KeybindMap};

/// One (chord, action) pair in a portable string form.
///
/// # Examples
///
/// ```
/// use sid_core::keybind_profile::ProfileEntry;
/// let e = ProfileEntry { chord: "Char('q')|2".into(), action: "app.quit".into() };
/// assert_eq!(e.action, "app.quit");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileEntry {
    /// Stringified [`KeyChord`].
    pub chord: String,
    /// Action id.
    pub action: String,
}

/// Stringify a [`KeyChord`] as `"{KeyCode:?}|{u8 mod bits}"`.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::event::KeyChord;
/// use sid_core::keybind_profile::chord_to_string;
///
/// let c = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
/// assert_eq!(chord_to_string(&c), "Char('q')|2");
/// ```
pub fn chord_to_string(c: &KeyChord) -> String {
    format!("{:?}|{}", c.code, c.mods.bits())
}

/// Parse a chord string produced by [`chord_to_string`].
///
/// # Errors
///
/// Returns a description string if the input is missing the `|` separator,
/// has unparseable mod bits, or names an unsupported keycode variant.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::keybind_profile::chord_from_string;
///
/// let c = chord_from_string("Char('q')|2").unwrap();
/// assert_eq!(c.code, KeyCode::Char('q'));
/// assert_eq!(c.mods, KeyModifiers::CONTROL);
/// ```
pub fn chord_from_string(s: &str) -> Result<KeyChord, String> {
    let (code_s, mods_s) = s
        .rsplit_once('|')
        .ok_or_else(|| format!("missing '|' in {s:?}"))?;
    let bits: u8 = mods_s.parse().map_err(|e| format!("bad mods bits: {e}"))?;
    let code = parse_keycode(code_s)?;
    let mods = KeyModifiers::from_bits(bits)
        .ok_or_else(|| format!("invalid mod bits {bits}"))?;
    Ok(KeyChord::new(code, mods))
}

fn parse_keycode(s: &str) -> Result<KeyCode, String> {
    if let Some(rest) = s.strip_prefix("Char('").and_then(|r| r.strip_suffix("')")) {
        let mut chars = rest.chars();
        let c = chars.next().ok_or_else(|| format!("empty Char in {s}"))?;
        if chars.next().is_some() {
            return Err(format!("multi-char Char({s})"));
        }
        return Ok(KeyCode::Char(c));
    }
    match s {
        "Left" => Ok(KeyCode::Left),
        "Right" => Ok(KeyCode::Right),
        "Up" => Ok(KeyCode::Up),
        "Down" => Ok(KeyCode::Down),
        "Enter" => Ok(KeyCode::Enter),
        "Esc" => Ok(KeyCode::Esc),
        "Tab" => Ok(KeyCode::Tab),
        "BackTab" => Ok(KeyCode::BackTab),
        "Backspace" => Ok(KeyCode::Backspace),
        "Delete" => Ok(KeyCode::Delete),
        "Home" => Ok(KeyCode::Home),
        "End" => Ok(KeyCode::End),
        "PageUp" => Ok(KeyCode::PageUp),
        "PageDown" => Ok(KeyCode::PageDown),
        "Insert" => Ok(KeyCode::Insert),
        "Null" => Ok(KeyCode::Null),
        other => {
            if let Some(rest) = other.strip_prefix("F(").and_then(|r| r.strip_suffix(')')) {
                let n: u8 = rest
                    .parse()
                    .map_err(|e| format!("bad F-key {other}: {e}"))?;
                return Ok(KeyCode::F(n));
            }
            Err(format!("unknown KeyCode: {other}"))
        }
    }
}

/// Snapshot a [`KeybindMap`] as a deterministically-ordered vector of
/// [`ProfileEntry`].
///
/// The order matches [`KeybindMap::iter`] (lexicographic on the internal
/// chord-key string), which means two snapshots of the same map are equal
/// byte-for-byte.
///
/// # Examples
///
/// ```
/// use sid_core::keybind::KeybindMap;
/// use sid_core::keybind_profile::from_map;
///
/// let m = KeybindMap::cosmos_default();
/// let a = from_map(&m);
/// let b = from_map(&m);
/// assert_eq!(a, b);
/// ```
pub fn from_map(map: &KeybindMap) -> Vec<ProfileEntry> {
    map.iter()
        .map(|(chord, action)| ProfileEntry {
            chord: chord_to_string(chord),
            action: action.as_str().to_string(),
        })
        .collect()
}

/// Convert a slice of [`ProfileEntry`] back into a [`KeybindMap`]. Entries
/// whose chord strings fail to parse are silently dropped — this lets the
/// loader tolerate forward-compatible additions to the chord encoding.
///
/// # Examples
///
/// ```
/// use sid_core::keybind::KeybindMap;
/// use sid_core::keybind_profile::{from_map, to_map};
///
/// let m = KeybindMap::cosmos_default();
/// let entries = from_map(&m);
/// let m2 = to_map(&entries);
/// assert_eq!(from_map(&m), from_map(&m2));
/// ```
pub fn to_map(entries: &[ProfileEntry]) -> KeybindMap {
    let mut m = KeybindMap::new();
    for e in entries {
        if let Ok(chord) = chord_from_string(&e.chord) {
            m.bind(KeyBinding {
                chord,
                action: ActionId::new(&e.action),
            });
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn round_trip_specials() {
        for code in [
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Insert,
        ] {
            let c = KeyChord::new(code, KeyModifiers::CONTROL);
            let s = chord_to_string(&c);
            let c2 = chord_from_string(&s).unwrap();
            assert_eq!(c.code, c2.code);
            assert_eq!(c.mods, c2.mods);
        }
    }

    #[test]
    fn round_trip_function_keys() {
        for n in 1..=12u8 {
            let c = KeyChord::new(KeyCode::F(n), KeyModifiers::NONE);
            let s = chord_to_string(&c);
            assert_eq!(chord_from_string(&s).unwrap().code, KeyCode::F(n));
        }
    }

    #[test]
    fn malformed_chord_string_returns_err() {
        assert!(chord_from_string("").is_err());
        assert!(chord_from_string("no-bar").is_err());
        assert!(chord_from_string("Junk|0").is_err());
        assert!(chord_from_string("Char('q')|999").is_err());
    }

    #[test]
    fn missing_separator_errors() {
        assert!(chord_from_string("Char('q')").is_err());
    }

    #[test]
    fn unknown_keycode_errors() {
        assert!(chord_from_string("Mystery|0").is_err());
    }

    #[test]
    fn empty_map_round_trips() {
        let m = KeybindMap::new();
        let entries = from_map(&m);
        assert!(entries.is_empty());
        let m2 = to_map(&entries);
        assert!(m2
            .lookup(&KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL))
            .is_none());
    }

    #[test]
    fn cosmos_default_round_trips() {
        let m = KeybindMap::cosmos_default();
        let entries = from_map(&m);
        let m2 = to_map(&entries);
        let quit = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            m.lookup(&quit).map(|a| a.as_str()),
            m2.lookup(&quit).map(|a| a.as_str()),
        );
    }

    #[test]
    fn entries_are_deterministic_order() {
        let m = KeybindMap::cosmos_default();
        let a = from_map(&m);
        let b = from_map(&m);
        assert_eq!(a, b);
    }

    #[test]
    fn unparseable_entries_are_silently_dropped() {
        let entries = vec![
            ProfileEntry {
                chord: "Char('q')|2".into(),
                action: "app.quit".into(),
            },
            ProfileEntry {
                chord: "garbage".into(),
                action: "nope".into(),
            },
        ];
        let m = to_map(&entries);
        assert!(m
            .lookup(&KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL))
            .is_some());
    }

    proptest! {
        #[test]
        fn prop_chord_string_round_trip(
            code in prop_oneof![
                Just(KeyCode::Left), Just(KeyCode::Right), Just(KeyCode::Up), Just(KeyCode::Down),
                Just(KeyCode::Enter), Just(KeyCode::Esc), Just(KeyCode::Tab),
                Just(KeyCode::BackTab), Just(KeyCode::Backspace), Just(KeyCode::Delete),
                Just(KeyCode::Home), Just(KeyCode::End), Just(KeyCode::PageUp),
                Just(KeyCode::PageDown), Just(KeyCode::Insert),
                (1u8..=24u8).prop_map(KeyCode::F),
                // Restrict to ASCII-printable chars: real keyboards emit those
                // for `KeyCode::Char`. Astral-plane characters round-trip via
                // `Debug` as `'\u{XXXX}'`, which the parser intentionally does
                // not accept (chord encodings stay human-readable).
                (0x20u8..=0x7Eu8).prop_filter("not quote/backslash", |b| {
                    *b != b'\'' && *b != b'\\'
                }).prop_map(|b| KeyCode::Char(b as char)),
            ],
            mods_bits in 0u8..=15u8,
        ) {
            let mods = KeyModifiers::from_bits(mods_bits).unwrap_or(KeyModifiers::NONE);
            let c = KeyChord::new(code, mods);
            let s = chord_to_string(&c);
            let c2 = chord_from_string(&s).expect("round-trip");
            prop_assert_eq!(c.code, c2.code);
            prop_assert_eq!(c.mods.bits(), c2.mods.bits());
        }
    }
}
