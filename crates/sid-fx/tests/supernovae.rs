//! Integration tests for `sid-fx` supernovae (Phase 6.2).
//!
//! Covers:
//!   - Determinism: identical seeds + identical ticks produce identical queues.
//!   - Idle cadence math: `supernova_idle_secs` × `fps` ticks per spawn.
//!   - `trigger_supernova` semantics: appends one entry per call, idempotent.
//!   - Lifetime: supernovae are dropped after `SUPERNOVA_LIFETIME_FRAMES`.
//!   - `enabled = false` short-circuits both idle and render paths.
//!   - Proptest: `render_supernovae` survives arbitrary areas / ages.
//!   - Insta snapshot of a 40×10 buffer with one centered supernova at age 0.

use proptest::prelude::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use sid_core::animation::{AnimationConfig, GlyphSet};
use sid_fx::{FxState, SUPERNOVA_LIFETIME_FRAMES, Supernova, SupernovaPalette, render_supernovae};
use sid_ui::themes::cosmos;

const SEED: u64 = 42;
const AREA: Rect = Rect {
    x: 0,
    y: 0,
    width: 40,
    height: 10,
};

/// Convenience: a config that disables idle spawns so non-idle tests aren't
/// surprised by ambient supernovae. Tests that exercise idle cadence opt back
/// in explicitly.
fn no_idle_cfg() -> AnimationConfig {
    AnimationConfig {
        supernova_idle_secs: 0,
        ..AnimationConfig::default()
    }
}

/// Two `FxState::with_seed(42)` instances ticked identically with idle
/// supernovae enabled produce byte-identical supernova queues.
#[test]
fn with_seed_supernovae_deterministic() {
    let cfg = AnimationConfig {
        // Spawn an idle supernova every second. With default fps=8 that's
        // every 8 ticks; over 50 ticks we get ~6 spawns, of which only the
        // most recent few survive the 6-frame lifetime. That's enough
        // material to detect divergence between the two states.
        supernova_idle_secs: 1,
        ..AnimationConfig::default()
    };
    let mut a = FxState::with_seed(SEED);
    let mut b = FxState::with_seed(SEED);
    for _ in 0..50 {
        a.tick(AREA, &cfg);
        b.tick(AREA, &cfg);
    }
    assert_eq!(
        a.supernovae.len(),
        b.supernovae.len(),
        "queue lengths must match"
    );
    assert_eq!(
        a.total_supernovae_spawned, b.total_supernovae_spawned,
        "spawn counts must match"
    );
    for (sa, sb) in a.supernovae.iter().zip(b.supernovae.iter()) {
        assert_eq!(sa.center, sb.center, "centers must match");
        assert_eq!(sa.age, sb.age, "ages must match");
        assert_eq!(sa.palette, sb.palette, "palettes must match");
    }
}

/// With `supernova_idle_secs = 2` and `fps = 10`, the cadence is one spawn
/// every `2 × 10 = 20` ticks. The test ticks in 20-tick increments and asserts
/// the cumulative spawn count grows by exactly 1 each block.
///
/// Math:
///   - At tick 20: `frames_since_last_idle = 20`, `20 / 10 = 2 >= 2` → spawn,
///     reset counter to 0. Total = 1.
///   - At tick 40: counter reaches 20 again → spawn. Total = 2.
///   - At tick `20 * N`: total spawned = `N`.
///
/// We assert via `total_supernovae_spawned`, not `supernovae.len()`, because
/// the queue drops entries past `SUPERNOVA_LIFETIME_FRAMES` and would
/// otherwise undercount.
#[test]
fn idle_supernova_spawns_at_configured_rate() {
    let cfg = AnimationConfig {
        supernova_idle_secs: 2,
        fps: 10,
        ..AnimationConfig::default()
    };
    let mut state = FxState::with_seed(SEED);

    // Before any tick: zero spawns.
    assert_eq!(state.total_supernovae_spawned, 0);

    // Tick 20 → exactly 1.
    for _ in 0..20 {
        state.tick(AREA, &cfg);
    }
    assert_eq!(
        state.total_supernovae_spawned, 1,
        "20 ticks at 10 fps × 2s = 1 spawn"
    );

    // Tick 20 more → exactly 2.
    for _ in 0..20 {
        state.tick(AREA, &cfg);
    }
    assert_eq!(
        state.total_supernovae_spawned, 2,
        "40 ticks total = 2 spawns"
    );

    // Tick another 60 → 3 more spawns for a total of 5.
    for _ in 0..60 {
        state.tick(AREA, &cfg);
    }
    assert_eq!(
        state.total_supernovae_spawned, 5,
        "100 ticks total = 5 spawns"
    );
}

