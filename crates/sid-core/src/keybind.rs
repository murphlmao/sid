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
/// Internally stores `(KeyChord, ActionId)` tuples keyed by a stable
/// [`ChordKey`] string, so [`KeybindMap::iter`] can yield the original
/// `KeyChord` back to callers that need to display bindings (e.g., the
/// Settings keybind editor).
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
    by_chord: BTreeMap<ChordKey, (KeyChord, ActionId)>,
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
        self.by_chord
            .insert(chord_key(&b.chord), (b.chord, b.action));
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
        self.by_chord.get(&chord_key(chord)).map(|(_c, a)| a)
    }

    /// Iterate over every binding as `(chord, action)` pairs.
    ///
    /// Order is the lexicographic order of the internal [`ChordKey`]
    /// representation; callers that need a different order should sort the
    /// result.
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
    /// map.bind(KeyBinding {
    ///     chord: KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
    ///     action: ActionId::new("app.quit"),
    /// });
    /// let pairs: Vec<_> = map.iter().collect();
    /// assert_eq!(pairs.len(), 1);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (&KeyChord, &ActionId)> {
        self.by_chord
            .values()
            .map(|(chord, action)| (chord, action))
    }

    /// Find the first chord currently bound to `action`, if any.
    ///
    /// Used by the Settings keybind editor to render "action *X* is bound to
    /// chord *Y*." If multiple chords map to the same action, returns
    /// whichever one comes first in [`KeybindMap::iter`]'s ordering.
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
    /// let action = ActionId::new("app.quit");
    /// map.bind(KeyBinding { chord, action: action.clone() });
    /// assert_eq!(map.chord_for_action(&action), Some(&chord));
    /// assert_eq!(map.chord_for_action(&ActionId::new("unbound")), None);
    /// ```
    pub fn chord_for_action(&self, action: &ActionId) -> Option<&KeyChord> {
        self.iter()
            .find_map(|(c, a)| if a == action { Some(c) } else { None })
    }

    /// Remove the binding for `chord`. Idempotent — unbinding a chord that
    /// was never bound is a no-op.
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
    /// map.unbind(&chord);
    /// assert!(map.lookup(&chord).is_none());
    /// // Unbinding an unbound chord is a no-op.
    /// map.unbind(&chord);
    /// ```
    pub fn unbind(&mut self, chord: &KeyChord) {
        self.by_chord.remove(&chord_key(chord));
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
            m.bind(KeyBinding {
                chord: KeyChord::new(code, mods),
                action: ActionId::new(action),
            });
        };
        bind(&mut m, KeyCode::Left, KeyModifiers::CONTROL, "tabs.prev");
        bind(&mut m, KeyCode::Right, KeyModifiers::CONTROL, "tabs.next");
        for i in 1..=6 {
            let c = char::from_digit(i, 10).unwrap();
            bind(
                &mut m,
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
                &format!("tabs.jump.{i}"),
            );
            // Alt fallback for terminals that don't deliver Ctrl+digit
            // as a distinct chord (most VT-style terminals).
            bind(
                &mut m,
                KeyCode::Char(c),
                KeyModifiers::ALT,
                &format!("tabs.jump.{i}"),
            );
        }
        bind(
            &mut m,
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
            "palette.open",
        );
        bind(
            &mut m,
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            "app.quit",
        );
        bind(
            &mut m,
            KeyCode::Char(','),
            KeyModifiers::CONTROL,
            "app.open_settings",
        );
        // Alt fallback for Ctrl+, (most terminals don't deliver it).
        bind(
            &mut m,
            KeyCode::Char(','),
            KeyModifiers::ALT,
            "app.open_settings",
        );
        // Tab close — Ctrl for kitty-protocol-aware terminals, Alt fallback.
        bind(
            &mut m,
            KeyCode::Char('w'),
            KeyModifiers::CONTROL,
            "tab.close",
        );
        bind(
            &mut m,
            KeyCode::Char('w'),
            KeyModifiers::ALT,
            "tab.close",
        );
        bind(
            &mut m,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            "tab.detach",
        );
        bind(
            &mut m,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
            "tab.attach",
        );
        bind(
            &mut m,
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
            "tab.reload",
        );
        m
    }
}
