//! Terminal-screen trait + styled-cell domain types. Implementations live in
//! `sid-term` (e.g. `Vt100Screen`).
//!
//! Widened from the POC's plain-string `TerminalScreen`: a screen now yields a
//! grid of styled [`TermCell`]s so a GPUI terminal grid can paint colors 1:1.

/// A foreground or background color for a terminal cell.
///
/// # Examples
///
/// ```
/// use sid_core::term::TermColor;
/// assert_eq!(TermColor::default(), TermColor::Default);
/// let _ = TermColor::Indexed(196);
/// let _ = TermColor::Rgb(10, 20, 30);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TermColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// One styled cell in a terminal grid.
///
/// # Examples
///
/// ```
/// use sid_core::term::{TermCell, TermColor};
/// let c = TermCell { text: "a".into(), bold: true, ..Default::default() };
/// assert!(c.bold);
/// assert_eq!(c.fg, TermColor::Default);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TermCell {
    pub text: String, // grapheme(s); empty ⇒ blank cell
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// A render-friendly snapshot of a terminal screen.
pub trait TerminalScreen: Send + Sync {
    fn feed(&mut self, bytes: &[u8]);
    fn resize(&mut self, rows: u16, cols: u16);
    fn size(&self) -> (u16, u16);
    fn cursor_position(&self) -> (u16, u16);
    /// Row-major styled snapshot; blank cells are `TermCell::default()`.
    // ponytail: full-grid clone per frame; damage-tracking iterator if profiling demands
    fn cells(&self) -> Vec<Vec<TermCell>>;
    /// Plain-text convenience (tests, logging).
    fn lines(&self) -> Vec<String> {
        self.cells()
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| if c.text.is_empty() { " " } else { &c.text })
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal in-memory screen used to exercise the `lines()` default impl
    /// without pulling in the vt100 impl crate.
    struct GridScreen {
        rows: u16,
        cols: u16,
        grid: Vec<Vec<TermCell>>,
    }

    impl TerminalScreen for GridScreen {
        fn feed(&mut self, _bytes: &[u8]) {}
        fn resize(&mut self, rows: u16, cols: u16) {
            self.rows = rows;
            self.cols = cols;
        }
        fn size(&self) -> (u16, u16) {
            (self.rows, self.cols)
        }
        fn cursor_position(&self) -> (u16, u16) {
            (0, 0)
        }
        fn cells(&self) -> Vec<Vec<TermCell>> {
            self.grid.clone()
        }
    }

    // Object-safety: the terminal view (Plan 3C) holds a `Box<dyn TerminalScreen>`.
    #[allow(dead_code)]
    fn assert_object_safe(_s: &dyn TerminalScreen) {}

    #[test]
    fn boxed_terminal_screen_constructs() {
        let screen = GridScreen {
            rows: 1,
            cols: 3,
            grid: vec![vec![
                TermCell {
                    text: "h".into(),
                    ..Default::default()
                },
                TermCell {
                    text: "i".into(),
                    ..Default::default()
                },
                TermCell::default(),
            ]],
        };
        let boxed: Box<dyn TerminalScreen> = Box::new(screen);
        assert_eq!(boxed.size(), (1, 3));
    }

    #[test]
    fn lines_default_impl_renders_blank_cells_as_space() {
        let screen = GridScreen {
            rows: 2,
            cols: 3,
            grid: vec![
                vec![
                    TermCell {
                        text: "a".into(),
                        ..Default::default()
                    },
                    TermCell::default(), // blank ⇒ space
                    TermCell {
                        text: "c".into(),
                        ..Default::default()
                    },
                ],
                vec![
                    TermCell::default(),
                    TermCell::default(),
                    TermCell::default(),
                ],
            ],
        };
        let lines = screen.lines();
        assert_eq!(lines, vec!["a c".to_string(), "   ".to_string()]);
    }
}
