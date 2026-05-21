use ratatui::{
    style::Style,
    text::Span,
    widgets::{Block, Borders},
};

use crate::theme::Theme;

/// Build a themed [`Block`] with borders and a decorated title.
///
/// The title is prefixed with the theme's `small_star` glyph and coloured with
/// the theme's foreground. The border is coloured with the theme's border colour.
/// The block background and default foreground are set from the theme.
///
/// # Examples
///
/// ```
/// use sid_ui::{helpers::styled_block, themes::cosmos};
///
/// let theme = cosmos();
/// let block = styled_block(&theme, "SSH");
/// // The block holds title/border config; rendering happens via Widget::render.
/// let _ = block;
/// ```
pub fn styled_block<'a>(theme: &Theme, title: &'a str) -> Block<'a> {
    let title_text = format!(" {} {} ", theme.glyphs.small_star, title);
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border.into()))
        .title(title_text)
        .title_style(Style::default().fg(theme.foreground.into()).bold())
        .style(Style::default().bg(theme.background.into()).fg(theme.foreground.into()))
}

/// Return a [`Span`] styled with the theme's primary accent colour.
///
/// Use for interactive highlights, active indicators, and call-to-action text.
///
/// # Examples
///
/// ```
/// use sid_ui::{helpers::accent_text, themes::cosmos};
/// use ratatui::style::{Color, Style};
///
/// let theme = cosmos();
/// let span = accent_text(&theme, "connect");
/// let expected: Color = theme.accent_primary.into();
/// assert_eq!(span.style, Style::default().fg(expected));
/// assert_eq!(span.content.as_ref(), "connect");
/// ```
pub fn accent_text<'a>(theme: &Theme, text: &'a str) -> Span<'a> {
    Span::styled(text, Style::default().fg(theme.accent_primary.into()))
}

/// Return a [`Span`] styled with the theme's muted colour.
///
/// Use for secondary metadata, timestamps, placeholder text, and
/// de-emphasised labels that should recede from primary content.
///
/// # Examples
///
/// ```
/// use sid_ui::{helpers::muted_text, themes::cosmos};
/// use ratatui::style::{Color, Style};
///
/// let theme = cosmos();
/// let span = muted_text(&theme, "last seen 3m ago");
/// let expected: Color = theme.muted.into();
/// assert_eq!(span.style, Style::default().fg(expected));
/// assert_eq!(span.content.as_ref(), "last seen 3m ago");
/// ```
pub fn muted_text<'a>(theme: &Theme, text: &'a str) -> Span<'a> {
    Span::styled(text, Style::default().fg(theme.muted.into()))
}
