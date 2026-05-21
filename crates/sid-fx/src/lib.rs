//! Animated cosmic background renderer for sid.
//!
//! Phase 6.1 of the UX overhaul (see
//! `docs/superpowers/plans/2026-05-22-sid-ux-overhaul.md`) — starfield. Phase
//! 6.2 added the supernovae layer: brief, deterministic blooms that fire on
//! idle and on widget-triggered celebration events.
//!
//! # Surface
//!
//! - [`FxState`] holds the live starfield and supernova queue (positions,
//!   phases, RNG state, idle counter).
//! - [`FxState::tick`] advances animation by one frame, rebalances the star
//!   count to match the configured density / area, ages live supernovae, and
//!   spawns idle supernovae when the configured cadence is reached.
//! - [`FxState::trigger_supernova`] queues a celebration bloom on demand.
//! - [`render_starfield`] paints stars into a ratatui `Buffer`.
//! - [`render_supernovae`] paints live supernovae into the same buffer. The
//!   parent calls them in order: stars first, supernovae second, widget body
//!   third — so widgets always win the cell.
//!
//! # Determinism
//!
//! The renderer uses [`rand_chacha::ChaCha8Rng`] under the hood so that
//! [`FxState::with_seed`] produces byte-identical starfields and supernova
//! queues across machines, Rust versions, and `cargo` invocations. Production
//! code uses [`FxState::new`] which seeds from `rand::random()`; tests pin the
//! seed.
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

use std::collections::VecDeque;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use sid_core::animation::{AnimationConfig, GlyphSet};
use sid_ui::theme::{Color as UiColor, Theme};

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

/// Live starfield + supernova state. Mutated each tick, read each frame.
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
    /// Live supernovae. Ordered by spawn time (oldest first). Drained as each
    /// supernova reaches [`SUPERNOVA_LIFETIME_FRAMES`].
    pub supernovae: VecDeque<Supernova>,
    /// Frames elapsed since the last idle supernova (or since construction).
    /// Reset to `0` each time the idle cadence fires.
    pub frames_since_last_idle: u32,
    /// Cumulative count of supernovae ever spawned (idle + triggered). Never
    /// decremented; useful for tests asserting spawn cadence without racing
    /// against the queue's drop-on-age behaviour.
    pub total_supernovae_spawned: u64,
}

/// A live supernova animation tracked by [`FxState`].
///
/// Each entry is a brief bloom anchored at [`Supernova::center`]. Its
/// [`Supernova::age`] advances by one each [`FxState::tick`], and the entry
/// is dropped from the queue once `age >= SUPERNOVA_LIFETIME_FRAMES`.
///
/// # Examples
///
/// ```
/// use sid_fx::{Supernova, SupernovaPalette};
///
/// let sn = Supernova {
///     center: (10, 5),
///     age: 0,
///     palette: SupernovaPalette::Cosmos,
/// };
/// assert_eq!(sn.age, 0);
/// assert_eq!(sn.palette, SupernovaPalette::Cosmos);
/// ```
#[derive(Debug, Clone)]
pub struct Supernova {
    /// Anchor cell. Glyph cluster paints around this point.
    pub center: (u16, u16),
    /// Frames the supernova has been alive. Capped at [`SUPERNOVA_LIFETIME_FRAMES`].
    pub age: u8,
    /// Colour palette. Chosen at spawn time by the trigger source.
    pub palette: SupernovaPalette,
}

/// Colour palette for one supernova. Different palettes are tied to the
/// trigger (idle ambient = [`SupernovaPalette::Cosmos`], success celebration
/// = [`SupernovaPalette::Celebrate`], warning/info = [`SupernovaPalette::Warm`]).
///
/// # Examples
///
/// ```
/// use sid_fx::SupernovaPalette;
///
/// assert_ne!(SupernovaPalette::Cosmos, SupernovaPalette::Celebrate);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupernovaPalette {
    /// Pink-red center fading to warm cream. Used for idle blooms and most
    /// general-purpose events.
    Cosmos,
    /// Soft galaxy green fading toward foreground white. Reserved for success
    /// events (commit landed, connection established, kill confirmed).
    Celebrate,
    /// Warm amber fading toward foreground. Reserved for warning / info
    /// notifications that still deserve a visual ping.
    Warm,
}

