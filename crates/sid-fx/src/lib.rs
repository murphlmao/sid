//! Animated cosmic background renderer for sid.
//!
//! Phase 6.1 of the UX overhaul (see
//! `docs/superpowers/plans/2026-05-22-sid-ux-overhaul.md`) — starfield only.
//! Supernovae land in Phase 6.2.
//!
//! # Surface
//!
//! - [`FxState`] holds the live starfield (positions, phases, RNG state).
//! - [`FxState::tick`] advances animation by one frame and rebalances the star
//!   count to match the configured density / area.
//! - [`render_starfield`] paints stars into a ratatui `Buffer`. Widgets render
//!   on top and freely overwrite cells.
//!
//! # Determinism
//!
//! The renderer uses [`rand_chacha::ChaCha8Rng`] under the hood so that
//! [`FxState::with_seed`] produces byte-identical starfields across machines,
//! Rust versions, and `cargo` invocations. Production code uses
//! [`FxState::new`] which seeds from `rand::random()`; tests pin the seed.
//!
//! # Adapter discipline
//!
//! `sid-fx` is a pure rendering crate. It depends on:
//!
//! - `ratatui` (for `Buffer`/`Rect`/`Color`/`Style`),
//! - `rand` + `rand_chacha` (for the deterministic PRNG),
//! - `sid-core` (for the [`AnimationConfig`] shape),
//! - `sid-ui` (for the [`Theme`] colour palette).
//!
//! It does **not** depend on `crossterm`, `tokio`, `redb`, or any I/O crate.
//! All side effects are scoped to a `&mut Buffer` passed by the caller.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use sid_core::animation::{AnimationConfig, GlyphSet};
use sid_ui::theme::Theme;

/// A single star in the starfield.
///
/// Stars are spawned at a fixed `(x, y)` and never move. Their visible
/// brightness varies each tick via a triangle wave on [`Star::phase`] —
/// brightness peaks at `phase == 128`, dims toward `phase == 0 || 255`.
#[derive(Debug, Clone)]
pub struct Star {
    /// Column. Stable for the lifetime of the star.
    pub x: u16,
    /// Row. Stable for the lifetime of the star.
    pub y: u16,
    /// Base brightness floor, in `0..=255`. The star never dims below this.
    pub base: u8,
    /// Twinkle phase in `0..=255`. Advances by [`Star::speed`] each tick.
    pub phase: u8,
    /// Twinkle speed in `1..=8`. Lower = slower drift.
    pub speed: u8,
    /// Glyph and colour class.
    pub kind: StarKind,
}

/// What kind of star this is — drives glyph and colour selection.
///
/// # Examples
///
/// ```
/// use sid_fx::StarKind;
/// // Bright stars are rarer and visually larger than Dot stars.
/// let k = StarKind::Bright;
/// assert!(matches!(k, StarKind::Bright));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarKind {
    /// Common dim star. Glyph: `·` (Cosmos/Minimal) or `.` (ASCII).
    Dot,
    /// Accent star — uses theme.accent_primary at peak brightness.
    /// Glyph: `✦` (Cosmos), `·` (Minimal), `.` (ASCII).
    Accent,
    /// Bright star. Glyph: `*` (Cosmos/Minimal/ASCII).
    Bright,
}

/// Live starfield state. Mutated each tick, read each frame.
///
/// Construction:
///
/// - [`FxState::new`] for production (entropy-seeded).
/// - [`FxState::with_seed`] for tests (deterministic).
///
/// # Examples
///
/// ```
/// use ratatui::layout::Rect;
/// use sid_core::animation::AnimationConfig;
/// use sid_fx::FxState;
///
/// let mut state = FxState::with_seed(42);
/// let cfg = AnimationConfig::default();
/// state.tick(Rect::new(0, 0, 80, 24), &cfg);
/// assert!(!state.stars().is_empty());
/// ```
pub struct FxState {
    rng: ChaCha8Rng,
    stars: Vec<Star>,
    /// Tick count since spawn. Wraps at `u64::MAX` (~10^11 years at 8 FPS).
    /// Public so animation-aware widgets can read the global frame counter.
    pub tick_count: u64,
    /// Last area we generated stars for. When the terminal resizes, we
    /// discard and re-generate so positions stay inside bounds.
    last_area: Rect,
}

