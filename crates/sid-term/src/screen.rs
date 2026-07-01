//! `Vt100Screen` — wraps `vt100::Parser` and exposes a styled-cell snapshot
//! suitable for a GPUI terminal grid.

use sid_core::term::{TermCell, TermColor, TerminalScreen};
use vt100::Parser;

/// VT100 screen state.
///
/// # Examples
///
/// ```
/// use sid_term::Vt100Screen;
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
    /// use sid_term::Vt100Screen;
    /// let _s = Vt100Screen::new(24, 80);
    /// ```
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, 0),
            rows,
            cols,
        }
    }

    /// Feed bytes from the remote shell (or local PTY) into the parser.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_term::Vt100Screen;
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
    /// use sid_term::Vt100Screen;
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
    /// use sid_term::Vt100Screen;
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
    /// use sid_term::Vt100Screen;
    /// let mut s = Vt100Screen::new(3, 10);
    /// s.feed(b"abc");
    /// assert_eq!(s.cursor_position(), (0, 3));
    /// ```
    pub fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// Row-major styled snapshot; blank cells are `TermCell::default()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_term::Vt100Screen;
    /// let mut s = Vt100Screen::new(1, 5);
    /// s.feed(b"hi");
    /// let cells = s.cells();
    /// assert_eq!(cells.len(), 1);
    /// assert_eq!(cells[0].len(), 5);
    /// assert_eq!(cells[0][0].text, "h");
    /// ```
    pub fn cells(&self) -> Vec<Vec<TermCell>> {
        let screen = self.parser.screen();
        let (rows, cols) = (self.rows, self.cols);
        let mut out = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let mut row = Vec::with_capacity(cols as usize);
            for c in 0..cols {
                row.push(match screen.cell(r, c) {
                    Some(cell) => TermCell {
                        text: cell.contents().to_string(),
                        fg: map_color(cell.fgcolor()),
                        bg: map_color(cell.bgcolor()),
                        bold: cell.bold(),
                        italic: cell.italic(),
                        underline: cell.underline(),
                        inverse: cell.inverse(),
                    },
                    None => TermCell::default(),
                });
            }
            out.push(row);
        }
        out
    }
}

/// Map a `vt100::Color` to the domain `TermColor`.
fn map_color(c: vt100::Color) -> TermColor {
    match c {
        vt100::Color::Default => TermColor::Default,
        vt100::Color::Idx(i) => TermColor::Indexed(i),
        vt100::Color::Rgb(r, g, b) => TermColor::Rgb(r, g, b),
    }
}

impl TerminalScreen for Vt100Screen {
    fn feed(&mut self, bytes: &[u8]) {
        Vt100Screen::feed(self, bytes);
    }
    fn resize(&mut self, rows: u16, cols: u16) {
        Vt100Screen::resize(self, rows, cols);
    }
    fn size(&self) -> (u16, u16) {
        Vt100Screen::size(self)
    }
    fn cursor_position(&self) -> (u16, u16) {
        Vt100Screen::cursor_position(self)
    }
    fn cells(&self) -> Vec<Vec<TermCell>> {
        Vt100Screen::cells(self)
    }
    // `lines()` uses the trait default over `cells()`.
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_core::term::{TermCell, TermColor, TerminalScreen};

    /// First cell of a freshly-fed screen (the SGR under test always starts at 0,0).
    fn first_cell(bytes: &[u8]) -> TermCell {
        let mut s = Vt100Screen::new(1, 20);
        s.feed(bytes);
        s.cells()
            .into_iter()
            .next()
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn indexed_fg() {
        // `\x1b[31m` = SGR 31 (red) → indexed color 1.
        let cell = first_cell(b"\x1b[31mX");
        assert_eq!(cell.text, "X");
        assert_eq!(cell.fg, TermColor::Indexed(1));
        assert_eq!(cell.bg, TermColor::Default);
    }

    #[test]
    fn bold_and_underline() {
        // `\x1b[1;4m` = bold + underline.
        let cell = first_cell(b"\x1b[1;4mX");
        assert!(cell.bold);
        assert!(cell.underline);
        assert!(!cell.italic);
        assert!(!cell.inverse);
    }

    #[test]
    fn indexed_256_fg() {
        // `\x1b[38;5;196m` = 256-color palette index 196.
        let cell = first_cell(b"\x1b[38;5;196mX");
        assert_eq!(cell.fg, TermColor::Indexed(196));
    }

    #[test]
    fn truecolor_fg() {
        // `\x1b[38;2;10;20;30m` = 24-bit RGB.
        let cell = first_cell(b"\x1b[38;2;10;20;30mX");
        assert_eq!(cell.fg, TermColor::Rgb(10, 20, 30));
    }

    #[test]
    fn inverse() {
        // `\x1b[7m` = inverse video.
        let cell = first_cell(b"\x1b[7mX");
        assert!(cell.inverse);
    }

    #[test]
    fn reset_clears_attributes() {
        // Set a pile of attributes, then `\x1b[0m` (reset) before writing.
        let cell = first_cell(b"\x1b[1;4;7;31m\x1b[0mX");
        assert_eq!(cell.text, "X");
        assert_eq!(cell.fg, TermColor::Default);
        assert_eq!(cell.bg, TermColor::Default);
        assert!(!cell.bold);
        assert!(!cell.underline);
        assert!(!cell.inverse);
        assert!(!cell.italic);
    }

    #[test]
    fn resize_preserves_size_and_cursor_invariants() {
        let mut s = Vt100Screen::new(3, 10);
        s.feed(b"abc");
        assert_eq!(s.size(), (3, 10));
        assert_eq!(s.cursor_position(), (0, 3));
        s.resize(5, 20);
        assert_eq!(s.size(), (5, 20));
        // Content survives the resize; cursor stays put on the same row/col.
        assert_eq!(s.cursor_position(), (0, 3));
        assert_eq!(s.cells()[0].len(), 20);
    }

    #[test]
    fn plain_text_round_trips_through_lines() {
        // POC parity: cursor after `abc` sits at (0, 3), and `lines()` (now the
        // trait default over `cells()`) reproduces the plain text.
        let mut s = Vt100Screen::new(3, 10);
        s.feed(b"abc");
        assert_eq!(s.cursor_position(), (0, 3));
        let lines: Vec<String> = TerminalScreen::lines(&s);
        assert!(lines[0].starts_with("abc"));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].chars().count(), 10);
    }

    #[test]
    fn blank_cells_are_default() {
        let mut s = Vt100Screen::new(1, 5);
        s.feed(b"hi");
        let cells = s.cells();
        // Columns 2..5 were never written ⇒ default cells.
        assert_eq!(cells[0][2], TermCell::default());
        assert_eq!(cells[0][4], TermCell::default());
    }

    #[test]
    fn object_safe_via_trait() {
        let mut boxed: Box<dyn TerminalScreen> = Box::new(Vt100Screen::new(2, 4));
        boxed.feed(b"ok");
        assert_eq!(boxed.size(), (2, 4));
        assert_eq!(boxed.cells()[0][0].text, "o");
    }
}
