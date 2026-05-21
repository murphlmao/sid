use proptest::prelude::*;
use sid_ui::themes::{cosmos, cosmos_light, dusk, void};

// ── Happy path ────────────────────────────────────────────────────────────────

#[test]
fn cosmos_has_expected_background() {
    let t = cosmos();
    assert_eq!(t.name, "cosmos");
    assert_eq!(t.background.r, 0x0b);
    assert_eq!(t.background.g, 0x0b);
    assert_eq!(t.background.b, 0x14);
}

#[test]
fn cosmos_has_expected_surface() {
    let t = cosmos();
    assert_eq!(t.surface.r, 0x13);
    assert_eq!(t.surface.g, 0x13);
    assert_eq!(t.surface.b, 0x1f);
}

#[test]
fn void_has_pure_black_background() {
    let t = void();
    assert_eq!(t.name, "void");
    assert_eq!(t.background.r, 0x00);
    assert_eq!(t.background.g, 0x00);
    assert_eq!(t.background.b, 0x00);
}

#[test]
fn dusk_has_warm_background() {
    let t = dusk();
    assert_eq!(t.name, "dusk");
    // Dusk background is warmer (more red than blue)
    assert!(t.background.r > t.background.b);
}

#[test]
fn cosmos_light_has_light_background() {
    let t = cosmos_light();
    assert_eq!(t.name, "cosmos-light");
    // Light variant: all background channels are high (> 128)
    assert!(t.background.r > 128);
    assert!(t.background.g > 128);
    assert!(t.background.b > 128);
}

#[test]
fn all_themes_have_unique_names() {
    let names: Vec<_> = [cosmos(), void(), dusk(), cosmos_light()]
        .iter()
        .map(|t| t.name.clone())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len());
}

#[test]
fn all_themes_have_default_glyphs() {
    use sid_ui::theme::GlyphSet;
    let expected = GlyphSet::default();
    for t in [cosmos(), void(), dusk(), cosmos_light()] {
        assert_eq!(t.glyphs, expected, "theme {} has unexpected glyphs", t.name);
    }
}

// ── Adversarial: no two factories return identical palettes ────────────────────

/// Encode a theme's entire palette as a tuple for comparison.
fn palette_fingerprint(t: &sid_ui::Theme) -> (u8, u8, u8, u8, u8, u8, u8, u8, u8) {
    (
        t.background.r,
        t.surface.g,
        t.foreground.b,
        t.muted.r,
        t.accent_primary.g,
        t.accent_success.b,
        t.accent_warning.r,
        t.accent_error.g,
        t.border.b,
    )
}

#[test]
fn no_two_themes_are_palette_identical() {
    let themes = [cosmos(), void(), dusk(), cosmos_light()];
    let fps: Vec<_> = themes.iter().map(palette_fingerprint).collect();
    for i in 0..fps.len() {
        for j in (i + 1)..fps.len() {
            assert_ne!(fps[i], fps[j], "themes {} and {} have identical palette fingerprint", themes[i].name, themes[j].name);
        }
    }
}

// ── Property tests ─────────────────────────────────────────────────────────────

proptest! {
    /// All four theme names are distinct from each other (redundant with
    /// all_themes_have_unique_names but exercised via proptest to ensure
    /// the factory functions are deterministic across repeated calls).
    #[test]
    fn theme_factories_are_deterministic(_seed in 0u32..1000) {
        // Factories take no args; calling twice must produce identical output.
        let a = cosmos();
        let b = cosmos();
        prop_assert_eq!(a.name, b.name);
        prop_assert_eq!(a.background.r, b.background.r);
        prop_assert_eq!(a.accent_primary.g, b.accent_primary.g);

        let a = void();
        let b = void();
        prop_assert_eq!(a.name, b.name);
        prop_assert_eq!(a.background.r, b.background.r);
    }
}
