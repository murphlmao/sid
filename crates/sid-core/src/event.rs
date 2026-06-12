use crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent};

/// Normalized event passed through the App event loop.
///
/// Constructed from a raw crossterm event via [`Event::from_crossterm`], or
/// synthesized by the runtime for ticks and custom signals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A key was pressed; modifiers are normalized.
    Key(KeyChord),
    /// A mouse event (raw crossterm value).
    Mouse(MouseEvent),
    /// The terminal was resized.
    Resize { width: u16, height: u16 },
    /// Periodic tick from the runtime (e.g., for animation, heartbeat).
    Tick,
    /// A focus-gained / focus-lost notification from the terminal.
    Focus(bool),
    /// A custom event injected by the runtime (e.g., job completion).
    Custom(String),
}

impl Event {
    /// Convert a raw crossterm event into a normalized [`Event`].
    ///
    /// Every crossterm variant is handled:
    /// - `Key` → [`Event::Key`] with a [`KeyChord`]
    /// - `Mouse` → [`Event::Mouse`] (passthrough)
    /// - `Resize(w, h)` → [`Event::Resize`]
    /// - `FocusGained` → [`Event::Focus`] with `true`
    /// - `FocusLost` → [`Event::Focus`] with `false`
    /// - `Paste(_)` → [`Event::Custom`] with `"paste"` (payload discarded in v1)
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
    /// use sid_core::event::{Event, KeyChord};
    ///
    /// let ct = CtEvent::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    /// let ev = Event::from_crossterm(ct);
    /// assert_eq!(ev, Event::Key(KeyChord::new(KeyCode::Char('q'), KeyModifiers::NONE)));
    /// ```
    pub fn from_crossterm(ev: CtEvent) -> Self {
        match ev {
            CtEvent::Key(KeyEvent {
                code, modifiers, ..
            }) => Event::Key(KeyChord::new(code, modifiers)),
            CtEvent::Mouse(m) => Event::Mouse(m),
            CtEvent::Resize(w, h) => Event::Resize {
                width: w,
                height: h,
            },
            CtEvent::FocusGained => Event::Focus(true),
            CtEvent::FocusLost => Event::Focus(false),
            CtEvent::Paste(_) => Event::Custom("paste".into()),
        }
    }
}

/// Intent of a chord at the tab-strip level (list focus only — pane-focused
/// widgets consume Tab themselves and these are never consulted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripNav {
    /// `Tab` / `Ctrl+Tab` — next tab.
    Next,
    /// `Shift+Tab` / `BackTab` / `Ctrl+Shift+Tab` — previous tab.
    Prev,
    /// Not a strip-navigation chord.
    None,
}

/// A key press with its modifier mask.
///
/// `KeyChord` is `Copy`, `Hash`, and `Eq`; it is safe to store in hash maps and sets.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::event::KeyChord;
///
/// let chord = KeyChord::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
/// assert_eq!(chord.code, KeyCode::Char('s'));
/// assert_eq!(chord.mods, KeyModifiers::CONTROL);
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyChord {
    /// Create a new `KeyChord` from a key code and modifier mask.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::KeyChord;
    ///
    /// // Plain 'a' — no modifiers
    /// let chord = KeyChord::new(KeyCode::Char('a'), KeyModifiers::NONE);
    /// assert_eq!(chord.code, KeyCode::Char('a'));
    /// assert!(chord.mods.is_empty());
    ///
    /// // Ctrl+C
    /// let ctrl_c = KeyChord::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    /// assert!(ctrl_c.mods.contains(KeyModifiers::CONTROL));
    /// ```
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }
    }

    /// Classify this chord for tab-strip navigation.
    ///
    /// `Ctrl+Tab` arrives only from terminals with the kitty keyboard
    /// protocol; legacy terminals send plain `Tab`/`BackTab`, which is why
    /// `Shift+Tab` is the universal "previous" fallback. `Ctrl+Tab` cycles
    /// forward (browser convention); `Shift` in the chord — with or without
    /// `Ctrl` — means previous.
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::{KeyChord, StripNav};
    /// let tab = KeyChord { code: KeyCode::Tab, mods: KeyModifiers::NONE };
    /// assert_eq!(tab.strip_nav(), StripNav::Next);
    /// let ctab = KeyChord { code: KeyCode::Tab, mods: KeyModifiers::CONTROL };
    /// assert_eq!(ctab.strip_nav(), StripNav::Next);
    /// let cstab = KeyChord { code: KeyCode::Tab, mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT };
    /// assert_eq!(cstab.strip_nav(), StripNav::Prev);
    /// let btab = KeyChord { code: KeyCode::BackTab, mods: KeyModifiers::NONE };
    /// assert_eq!(btab.strip_nav(), StripNav::Prev);
    /// ```
    pub fn strip_nav(&self) -> StripNav {
        match self.code {
            crossterm::event::KeyCode::Tab => {
                if self.mods.contains(crossterm::event::KeyModifiers::SHIFT) {
                    StripNav::Prev
                } else {
                    StripNav::Next
                }
            }
            crossterm::event::KeyCode::BackTab => StripNav::Prev,
            _ => StripNav::None,
        }
    }

    /// True when this chord means "open in background tab": `Ctrl+Enter`
    /// (kitty-protocol terminals) or `Shift+O` (universal fallback).
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::KeyChord;
    /// let ce = KeyChord { code: KeyCode::Enter, mods: KeyModifiers::CONTROL };
    /// assert!(ce.is_background_open());
    /// let o = KeyChord { code: KeyCode::Char('O'), mods: KeyModifiers::SHIFT };
    /// assert!(o.is_background_open());
    /// let plain = KeyChord { code: KeyCode::Enter, mods: KeyModifiers::NONE };
    /// assert!(!plain.is_background_open());
    /// ```
    pub fn is_background_open(&self) -> bool {
        match self.code {
            crossterm::event::KeyCode::Enter => {
                self.mods.contains(crossterm::event::KeyModifiers::CONTROL)
            }
            crossterm::event::KeyCode::Char('O') => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::{KeyChord, StripNav};

    #[test]
    fn ctrl_shift_tab_still_prev() {
        let c = KeyChord {
            code: KeyCode::Tab,
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        };
        assert_eq!(c.strip_nav(), StripNav::Prev);
        // ...while plain Ctrl+Tab cycles forward, per browser convention.
        let fwd = KeyChord {
            code: KeyCode::Tab,
            mods: KeyModifiers::CONTROL,
        };
        assert_eq!(fwd.strip_nav(), StripNav::Next);
    }

    #[test]
    fn unrelated_keys_are_none_and_not_background_open() {
        let c = KeyChord {
            code: KeyCode::Char('x'),
            mods: KeyModifiers::empty(),
        };
        assert_eq!(c.strip_nav(), StripNav::None);
        assert!(!c.is_background_open());
    }
}
