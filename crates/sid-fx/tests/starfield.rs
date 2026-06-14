//! Integration tests for the `sid-fx` starfield renderer.
//!
//! Covers:
//!   - Determinism under a fixed seed.
//!   - Phase advancement on tick.
//!   - Density rebalancing (0 → 0 stars; ~100 → ~proportional).
//!   - The `enabled = false` escape hatch leaves buffers untouched.
//!   - Snapshot of a 40×10 buffer after 5 ticks with seed=42.
//!   - Proptest that random areas + configs never panic the renderer.

use proptest::prelude::*;
use ratatui::{buffer::Buffer, layout::Rect};
use sid_core::animation::{AnimationConfig, GlyphSet};
use sid_fx::{FxState, render_starfield};
use sid_ui::themes::cosmos;

const SEED: u64 = 42;
const AREA: Rect = Rect {
    x: 0,
    y: 0,
    width: 40,
    height: 10,
};

fn default_cfg() -> AnimationConfig {
    AnimationConfig::default()
}

/// Two `FxState::with_seed(42)` instances ticked identically produce the same
/// star positions, brightness floors, twinkle phases, and kinds.
#[test]
fn with_seed_is_deterministic() {
    let mut a = FxState::with_seed(SEED);
    let mut b = FxState::with_seed(SEED);
    let cfg = default_cfg();

    a.tick(AREA, &cfg);
    b.tick(AREA, &cfg);

    assert_eq!(a.stars().len(), b.stars().len(), "star counts must match");
    assert!(!a.stars().is_empty(), "should have spawned stars");
    for (s1, s2) in a.stars().iter().zip(b.stars().iter()) {
        assert_eq!((s1.x, s1.y), (s2.x, s2.y), "positions must match");
        assert_eq!(s1.base, s2.base, "base brightness must match");
        assert_eq!(s1.phase, s2.phase, "phase must match");
        assert_eq!(s1.speed, s2.speed, "speed must match");
        assert_eq!(s1.kind, s2.kind, "kind must match");
    }
}

/// `tick` runs for many frames without any divergence between two seeded
/// states. This guards against time-dependent or platform-dependent calls
/// sneaking into the renderer.
#[test]
fn determinism_holds_over_many_ticks() {
    let mut a = FxState::with_seed(SEED);
    let mut b = FxState::with_seed(SEED);
    let cfg = default_cfg();
    for _ in 0..50 {
        a.tick(AREA, &cfg);
        b.tick(AREA, &cfg);
    }
    for (s1, s2) in a.stars().iter().zip(b.stars().iter()) {
        assert_eq!(s1.phase, s2.phase, "phases must stay in sync over time");
    }
}

/// After one `tick`, every star's phase has advanced by its `speed`.
///
/// We use Drift mode (no boost) so we can assert the exact per-speed
/// increment. Twinkle and Cosmos modes apply a visible boost multiplier
/// (`TWINKLE_BOOST`) on top of `speed`; that path is tested separately in
/// `twinkle_brightness_changes_across_ticks` (unit tests).
#[test]
fn tick_advances_phase() {
    use sid_core::animation::MotionStyle;
    let mut state = FxState::with_seed(SEED);
    // Use Drift mode so phase advances exactly by `speed` (no boost).
    let cfg = AnimationConfig {
        motion: MotionStyle::Drift,
        ..default_cfg()
    };
    // First tick spawns stars. Capture (speed, phase) pairs immediately after.
    state.tick(AREA, &cfg);
    let snapshot: Vec<(u8, u8)> = state.stars().iter().map(|s| (s.speed, s.phase)).collect();

    state.tick(AREA, &cfg);

    assert_eq!(
        state.stars().len(),
        snapshot.len(),
        "star count must be stable between ticks at same density"
    );
    for (s, (old_speed, old_phase)) in state.stars().iter().zip(snapshot.iter()) {
        assert_eq!(s.speed, *old_speed, "speed must not drift");
        assert_eq!(
            s.phase,
            old_phase.wrapping_add(*old_speed),
            "phase must advance by exactly `speed` per tick in Drift mode"
        );
    }
}

/// `cfg.density = 0` → 0 stars. `cfg.density = 100` at 80×24 → ~80 stars.
#[test]
fn tick_respects_density() {
    let area = Rect::new(0, 0, 80, 24);
    let mut zero_state = FxState::with_seed(SEED);
    let zero_cfg = AnimationConfig {
        density: 0,
        ..default_cfg()
    };
    zero_state.tick(area, &zero_cfg);
    assert_eq!(
        zero_state.stars().len(),
        0,
        "density=0 must produce 0 stars"
    );

    let mut full_state = FxState::with_seed(SEED);
    let full_cfg = AnimationConfig {
        density: 100,
        ..default_cfg()
    };
    full_state.tick(area, &full_cfg);
    // At density=100 on 80×24, the spec says ~80 stars (100 stars per 80×24
    // would be 100 stars at baseline, but baseline_cells=80*24 means
    // density 100 → 100 stars exactly).
    let n = full_state.stars().len();
    assert!(
        (50..=200).contains(&n),
        "density=100 expected ~100 stars, got {n}"
    );
}

