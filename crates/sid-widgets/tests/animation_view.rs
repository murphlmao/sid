//! Tests for [`AnimationView`] — the Settings tab's animation tuner.
//!
//! Unit-style coverage (focus, mutation, dirty tracking, store round-trip) and
//! two insta snapshots pin the rendered ASCII body.

use sid_core::animation::{AnimationConfig, GlyphSet};
use sid_store::{OpenStore, RedbStore};
use sid_widgets::settings::animation::{AnimationField, AnimationView};
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, RedbStore) {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("anim.redb")).unwrap();
    (d, s)
}

#[test]
fn new_starts_clean_with_provided_config() {
    let cfg = AnimationConfig {
        density: 42,
        fps: 12,
        ..AnimationConfig::default()
    };
    let v = AnimationView::new(cfg.clone());
    assert!(!v.is_dirty());
    assert_eq!(v.config(), &cfg);
    assert_eq!(v.focused_field(), AnimationField::Enabled);
}

#[test]
fn focus_next_wraps_across_all_six_fields() {
    let mut v = AnimationView::new(AnimationConfig::default());
    let expected = [
        AnimationField::Enabled,
        AnimationField::Density,
        AnimationField::Fps,
        AnimationField::SupernovaIdleSecs,
        AnimationField::SupernovaOnEvent,
        AnimationField::GlyphSet,
    ];
    assert_eq!(v.focused_field(), expected[0]);
    for f in &expected[1..] {
        v.focus_next();
        assert_eq!(v.focused_field(), *f);
    }
    v.focus_next();
    assert_eq!(v.focused_field(), AnimationField::Enabled);
}

#[test]
fn focus_prev_wraps_in_reverse() {
    let mut v = AnimationView::new(AnimationConfig::default());
    // From Enabled, prev should wrap to GlyphSet.
    v.focus_prev();
    assert_eq!(v.focused_field(), AnimationField::GlyphSet);
    v.focus_prev();
    assert_eq!(v.focused_field(), AnimationField::SupernovaOnEvent);
    v.focus_prev();
    assert_eq!(v.focused_field(), AnimationField::SupernovaIdleSecs);
    v.focus_prev();
    assert_eq!(v.focused_field(), AnimationField::Fps);
    v.focus_prev();
    assert_eq!(v.focused_field(), AnimationField::Density);
    v.focus_prev();
    assert_eq!(v.focused_field(), AnimationField::Enabled);
}

#[test]
fn adjust_focused_toggles_enabled() {
    let mut v = AnimationView::new(AnimationConfig::default());
    assert!(v.config().enabled);
    v.adjust_focused(1);
    assert!(!v.config().enabled);
    v.adjust_focused(-1);
    assert!(v.config().enabled);
    v.adjust_focused(0);
    assert!(!v.config().enabled);
    assert!(v.is_dirty());
}

#[test]
fn adjust_focused_clamps_density_to_0_100() {
    let mut v = AnimationView::new(AnimationConfig::default());
    v.focus_next(); // Density
    assert_eq!(v.focused_field(), AnimationField::Density);
    // Crank up well past 100.
    for _ in 0..50 {
        v.adjust_focused(1);
    }
    assert_eq!(v.config().density, 100);
    // Crank down well past 0.
    for _ in 0..50 {
        v.adjust_focused(-1);
    }
    assert_eq!(v.config().density, 0);
}

#[test]
fn adjust_focused_cycles_glyph_set() {
    let mut v = AnimationView::new(AnimationConfig::default());
    // Jump focus to GlyphSet.
    for _ in 0..5 {
        v.focus_next();
    }
    assert_eq!(v.focused_field(), AnimationField::GlyphSet);
    assert_eq!(v.config().glyph_set, GlyphSet::Cosmos);
    v.adjust_focused(1);
    assert_eq!(v.config().glyph_set, GlyphSet::Minimal);
    v.adjust_focused(1);
    assert_eq!(v.config().glyph_set, GlyphSet::Ascii);
    v.adjust_focused(1);
    assert_eq!(v.config().glyph_set, GlyphSet::Cosmos);
    // Reverse wraps the other way.
    v.adjust_focused(-1);
    assert_eq!(v.config().glyph_set, GlyphSet::Ascii);
}