/// Calling `trigger_supernova` appends exactly one entry per call. Two calls
/// produce two entries, regardless of identical position.
#[test]
fn trigger_supernova_appends_to_queue() {
    let mut state = FxState::with_seed(SEED);
    assert_eq!(state.supernovae.len(), 0);
    assert_eq!(state.total_supernovae_spawned, 0);

    state.trigger_supernova(AREA, SupernovaPalette::Cosmos);
    assert_eq!(state.supernovae.len(), 1);
    assert_eq!(state.total_supernovae_spawned, 1);
    assert_eq!(state.supernovae[0].palette, SupernovaPalette::Cosmos);
    assert_eq!(state.supernovae[0].age, 0);

    state.trigger_supernova(AREA, SupernovaPalette::Celebrate);
    assert_eq!(state.supernovae.len(), 2);
    assert_eq!(state.total_supernovae_spawned, 2);
    assert_eq!(state.supernovae[1].palette, SupernovaPalette::Celebrate);
}

/// A supernova ages by 1 each tick and is dropped from the queue once its
/// age reaches `SUPERNOVA_LIFETIME_FRAMES`. Ticking `LIFETIME + 1` times
/// after a single trigger leaves the queue empty.
#[test]
fn tick_ages_and_drops_supernovae() {
    let cfg = no_idle_cfg();
    let mut state = FxState::with_seed(SEED);
    state.trigger_supernova(AREA, SupernovaPalette::Cosmos);
    assert_eq!(state.supernovae.len(), 1);

    // After exactly LIFETIME ticks, the supernova hits age == LIFETIME and
    // gets dropped. One extra tick guards against an off-by-one.
    for _ in 0..(SUPERNOVA_LIFETIME_FRAMES + 1) {
        state.tick(AREA, &cfg);
    }
    assert!(
        state.supernovae.is_empty(),
        "supernova should be dropped after lifetime; queue len = {}",
        state.supernovae.len()
    );
    // Cumulative count is unaffected by drops.
    assert_eq!(state.total_supernovae_spawned, 1);
}

/// Idle supernovae do not spawn when `cfg.enabled = false`, even if
/// `supernova_idle_secs` is set. The master switch wins.
#[test]
fn enabled_false_does_not_spawn_idle() {
    let cfg = AnimationConfig {
        enabled: false,
        supernova_idle_secs: 1,
        fps: 8,
        ..AnimationConfig::default()
    };
    let mut state = FxState::with_seed(SEED);
    for _ in 0..50 {
        state.tick(AREA, &cfg);
    }
    assert_eq!(state.total_supernovae_spawned, 0);
    assert!(state.supernovae.is_empty());
}

/// `cfg.supernova_idle_secs = 0` disables idle spawning even with the master
/// switch on.
#[test]
fn zero_idle_secs_does_not_spawn_idle() {
    let cfg = AnimationConfig {
        supernova_idle_secs: 0,
        ..AnimationConfig::default()
    };
    let mut state = FxState::with_seed(SEED);
    for _ in 0..200 {
        state.tick(AREA, &cfg);
    }
    assert_eq!(state.total_supernovae_spawned, 0);
}

