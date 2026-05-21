//! `Vt100Screen` — wraps `vt100::Parser` and exposes a snapshot suitable for
//! ratatui rendering. Filled in by Task 15.

/// VT100 screen state. Construct with a `(rows, cols)` size; feed bytes;
/// `lines()` returns the current visible buffer as plain strings.
///
/// # Examples
///
/// ```
/// use sid_pty::Vt100Screen;
/// let _s = Vt100Screen::new(24, 80);
/// ```
pub struct Vt100Screen {
    _placeholder: (),
}

impl Vt100Screen {
    /// Construct an empty screen of the given size.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::Vt100Screen;
    /// let _s = Vt100Screen::new(24, 80);
    /// ```
    pub fn new(_rows: u16, _cols: u16) -> Self {
        Self { _placeholder: () }
    }
}
