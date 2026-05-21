use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget as RatatuiWidget,
};
use sid_ui::{
    helpers::{accent_text, muted_text, styled_block},
    themes::cosmos,
};

// ── Happy path ────────────────────────────────────────────────────────────────

#[test]
fn styled_block_does_not_panic() {
    let t = cosmos();
    let block = styled_block(&t, "title");
    let _ = block;
}

#[test]
fn styled_block_renders_into_buffer_without_panic() {
    let t = cosmos();
    let block = styled_block(&t, "My Tab");
    let area = Rect::new(0, 0, 20, 5);
    let mut buf = Buffer::empty(area);
    block.render(area, &mut buf);
    // The buffer should contain border chars in the first row
    let first_row: String = (0..20).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    // Top border contains a horizontal line character
    assert!(
        first_row.contains('─') || first_row.contains('┌') || first_row.contains('┐'),
        "expected border chars in first row, got: {first_row:?}"
    );
}

#[test]
fn styled_block_title_contains_glyph_and_text() {
    let t = cosmos();
    let block = styled_block(&t, "Workspaces");
    let area = Rect::new(0, 0, 40, 5);
    let mut buf = Buffer::empty(area);
    block.render(area, &mut buf);
    // The first row should contain the small_star glyph (✦) and the title text
    let first_row: String = (0..40).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    assert!(
        first_row.contains('✦'),
        "expected small_star glyph in title row, got: {first_row:?}"
    );
    assert!(
        first_row.contains("Workspaces"),
        "expected title text in first row, got: {first_row:?}"
    );
}

#[test]
fn accent_text_returns_span_with_primary_color() {
    let t = cosmos();
    let span = accent_text(&t, "alert");
    let expected: Color = t.accent_primary.into();
    assert_eq!(span.style, Style::default().fg(expected));
    assert_eq!(span.content.as_ref(), "alert");
}

#[test]
fn muted_text_returns_span_with_muted_color() {
    let t = cosmos();
    let span = muted_text(&t, "secondary");
    let expected: Color = t.muted.into();
    assert_eq!(span.style, Style::default().fg(expected));
    assert_eq!(span.content.as_ref(), "secondary");
}

#[test]
fn accent_and_muted_use_different_colors() {
    let t = cosmos();
    let accent = accent_text(&t, "x");
    let muted = muted_text(&t, "x");
    // The two spans should have different foreground colors
    assert_ne!(accent.style, muted.style);
}

// ── Adversarial ───────────────────────────────────────────────────────────────

#[test]
fn accent_text_empty_string() {
    let t = cosmos();
    let span = accent_text(&t, "");
    assert_eq!(span.content.as_ref(), "");
    let expected: Color = t.accent_primary.into();
    assert_eq!(span.style, Style::default().fg(expected));
}

#[test]
fn muted_text_empty_string() {
    let t = cosmos();
    let span = muted_text(&t, "");
    assert_eq!(span.content.as_ref(), "");
}

#[test]
fn styled_block_empty_title() {
    let t = cosmos();
    let block = styled_block(&t, "");
    let area = Rect::new(0, 0, 20, 4);
    let mut buf = Buffer::empty(area);
    // Must not panic
    block.render(area, &mut buf);
}

#[test]
fn styled_block_very_long_title() {
    let t = cosmos();
    let long_title = "A".repeat(200);
    let block = styled_block(&t, &long_title);
    let area = Rect::new(0, 0, 10, 4); // narrower than title — must not panic
    let mut buf = Buffer::empty(area);
    block.render(area, &mut buf);
}

#[test]
fn accent_text_multi_codepoint_emoji() {
    let t = cosmos();
    // Family emoji: multiple Unicode scalar values (ZWJ sequence)
    let span = accent_text(&t, "👨‍👩‍👧");
    // Content is preserved as-is
    assert!(span.content.contains('👨'));
    let expected: Color = t.accent_primary.into();
    assert_eq!(span.style, Style::default().fg(expected));
}

#[test]
fn muted_text_multi_codepoint_emoji() {
    let t = cosmos();
    let span = muted_text(&t, "🏳️‍🌈");
    assert!(!span.content.is_empty());
    let expected: Color = t.muted.into();
    assert_eq!(span.style, Style::default().fg(expected));
}

// ── Insta snapshot test ───────────────────────────────────────────────────────

#[test]
fn styled_block_snapshot() {
    let t = cosmos();
    let block = styled_block(&t, "SSH");
    let area = Rect::new(0, 0, 30, 5);
    let mut buf = Buffer::empty(area);
    block.render(area, &mut buf);

    // Render each row as a string, trimming trailing spaces for snapshot stability.
    let rendered: Vec<String> = (0..area.height)
        .map(|y| {
            let row: String = (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            row.trim_end().to_string()
        })
        .collect();

    insta::assert_yaml_snapshot!("styled_block_cosmos_ssh_30x5", rendered);
}
