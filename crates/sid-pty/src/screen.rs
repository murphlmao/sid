//! `Vt100Screen` — wraps `vt100::Parser` and exposes a snapshot suitable for
//! ratatui rendering.

use vt100::Parser;

/// VT100 screen state.
///
/// # Examples
///
/// ```
/// use sid_pty::Vt100Screen;
/// let s = Vt100Screen::new(24, 80);
/// assert_eq!(s.size(), (24, 80));
/// ```
pub struct Vt100Screen {
    parser: Parser,
    rows: u16,
    cols: u16,
}

impl Vt100Screen {
    /// Construct a blank screen of the given size.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::Vt100Screen;
    /// let _s = Vt100Screen::new(24, 80);
    /// ```
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, 0),
            rows,
            cols,
        }
    }

    /// Feed bytes from the PTY (or remote shell) into the parser.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::Vt100Screen;
    /// let mut s = Vt100Screen::new(3, 10);
    /// s.feed(b"hi");
    /// ```
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Resize the underlying screen.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::Vt100Screen;
    /// let mut s = Vt100Screen::new(3, 10);
    /// s.resize(5, 20);
    /// assert_eq!(s.size(), (5, 20));
    /// ```
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        self.rows = rows;
        self.cols = cols;
    }

    /// Current size as `(rows, cols)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::Vt100Screen;
    /// let s = Vt100Screen::new(24, 80);
    /// assert_eq!(s.size(), (24, 80));
    /// ```
    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Cursor position as `(row, col)`, both zero-indexed.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::Vt100Screen;
    /// let mut s = Vt100Screen::new(3, 10);
    /// s.feed(b"abc");
    /// assert_eq!(s.cursor_position(), (0, 3));
    /// ```
    pub fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// Render the current screen as plain (un-styled) lines.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::Vt100Screen;
    /// let mut s = Vt100Screen::new(3, 10);
    /// s.feed(b"hello");
    /// assert!(s.lines()[0].contains("hello"));
    /// ```
    pub fn lines(&self) -> Vec<String> {
        let screen = self.parser.screen();
        let (rows, cols) = (self.rows, self.cols);
        let mut out = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let mut s = String::with_capacity(cols as usize);
            for c in 0..cols {
                let cell = screen.cell(r, c);
                let glyph = cell.map(|c| c.contents()).unwrap_or_default();
                if glyph.is_empty() {
                    s.push(' ');
                } else {
                    s.push_str(glyph);
                }
            }
            out.push(s);
        }
        out
    }
}
