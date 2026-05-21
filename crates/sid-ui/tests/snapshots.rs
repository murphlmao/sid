/// Insta snapshot tests for sid-ui rendering stability and palette serialization.
///
/// These tests ensure:
///   - Theme JSON serialization does not drift across refactors.
///   - Rendered widget output (styled_block, accent_text, muted_text) is
///     byte-stable for each built-in theme.
///
/// All snapshots are stored in `tests/snapshots/` and are accepted on first run.
/// Subsequent runs assert against the accepted snapshots.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::Widget as RatatuiWidget,
};
use sid_ui::{
    helpers::{accent_text, muted_text, styled_block},
    themes::{cosmos, cosmos_light, dusk, void},
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Render a widget into a buffer and convert it to a Vec of trimmed row strings.
fn render_rows(area: Rect, render_fn: impl FnOnce(&mut Buffer)) -> Vec<String> {
    let mut buf = Buffer::empty(area);
    render_fn(&mut buf);
    (0..area.height)
        .map(|y| {
            let row: String =
                (0..area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
            row.trim_end().to_string()
        })
        .collect()
}

/// Render a single-line span into a 1-row buffer and return the trimmed content.
fn render_span_row(width: u16, render_fn: impl FnOnce(&mut Buffer)) -> String {
    let area = Rect::new(0, 0, width, 1);
    let mut buf = Buffer::empty(area);
    render_fn(&mut buf);
    let row: String = (0..width).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    row.trim_end().to_string()
}

// ── JSON palette snapshots ─────────────────────────────────────────────────────

/// cosmos() serializes to stable JSON — palette must not drift.
#[test]
fn snapshot_cosmos_json() {
    let t = cosmos();
    let json = serde_json::to_string_pretty(&t).expect("cosmos serialization must succeed");
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("palette_cosmos_json", json);
    });
}

/// void() serializes to stable JSON — palette must not drift.
#[test]
fn snapshot_void_json() {
    let t = void();
    let json = serde_json::to_string_pretty(&t).expect("void serialization must succeed");
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("palette_void_json", json);
    });
}

/// dusk() serializes to stable JSON — palette must not drift.
#[test]
fn snapshot_dusk_json() {
    let t = dusk();
    let json = serde_json::to_string_pretty(&t).expect("dusk serialization must succeed");
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("palette_dusk_json", json);
    });
}

/// cosmos_light() serializes to stable JSON — palette must not drift.
#[test]
fn snapshot_cosmos_light_json() {
    let t = cosmos_light();
    let json =
        serde_json::to_string_pretty(&t).expect("cosmos_light serialization must succeed");
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("palette_cosmos_light_json", json);
    });
}

// ── styled_block rendered buffer snapshots ────────────────────────────────────