/// Density changes mid-flight rebalance the star count on the next tick.
#[test]
fn density_change_rebalances() {
    let area = Rect::new(0, 0, 80, 24);
    let mut state = FxState::with_seed(SEED);

    let low_cfg = AnimationConfig {
        density: 10,
        ..default_cfg()
    };
    state.tick(area, &low_cfg);
    let low_count = state.stars().len();

    let high_cfg = AnimationConfig {
        density: 60,
        ..default_cfg()
    };
    state.tick(area, &high_cfg);
    let high_count = state.stars().len();

    assert!(
        high_count > low_count,
        "expected more stars after density increase ({low_count} -> {high_count})"
    );
}

/// `render_starfield` with `enabled = false` writes no cells.
#[test]
fn enabled_false_renders_nothing() {
    let area = AREA;
    let mut state = FxState::with_seed(SEED);
    // Build a state with stars first, then render with enabled=false.
    let on_cfg = default_cfg();
    state.tick(area, &on_cfg);
    assert!(!state.stars().is_empty(), "precondition: stars exist");

    let mut buf = Buffer::empty(area);
    let baseline: Vec<String> = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();

    let off_cfg = AnimationConfig {
        enabled: false,
        ..default_cfg()
    };
    let theme = cosmos();
    render_starfield(&mut buf, area, &state, &off_cfg, &theme);

    let after: Vec<String> = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();

    assert_eq!(baseline, after, "disabled renderer must not touch buffer");
}

/// Snapshot test — a 40×10 buffer after 5 ticks with seed=42 must produce
/// stable byte-identical output. If the renderer changes, this snapshot
/// fails and the operator chooses to re-accept or fix the change.
#[test]
fn render_into_buffer_snapshot() {
    let area = AREA;
    let mut state = FxState::with_seed(SEED);
    let cfg = default_cfg();
    let theme = cosmos();
    let mut buf = Buffer::empty(area);

    for _ in 0..5 {
        state.tick(area, &cfg);
    }
    render_starfield(&mut buf, area, &state, &cfg, &theme);

    let rendered: String = (0..area.height)
        .map(|y| {
            let row: String = (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            format!("{row}\n")
        })
        .collect();

    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("starfield_40x10_seed42_5ticks", rendered);
    });
}

proptest! {
    /// `render_starfield` must not panic for any plausible terminal size.
    ///
    /// We sweep area dimensions 0..200 × 0..60 (covers everything from "empty
    /// region after a resize" to "ultra-wide tmux pane") and all glyph sets.
    /// Density and FPS are bounded to the spec ranges (0..=100 and 1..=30).
    #[test]
    fn prop_render_never_panics(
        w in 0u16..200,
        h in 0u16..60,
        density in 0u8..=100,
        fps in 1u8..=30,
        glyph in prop_oneof![
            Just(GlyphSet::Cosmos),
            Just(GlyphSet::Minimal),
            Just(GlyphSet::Ascii),
        ],
        enabled in any::<bool>(),
        seed in any::<u64>(),
    ) {
        let area = Rect::new(0, 0, w, h);
        let cfg = AnimationConfig {
            enabled,
            density,
            fps,
            glyph_set: glyph,
            ..AnimationConfig::default()
        };
        let theme = cosmos();
        let mut state = FxState::with_seed(seed);
        state.tick(area, &cfg);
        let mut buf = Buffer::empty(area);
        render_starfield(&mut buf, area, &state, &cfg, &theme);
    }

    /// Disabled config produces 0 stars regardless of density.
    #[test]
    fn prop_disabled_produces_no_stars(
        w in 1u16..200,
        h in 1u16..60,
        density in 0u8..=100,
        seed in any::<u64>(),
    ) {
        let area = Rect::new(0, 0, w, h);
        let cfg = AnimationConfig {
            enabled: false,
            density,
            ..AnimationConfig::default()
        };
        let mut state = FxState::with_seed(seed);
        state.tick(area, &cfg);
        prop_assert_eq!(state.stars().len(), 0);
    }

    /// Stars stay inside the area they were spawned for.
    ///
    /// Adversarial: spawn into a tiny area then re-tick with the same area —
    /// every star must still be inside.
    #[test]
    fn prop_stars_stay_inside_area(
        w in 1u16..120,
        h in 1u16..40,
        density in 1u8..=100,
        seed in any::<u64>(),
    ) {
        let area = Rect::new(0, 0, w, h);
        let cfg = AnimationConfig { density, ..AnimationConfig::default() };
        let mut state = FxState::with_seed(seed);
        state.tick(area, &cfg);
        for s in state.stars() {
            prop_assert!(s.x < w, "x={} >= w={}", s.x, w);
            prop_assert!(s.y < h, "y={} >= h={}", s.y, h);
        }
    }
}
