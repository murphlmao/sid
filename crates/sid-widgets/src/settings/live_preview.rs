//! Render a small "this is what your terminal looks like" preview in a given
//! theme — the right-hand pane of the [`super::theme_picker::ThemePickerView`].
//!
//! The render is intentionally tiny (40x12 by default): a title with the theme
//! name and a small star, a bordered block, three sample rows in different
//! accent colours, and a footer hint. The result is returned as a plain
//! string so unit/snapshot tests can compare it byte-for-byte without parsing
//! a ratatui buffer.
//!
//! # Examples
//!
//! ```
//! use sid_ui::themes::cosmos;
//! use sid_widgets::settings::live_preview::render_preview;
//!
//! let s = render_preview(&cosmos(), 40, 12);
//! // Title prefixed with the theme name.
//! assert!(s.contains("cosmos"));
//! ```

use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color as RatColor, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
};
use sid_ui::theme::{Color, Theme};

/// Render the preview block for `theme` at `width × height` cells. Returns an
/// ASCII string with one row per line (newline-separated). A zero dimension
/// returns an empty string.
///
/// # Examples
///
/// ```
/// use sid_ui::themes::void;
/// use sid_widgets::settings::live_preview::render_preview;
///
/// // Zero-area renders as an empty string (no panic).
/// assert_eq!(render_preview(&void(), 0, 0), "");
/// // Non-zero render contains the theme name in its title.
/// let s = render_preview(&void(), 40, 12);
/// assert!(s.contains("void"));
/// ```
pub fn render_preview(theme: &Theme, width: u16, height: u16) -> String {
    if width == 0 || height == 0 {
        return String::new();
    }
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| draw(f, f.area(), theme)).unwrap();
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell_sym = buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" ");
            // Replace any NUL bytes with a space — defensive against
            // pathological themes that set `glyph.star = '\0'`.
            for ch in cell_sym.chars() {
                if ch == '\0' {
                    s.push(' ');
                } else {
                    s.push(ch);
                }
            }
        }
        s.push('\n');
    }
    s
}

fn ratcolor(c: Color) -> RatColor {
    RatColor::Rgb(c.r, c.g, c.b)
}

fn draw(f: &mut ratatui::Frame<'_>, area: Rect, theme: &Theme) {
    // Background fill via a block that owns the whole area.
    let outer = Block::default().borders(Borders::ALL).style(
        Style::default()
            .bg(ratcolor(theme.background))
            .fg(ratcolor(theme.border)),
    );
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    // Title row.
    let title = format!(
        "{star} {name} {star}",
        star = theme.glyphs.small_star,
        name = theme.name
    );
    let title_par = Paragraph::new(title).style(
        Style::default()
            .bg(ratcolor(theme.background))
            .fg(ratcolor(theme.accent_primary))
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(title_par, chunks[0]);

    // Body — three sample rows.
    if chunks[1].height >= 1 {
        let row0 = Paragraph::new("  primary accent row").style(
            Style::default()
                .bg(ratcolor(theme.background))
                .fg(ratcolor(theme.accent_primary)),
        );
        f.render_widget(
            row0,
            Rect {
                x: chunks[1].x,
                y: chunks[1].y,
                width: chunks[1].width,
                height: 1,
            },
        );
    }
    if chunks[1].height >= 2 {
        let row1 = Paragraph::new("  muted secondary text").style(
            Style::default()
                .bg(ratcolor(theme.background))
                .fg(ratcolor(theme.muted)),
        );
        f.render_widget(
            row1,
            Rect {
                x: chunks[1].x,
                y: chunks[1].y + 1,
                width: chunks[1].width,
                height: 1,
            },
        );
    }
    if chunks[1].height >= 3 {
        let row2 = Paragraph::new("  success ok").style(
            Style::default()
                .bg(ratcolor(theme.background))
                .fg(ratcolor(theme.accent_success)),
        );
        f.render_widget(
            row2,
            Rect {
                x: chunks[1].x,
                y: chunks[1].y + 2,
                width: chunks[1].width,
                height: 1,
            },
        );
    }

    // Footer hint.
    let hint = Paragraph::new("Enter to apply  ·  Esc to cancel").style(
        Style::default()
            .bg(ratcolor(theme.background))
            .fg(ratcolor(theme.muted)),
    );
    f.render_widget(hint, chunks[2]);
}

#[cfg(test)]
mod tests {
    use sid_ui::{
        theme::GlyphSet,
        themes::{cosmos, cosmos_light, dusk, void},
    };

    use super::*;

    #[test]
    fn zero_width_returns_empty_string() {
        assert_eq!(render_preview(&cosmos(), 0, 12), "");
    }

    #[test]
    fn zero_height_returns_empty_string() {
        assert_eq!(render_preview(&cosmos(), 40, 0), "");
    }

    #[test]
    fn one_by_one_does_not_panic() {
        let s = render_preview(&cosmos(), 1, 1);
        assert!(!s.is_empty());
    }

    #[test]
    fn renders_theme_name_in_title() {
        let s = render_preview(&cosmos(), 40, 12);
        assert!(s.contains("cosmos"));
    }

    #[test]
    fn null_glyph_does_not_emit_null_byte() {
        let mut theme = cosmos();
        theme.glyphs = GlyphSet {
            star: '\0',
            small_star: '\0',
            dot: '\0',
        };
        let s = render_preview(&theme, 40, 12);
        assert!(!s.contains('\0'));
    }

    #[test]
    fn dimensions_match_request() {
        let s = render_preview(&cosmos(), 40, 12);
        let rows: Vec<&str> = s.lines().collect();
        assert_eq!(rows.len(), 12, "expected 12 lines, got\n{s}");
        for r in &rows {
            assert_eq!(r.chars().count(), 40);
        }
    }

    #[test]
    fn preview_snapshot_cosmos() {
        let s = render_preview(&cosmos(), 40, 12);
        insta::assert_snapshot!("preview_cosmos", s);
    }

    #[test]
    fn preview_snapshot_void() {
        let s = render_preview(&void(), 40, 12);
        insta::assert_snapshot!("preview_void", s);
    }

    #[test]
    fn preview_snapshot_dusk() {
        let s = render_preview(&dusk(), 40, 12);
        insta::assert_snapshot!("preview_dusk", s);
    }

    #[test]
    fn preview_snapshot_cosmos_light() {
        let s = render_preview(&cosmos_light(), 40, 12);
        insta::assert_snapshot!("preview_cosmos_light", s);
    }
}
