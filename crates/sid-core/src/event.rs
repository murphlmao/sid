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
            CtEvent::Key(KeyEvent { code, modifiers, .. }) => Event::Key(KeyChord::new(code, modifiers)),
            CtEvent::Mouse(m) => Event::Mouse(m),
            CtEvent::Resize(w, h) => Event::Resize { width: w, height: h },
            CtEvent::FocusGained => Event::Focus(true),
            CtEvent::FocusLost => Event::Focus(false),
            CtEvent::Paste(_) => Event::Custom("paste".into()),
        }
    }
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
}
