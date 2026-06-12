//! Renderer for [`FormPane`] — framed input boxes, Info rows, validation
//! errors, and the primary Save button.

use super::pane::{FormPane, PaneFocusState};
use super::spec::SectionKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget as _};
use sid_ui::Theme;

// Width of the label gutter (left column, right-aligned, truncated with `…`).
const LABEL_GUTTER: u16 = 12;

// Height of a framed Editable field box (border top + value row + border bot).
const FIELD_BOX_HEIGHT: u16 = 3;

// Height consumed by one Info row.
const INFO_ROW_HEIGHT: u16 = 1;

// Height consumed by the section heading row.
const SECTION_HEADING_HEIGHT: u16 = 1;

// Extra blank row after a section heading.
const SECTION_HEADING_PAD: u16 = 1;

// Rows reserved at the bottom for the Save button (blank row + button row).
const SAVE_BUTTON_RESERVE: u16 = 2;

/// Render the form pane into `area` (the right split of the tab body).
///
/// Layout, top to bottom: title bar (pane border carries the form title),
/// then per section: section heading, then per field —
/// * Editable: label gutter (left, 12 cols) + one framed single-row box with
///   the value (Password renders bullets; Choice renders `(●) opt  ( ) opt`
///   spans; Toggle renders `[x]`/`[ ]`), focused field's frame uses the
///   accent color, error line in red directly underneath.
/// * Info: `label  value` muted row, no frame.
/// * Bottom: `[ <primary_label> ⏎ ]` button, accent-inverted when focused.
///
/// The bottom two rows of the inner area are reserved for the Save button
/// before any section content is laid out, so content never overwrites the
/// button regardless of how many fields are rendered.
///
/// Early-returns without drawing anything when `area.width < 20`.
///
/// # Examples
///
/// ```
/// use ratatui::buffer::Buffer;
/// use ratatui::layout::Rect;
/// use sid_ui::themes::cosmos;
/// use sid_widgets::form::{
///     FormField, FormPane, FormSection, FormSpec, SectionKind,
/// };
/// use sid_widgets::modal::Field;
///
/// let spec = FormSpec::new(
///     "ex",
///     "Example",
///     vec![FormSection {
///         title: "Details".into(),
///         kind: SectionKind::Editable,
///         fields: vec![FormField::new(
///             "name",
///             Field::Text {
///                 label: "name".into(),
///                 value: "sid".into(),
///                 placeholder: None,
///             },
///         )],
///     }],
/// );
/// let pane = FormPane::new(spec);
/// let theme = cosmos();
/// let area = Rect::new(0, 0, 60, 24);
/// let mut buf = Buffer::empty(area);
/// sid_widgets::form::render_form_pane(&mut buf, area, &pane, &theme);
/// // The buffer now contains the rendered form — no panic, title present.
/// let content: String = (0..buf.area.width)
///     .map(|x| buf.cell((x, 0)).map(|c| c.symbol()).unwrap_or(" ").to_string())
///     .collect();
/// assert!(content.contains("E"), "border should be present");
/// ```
pub fn render_form_pane(buf: &mut Buffer, area: Rect, pane: &FormPane, theme: &Theme) {
    if area.width < 20 {
        return;
    }

    // Outer border carrying the form title.
    let border_style = Style::default()
        .fg(theme.accent_primary.into())
        .add_modifier(Modifier::BOLD);
    let title = format!(" {} {} ", theme.glyphs.small_star, pane.spec.title);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title)
        .title_style(
            Style::default()
                .fg(theme.foreground.into())
                .add_modifier(Modifier::BOLD),
        )
        .style(
            Style::default()
                .bg(theme.surface.into())
                .fg(theme.foreground.into()),
        );

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Reserve the bottom two rows for the Save button before laying out any
    // section content. This prevents content from ever stamping over the button.
    if inner.height < SAVE_BUTTON_RESERVE + 1 {
        // Not enough room to show content AND the button — just draw the button.
        let btn_y = inner.bottom().saturating_sub(1);
        let primary_focused = pane.focus == PaneFocusState::Primary;
        render_save_button(
            buf,
            inner.x,
            btn_y,
            inner.width,
            pane,
            primary_focused,
            theme,
        );
        return;
    }
    let content_bottom = inner.bottom().saturating_sub(SAVE_BUTTON_RESERVE);
    let btn_y = inner.bottom().saturating_sub(1);

    let mut y = inner.y;

    // Track flat editable-field index across all sections to map PaneFocusState::Field(i).
    let mut editable_idx = 0usize;

    'sections: for section in pane.spec.sections.iter() {
        if y >= content_bottom {
            break;
        }

        // Section heading.
        let heading = Paragraph::new(Line::from(vec![Span::styled(
            section.title.clone(),
            Style::default()
                .fg(theme.muted.into())
                .add_modifier(Modifier::BOLD),
        )]));
        heading.render(
            Rect::new(inner.x, y, inner.width, SECTION_HEADING_HEIGHT),
            buf,
        );
        y = y.saturating_add(SECTION_HEADING_HEIGHT + SECTION_HEADING_PAD);

        for form_field in section.fields.iter() {
            if y >= content_bottom {
                break 'sections;
            }

            match section.kind {
                SectionKind::Editable => {
                    let focused =
                        matches!(pane.focus, PaneFocusState::Field(i) if i == editable_idx);
                    let drew = render_editable_field(
                        buf,
                        inner.x,
                        &mut y,
                        content_bottom,
                        inner.width,
                        form_field,
                        focused,
                        theme,
                    );
                    // Error row directly under the field box — only when the
                    // field was actually drawn (drew == true).
                    if drew {
                        if let Some(err) = &form_field.error {
                            if y < content_bottom {
                                let err_span = Paragraph::new(Line::from(vec![
                                    Span::raw("  "),
                                    Span::styled(
                                        err.clone(),
                                        Style::default().fg(theme.accent_error.into()),
                                    ),
                                ]));
                                err_span.render(Rect::new(inner.x, y, inner.width, 1), buf);
                                y = y.saturating_add(1);
                            }
                        }
                    }
                    editable_idx += 1;
                }
                SectionKind::Info => {
                    render_info_row(buf, inner.x, &mut y, inner.width, form_field, theme);
                }
            }
        }

        // Blank separator between sections.
        y = y.saturating_add(1);
    }

    // Save button on the reserved row.
    let primary_focused = pane.focus == PaneFocusState::Primary;
    render_save_button(
        buf,
        inner.x,
        btn_y,
        inner.width,
        pane,
        primary_focused,
        theme,
    );
}

