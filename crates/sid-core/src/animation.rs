//! Configuration for the animated cosmic background.
//!
//! Lives in `sid-core` so widgets, the binary, and `sid-fx` can all reference
//! the same shape. The `sid-fx` crate consumes this struct; the Settings tab
//! mutates it; the store persists it under [`SETTING_ANIMATION_KEY`].
//!
//! `AnimationConfig` is a plain data struct with `Serialize`/`Deserialize`
//! derives. It has no behaviour and pulls in no PRNG or rendering deps; those
//! belong to `sid-fx`.

use serde::{Deserialize, Serialize};

/// Storage key under which the [`AnimationConfig`] blob is persisted in the
/// `settings` table.
///
/// Centralised here so the store accessor (in `sid-store`), the Settings
/// sub-view (in `sid-widgets`), and the runtime wiring (in `sid/`) cannot
/// drift apart.
///
/// # Examples
///
/// ```
/// use sid_core::animation::SETTING_ANIMATION_KEY;
/// assert_eq!(SETTING_ANIMATION_KEY, "animation");
/// ```
pub const SETTING_ANIMATION_KEY: &str = "animation";

/// Per-user configuration for the cosmic background renderer.
///
/// The background is one shared visual layer behind every tab. This struct
/// controls how it behaves: density of the starfield, animation rate, and
/// whether the (still-unimplemented) supernova bursts fire on idle / on
/// significant widget events.
///
/// Supernova fields (`supernova_idle_secs`, `supernova_on_event`) are
/// reserved spec hooks — they are read but ignored by Phase 6.1's
/// starfield-only renderer. Phase 6.2 wires them up.
///
/// # Examples
///
/// ```
/// use sid_core::animation::{AnimationConfig, GlyphSet};
///
/// let cfg = AnimationConfig::default();
/// assert!(cfg.enabled);
/// assert_eq!(cfg.density, 30);
/// assert_eq!(cfg.fps, 8);
/// assert_eq!(cfg.glyph_set, GlyphSet::Cosmos);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationConfig {
    /// Master switch. When `false`, the background renderer produces no output
    /// and the runtime is free to skip animation ticks entirely.
    pub enabled: bool,
    /// Stars per 80×24 cells, scaled by area for larger terminals.
    ///
    /// Range: `0..=100`. The renderer treats `0` as "no stars" and caps the
    /// resulting count at a hard upper bound to protect huge terminals.
    pub density: u8,
    /// Animation frames per second.
    ///
    /// Range: `1..=30`. Higher values give smoother twinkles but cost more CPU
    /// per second. The runtime ticks the renderer at this rate.
    pub fps: u8,
    /// Seconds between idle supernova events. `0` disables idle supernovae.
    ///
    /// Spec hook only — supernovae land in Phase 6.2.
    pub supernova_idle_secs: u32,
    /// Whether widget-event triggers (commit, kill, etc.) spawn a celebratory
    /// supernova.
    ///
    /// Spec hook only — supernovae land in Phase 6.2.
    pub supernova_on_event: bool,
    /// Which glyph palette the renderer draws stars from.
    pub glyph_set: GlyphSet,
    /// How the starfield moves over time. See [`MotionStyle`].
    ///
    /// `#[serde(default)]` keeps `AnimationConfig` blobs persisted before this
    /// field existed deserialising cleanly: they decode with
    /// [`MotionStyle::Cosmos`] (the default) and every other field preserved,
    /// rather than failing the whole struct and resetting the user's config.
    #[serde(default)]
    pub motion: MotionStyle,
}

/// How the starfield moves over time.
///
/// Selected in the Animation settings sub-view and persisted as part of
/// [`AnimationConfig`]. The renderer (`sid-fx`) branches on this every tick;
/// each variant is a distinct visual feel.
///
/// # Examples
///
/// ```
/// use sid_core::animation::MotionStyle;
/// assert_eq!(MotionStyle::default(), MotionStyle::Cosmos);
/// ```
// No `rename_all`: serialise as the variant name (PascalCase) to match the
// sibling `GlyphSet` enum's wire form inside the same `AnimationConfig` JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MotionStyle {
    /// Stars hold fixed positions and pulse in brightness (a twinkle), tuned
    /// to be clearly visible rather than the original near-imperceptible drift.
    Twinkle,
    /// Stars drift slowly across the field and wrap at the edges. Brighter
    /// stars drift faster, giving a parallax sense of depth.
    Drift,
    /// The full cosmos: drift + a pronounced twinkle + occasional shooting-star
    /// streaks and idle supernova blooms. The default.
    #[default]
    Cosmos,
}