#[test]
fn dirty_flag_set_after_any_mutation() {
    let mut v = AnimationView::new(AnimationConfig::default());
    assert!(!v.is_dirty());
    v.focus_next(); // Density
    v.adjust_focused(1);
    assert!(v.is_dirty());

    // Idempotent at the clamp boundary: no further change once at min.
    let mut v2 = AnimationView::new(AnimationConfig {
        density: 0,
        ..AnimationConfig::default()
    });
    v2.focus_next();
    v2.adjust_focused(-1);
    assert!(!v2.is_dirty(), "no-op at clamp must not dirty");
}

#[test]
fn flush_then_load_round_trips_through_store() {
    let (_d, store) = store();
    let mut v = AnimationView::new(AnimationConfig::default());
    // Mutate a handful of fields.
    v.adjust_focused(1); // Enabled -> false
    v.focus_next(); // Density
    v.adjust_focused(1); // 30 -> 35
    v.focus_next(); // Fps
    v.adjust_focused(1); // 8 -> 9
    for _ in 0..3 {
        v.focus_next();
    }
    // Now at GlyphSet.
    assert_eq!(v.focused_field(), AnimationField::GlyphSet);
    v.adjust_focused(1); // Cosmos -> Minimal

    let saved = v.config().clone();
    assert!(v.is_dirty());
    v.flush_dirty(&store).unwrap();
    assert!(!v.is_dirty());

    let mut v2 = AnimationView::new(AnimationConfig::default());
    v2.load_from_store(&store).unwrap();
    assert_eq!(v2.config(), &saved);
    assert!(!v2.is_dirty());
}

// ---------------------------------------------------------------------------
// Snapshot tests
// ---------------------------------------------------------------------------

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use sid_ui::themes::cosmos;

fn render_view_to_string(v: &AnimationView, width: u16, height: u16, focused: bool) -> String {
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    term.draw(|f| v.render_into_frame(f, f.area(), &theme, focused))
        .unwrap();
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        s.push('\n');
    }
    s
}

#[test]
fn animation_view_default_render() {
    let v = AnimationView::new(AnimationConfig::default());
    let s = render_view_to_string(&v, 60, 14, true);
    insta::assert_snapshot!("animation_view_default_render", s);
}

#[test]
fn animation_view_density_focused_at_50() {
    let mut v = AnimationView::new(AnimationConfig {
        density: 50,
        ..AnimationConfig::default()
    });
    v.focus_next(); // Density
    assert_eq!(v.focused_field(), AnimationField::Density);
    let s = render_view_to_string(&v, 60, 14, true);
    insta::assert_snapshot!("animation_view_density_focused_at_50", s);
}

/// Helper that, like `render_view_to_string`, captures the character grid
/// plus the foreground style of the top-left border cell and the title-bar
/// character so the snapshot distinguishes focused (accent_primary + bold)
/// from unfocused (muted, non-bold) renders.
fn render_view_with_style(v: &AnimationView, width: u16, height: u16, focused: bool) -> String {
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    term.draw(|f| v.render_into_frame(f, f.area(), &theme, focused))
        .unwrap();
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        s.push('\n');
    }
    let tl = buf.cell((0, 0)).unwrap();
    s.push_str(&format!(
        "border_top_left: fg={:?} modifier={:?}\n",
        tl.fg, tl.modifier
    ));
    let title_cell = buf.cell((2, 0)).unwrap();
    s.push_str(&format!(
        "title_first_char: symbol={:?} fg={:?} modifier={:?}\n",
        title_cell.symbol(),
        title_cell.fg,
        title_cell.modifier
    ));
    s
}

#[test]
fn animation_view_render_focused() {
    let v = AnimationView::new(AnimationConfig::default());
    let s = render_view_with_style(&v, 60, 14, true);
    insta::assert_snapshot!("animation_view_render_focused", s);
}

#[test]
fn animation_view_render_unfocused() {
    let v = AnimationView::new(AnimationConfig::default());
    let s = render_view_with_style(&v, 60, 14, false);
    insta::assert_snapshot!("animation_view_render_unfocused", s);
}