/// Render a single Editable field: label gutter on the left, framed value box
/// on the right. Advances `*y` by [`FIELD_BOX_HEIGHT`] when there is vertical
/// room to draw; returns `true` if the field was drawn, `false` if it was
/// skipped due to insufficient space.
#[allow(clippy::too_many_arguments)]
fn render_editable_field(
    buf: &mut Buffer,
    x: u16,
    y: &mut u16,
    bottom: u16,
    width: u16,
    form_field: &super::spec::FormField,
    focused: bool,
    theme: &Theme,
) -> bool {
    if (*y).saturating_add(FIELD_BOX_HEIGHT) > bottom {
        // Not enough vertical room — skip without advancing y.
        return false;
    }

    let gutter_w = LABEL_GUTTER.min(width / 2);
    let box_x = x + gutter_w + 1; // 1 gap between gutter and box
    let box_w = width.saturating_sub(gutter_w + 1);

    if box_w < 3 {
        // Not enough horizontal room to draw a framed box — advance y and bail.
        *y = y.saturating_add(FIELD_BOX_HEIGHT);
        return true;
    }

    // Label gutter: right-aligned, truncated with `…`.
    let label = field_label(form_field);
    let label_truncated = truncate_right(label, gutter_w as usize);
    let gutter_para = Paragraph::new(Line::from(vec![Span::styled(
        format!("{:>width$}", label_truncated, width = gutter_w as usize),
        Style::default().fg(theme.muted.into()),
    )]));
    // Vertically centre the label on the middle row of the 3-row box.
    gutter_para.render(Rect::new(x, *y + 1, gutter_w, 1), buf);

    // Framed box.
    let frame_style = if focused {
        Style::default().fg(theme.accent_primary.into())
    } else {
        Style::default().fg(theme.border.into())
    };
    let frame = Block::default()
        .borders(Borders::ALL)
        .border_style(frame_style);
    let box_area = Rect::new(box_x, *y, box_w, FIELD_BOX_HEIGHT);
    let value_area = frame.inner(box_area);
    frame.render(box_area, buf);

    // Value line inside the box.
    if value_area.width > 0 {
        let value_line = render_field_value_line(theme, form_field, focused);
        Paragraph::new(value_line).render(value_area, buf);
    }

    *y = y.saturating_add(FIELD_BOX_HEIGHT);
    true
}

/// Render a single Info row: `label  value`, both muted. Advances `y` by 1.
fn render_info_row(
    buf: &mut Buffer,
    x: u16,
    y: &mut u16,
    width: u16,
    form_field: &super::spec::FormField,
    theme: &Theme,
) {
    let label = field_label(form_field);
    let value = form_field.value_string();
    let label_col = truncate_right(label, LABEL_GUTTER as usize);
    let text = format!(
        "{:<width$}  {}",
        label_col,
        value,
        width = LABEL_GUTTER as usize
    );
    let para = Paragraph::new(Line::from(vec![Span::styled(
        text,
        Style::default().fg(theme.muted.into()),
    )]));
    para.render(Rect::new(x, *y, width, INFO_ROW_HEIGHT), buf);
    *y = y.saturating_add(INFO_ROW_HEIGHT);
}