/// `render_supernovae` is a no-op when `cfg.enabled = false`.
#[test]
fn render_supernovae_disabled_is_noop() {
    let mut state = FxState::with_seed(SEED);
    state.trigger_supernova(AREA, SupernovaPalette::Cosmos);

    let mut buf = Buffer::empty(AREA);
    let baseline: Vec<String> = (0..AREA.height)
        .map(|y| {
            (0..AREA.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();

    let off_cfg = AnimationConfig {
        enabled: false,
        ..AnimationConfig::default()
    };
    let theme = cosmos();
    render_supernovae(&mut buf, AREA, &state, &off_cfg, &theme);

    let after: Vec<String> = (0..AREA.height)
        .map(|y| {
            (0..AREA.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    assert_eq!(baseline, after);
}

/// Snapshot test — a 40×10 buffer with a single centered supernova at
/// age=0 must produce stable byte-identical output. The snapshot exercises
/// the full 11-cell glyph cluster at peak brightness.
#[test]
fn render_supernovae_snapshot() {
    let mut state = FxState::with_seed(SEED);
    // Hand-place a supernova at the center of the area; do NOT rely on
    // `trigger_supernova`'s RNG-picked position so the snapshot is robust
    // against future RNG churn.
    state.supernovae.push_back(Supernova {
        center: (20, 5),
        age: 0,
        palette: SupernovaPalette::Cosmos,
    });

    let cfg = AnimationConfig::default();
    let theme = cosmos();
    let mut buf = Buffer::empty(AREA);
    render_supernovae(&mut buf, AREA, &state, &cfg, &theme);

    let rendered: String = (0..AREA.height)
        .map(|y| {
            let row: String = (0..AREA.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            format!("{row}\n")
        })
        .collect();

    insta::with_settings!({ snapshot_path => "snapshots" }, {
        insta::assert_snapshot!("supernova_40x10_center_age0", rendered);
    });
}

/// Trigger lands every entry inside the area for any plausible terminal size.
#[test]
fn trigger_keeps_center_inside_area() {
    let mut state = FxState::with_seed(SEED);
    let area = Rect::new(0, 0, 80, 24);
    for _ in 0..100 {
        state.trigger_supernova(area, SupernovaPalette::Cosmos);
    }
    for sn in &state.supernovae {
        assert!(
            sn.center.0 < area.width,
            "x={} >= width={}",
            sn.center.0,
            area.width
        );
        assert!(
            sn.center.1 < area.height,
            "y={} >= height={}",
            sn.center.1,
            area.height
        );
    }
}

proptest! {
    /// `render_supernovae` must not panic for any plausible terminal size or
    /// supernova state. We sweep area dimensions, palette, age, and the
    /// supernova's center (allowed to escape the area so the bounds check
    /// gets exercised on both sides).
    #[test]
    fn render_supernovae_never_panics(
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
        cx in 0u16..220,
        cy in 0u16..70,
        age in 0u8..=10,
        palette_pick in 0u8..3,
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
        let palette = match palette_pick {
            0 => SupernovaPalette::Cosmos,
            1 => SupernovaPalette::Celebrate,
            _ => SupernovaPalette::Warm,
        };
        // Push a supernova at the proptest-chosen center (may be outside
        // area) with the proptest-chosen age (may exceed LIFETIME so we
        // also exercise the saturation in render_supernovae).
        state.supernovae.push_back(Supernova {
            center: (cx, cy),
            age,
            palette,
        });
        let mut buf = Buffer::empty(area);
        render_supernovae(&mut buf, area, &state, &cfg, &theme);
    }

    /// Determinism property: two `FxState`s with the same seed remain in lockstep
    /// over many ticks at any idle rate.
    #[test]
    fn prop_supernova_queue_stays_in_sync(
        seed in any::<u64>(),
        idle in 0u32..=4,
        fps in 1u8..=30,
        ticks in 0u32..=80,
    ) {
        let cfg = AnimationConfig {
            supernova_idle_secs: idle,
            fps,
            ..AnimationConfig::default()
        };
        let mut a = FxState::with_seed(seed);
        let mut b = FxState::with_seed(seed);
        for _ in 0..ticks {
            a.tick(AREA, &cfg);
            b.tick(AREA, &cfg);
        }
        prop_assert_eq!(a.supernovae.len(), b.supernovae.len());
        prop_assert_eq!(a.total_supernovae_spawned, b.total_supernovae_spawned);
        for (sa, sb) in a.supernovae.iter().zip(b.supernovae.iter()) {
            prop_assert_eq!(sa.center, sb.center);
            prop_assert_eq!(sa.age, sb.age);
            prop_assert_eq!(sa.palette, sb.palette);
        }
    }
}