/// Lifetime of a supernova in frames.
///
/// At the default `fps = 8`, a supernova with this lifetime renders for
/// roughly `6 / 8 = 750 ms`. Each rendered frame multiplies cell brightness by
/// `(1 - age / LIFETIME)`, so the visible bloom decays linearly from full
/// brightness at `age = 0` to fully transparent at `age = LIFETIME`.
///
/// # Examples
///
/// ```
/// use sid_fx::SUPERNOVA_LIFETIME_FRAMES;
///
/// assert_eq!(SUPERNOVA_LIFETIME_FRAMES, 6);
/// ```
pub const SUPERNOVA_LIFETIME_FRAMES: u8 = 6;

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
            supernovae: VecDeque::new(),
            frames_since_last_idle: 0,
            total_supernovae_spawned: 0,
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

        // 4. Idle supernova cadence. The runtime ticks at `cfg.fps` frames per
        //    second, so `frames_since_last_idle / max(fps, 1)` is the number
        //    of seconds elapsed since the last idle bloom. When the master
        //    switch is off OR the user disabled idle blooms with
        //    `supernova_idle_secs = 0`, we skip the spawn entirely (but still
        //    advance the counter so re-enabling resumes from a known state).
        self.frames_since_last_idle = self.frames_since_last_idle.saturating_add(1);
        if cfg.enabled && cfg.supernova_idle_secs > 0 {
            let fps = cfg.fps.max(1) as u32;
            let elapsed_secs = self.frames_since_last_idle / fps;
            if elapsed_secs >= cfg.supernova_idle_secs {
                self.trigger_supernova(area, SupernovaPalette::Cosmos);
                self.frames_since_last_idle = 0;
            }
        }

        // 5. Age every live supernova and drop ones past their lifetime.
        //    `retain_mut` is stable since 1.61 and avoids a second pass.
        self.supernovae.retain_mut(|sn| {
            sn.age = sn.age.saturating_add(1).min(SUPERNOVA_LIFETIME_FRAMES);
            sn.age < SUPERNOVA_LIFETIME_FRAMES
        });

        self.tick_count = self.tick_count.wrapping_add(1);
    }

    /// Spawn one supernova at a random off-content cell within `area`.
    ///
    /// The center is chosen by [`Self::rng`] so two `FxState`s sharing a seed
    /// emit identical supernova queues under identical tick sequences.
    ///
    /// To reduce visual collision with selected list rows in widget bodies —
    /// which conventionally land on even-y cells — the spawn picks an odd-y
    /// row. When the area has no odd-y row available (height `0` or `1`), the
    /// center clamps to the top-left so the queue still receives an entry;
    /// the brightness fade renders the off-screen bloom as a no-op for cells
    /// outside `area`.
    ///
    /// This method is idempotent in the sense that every call appends exactly
    /// one entry to `self.supernovae` regardless of state; it does **not**
    /// deduplicate by location.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::Rect;
    /// use sid_fx::{FxState, SupernovaPalette};
    ///
    /// let mut state = FxState::with_seed(7);
    /// state.trigger_supernova(Rect::new(0, 0, 40, 10), SupernovaPalette::Celebrate);
    /// assert_eq!(state.supernovae.len(), 1);
    /// assert_eq!(state.supernovae[0].palette, SupernovaPalette::Celebrate);
    /// ```
    pub fn trigger_supernova(&mut self, area: Rect, palette: SupernovaPalette) {
        let x = if area.width == 0 {
            area.x
        } else {
            area.x + self.rng.random_range(0..area.width)
        };
        // Pick an odd-y row to dodge the typical even-y selected list row.
        // Height 0..=1 has no odd-y candidate; clamp to the top.
        let y = if area.height < 2 {
            area.y
        } else {
            // Range of odd rows: 1, 3, 5, ... up to `height - 1` (inclusive
            // when height is even) or `height - 2` (inclusive when height is
            // odd). Compute the count, pick one, then convert back to y.
            let odd_rows = area.height / 2;
            let pick = self.rng.random_range(0..odd_rows);
            area.y + (pick * 2 + 1)
        };
        self.supernovae.push_back(Supernova {
            center: (x, y),
            age: 0,
            palette,
        });
        self.total_supernovae_spawned = self.total_supernovae_spawned.saturating_add(1);
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

/// Convert a `sid_ui::theme::Color` into a raw `(r, g, b)` tuple for
/// [`lerp_rgb`].
#[inline]
fn ui_rgb(c: UiColor) -> (u8, u8, u8) {
    (c.r, c.g, c.b)
}

/// Sparse 11-cell glyph cluster painted around a supernova center.
///
/// Each entry is `(dx, dy, intensity)` where `intensity` is the cell's
/// brightness weight at `age = 0`. The actual on-screen brightness is
/// `intensity * (1 - age / LIFETIME)`, so older blooms fade uniformly.
///
/// Shape (`.` = dot at intensity ~100, `+` = accent ~200, `*` = peak ~255,
/// space = empty cell):
///
/// ```text
///     . + .
///   + * + * +
///     . + .
/// ```
///
/// The outer-row dots and the side accents on the middle row create the
/// "bloom with rays" silhouette without overlapping the widget's selected
/// row (the center sits on an odd-y cell; the rays land on even-y rows
/// which are typically widget chrome, not selected content).
const SUPERNOVA_PATTERN: &[(i16, i16, u8)] = &[
    // Top arm: 1 accent + 2 dots.
    (-1, -1, 100),
    (0, -1, 200),
    (1, -1, 100),
    // Middle row: 5 cells. Center is peak; outer accents fan the bloom.
    (-2, 0, 180),
    (-1, 0, 230),
    (0, 0, 255),
    (1, 0, 230),
    (2, 0, 180),
    // Bottom arm: mirror of top.
    (-1, 1, 100),
    (0, 1, 200),
    (1, 1, 100),
];

/// Render every live supernova into `buf`, restricted to `area`.
///
/// Pattern paints around [`Supernova::center`] using [`SUPERNOVA_PATTERN`];
/// each cell's brightness multiplies by `1 - age / SUPERNOVA_LIFETIME_FRAMES`
/// so the bloom fades smoothly over its 6-frame lifetime. The displayed
/// colour interpolates between two anchors from the active [`Theme`] based on
/// the supernova's [`SupernovaPalette`]:
///
/// | Palette | Age 0 anchor | Age `LIFETIME` anchor |
/// |---|---|---|
/// | [`SupernovaPalette::Cosmos`]    | `accent_primary` | `accent_warning` |
/// | [`SupernovaPalette::Celebrate`] | `accent_success` | `foreground` |
/// | [`SupernovaPalette::Warm`]      | `accent_warning` | `foreground` |
///
/// Cells outside `area` are skipped silently. When `cfg.enabled == false`
/// this function is a no-op and the buffer is left untouched.
///
/// # Examples
///
/// ```
/// use ratatui::buffer::Buffer;
/// use ratatui::layout::Rect;
/// use sid_core::animation::AnimationConfig;
/// use sid_fx::{FxState, SupernovaPalette, render_supernovae};
/// use sid_ui::themes::cosmos;
///
/// let area = Rect::new(0, 0, 40, 10);
/// let mut state = FxState::with_seed(42);
/// state.trigger_supernova(area, SupernovaPalette::Cosmos);
///
/// let cfg = AnimationConfig::default();
/// let theme = cosmos();
/// let mut buf = Buffer::empty(area);
/// render_supernovae(&mut buf, area, &state, &cfg, &theme);
/// ```
pub fn render_supernovae(
    buf: &mut Buffer,
    area: Rect,
    state: &FxState,
    cfg: &AnimationConfig,
    theme: &Theme,
) {
    if !cfg.enabled {
        return;
    }
    for sn in &state.supernovae {
        // Fade ramp in 0..=255. age=0 → 255 (full brightness);
        // age=LIFETIME → 0 (invisible — but we drop before rendering, so the
        // boundary case never actually paints).
        let age_clamped = sn.age.min(SUPERNOVA_LIFETIME_FRAMES);
        let life = SUPERNOVA_LIFETIME_FRAMES as u16;
        let fade_in_255 =
            ((life.saturating_sub(age_clamped as u16)) * 255 / life.max(1)).min(255) as u8;
        let palette_t = ((age_clamped as u16) * 255 / life.max(1)).min(255) as u8;

        let (start, end) = palette_anchors(sn.palette, theme);
        let bloom = lerp_rgb(ui_rgb(start), ui_rgb(end), palette_t);
        let bg = ui_rgb(theme.background);
        let glyph_accent = glyph_for(StarKind::Accent, cfg.glyph_set);
        let glyph_dot = glyph_for(StarKind::Dot, cfg.glyph_set);
        let glyph_bright = glyph_for(StarKind::Bright, cfg.glyph_set);

        for (dx, dy, intensity) in SUPERNOVA_PATTERN.iter().copied() {
            // i32 math keeps the bounds check clean for all u16 area sizes.
            let cx = sn.center.0 as i32 + dx as i32;
            let cy = sn.center.1 as i32 + dy as i32;
            if cx < 0 || cy < 0 {
                continue;
            }
            let pos = Position {
                x: cx as u16,
                y: cy as u16,
            };
            if !area.contains(pos) {
                continue;
            }
            // Cell brightness = pattern_intensity × fade. u16 math avoids u8
            // overflow; final value saturates at 255.
            let cell_weight = ((intensity as u16) * (fade_in_255 as u16) / 255).min(255) as u8;
            // Lerp the palette colour against the theme background by
            // `cell_weight`. Lower weight → closer to background (the bloom
            // dissolves into the void).
            let (r, g, b) = lerp_rgb(bg, bloom, cell_weight);
            let glyph = match intensity {
                255 => glyph_bright,
                180..=254 => glyph_accent,
                _ => glyph_dot,
            };
            let cell = &mut buf[pos];
            cell.set_char(glyph);
            cell.set_style(Style::default().fg(Color::Rgb(r, g, b)));
        }
    }
}

/// Pick the `(start, end)` theme colours for a supernova palette.
///
/// `start` is the colour at age `0`; `end` is the colour at age
/// `SUPERNOVA_LIFETIME_FRAMES`. The renderer interpolates between them by
/// `age / LIFETIME`.
fn palette_anchors(palette: SupernovaPalette, theme: &Theme) -> (UiColor, UiColor) {
    match palette {
        SupernovaPalette::Cosmos => (theme.accent_primary, theme.accent_warning),
        SupernovaPalette::Celebrate => (theme.accent_success, theme.foreground),
        SupernovaPalette::Warm => (theme.accent_warning, theme.foreground),
    }
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

    #[test]
    fn supernova_lifetime_constant_matches_spec() {
        // 6 frames @ 8 FPS ≈ 750 ms. Spec calls for "5-frame animation" so
        // 6 covers the bloom + final fade-to-empty without leaving cells lit.
        assert_eq!(SUPERNOVA_LIFETIME_FRAMES, 6);
    }

    #[test]
    fn palette_anchors_cosmos_uses_warning_endpoint() {
        let theme = cosmos();
        let (start, end) = palette_anchors(SupernovaPalette::Cosmos, &theme);
        assert_eq!(start, theme.accent_primary);
        assert_eq!(end, theme.accent_warning);
    }

    #[test]
    fn palette_anchors_celebrate_lerps_success_to_foreground() {
        let theme = cosmos();
        let (start, end) = palette_anchors(SupernovaPalette::Celebrate, &theme);
        assert_eq!(start, theme.accent_success);
        assert_eq!(end, theme.foreground);
    }

    #[test]
    fn palette_anchors_warm_lerps_warning_to_foreground() {
        let theme = cosmos();
        let (start, end) = palette_anchors(SupernovaPalette::Warm, &theme);
        assert_eq!(start, theme.accent_warning);
        assert_eq!(end, theme.foreground);
    }

    #[test]
    fn trigger_supernova_picks_odd_y() {
        // With height>=2 the spawn always lands on an odd-y row, avoiding the
        // even-y selected rows widgets typically use.
        let mut state = FxState::with_seed(42);
        for _ in 0..50 {
            state.trigger_supernova(Rect::new(0, 0, 40, 10), SupernovaPalette::Cosmos);
        }
        for sn in &state.supernovae {
            assert_eq!(sn.center.1 % 2, 1, "y={} must be odd", sn.center.1);
        }
    }

    #[test]
    fn trigger_supernova_clamps_to_origin_in_tiny_area() {
        // Width 0 → x clamps to area.x. Height 0..=1 → y clamps to area.y.
        let mut state = FxState::with_seed(1);
        state.trigger_supernova(Rect::new(5, 7, 0, 0), SupernovaPalette::Cosmos);
        assert_eq!(state.supernovae[0].center, (5, 7));
        state.trigger_supernova(Rect::new(2, 4, 6, 1), SupernovaPalette::Cosmos);
        assert_eq!(state.supernovae[1].center.1, 4);
    }
}