/// Render the primary Save button row.
fn render_save_button(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    pane: &FormPane,
    focused: bool,
    theme: &Theme,
) {
    let label = format!("[ {} ⏎ ]", pane.spec.primary_label);
    let style = if focused {
        Style::default()
            .bg(theme.accent_primary.into())
            .fg(theme.surface.into())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.accent_primary.into())
            .add_modifier(Modifier::BOLD)
    };
    let para = Paragraph::new(Line::from(vec![Span::styled(label, style)]));
    para.render(Rect::new(x, y, width, 1), buf);
}

/// Extract the label string from a [`FormField`].
fn field_label(form_field: &super::spec::FormField) -> &str {
    use crate::modal::Field;
    match &form_field.field {
        Field::Text { label, .. }
        | Field::Password { label, .. }
        | Field::Toggle { label, .. }
        | Field::Choice { label, .. }
        | Field::Picker { label, .. }
        | Field::Display { label, .. } => label,
    }
}

/// Truncate a string to `max_chars` characters, replacing the tail with `…`
/// when truncated.
fn truncate_right(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else if max_chars == 1 {
        "…".to_string()
    } else {
        let keep: String = chars[..max_chars - 1].iter().collect();
        format!("{keep}…")
    }
}

/// Build the value [`Line`] for an Editable field (same glyphs as modal.rs).
fn render_field_value_line<'a>(
    theme: &'a Theme,
    form_field: &'a super::spec::FormField,
    focused: bool,
) -> Line<'a> {
    use crate::modal::Field;

    const PASSWORD_BULLET: char = '\u{2022}';

    let prefix = if focused { "> " } else { "  " };
    let mut spans = vec![Span::raw(prefix)];

    match &form_field.field {
        Field::Text {
            value, placeholder, ..
        } => {
            if value.is_empty() {
                if let Some(ph) = placeholder {
                    spans.push(Span::styled(
                        ph.clone(),
                        Style::default().fg(theme.muted.into()),
                    ));
                }
            } else {
                spans.push(Span::raw(value.clone()));
            }
        }
        Field::Password { value, .. } => {
            let bullets: String =
                std::iter::repeat_n(PASSWORD_BULLET, value.chars().count()).collect();
            spans.push(Span::raw(bullets));
        }
        Field::Toggle { value, .. } => {
            let mark = if *value { "[x]" } else { "[ ]" };
            spans.push(Span::styled(
                mark.to_string(),
                Style::default().fg(theme.accent_primary.into()),
            ));
        }
        Field::Choice {
            options, selected, ..
        } => {
            for (i, opt) in options.iter().enumerate() {
                let glyph = if i == *selected { "(●)" } else { "( )" };
                spans.push(Span::raw(glyph.to_string()));
                spans.push(Span::raw(" "));
                spans.push(Span::raw(opt.clone()));
                if i + 1 < options.len() {
                    spans.push(Span::raw("  "));
                }
            }
        }
        Field::Picker { value, hint, .. } => {
            if value.is_empty() {
                spans.push(Span::styled(
                    "(no path)".to_string(),
                    Style::default().fg(theme.muted.into()),
                ));
            } else {
                spans.push(Span::raw(value.clone()));
            }
            if !hint.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("[{hint}]"),
                    Style::default().fg(theme.muted.into()),
                ));
            }
        }
        Field::Display { body, .. } => {
            let first = body.lines().next().unwrap_or("");
            spans.push(Span::styled(
                first.to_string(),
                Style::default().fg(theme.muted.into()),
            ));
        }
    }

    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::pane::FormPane;
    use super::super::spec::{FormField, FormSection, FormSpec, SectionKind, Validate};
    use super::*;
    use crate::modal::Field;

    /// Render `pane` into a fresh `width × height` buffer using the cosmos
    /// theme, returning the concatenated cell symbols as a single
    /// `\n`-terminated string per row.
    fn render_to_string(pane: &FormPane, width: u16, height: u16) -> String {
        use sid_ui::themes::cosmos;
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = cosmos();
        render_form_pane(&mut buf, area, pane, &theme);
        let mut s = String::new();
        for y in 0..height {
            for x in 0..width {
                s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            s.push('\n');
        }
        s
    }

    /// Build the standard two-field form used across tests, plus an Info section.
    fn connection_form() -> FormPane {
        FormPane::new(FormSpec::new(
            "conn.edit",
            "Connection",
            vec![
                FormSection {
                    title: "Connection".into(),
                    kind: SectionKind::Editable,
                    fields: vec![
                        FormField::new(
                            "name",
                            Field::Text {
                                label: "name".into(),
                                value: String::new(),
                                placeholder: None,
                            },
                        )
                        .with_validate(vec![Validate::NonEmpty]),
                        FormField::new(
                            "port",
                            Field::Text {
                                label: "port".into(),
                                value: "5432".into(),
                                placeholder: None,
                            },
                        )
                        .with_validate(vec![Validate::Port]),
                    ],
                },
                FormSection {
                    title: "Derived".into(),
                    kind: SectionKind::Info,
                    fields: vec![FormField::new(
                        "dsn",
                        Field::Text {
                            label: "dsn".into(),
                            value: "postgres://10.0.0.5:5432/app".into(),
                            placeholder: None,
                        },
                    )],
                },
            ],
        ))
    }

    #[test]
    fn snapshot_default_form() {
        let pane = connection_form();
        let s = render_to_string(&pane, 60, 24);
        insta::assert_snapshot!("form_render_default", s);
    }

    #[test]
    fn snapshot_validation_error_under_field() {
        let mut pane = connection_form();
        // Trigger validation on the empty name field.
        // Directly set the error to simulate post-revalidate state.
        pane.spec.sections[0].fields[0].error = Some("required".to_string());
        let s = render_to_string(&pane, 60, 24);
        insta::assert_snapshot!("form_render_validation_error", s);
    }

    #[test]
    fn snapshot_info_section_is_unframed_and_muted() {
        // Form with only an Info section (no editable fields).
        let pane = FormPane::new(FormSpec::new(
            "info.only",
            "Info Only",
            vec![FormSection {
                title: "Derived".into(),
                kind: SectionKind::Info,
                fields: vec![
                    FormField::new(
                        "dsn",
                        Field::Text {
                            label: "dsn".into(),
                            value: "postgres://10.0.0.5:5432/app".into(),
                            placeholder: None,
                        },
                    ),
                    FormField::new(
                        "status",
                        Field::Text {
                            label: "status".into(),
                            value: "connected".into(),
                            placeholder: None,
                        },
                    ),
                ],
            }],
        ));
        let s = render_to_string(&pane, 60, 24);
        insta::assert_snapshot!("form_render_info_only", s);
    }

    #[test]
    fn snapshot_focused_save_button() {
        use ratatui::style::Color;
        use sid_ui::themes::cosmos;

        let mut pane = connection_form();
        pane.focus = PaneFocusState::Primary;
        let width: u16 = 60;
        let height: u16 = 24;
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = cosmos();
        render_form_pane(&mut buf, area, &pane, &theme);

        // The button is on the last inner row (height - 2 = row 22, 0-indexed).
        // inner area is area inset by 1 on each side → inner y starts at 1,
        // inner height = 22 rows, btn_y = inner.bottom() - 1 = 1+22-1 = 22.
        let btn_y: u16 = height - 2;
        // First cell of the button label "[ Save ⏎ ]" — '[' should have
        // bg == accent_primary when focused.
        let cell = buf.cell((1, btn_y)).expect("button cell exists");
        let expected_bg: Color = theme.accent_primary.into();
        assert_eq!(
            cell.style().bg,
            Some(expected_bg),
            "focused Save button cell must have bg == accent_primary"
        );

        let s: String = (0..height)
            .map(|y| {
                let row: String = (0..width)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol())
                            .unwrap_or(" ")
                            .to_string()
                    })
                    .collect();
                format!("{row}\n")
            })
            .collect();
        insta::assert_snapshot!("form_render_save_focused", s);
    }

    /// Height-constrained render: 60×11, two fields + error on field 0.
    /// Pins that no orphaned error rows appear, the button is on its reserved
    /// row, and the port box border is not overwritten by the button.
    #[test]
    fn snapshot_height_constrained_no_orphan_errors() {
        let mut pane = connection_form();
        pane.spec.sections[0].fields[0].error = Some("required".to_string());
        let s = render_to_string(&pane, 60, 11);
        insta::assert_snapshot!("form_render_height_constrained", s);
    }

    #[test]
    fn narrow_area_renders_without_panic() {
        // 10x5 is below the width=20 guard; buffer should be unchanged.
        use sid_ui::themes::cosmos;
        let pane = connection_form();
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        let theme = cosmos();
        // Must not panic; buffer stays blank.
        render_form_pane(&mut buf, area, &pane, &theme);
        // Verify all cells are blank (default symbol is a space).
        for y in 0..5u16 {
            for x in 0..10u16 {
                let cell = buf.cell((x, y)).expect("cell exists");
                assert_eq!(
                    cell.symbol(),
                    " ",
                    "cell ({x},{y}) should be blank after narrow-area early return"
                );
            }
        }
    }
}