/// Glyph palette for the starfield renderer.
///
/// Distinct from [`sid_ui::theme::GlyphSet`] which is a struct of named
/// decoration glyphs for UI chrome. This enum is the *animation* palette: it
/// picks which glyphs the renderer uses for individual stars.
///
/// # Examples
///
/// ```
/// use sid_core::animation::GlyphSet;
/// assert_eq!(GlyphSet::default(), GlyphSet::Cosmos);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlyphSet {
    /// Mixed unicode: `·`, `✦`, `·`, `*`. The default cosmos aesthetic.
    #[default]
    Cosmos,
    /// Two-glyph minimum: `·`, `*`. Cleaner on narrow / low-DPI terminals.
    Minimal,
    /// ASCII-only fallback: `.`. For terminals without unicode support.
    Ascii,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            density: 30,
            fps: 8,
            supernova_idle_secs: 90,
            supernova_on_event: true,
            glyph_set: GlyphSet::Cosmos,
            motion: MotionStyle::Cosmos,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_spec() {
        let cfg = AnimationConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.density, 30);
        assert_eq!(cfg.fps, 8);
        assert_eq!(cfg.supernova_idle_secs, 90);
        assert!(cfg.supernova_on_event);
        assert_eq!(cfg.glyph_set, GlyphSet::Cosmos);
        assert_eq!(cfg.motion, MotionStyle::Cosmos);
    }

    #[test]
    fn glyph_set_default_is_cosmos() {
        assert_eq!(GlyphSet::default(), GlyphSet::Cosmos);
    }

    #[test]
    fn motion_default_is_cosmos() {
        assert_eq!(MotionStyle::default(), MotionStyle::Cosmos);
    }

    #[test]
    fn config_round_trips_with_all_motion_styles() {
        for motion in [
            MotionStyle::Twinkle,
            MotionStyle::Drift,
            MotionStyle::Cosmos,
        ] {
            let cfg = AnimationConfig {
                motion,
                ..AnimationConfig::default()
            };
            let s = serde_json::to_string(&cfg).expect("serialize");
            let back: AnimationConfig = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(cfg, back);
            assert_eq!(back.motion, motion);
        }
    }

    #[test]
    fn motion_serializes_as_variant_name() {
        // Wire form is the PascalCase variant name, matching sibling `GlyphSet`.
        assert_eq!(
            serde_json::to_string(&MotionStyle::Cosmos).unwrap(),
            "\"Cosmos\""
        );
        assert_eq!(
            serde_json::to_string(&MotionStyle::Twinkle).unwrap(),
            "\"Twinkle\""
        );
        assert_eq!(
            serde_json::to_string(&MotionStyle::Drift).unwrap(),
            "\"Drift\""
        );
    }

    #[test]
    fn legacy_config_without_motion_field_deserializes_to_cosmos() {
        // A blob persisted before `motion` existed: a JSON object with every
        // field EXCEPT `motion`. `#[serde(default)]` must let it decode with
        // motion = Cosmos and all other fields preserved — NOT reject the
        // whole struct (which would silently reset the user's saved config).
        let legacy = r#"{
            "enabled": false,
            "density": 17,
            "fps": 12,
            "supernova_idle_secs": 42,
            "supernova_on_event": false,
            "glyph_set": "Minimal"
        }"#;
        let back: AnimationConfig =
            serde_json::from_str(legacy).expect("legacy config must still deserialize");
        assert_eq!(
            back.motion,
            MotionStyle::Cosmos,
            "missing motion -> default"
        );
        // Other fields survive intact.
        assert!(!back.enabled);
        assert_eq!(back.density, 17);
        assert_eq!(back.fps, 12);
        assert_eq!(back.supernova_idle_secs, 42);
        assert!(!back.supernova_on_event);
        assert_eq!(back.glyph_set, GlyphSet::Minimal);
    }

    #[test]
    fn equality_distinguishes_motion() {
        let cosmos = AnimationConfig::default();
        let drift = AnimationConfig {
            motion: MotionStyle::Drift,
            ..AnimationConfig::default()
        };
        assert_ne!(cosmos, drift);
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = AnimationConfig::default();
        let s = serde_json::to_string(&cfg).expect("serialize");
        let back: AnimationConfig = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn config_round_trips_with_all_glyph_sets() {
        for glyph in [GlyphSet::Cosmos, GlyphSet::Minimal, GlyphSet::Ascii] {
            let cfg = AnimationConfig {
                glyph_set: glyph,
                ..AnimationConfig::default()
            };
            let s = serde_json::to_string(&cfg).expect("serialize");
            let back: AnimationConfig = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(cfg, back);
            assert_eq!(back.glyph_set, glyph);
        }
    }

    #[test]
    fn setting_key_constant_is_animation() {
        assert_eq!(SETTING_ANIMATION_KEY, "animation");
    }

    #[test]
    fn equality_distinguishes_disabled() {
        let on = AnimationConfig::default();
        let off = AnimationConfig {
            enabled: false,
            ..AnimationConfig::default()
        };
        assert_ne!(on, off);
    }
}
