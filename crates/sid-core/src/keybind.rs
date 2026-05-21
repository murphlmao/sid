//! Key binding types and the map from [`KeyChord`] to [`ActionId`].
//!
//! [`KeybindMap`] stores bindings with a stable string key derived from the
//! chord so it can be ordered and stored in a `BTreeMap` without requiring
//! `KeyCode` or `KeyModifiers` to be `Ord`.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::action::ActionId;
use crate::event::KeyChord;

/// A single binding from a key chord to an action id.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::action::ActionId;
/// use sid_core::event::KeyChord;
/// use sid_core::keybind::KeyBinding;
///
/// let b = KeyBinding {
///     chord: KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
///     action: ActionId::new("app.quit"),
/// };
/// assert_eq!(b.action.as_str(), "app.quit");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    pub chord: KeyChord,
    pub action: ActionId,
}

/// A map from key chords to action ids.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::action::ActionId;
/// use sid_core::event::KeyChord;
/// use sid_core::keybind::{KeyBinding, KeybindMap};
///
/// let mut map = KeybindMap::new();
/// let chord = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
/// map.bind(KeyBinding { chord, action: ActionId::new("app.quit") });
/// assert_eq!(map.lookup(&chord).map(|a| a.as_str()), Some("app.quit"));
/// ```
#[derive(Default)]
pub struct KeybindMap {
    by_chord: BTreeMap<ChordKey, ActionId>,
}

/// Stable, ordered string key for a [`KeyChord`].
///
/// Derives `Ord` so it can be used as a `BTreeMap` key without requiring
/// `KeyCode` or `KeyModifiers` to implement `Ord`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ChordKey(String);

fn chord_key(c: &KeyChord) -> ChordKey {
    ChordKey(format!("{:?}|{:?}", c.code, c.mods.bits()))
}

impl KeybindMap {
    /// Create an empty keybind map.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::keybind::KeybindMap;
    ///
    /// let map = KeybindMap::new();
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a chord to an action. If the chord was already bound, the old
    /// action is replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::action::ActionId;
    /// use sid_core::event::KeyChord;
    /// use sid_core::keybind::{KeyBinding, KeybindMap};
    ///
    /// let mut map = KeybindMap::new();
    /// let chord = KeyChord::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
    /// map.bind(KeyBinding { chord, action: ActionId::new("palette.open") });
    /// assert!(map.lookup(&chord).is_some());
    /// ```
    pub fn bind(&mut self, b: KeyBinding) {
        self.by_chord.insert(chord_key(&b.chord), b.action);
    }

    /// Look up the action bound to a chord.
    ///
    /// Returns `None` if the chord is not in the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::action::ActionId;
    /// use sid_core::event::KeyChord;
    /// use sid_core::keybind::{KeyBinding, KeybindMap};
    ///
    /// let mut map = KeybindMap::new();
    /// let chord = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    /// let unbound = KeyChord::new(KeyCode::Char('x'), KeyModifiers::NONE);
    ///
    /// map.bind(KeyBinding { chord, action: ActionId::new("app.quit") });
    ///
    /// assert!(map.lookup(&chord).is_some());
    /// assert!(map.lookup(&unbound).is_none());
    /// ```
    pub fn lookup(&self, chord: &KeyChord) -> Option<&ActionId> {
        self.by_chord.get(&chord_key(chord))
    }

    /// Return the built-in "cosmos" default keybind profile.
    ///
    /// Bindings:
    /// - `Ctrl+Left` / `Ctrl+Right` — previous / next tab
    /// - `Ctrl+1` .. `Ctrl+6` — jump to tab N
    /// - `Ctrl+F` — open command palette
    /// - `Ctrl+Q` — quit
    /// - `Ctrl+,` — open settings
    /// - `Ctrl+D` — detach tab
    /// - `Ctrl+A` — attach tab
    /// - `Ctrl+R` — reload tab
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::KeyChord;
    /// use sid_core::keybind::KeybindMap;
    ///
    /// let map = KeybindMap::cosmos_default();
    /// let quit_chord = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    /// assert_eq!(map.lookup(&quit_chord).map(|a| a.as_str()), Some("app.quit"));
    /// ```
    pub fn cosmos_default() -> Self {
        let mut m = Self::new();
        let bind = |m: &mut Self, code: KeyCode, mods: KeyModifiers, action: &str| {
            m.bind(KeyBinding { chord: KeyChord::new(code, mods), action: ActionId::new(action) });
        };
        bind(&mut m, KeyCode::Left, KeyModifiers::CONTROL, "tabs.prev");
        bind(&mut m, KeyCode::Right, KeyModifiers::CONTROL, "tabs.next");
        for i in 1..=6 {
            let c = char::from_digit(i, 10).unwrap();
            bind(&mut m, KeyCode::Char(c), KeyModifiers::CONTROL, &format!("tabs.jump.{i}"));
        }
        bind(&mut m, KeyCode::Char('f'), KeyModifiers::CONTROL, "palette.open");
        bind(&mut m, KeyCode::Char('q'), KeyModifiers::CONTROL, "app.quit");
        bind(&mut m, KeyCode::Char(','), KeyModifiers::CONTROL, "app.open_settings");
        bind(&mut m, KeyCode::Char('d'), KeyModifiers::CONTROL, "tab.detach");
        bind(&mut m, KeyCode::Char('a'), KeyModifiers::CONTROL, "tab.attach");
        bind(&mut m, KeyCode::Char('r'), KeyModifiers::CONTROL, "tab.reload");
        m
    }
}
