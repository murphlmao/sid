use crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent};

/// Normalized event passed through the App event loop.
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyChord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }
    }
}