/// cosmos styled_block renders stably into 40×6.
#[test]
fn snapshot_styled_block_cosmos_40x6() {
    let t = cosmos();
    let area = Rect::new(0, 0, 40, 6);
    let rows = render_rows(area, |buf| {
        styled_block(&t, "  example  ").render(area, buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_yaml_snapshot!("styled_block_cosmos_40x6", rows);
    });
}

/// void styled_block renders stably into 40×6.
#[test]
fn snapshot_styled_block_void_40x6() {
    let t = void();
    let area = Rect::new(0, 0, 40, 6);
    let rows = render_rows(area, |buf| {
        styled_block(&t, "  example  ").render(area, buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_yaml_snapshot!("styled_block_void_40x6", rows);
    });
}

/// dusk styled_block renders stably into 40×6.
#[test]
fn snapshot_styled_block_dusk_40x6() {
    let t = dusk();
    let area = Rect::new(0, 0, 40, 6);
    let rows = render_rows(area, |buf| {
        styled_block(&t, "  example  ").render(area, buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_yaml_snapshot!("styled_block_dusk_40x6", rows);
    });
}

/// cosmos_light styled_block renders stably into 40×6.
#[test]
fn snapshot_styled_block_cosmos_light_40x6() {
    let t = cosmos_light();
    let area = Rect::new(0, 0, 40, 6);
    let rows = render_rows(area, |buf| {
        styled_block(&t, "  example  ").render(area, buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_yaml_snapshot!("styled_block_cosmos_light_40x6", rows);
    });
}

// ── accent_text / muted_text 1-line buffer snapshots ─────────────────────────
//
// These validate that the span text content is stable across themes.
// Style attributes (colours) are validated by unit tests in helpers.rs;
// here we focus on rendered cell content stability.

/// accent_text("ALERT") rendered for cosmos — content must not drift.
#[test]
fn snapshot_accent_text_cosmos() {
    let t = cosmos();
    let row = render_span_row(20, |buf| {
        use ratatui::text::Line;
        Line::from(accent_text(&t, "ALERT")).render(Rect::new(0, 0, 20, 1), buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("accent_text_cosmos", row);
    });
}

/// accent_text("ALERT") rendered for void.
#[test]
fn snapshot_accent_text_void() {
    let t = void();
    let row = render_span_row(20, |buf| {
        use ratatui::text::Line;
        Line::from(accent_text(&t, "ALERT")).render(Rect::new(0, 0, 20, 1), buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("accent_text_void", row);
    });
}

/// accent_text("ALERT") rendered for dusk.
#[test]
fn snapshot_accent_text_dusk() {
    let t = dusk();
    let row = render_span_row(20, |buf| {
        use ratatui::text::Line;
        Line::from(accent_text(&t, "ALERT")).render(Rect::new(0, 0, 20, 1), buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("accent_text_dusk", row);
    });
}

/// accent_text("ALERT") rendered for cosmos_light.
#[test]
fn snapshot_accent_text_cosmos_light() {
    let t = cosmos_light();
    let row = render_span_row(20, |buf| {
        use ratatui::text::Line;
        Line::from(accent_text(&t, "ALERT")).render(Rect::new(0, 0, 20, 1), buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("accent_text_cosmos_light", row);
    });
}

/// muted_text("muted") rendered for cosmos.
#[test]
fn snapshot_muted_text_cosmos() {
    let t = cosmos();
    let row = render_span_row(20, |buf| {
        use ratatui::text::Line;
        Line::from(muted_text(&t, "muted")).render(Rect::new(0, 0, 20, 1), buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("muted_text_cosmos", row);
    });
}

/// muted_text("muted") rendered for void.
#[test]
fn snapshot_muted_text_void() {
    let t = void();
    let row = render_span_row(20, |buf| {
        use ratatui::text::Line;
        Line::from(muted_text(&t, "muted")).render(Rect::new(0, 0, 20, 1), buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("muted_text_void", row);
    });
}

/// muted_text("muted") rendered for dusk.
#[test]
fn snapshot_muted_text_dusk() {
    let t = dusk();
    let row = render_span_row(20, |buf| {
        use ratatui::text::Line;
        Line::from(muted_text(&t, "muted")).render(Rect::new(0, 0, 20, 1), buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("muted_text_dusk", row);
    });
}

/// muted_text("muted") rendered for cosmos_light.
#[test]
fn snapshot_muted_text_cosmos_light() {
    let t = cosmos_light();
    let row = render_span_row(20, |buf| {
        use ratatui::text::Line;
        Line::from(muted_text(&t, "muted")).render(Rect::new(0, 0, 20, 1), buf);
    });
    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("muted_text_cosmos_light", row);
    });
}

// ── Property tests: palette distinctness ─────────────────────────────────────

/// All 9 palette colours in every built-in theme are pairwise distinct.
///
/// If two roles share an identical colour token the theme probably has a copy-paste
/// error. (Note: muted ≠ border is aesthetically important; accent_success ≠
/// accent_warning is safety-critical for status readability.)
#[test]
fn all_palette_colours_are_distinct_cosmos() {
    palette_colours_are_distinct(&cosmos());
}

#[test]
fn all_palette_colours_are_distinct_void() {
    palette_colours_are_distinct(&void());
}

#[test]
fn all_palette_colours_are_distinct_dusk() {
    palette_colours_are_distinct(&dusk());
}

#[test]
fn all_palette_colours_are_distinct_cosmos_light() {
    palette_colours_are_distinct(&cosmos_light());
}

fn palette_colours_are_distinct(t: &sid_ui::Theme) {
    use sid_ui::theme::Color;
    let colours: &[(&str, Color)] = &[
        ("background", t.background),
        ("surface", t.surface),
        ("foreground", t.foreground),
        ("muted", t.muted),
        ("accent_primary", t.accent_primary),
        ("accent_success", t.accent_success),
        ("accent_warning", t.accent_warning),
        ("accent_error", t.accent_error),
        ("border", t.border),
    ];
    for i in 0..colours.len() {
        for j in (i + 1)..colours.len() {
            let (name_i, c_i) = colours[i];
            let (name_j, c_j) = colours[j];
            assert_ne!(
                c_i, c_j,
                "theme '{}': '{}' and '{}' have the same colour {:?}",
                t.name, name_i, name_j, c_i
            );
        }
    }
}

// ── Property tests: glyphs are non-empty chars ───────────────────────────────

/// GlyphSet chars must all be non-NUL (a NUL glyph would produce invisible
/// titles or corrupt buffer output).
#[test]
fn glyphs_are_non_nul_cosmos() {
    glyphs_are_non_nul(&cosmos());
}

#[test]
fn glyphs_are_non_nul_void() {
    glyphs_are_non_nul(&void());
}

#[test]
fn glyphs_are_non_nul_dusk() {
    glyphs_are_non_nul(&dusk());
}

#[test]
fn glyphs_are_non_nul_cosmos_light() {
    glyphs_are_non_nul(&cosmos_light());
}

fn glyphs_are_non_nul(t: &sid_ui::Theme) {
    assert_ne!(t.glyphs.star, '\0', "theme '{}': star glyph must not be NUL", t.name);
    assert_ne!(
        t.glyphs.small_star, '\0',
        "theme '{}': small_star glyph must not be NUL",
        t.name
    );
    assert_ne!(t.glyphs.dot, '\0', "theme '{}': dot glyph must not be NUL", t.name);
}

// ── Property tests: Color → ratatui round-trip via proptest ──────────────────

use proptest::prelude::*;

proptest! {
    /// `Color::rgb(r, g, b)` round-trips through `ratatui::style::Color::Rgb`
    /// with exact identity: no component is truncated, swapped, or lost.
    #[test]
    fn prop_color_rgb_roundtrip_is_identity(r in 0u8..=255, g in 0u8..=255, b in 0u8..=255) {
        let c = sid_ui::theme::Color::rgb(r, g, b);
        let rc: ratatui::style::Color = c.into();
        prop_assert!(
            matches!(rc, ratatui::style::Color::Rgb(rr, gg, bb) if rr == r && gg == g && bb == b),
            "round-trip failed: ({r},{g},{b}) → {rc:?}"
        );
    }
}

// ── JSON serde round-trip: Color::rgb(0xFF,0xFF,0xFF) ─────────────────────────

/// `Color::rgb(255, 255, 255)` survives a JSON round-trip without drift.
///
/// This catches any future serde rename or field-order change that would break
/// colour deserialization for white (#ffffff).
#[test]
fn color_white_json_roundtrip_no_drift() {
    use sid_ui::theme::Color;
    let c = Color::rgb(0xFF, 0xFF, 0xFF);
    let json = serde_json::to_string(&c).expect("serialize must succeed");
    let back: Color = serde_json::from_str(&json).expect("deserialize must succeed");
    assert_eq!(back, c, "Color::rgb(255,255,255) JSON round-trip produced a different value");
}

// ── Adversarial: styled_block edge cases ─────────────────────────────────────

/// styled_block with a NUL character in the title must not panic.
///
/// Ratatui treats NUL as a displayable (or at least non-crashing) input;
/// this test guards against a future tightening that panics on control chars.
#[test]
fn styled_block_nul_title_does_not_panic() {
    let t = cosmos();
    let title_with_nul = "hello\0world";
    let block = styled_block(&t, title_with_nul);
    let area = Rect::new(0, 0, 40, 6);
    let mut buf = Buffer::empty(area);
    // Must not panic — any output is acceptable.
    block.render(area, &mut buf);
}

/// styled_block with a title far longer than the area width must not panic.
///
/// Ratatui is expected to truncate or clip the title; what it must not do is
/// index out of bounds or panic.
#[test]
fn styled_block_title_longer_than_area_does_not_panic() {
    let t = cosmos();
    // Title is 300 chars; area is only 10 wide.
    let long_title = "X".repeat(300);
    let block = styled_block(&t, &long_title);
    let area = Rect::new(0, 0, 10, 4);
    let mut buf = Buffer::empty(area);
    block.render(area, &mut buf);
}

/// accent_text with a title made entirely of zero-width combining diacritics must
/// not panic when rendered into a buffer (Unicode width calculation edge case).
#[test]
fn accent_text_zero_width_chars_does_not_panic() {
    let t = cosmos();
    // U+0300 COMBINING GRAVE ACCENT — zero display width
    let combining = "\u{0300}\u{0301}\u{0302}\u{0303}";
    let span = accent_text(&t, combining);
    // Verify the span itself is constructed without panic
    assert!(!span.content.is_empty());
    // Render into a buffer — should not panic even with zero-width chars
    use ratatui::text::Line;
    let area = Rect::new(0, 0, 20, 1);
    let mut buf = Buffer::empty(area);
    Line::from(span).render(area, &mut buf);
}

/// All four themes survive a full JSON round-trip with no field loss.
///
/// This is the serde contract test for the Theme struct: every field that
/// goes in must come back out identically.
#[test]
fn all_themes_json_roundtrip_intact() {
    for t in [cosmos(), void(), dusk(), cosmos_light()] {
        let json = serde_json::to_string(&t).expect("serialize must succeed");
        let back: sid_ui::Theme =
            serde_json::from_str(&json).expect("deserialize must succeed");
        assert_eq!(back.name, t.name);
        assert_eq!(back.background, t.background);
        assert_eq!(back.surface, t.surface);
        assert_eq!(back.foreground, t.foreground);
        assert_eq!(back.muted, t.muted);
        assert_eq!(back.accent_primary, t.accent_primary);
        assert_eq!(back.accent_success, t.accent_success);
        assert_eq!(back.accent_warning, t.accent_warning);
        assert_eq!(back.accent_error, t.accent_error);
        assert_eq!(back.border, t.border);
        assert_eq!(back.glyphs, t.glyphs);
    }
}