impl FxState {
    /// Construct an `FxState` with an entropy-seeded PRNG.
    ///
    /// Use this in production. Tests should prefer [`FxState::with_seed`]
    /// for deterministic snapshots.
    pub fn new() -> Self {
        Self::with_seed(rand::random())
    }

    /// Construct an `FxState` with a fixed seed.
    ///
    /// Two `FxState` instances with the same seed, ticked identically,
    /// produce byte-identical starfields.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::Rect;
    /// use sid_core::animation::AnimationConfig;
    /// use sid_fx::FxState;
    ///
    /// let mut a = FxState::with_seed(1);
    /// let mut b = FxState::with_seed(1);
    /// let area = Rect::new(0, 0, 40, 10);
    /// let cfg = AnimationConfig::default();
    /// a.tick(area, &cfg);
    /// b.tick(area, &cfg);
    /// // Same seed -> same positions.
    /// assert_eq!(a.stars().len(), b.stars().len());
    /// for (s1, s2) in a.stars().iter().zip(b.stars().iter()) {
    ///     assert_eq!((s1.x, s1.y), (s2.x, s2.y));
    /// }
    /// ```
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: ChaCha8Rng::seed_from_u64(seed),
            stars: Vec::new(),
            tick_count: 0,
            last_area: Rect::new(0, 0, 0, 0),
        }
    }

    /// Read-only view of the current star set.
    pub fn stars(&self) -> &[Star] {
        &self.stars
    }

    /// Advance the animation by one tick.
    ///
    /// - If `area` differs from the last call, regenerates the entire
    ///   starfield to fit the new dimensions.
    /// - Advances [`Star::phase`] on every existing star by its [`Star::speed`].
    /// - If [`AnimationConfig::density`] changed (or area scaled), spawns or
    ///   pops stars to match the new target count.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::Rect;
    /// use sid_core::animation::AnimationConfig;
    /// use sid_fx::FxState;
    ///
    /// let mut state = FxState::with_seed(7);
    /// let area = Rect::new(0, 0, 80, 24);
    /// let cfg = AnimationConfig::default();
    /// state.tick(area, &cfg);
    /// assert_eq!(state.tick_count, 1);
    /// ```
    pub fn tick(&mut self, area: Rect, cfg: &AnimationConfig) {
        // 1. Detect area change → regenerate from scratch.
        if area != self.last_area {
            self.stars.clear();
            let target = star_count_for(area, cfg);
            for _ in 0..target {
                let s = self.spawn_star(area, cfg);
                self.stars.push(s);
            }
            self.last_area = area;
        }

        // 2. Advance phase on each existing star.
        for s in self.stars.iter_mut() {
            s.phase = s.phase.wrapping_add(s.speed);
        }

        // 3. Rebalance to match (possibly-updated) density.
        let target = star_count_for(area, cfg);
        while self.stars.len() < target {
            let s = self.spawn_star(area, cfg);
            self.stars.push(s);
        }
        while self.stars.len() > target {
            self.stars.pop();
        }

        self.tick_count = self.tick_count.wrapping_add(1);
    }

    /// Spawn a single star at a random position within `area`.
    ///
    /// Returns the star. Caller pushes it onto `self.stars`.
    fn spawn_star(&mut self, area: Rect, cfg: &AnimationConfig) -> Star {
        // Empty area → produce a no-op star at the origin. `star_count_for`
        // returns 0 for empty rects so spawn shouldn't normally be reached
        // with width=0 or height=0, but be defensive.
        let x = if area.width == 0 {
            0
        } else {
            area.x + self.rng.random_range(0..area.width)
        };
        let y = if area.height == 0 {
            0
        } else {
            area.y + self.rng.random_range(0..area.height)
        };
        // Most stars are Dot; ~15% Accent; ~10% Bright. Tunable.
        let roll: u8 = self.rng.random();
        let kind = if roll < 26 {
            StarKind::Bright
        } else if roll < 64 {
            StarKind::Accent
        } else {
            StarKind::Dot
        };
        // Base brightness varies by kind. Brighter classes also start brighter.
        let base = match kind {
            StarKind::Dot => self.rng.random_range(70..=130),
            StarKind::Accent => self.rng.random_range(120..=200),
            StarKind::Bright => self.rng.random_range(170..=230),
        };
        // Phase offset gives each star a unique twinkle starting point.
        let phase = self.rng.random();
        // Speed 1..=8 — lower = slower twinkle. Bright stars twinkle slower.
        let speed = match kind {
            StarKind::Bright => self.rng.random_range(1..=3),
            StarKind::Accent => self.rng.random_range(2..=5),
            StarKind::Dot => self.rng.random_range(3..=8),
        };
        // cfg is reserved for spawn-time use (glyph variation, brightness
        // ranges per glyph_set). Currently only star count consumes it.
        let _ = cfg;
        Star {
            x,
            y,
            base,
            phase,
            speed,
            kind,
        }
    }
}

