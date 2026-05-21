use proptest::prelude::*;
use sid_ui::theme::{Color, GlyphSet, Theme};

// ── Happy path ────────────────────────────────────────────────────────────────

#[test]
fn theme_holds_palette() {
    let t = Theme {
        name: "test".into(),
        background: Color::rgb(0x0b, 0x0b, 0x14),
        surface: Color::rgb(0x13, 0x13, 0x1f),
        foreground: Color::rgb(0xe6, 0xe6, 0xf0),
        muted: Color::rgb(0x4a, 0x4a, 0x60),
        accent_primary: Color::rgb(0xd4, 0x41, 0x41),
        accent_success: Color::rgb(0xa8, 0xd8, 0xe8),
        accent_warning: Color::rgb(0xe8, 0xb0, 0x4a),
        accent_error: Color::rgb(0xff, 0x55, 0x70),
        border: Color::rgb(0x1f, 0x1f, 0x2e),
        glyphs: Default::default(),
    };
    assert_eq!(t.name, "test");
    assert_eq!(t.background.r, 0x0b);
    assert_eq!(t.background.g, 0x0b);
    assert_eq!(t.background.b, 0x14);
}

#[test]
fn color_to_ratatui_round_trips_rgb() {
    let c = Color::rgb(0x12, 0x34, 0x56);
    let rt: ratatui::style::Color = c.into();
    assert!(matches!(rt, ratatui::style::Color::Rgb(0x12, 0x34, 0x56)));
}

#[test]
fn glyph_set_default_values() {
    let g = GlyphSet::default();
    assert_eq!(g.star, '★');
    assert_eq!(g.small_star, '✦');
    assert_eq!(g.dot, '·');
}

// ── Adversarial: extreme Color values ─────────────────────────────────────────

#[test]
fn color_min_black_round_trips_ratatui() {
    let c = Color::rgb(0, 0, 0);
    let rt: ratatui::style::Color = c.into();
    assert_eq!(rt, ratatui::style::Color::Rgb(0, 0, 0));
}

#[test]
fn color_max_white_round_trips_ratatui() {
    let c = Color::rgb(255, 255, 255);
    let rt: ratatui::style::Color = c.into();
    assert_eq!(rt, ratatui::style::Color::Rgb(255, 255, 255));
}

#[test]
fn color_copy_semantics() {
    let a = Color::rgb(10, 20, 30);
    let b = a; // copy
    assert_eq!(a.r, b.r);
    assert_eq!(a.g, b.g);
    assert_eq!(a.b, b.b);
}

#[test]
fn color_equality() {
    assert_eq!(Color::rgb(1, 2, 3), Color::rgb(1, 2, 3));
    assert_ne!(Color::rgb(1, 2, 3), Color::rgb(1, 2, 4));
}

// ── Property tests ─────────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn color_rgb_round_trips_through_ratatui(r in 0u8..=255, g in 0u8..=255, b in 0u8..=255) {
        let c = Color::rgb(r, g, b);
        let rt: ratatui::style::Color = c.into();
        prop_assert!(matches!(rt, ratatui::style::Color::Rgb(rr, gg, bb) if rr == r && gg == g && bb == b));
    }

    #[test]
    fn color_fields_preserved(r in 0u8..=255, g in 0u8..=255, b in 0u8..=255) {
        let c = Color::rgb(r, g, b);
        prop_assert_eq!(c.r, r);
        prop_assert_eq!(c.g, g);
        prop_assert_eq!(c.b, b);
    }
}