impl Default for FxState {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the target star count from density and area.
///
/// Baseline: an 80×24 terminal at `density=30` yields ~30 stars. Larger
/// terminals scale linearly with cell count. Hard-capped at 1000 stars so
/// pathologically huge terminals don't burn CPU.
///
/// `cfg.enabled == false` always returns 0.
///
/// # Examples
///
/// ```
/// use ratatui::layout::Rect;
/// use sid_core::animation::AnimationConfig;
/// use sid_fx::star_count_for;
///
/// let cfg = AnimationConfig::default();
/// // 80×24 with default density 30 → 30 stars.
/// assert_eq!(star_count_for(Rect::new(0, 0, 80, 24), &cfg), 30);
/// // Disabled → 0 stars regardless of density.
/// let off = AnimationConfig { enabled: false, ..AnimationConfig::default() };
/// assert_eq!(star_count_for(Rect::new(0, 0, 80, 24), &off), 0);
/// ```
pub fn star_count_for(area: Rect, cfg: &AnimationConfig) -> usize {
    if !cfg.enabled {
        return 0;
    }
    let cells = (area.width as usize) * (area.height as usize);
    if cells == 0 {
        return 0;
    }
    let baseline_cells = 80usize * 24usize;
    let scaled = (cfg.density as usize) * cells / baseline_cells;
    scaled.min(1000)
}

/// Render the starfield into `buf`, restricted to `area`.
///
/// Stars outside `area` (e.g. left over from a larger previous render) are
/// silently skipped — the area-change branch in [`FxState::tick`] is the
/// canonical path that prevents that, but the bounds check protects against
/// callers passing a sub-rect of the state's known area.
///
/// When `cfg.enabled == false`, this function is a no-op and the buffer is
/// left untouched.
///
/// # Examples
///
/// ```
/// use ratatui::buffer::Buffer;
/// use ratatui::layout::Rect;
/// use sid_core::animation::AnimationConfig;
/// use sid_fx::{FxState, render_starfield};
/// use sid_ui::themes::cosmos;
///
/// let area = Rect::new(0, 0, 40, 10);
/// let mut buf = Buffer::empty(area);
/// let mut state = FxState::with_seed(42);
/// let cfg = AnimationConfig::default();
/// let theme = cosmos();
/// state.tick(area, &cfg);
/// render_starfield(&mut buf, area, &state, &cfg, &theme);
/// ```
pub fn render_starfield(
    buf: &mut Buffer,
    area: Rect,
    state: &FxState,
    cfg: &AnimationConfig,
    theme: &Theme,
) {
    if !cfg.enabled {
        return;
    }
    for star in &state.stars {
        let pos = Position {
            x: star.x,
            y: star.y,
        };
        if !area.contains(pos) {
            continue;
        }
        let glyph = glyph_for(star.kind, cfg.glyph_set);
        let brightness = compute_brightness(star.base, star.phase);
        let color = color_for(star.kind, brightness, theme);
        let cell = &mut buf[pos];
        cell.set_char(glyph);
        cell.set_style(Style::default().fg(color));
    }
}

/// Pick the glyph for a star given its kind and the active palette.
///
/// # Examples
///
/// ```
/// use sid_core::animation::GlyphSet;
/// use sid_fx::{StarKind, glyph_for};
///
/// assert_eq!(glyph_for(StarKind::Dot, GlyphSet::Cosmos), '·');
/// assert_eq!(glyph_for(StarKind::Accent, GlyphSet::Cosmos), '✦');
/// assert_eq!(glyph_for(StarKind::Bright, GlyphSet::Cosmos), '*');
/// assert_eq!(glyph_for(StarKind::Accent, GlyphSet::Minimal), '·');
/// assert_eq!(glyph_for(StarKind::Bright, GlyphSet::Ascii), '.');
/// ```
pub fn glyph_for(kind: StarKind, glyph_set: GlyphSet) -> char {
    match (glyph_set, kind) {
        (GlyphSet::Cosmos, StarKind::Dot) => '·',
        (GlyphSet::Cosmos, StarKind::Accent) => '✦',
        (GlyphSet::Cosmos, StarKind::Bright) => '*',
        (GlyphSet::Minimal, StarKind::Bright) => '*',
        (GlyphSet::Minimal, _) => '·',
        (GlyphSet::Ascii, _) => '.',
    }
}

/// Compute the displayed brightness of a star for the current frame.
///
/// A triangle wave on `phase`: the brightness floor is `base`; it ramps up
/// to `255` at `phase == 128` and back down at `phase == 0 || 255`. The
/// triangle is cheaper than `sin` and integer-only, so renders deterministically
/// without depending on platform `libm`.
///
/// Returned value is clamped to `base..=255`.
///
/// # Examples
///
/// ```
/// use sid_fx::compute_brightness;
///
/// // Peak brightness at phase = 128.
/// assert_eq!(compute_brightness(100, 128), 255);
/// // Trough at phase = 0 returns base.
/// assert_eq!(compute_brightness(100, 0), 100);
/// // Trough at phase = 255 also returns near base.
/// let dim = compute_brightness(100, 255);
/// assert!(dim <= 110, "expected near base, got {dim}");
/// ```
pub fn compute_brightness(base: u8, phase: u8) -> u8 {
    // tri = how close phase is to 128, in 0..=128.
    //   phase=0    → tri=0
    //   phase=128  → tri=128
    //   phase=255  → tri=1
    let tri = if phase <= 128 {
        phase
    } else {
        // 129..=255 maps to 127..=1, capped at 128 to match peak symmetry.
        255u8.saturating_sub(phase).saturating_add(1).min(128)
    };
    // Lerp from base to 255 by tri/128.
    let span = 255u16.saturating_sub(base as u16);
    let bump = span * (tri as u16) / 128;
    base.saturating_add(bump as u8)
}

/// Pick the foreground colour for a star given its brightness and the theme.
///
/// Linear blend on each channel:
///
/// - `StarKind::Dot` lerps `theme.muted` → `theme.foreground`.
/// - `StarKind::Accent` lerps `theme.muted` → `theme.accent_primary`.
/// - `StarKind::Bright` stays at `theme.foreground` (already at peak).
///
/// Brightness `0` → start colour. Brightness `255` → end colour.
///
/// # Examples
///
/// ```
/// use ratatui::style::Color as RatColor;
/// use sid_fx::{StarKind, color_for};
/// use sid_ui::themes::cosmos;
///
/// let theme = cosmos();
/// // Dim Dot star → muted-ish.
/// let dim = color_for(StarKind::Dot, 0, &theme);
/// assert!(matches!(dim, RatColor::Rgb(_, _, _)));
/// ```
pub fn color_for(kind: StarKind, brightness: u8, theme: &Theme) -> Color {
    let (start, end) = match kind {
        StarKind::Dot => (theme.muted, theme.foreground),
        StarKind::Accent => (theme.muted, theme.accent_primary),
        StarKind::Bright => (theme.foreground, theme.foreground),
    };
    let (r, g, b) = lerp_rgb(
        (start.r, start.g, start.b),
        (end.r, end.g, end.b),
        brightness,
    );
    Color::Rgb(r, g, b)
}

/// Linear interpolation between two RGB triples by `t / 255`.
fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: u8) -> (u8, u8, u8) {
    let mix = |x: u8, y: u8| -> u8 {
        let xi = x as i32;
        let yi = y as i32;
        let ti = t as i32;
        (xi + (yi - xi) * ti / 255).clamp(0, 255) as u8
    };
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_ui::themes::cosmos;

    #[test]
    fn brightness_is_monotone_toward_peak() {
        let a = compute_brightness(100, 0);
        let b = compute_brightness(100, 64);
        let c = compute_brightness(100, 128);
        assert!(a <= b);
        assert!(b <= c);
        assert_eq!(c, 255);
    }

    #[test]
    fn brightness_floor_is_base() {
        assert_eq!(compute_brightness(100, 0), 100);
        assert_eq!(compute_brightness(50, 0), 50);
        assert_eq!(compute_brightness(200, 0), 200);
    }

    #[test]
    fn star_count_for_baseline() {
        let cfg = AnimationConfig::default();
        // Default density 30 at 80×24 → exactly 30.
        assert_eq!(star_count_for(Rect::new(0, 0, 80, 24), &cfg), 30);
    }

    #[test]
    fn star_count_for_zero_density_yields_zero() {
        let cfg = AnimationConfig {
            density: 0,
            ..AnimationConfig::default()
        };
        assert_eq!(star_count_for(Rect::new(0, 0, 80, 24), &cfg), 0);
    }

    #[test]
    fn star_count_for_zero_area_yields_zero() {
        let cfg = AnimationConfig::default();
        assert_eq!(star_count_for(Rect::new(0, 0, 0, 0), &cfg), 0);
        assert_eq!(star_count_for(Rect::new(0, 0, 80, 0), &cfg), 0);
        assert_eq!(star_count_for(Rect::new(0, 0, 0, 24), &cfg), 0);
    }

    #[test]
    fn star_count_for_disabled_yields_zero() {
        let cfg = AnimationConfig {
            enabled: false,
            ..AnimationConfig::default()
        };
        assert_eq!(star_count_for(Rect::new(0, 0, 80, 24), &cfg), 0);
    }

    #[test]
    fn star_count_for_caps_at_1000() {
        let cfg = AnimationConfig {
            density: 100,
            ..AnimationConfig::default()
        };
        // Pathologically huge terminal — 1000 cap kicks in.
        let n = star_count_for(Rect::new(0, 0, 1000, 1000), &cfg);
        assert!(n <= 1000);
    }

    #[test]
    fn glyph_for_cosmos_palette() {
        assert_eq!(glyph_for(StarKind::Dot, GlyphSet::Cosmos), '·');
        assert_eq!(glyph_for(StarKind::Accent, GlyphSet::Cosmos), '✦');
        assert_eq!(glyph_for(StarKind::Bright, GlyphSet::Cosmos), '*');
    }

    #[test]
    fn glyph_for_minimal_palette() {
        assert_eq!(glyph_for(StarKind::Dot, GlyphSet::Minimal), '·');
        assert_eq!(glyph_for(StarKind::Accent, GlyphSet::Minimal), '·');
        assert_eq!(glyph_for(StarKind::Bright, GlyphSet::Minimal), '*');
    }

    #[test]
    fn glyph_for_ascii_palette() {
        assert_eq!(glyph_for(StarKind::Dot, GlyphSet::Ascii), '.');
        assert_eq!(glyph_for(StarKind::Accent, GlyphSet::Ascii), '.');
        assert_eq!(glyph_for(StarKind::Bright, GlyphSet::Ascii), '.');
    }

    #[test]
    fn lerp_endpoints() {
        let a = (10, 20, 30);
        let b = (200, 210, 220);
        assert_eq!(lerp_rgb(a, b, 0), a);
        assert_eq!(lerp_rgb(a, b, 255), b);
    }

    #[test]
    fn color_for_returns_rgb() {
        let theme = cosmos();
        let c = color_for(StarKind::Accent, 200, &theme);
        assert!(matches!(c, Color::Rgb(_, _, _)));
    }

    #[test]
    fn new_default_does_not_panic() {
        let _ = FxState::new();
        let _ = FxState::default();
    }
}
