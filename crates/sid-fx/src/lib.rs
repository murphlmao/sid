//! Animated cosmic background renderer for sid.
//!
//! [`FxState`] holds the live starfield and supernova queue (positions,
//! phases, RNG state, idle counter). [`FxState::tick`] advances animation by
//! one frame, rebalances star count to match configured density / area, ages
//! live supernovae, and spawns idle supernovae when the configured cadence is
//! reached. [`FxState::trigger_supernova`] queues a celebration bloom on
//! demand.
//!
//! [`render_starfield`] paints stars into ratatui [`Buffer`].
//! [`render_supernovae`] paints live supernovae into the same buffer.
//! [`render_shooting_stars`] paints brief bright streaks (Cosmos mode only).
//!
//! Stars, supernovae, and shooting stars are painted lowest to highest — a
//! caller stacking multiple layers should paint stars first, supernovae
//! second, shooting stars third — widgets always win the cell.
//!
//! The renderer uses [`rand_chacha::ChaCha8Rng`] seeded via
//! [`FxState::with_seed`] for byte-identical starfields across runs.
//! [`FxState::new`] seeds from `rand::random()`.
//!
//! # Fixed-point position convention
//!
//! Every star carries a sub-cell position in **Q8.8 fixed point**:
//! `xq` and `yq` are 32-bit integers where the integer part is in bits
//! `[31:8]` and the fractional part is in bits `[7:0]`. One unit in `xq`/`yq`
//! equals `1/256` of a terminal cell. The rendered column/row is derived as
//! `x = (xq >> 8) as u16` (similarly for y). Velocity fields `vxq`/`vyq` use
//! the same unit (1/256 cell per tick), so a value of `16` means the star
//! drifts one cell every 16 ticks.
//!
//! Tick count wraps at `u64::MAX` (~10^11 years at 8 FPS).

use std::collections::VecDeque;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use sid_core::animation::{AnimationConfig, GlyphSet, MotionStyle};
use sid_ui::theme::{Color as UiColor, Theme};

/// A single star in the starfield.
///
/// Each star carries both a cell-resolution position (`x`, `y`) **and** a
/// Q8.8 fixed-point sub-cell position (`xq`, `yq`) that enables smooth drift
/// across ticks without stuttering. The cell position is always derived from
/// the fixed-point position: `x = (xq >> 8) as u16`, `y = (yq >> 8) as u16`.
///
/// In [`MotionStyle::Twinkle`] mode the velocity fields are ignored and the
/// position is static. In [`MotionStyle::Drift`] and
/// [`MotionStyle::Cosmos`] modes `vxq`/`vyq` are integrated each tick and
/// the star wraps at area edges.
#[derive(Debug, Clone)]
pub struct Star {
    /// Rendered column. Always `(xq >> 8) as u16`. Updated each tick in drift
    /// modes.
    pub x: u16,
    /// Rendered row. Always `(yq >> 8) as u16`. Updated each tick in drift
    /// modes.
    pub y: u16,
    /// Sub-cell X position in Q8.8 fixed point (1/256 cell per unit).
    /// Integer part: `xq >> 8`. Fractional part: `xq & 0xFF`.
    pub xq: i32,
    /// Sub-cell Y position in Q8.8 fixed point (1/256 cell per unit).
    pub yq: i32,
    /// X velocity in Q8.8 fixed-point units per tick (1/256 cell / tick).
    /// Positive = rightward drift; negative = leftward.
    pub vxq: i32,
    /// Y velocity in Q8.8 fixed-point units per tick (1/256 cell / tick).
    /// Positive = downward; negative = upward.
    pub vyq: i32,
    /// Base brightness floor, in `0..=255`. The star never dims below this.
    pub base: u8,
    /// Twinkle phase in `0..=255`. Advances by [`Star::speed`] each tick.
    pub phase: u8,
    /// Twinkle speed in `1..=8`. Multiplied by a boost factor in
    /// [`MotionStyle::Twinkle`] and [`MotionStyle::Cosmos`] for visible shimmer.
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

/// A transient shooting-star streak (Cosmos mode only).
///
/// Each shooting star moves fast across the field over a short lifetime and
/// then expires. [`render_shooting_stars`] paints a bright head and a short
/// dimming trail behind it.
///
/// # Examples
///
/// ```
/// use sid_fx::ShootingStar;
///
/// let ss = ShootingStar {
///     xq: 10 << 8,
///     yq: 5 << 8,
///     vxq: 512,
///     vyq: 256,
///     age: 0,
///     lifetime: 8,
/// };
/// assert!(ss.age < ss.lifetime);
/// ```
#[derive(Debug, Clone)]
pub struct ShootingStar {
    /// Head X position in Q8.8 fixed-point.
    pub xq: i32,
    /// Head Y position in Q8.8 fixed-point.
    pub yq: i32,
    /// X velocity in Q8.8 fixed-point units per tick. Fast: typically
    /// 256..=768 (1..=3 cells/tick).
    pub vxq: i32,
    /// Y velocity in Q8.8 fixed-point units per tick.
    pub vyq: i32,
    /// Frames alive.
    pub age: u16,
    /// Total frames before the streak expires.
    pub lifetime: u16,
}

/// Live starfield + supernova + shooting-star state. Mutated each tick, read
/// each frame.
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
    pub tick_count: u64,
    last_area: Rect,
    /// Live supernovae. Ordered by spawn time. Drained as each supernova
    /// reaches [`SUPERNOVA_LIFETIME_FRAMES`].
    pub supernovae: VecDeque<Supernova>,
    /// Frames elapsed since last idle supernova. Reset to `0` on each spawn.
    pub frames_since_last_idle: u32,
    /// Cumulative supernovae spawned. Never decremented.
    pub total_supernovae_spawned: u64,
    /// Live shooting stars (Cosmos mode only). Auto-expired after their
    /// lifetime elapses.
    pub shooting_stars: VecDeque<ShootingStar>,
}

/// A live supernova animation tracked by [`FxState`].
///
/// Each entry is a brief bloom anchored at [`Supernova::center`].
/// [`Supernova::age`] advances one per [`FxState::tick`]; the entry is
/// dropped once `age >= SUPERNOVA_LIFETIME_FRAMES`.
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

/// Phase boost multiplier applied in [`MotionStyle::Twinkle`] and
/// [`MotionStyle::Cosmos`] to make the brightness shimmer clearly visible.
///
/// At the default `fps=8` and base `speed` of 1..=8, a multiplier of 4 means
/// a `speed=1` star completes one full twinkle cycle in `256 / (1 * 4) = 64`
/// ticks = ~8 seconds; a `speed=8` star completes one in 8 ticks = ~1 second.
/// Without boosting, a `speed=1` star at 8 fps takes 32 seconds per cycle —
/// imperceptibly slow.
const TWINKLE_BOOST: u8 = 4;

/// Per-tick probability (out of 1000) of spawning a shooting star in
/// [`MotionStyle::Cosmos`] mode. At 8 fps, ~3/1000 per frame ≈ roughly one
/// every 40 seconds on average; perceived rate is higher when the terminal
/// is large (more star activity).
const SHOOTING_STAR_SPAWN_PROB_PER_MILLE: u32 = 4;

/// Lifetime in frames of a newly spawned shooting star. At 8 fps this is
/// approximately 1 second.
const SHOOTING_STAR_LIFETIME: u16 = 8;

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
            shooting_stars: VecDeque::new(),
        }
    }

    /// Read-only view of the current star set.
    pub fn stars(&self) -> &[Star] {
        &self.stars
    }

    /// Advance the animation by one tick.
    ///
    /// Behaviour depends on [`AnimationConfig::motion`]:
    ///
    /// - **[`MotionStyle::Twinkle`]** — positions are fixed; phase advances at
    ///   a boosted rate ([`TWINKLE_BOOST`]×) so the brightness shimmer is
    ///   clearly visible.
    /// - **[`MotionStyle::Drift`]** — each star's Q8.8 fixed-point position is
    ///   integrated by its velocity each tick. Stars wrap at area edges. Phase
    ///   advances at the base rate (mild twinkle).
    /// - **[`MotionStyle::Cosmos`]** — drift integration + boosted twinkle +
    ///   occasional shooting-star spawns + tighter idle supernova cadence.
    ///
    /// In all modes: if `area` differs from last call the entire starfield is
    /// regenerated; the star count is rebalanced to match the current density;
    /// live supernovae age by one frame.
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
            self.shooting_stars.clear();
            let target = star_count_for(area, cfg);
            for _ in 0..target {
                let s = self.spawn_star(area, cfg);
                self.stars.push(s);
            }
            self.last_area = area;
        }

        // 2. Per-motion update: phase and/or position.
        match cfg.motion {
            MotionStyle::Twinkle => {
                // Positions are fixed; advance phase with a visible boost.
                for s in self.stars.iter_mut() {
                    let step = s.speed.saturating_mul(TWINKLE_BOOST);
                    s.phase = s.phase.wrapping_add(step);
                }
            }
            MotionStyle::Drift => {
                // Integrate velocity; mild base-rate twinkle.
                for s in self.stars.iter_mut() {
                    s.xq += s.vxq;
                    s.yq += s.vyq;
                    wrap_star(s, area);
                    s.phase = s.phase.wrapping_add(s.speed);
                }
            }
            MotionStyle::Cosmos => {
                // Drift + boosted twinkle.
                for s in self.stars.iter_mut() {
                    s.xq += s.vxq;
                    s.yq += s.vyq;
                    wrap_star(s, area);
                    let step = s.speed.saturating_mul(TWINKLE_BOOST);
                    s.phase = s.phase.wrapping_add(step);
                }
                // Possibly spawn a shooting star this tick.
                let roll: u32 = self.rng.random_range(0..1000);
                if cfg.enabled && roll < SHOOTING_STAR_SPAWN_PROB_PER_MILLE {
                    if let Some(ss) = self.spawn_shooting_star(area) {
                        self.shooting_stars.push_back(ss);
                    }
                }
            }
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

        // 4. Age + expire shooting stars.
        self.shooting_stars.retain_mut(|ss| {
            ss.age = ss.age.saturating_add(1);
            ss.age < ss.lifetime
        });

        // 5. Idle supernova cadence. The runtime ticks at `cfg.fps` frames per
        //    second, so `frames_since_last_idle / max(fps, 1)` is the number
        //    of seconds elapsed since the last idle bloom. When the master
        //    switch is off OR the user disabled idle blooms with
        //    `supernova_idle_secs = 0`, we skip the spawn entirely (but still
        //    advance the counter so re-enabling resumes from a known state).
        //    In Cosmos mode we tighten the cadence so the bloom is actually
        //    seen — treat the configured idle period as a ceiling and cap the
        //    effective wait at 30 s (240 frames at default fps).
        self.frames_since_last_idle = self.frames_since_last_idle.saturating_add(1);
        if cfg.enabled && cfg.supernova_idle_secs > 0 {
            let fps = cfg.fps.max(1) as u32;
            let effective_idle_secs = if cfg.motion == MotionStyle::Cosmos {
                cfg.supernova_idle_secs.min(30)
            } else {
                cfg.supernova_idle_secs
            };
            let elapsed_secs = self.frames_since_last_idle / fps;
            if elapsed_secs >= effective_idle_secs {
                self.trigger_supernova(area, SupernovaPalette::Cosmos);
                self.frames_since_last_idle = 0;
            }
        }

        // 6. Age every live supernova and drop ones past their lifetime.
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
        // Widen the Dot floor from 70 to 30 to make the dim↔bright swing
        // perceptible even on Twinkle/Cosmos modes.
        let base = match kind {
            StarKind::Dot => self.rng.random_range(30..=100),
            StarKind::Accent => self.rng.random_range(100..=190),
            StarKind::Bright => self.rng.random_range(160..=220),
        };
        // Phase offset gives each star a unique twinkle starting point.
        let phase = self.rng.random();
        // Speed 1..=8 — lower = slower twinkle. Bright stars twinkle slower.
        let speed = match kind {
            StarKind::Bright => self.rng.random_range(1..=3),
            StarKind::Accent => self.rng.random_range(2..=5),
            StarKind::Dot => self.rng.random_range(3..=8),
        };

        // Q8.8 fixed-point initial position: integer part in high bits.
        let xq = (x as i32) << 8;
        let yq = (y as i32) << 8;

        // Velocity: Q8.8 units per tick. Brighter / rarer stars drift faster
        // (parallax depth illusion). Magnitude range: 4..=40 Q8.8 units/tick
        // (0.016..=0.156 cells/tick). Direction chosen uniformly per axis.
        // cfg controls motion mode but velocity is always assigned at spawn;
        // it is simply not integrated in Twinkle mode.
        let speed_scale: i32 = match kind {
            StarKind::Bright => self.rng.random_range(16..=40),
            StarKind::Accent => self.rng.random_range(8..=24),
            StarKind::Dot => self.rng.random_range(4..=16),
        };
        // Each axis independently picks a direction and applies a fractional
        // split so the resulting path is a gentle diagonal.
        let vx_sign: i32 = if self.rng.random::<bool>() { 1 } else { -1 };
        let vy_sign: i32 = if self.rng.random::<bool>() { 1 } else { -1 };
        // Bias: ~60% of speed along X, ~40% along Y to reflect terminal
        // aspect ratios (cells are ~2:1 tall:wide in pixels).
        let vxq = vx_sign * (speed_scale * 6 / 10).max(1);
        let vyq = vy_sign * (speed_scale * 4 / 10).max(1);

        // cfg is reserved for future spawn-time customisation (glyph
        // variation per glyph_set). Currently only star count consumes it.
        let _ = cfg;

        Star {
            x,
            y,
            xq,
            yq,
            vxq,
            vyq,
            base,
            phase,
            speed,
            kind,
        }
    }

    /// Spawn a single shooting star at a random edge of `area`.
    ///
    /// Returns `None` for empty areas. The shooting star is fired from a
    /// random point on the top or left edge and moves diagonally across the
    /// field.
    fn spawn_shooting_star(&mut self, area: Rect) -> Option<ShootingStar> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        // Pick a random starting edge (top=0, right=1, bottom=2, left=3).
        let edge: u8 = self.rng.random_range(0..4);
        let (start_x, start_y) = match edge {
            0 => {
                // Top edge — fire downward/sideways.
                let sx = area.x + self.rng.random_range(0..area.width);
                (sx, area.y)
            }
            1 => {
                // Right edge — fire leftward/slightly down.
                let sy = area.y + self.rng.random_range(0..area.height);
                (area.x + area.width - 1, sy)
            }
            2 => {
                // Bottom edge — fire upward/sideways.
                let sx = area.x + self.rng.random_range(0..area.width);
                (sx, area.y + area.height - 1)
            }
            _ => {
                // Left edge — fire rightward/slightly down.
                let sy = area.y + self.rng.random_range(0..area.height);
                (area.x, sy)
            }
        };
        // Speed: 192..=512 Q8.8 units/tick = 0.75..=2 cells/tick.
        // Direction: biased across the field so the streak is diagonal.
        let spd: i32 = self.rng.random_range(192..=512);
        let vxq = if self.rng.random::<bool>() { spd } else { -spd };
        let vy_frac: i32 = self.rng.random_range(64..=192);
        let vyq = if self.rng.random::<bool>() {
            vy_frac
        } else {
            -vy_frac
        };

        Some(ShootingStar {
            xq: (start_x as i32) << 8,
            yq: (start_y as i32) << 8,
            vxq,
            vyq,
            age: 0,
            lifetime: SHOOTING_STAR_LIFETIME,
        })
    }
}

impl Default for FxState {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrap a star's Q8.8 position so it stays within `area`.
///
/// When the derived cell coordinate (`xq >> 8`) falls outside the area, the
/// star wraps to the opposite edge (preserving the fractional part) so the
/// star count inside the area stays constant.
fn wrap_star(s: &mut Star, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let left = (area.x as i32) << 8;
    let right = ((area.x + area.width) as i32) << 8;
    let top = (area.y as i32) << 8;
    let bottom = ((area.y + area.height) as i32) << 8;
    let width_q = (area.width as i32) << 8;
    let height_q = (area.height as i32) << 8;

    // Wrap X.
    if s.xq < left {
        s.xq += width_q;
    } else if s.xq >= right {
        s.xq -= width_q;
    }
    // Wrap Y.
    if s.yq < top {
        s.yq += height_q;
    } else if s.yq >= bottom {
        s.yq -= height_q;
    }

    // Recompute cell coords from fixed-point.
    s.x = (s.xq >> 8) as u16;
    s.y = (s.yq >> 8) as u16;
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

/// Render every live shooting star into `buf`, restricted to `area`.
///
/// Only active in [`MotionStyle::Cosmos`] mode; this function is a no-op when
/// `cfg.motion != Cosmos` or `cfg.enabled == false`. Each shooting star is
/// painted as a bright head at its current Q8.8 position plus a short
/// dimming trail of up to 3 cells behind it (proportional to its remaining
/// lifetime). Trail cells are dimmer than the head.
///
/// Cells outside `area` are silently skipped.
///
/// # Examples
///
/// ```
/// use ratatui::buffer::Buffer;
/// use ratatui::layout::Rect;
/// use sid_core::animation::{AnimationConfig, MotionStyle};
/// use sid_fx::{FxState, render_shooting_stars};
/// use sid_ui::themes::cosmos;
///
/// let area = Rect::new(0, 0, 80, 24);
/// let mut state = FxState::with_seed(99);
/// let cfg = AnimationConfig { motion: MotionStyle::Cosmos, ..AnimationConfig::default() };
/// let theme = cosmos();
/// // Tick many frames so at least one shooting star can spawn.
/// for _ in 0..500 {
///     state.tick(area, &cfg);
/// }
/// let mut buf = Buffer::empty(area);
/// render_shooting_stars(&mut buf, area, &state, &cfg, &theme);
/// ```
pub fn render_shooting_stars(
    buf: &mut Buffer,
    area: Rect,
    state: &FxState,
    cfg: &AnimationConfig,
    theme: &Theme,
) {
    if !cfg.enabled || cfg.motion != MotionStyle::Cosmos {
        return;
    }
    for ss in &state.shooting_stars {
        let head_x = ss.xq >> 8;
        let head_y = ss.yq >> 8;

        // Brightness of the head: full at age=0, decays to ~50 by end.
        let life = ss.lifetime.max(1) as i32;
        let age = ss.age as i32;
        let head_brightness = ((255i32 * (life - age)) / life).clamp(50, 255) as u8;

        // Paint head.
        paint_shooting_cell(buf, area, head_x, head_y, head_brightness, theme);

        // Paint up to 3 trail cells stepping backward along the velocity
        // direction. Trail brightness falls off by 60 per step.
        let step_x = -(ss.vxq >> 8);
        let step_y = -(ss.vyq >> 8);
        for t in 1..=3i32 {
            let tx = head_x + step_x * t;
            let ty = head_y + step_y * t;
            let trail_brightness =
                (head_brightness as i32 - (t * 60)).clamp(0, 255) as u8;
            if trail_brightness == 0 {
                break;
            }
            paint_shooting_cell(buf, area, tx, ty, trail_brightness, theme);
        }
    }
}

/// Paint a single shooting-star cell (head or trail) into `buf`.
///
/// Skips silently when the cell is outside `area`.
fn paint_shooting_cell(
    buf: &mut Buffer,
    area: Rect,
    cx: i32,
    cy: i32,
    brightness: u8,
    theme: &Theme,
) {
    if cx < 0 || cy < 0 {
        return;
    }
    let pos = Position {
        x: cx as u16,
        y: cy as u16,
    };
    if !area.contains(pos) {
        return;
    }
    let bg = ui_rgb(theme.background);
    let fg = ui_rgb(theme.foreground);
    let (r, g, b) = lerp_rgb(bg, fg, brightness);
    let cell = &mut buf[pos];
    cell.set_char('-');
    cell.set_style(Style::default().fg(Color::Rgb(r, g, b)));
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
    use sid_core::animation::{AnimationConfig, MotionStyle};
    use sid_ui::themes::cosmos;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn default_area() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    fn cosmos_cfg() -> AnimationConfig {
        AnimationConfig::default() // motion = Cosmos
    }

    fn twinkle_cfg() -> AnimationConfig {
        AnimationConfig {
            motion: MotionStyle::Twinkle,
            ..AnimationConfig::default()
        }
    }

    fn drift_cfg() -> AnimationConfig {
        AnimationConfig {
            motion: MotionStyle::Drift,
            ..AnimationConfig::default()
        }
    }

    // ── existing tests (preserved) ────────────────────────────────────────────

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

    // ── tick_count still increments ──────────────────────────────────────────

    #[test]
    fn tick_count_increments_each_tick() {
        let mut state = FxState::with_seed(5);
        let area = default_area();
        let cfg = cosmos_cfg();
        for i in 1..=10u64 {
            state.tick(area, &cfg);
            assert_eq!(state.tick_count, i);
        }
    }

    // ── area-change regen ────────────────────────────────────────────────────

    #[test]
    fn area_change_regenerates_stars() {
        let mut state = FxState::with_seed(3);
        let a1 = Rect::new(0, 0, 40, 10);
        let a2 = Rect::new(0, 0, 80, 24);
        let cfg = cosmos_cfg();
        state.tick(a1, &cfg);
        let count_a1 = state.stars().len();
        state.tick(a2, &cfg);
        let count_a2 = state.stars().len();
        // Both areas should produce stars.
        assert!(count_a1 > 0);
        assert!(count_a2 > 0);
        // Larger area → more stars.
        assert!(count_a2 > count_a1);
    }

    // ── Twinkle: positions fixed, brightness changes ──────────────────────────

    #[test]
    fn twinkle_positions_fixed_across_many_ticks() {
        // In Twinkle mode xq/yq must not change across ticks.
        let mut state = FxState::with_seed(42);
        let area = default_area();
        let cfg = twinkle_cfg();
        state.tick(area, &cfg);
        // Snapshot positions after tick 1.
        let initial: Vec<(i32, i32)> = state.stars().iter().map(|s| (s.xq, s.yq)).collect();
        for _ in 0..50 {
            state.tick(area, &cfg);
        }
        let final_pos: Vec<(i32, i32)> = state.stars().iter().map(|s| (s.xq, s.yq)).collect();
        assert_eq!(
            initial, final_pos,
            "Twinkle mode must not move stars"
        );
    }

    #[test]
    fn twinkle_brightness_changes_across_ticks() {
        // Phase must advance in Twinkle mode so brightness visibly changes.
        let mut state = FxState::with_seed(77);
        let area = default_area();
        let cfg = twinkle_cfg();
        state.tick(area, &cfg);
        let phases_before: Vec<u8> = state.stars().iter().map(|s| s.phase).collect();
        state.tick(area, &cfg);
        let phases_after: Vec<u8> = state.stars().iter().map(|s| s.phase).collect();
        let changed = phases_before
            .iter()
            .zip(phases_after.iter())
            .any(|(a, b)| a != b);
        assert!(changed, "Twinkle must advance phase each tick");
    }

    // ── Boosted twinkle: wide brightness swing ───────────────────────────────

    #[test]
    fn boosted_twinkle_has_wide_brightness_swing() {
        // A star with base=30 (Dot floor) should swing from near 30 to 255
        // across a full phase cycle. Assert the swing is at least 100 wide.
        let base = 30u8;
        let mut min_b = 255u8;
        let mut max_b = 0u8;
        for phase in 0u8..=255 {
            let b = compute_brightness(base, phase);
            min_b = min_b.min(b);
            max_b = max_b.max(b);
        }
        let swing = max_b.saturating_sub(min_b);
        assert!(
            swing >= 100,
            "brightness swing should be >= 100, got {swing} (min={min_b} max={max_b})"
        );
    }

    // ── Drift: star count preserved and cells inside area ────────────────────

    #[test]
    fn drift_preserves_star_count_and_bounds() {
        // After many ticks every star's derived cell must stay inside area.
        let area = Rect::new(0, 0, 80, 24);
        let cfg = drift_cfg();
        let expected = star_count_for(area, &cfg);
        for seed in [0u64, 1, 7, 42, 999] {
            let mut state = FxState::with_seed(seed);
            state.tick(area, &cfg); // initial spawn
            for _ in 0..200 {
                state.tick(area, &cfg);
                assert_eq!(
                    state.stars().len(),
                    expected,
                    "seed={seed}: star count drifted"
                );
                for s in state.stars() {
                    let pos = Position { x: s.x, y: s.y };
                    assert!(
                        area.contains(pos),
                        "seed={seed}: star ({},{}) outside area {:?}",
                        s.x,
                        s.y,
                        area
                    );
                }
            }
        }
    }

    #[test]
    fn drift_is_deterministic() {
        // Two FxState instances with the same seed produce identical positions
        // after the same number of ticks.
        let area = default_area();
        let cfg = drift_cfg();
        let mut a = FxState::with_seed(7);
        let mut b = FxState::with_seed(7);
        for _ in 0..50 {
            a.tick(area, &cfg);
            b.tick(area, &cfg);
        }
        let pa: Vec<(u16, u16)> = a.stars().iter().map(|s| (s.x, s.y)).collect();
        let pb: Vec<(u16, u16)> = b.stars().iter().map(|s| (s.x, s.y)).collect();
        assert_eq!(pa, pb, "Drift must be deterministic across equal seeds");
    }

    #[test]
    fn drift_positions_actually_change() {
        // Stars should move, not stay frozen.
        let area = default_area();
        let cfg = drift_cfg();
        let mut state = FxState::with_seed(13);
        state.tick(area, &cfg);
        let before: Vec<(i32, i32)> = state.stars().iter().map(|s| (s.xq, s.yq)).collect();
        // Tick enough times that stars with the smallest velocity have moved.
        for _ in 0..20 {
            state.tick(area, &cfg);
        }
        let after: Vec<(i32, i32)> = state.stars().iter().map(|s| (s.xq, s.yq)).collect();
        // At least some (not all need to have the same velocity direction) stars
        // should have different xq or yq values.
        let moved = before.iter().zip(after.iter()).any(|(b, a)| b != a);
        assert!(moved, "Drift must actually move stars");
    }

    // ── Cosmos: shooting stars ───────────────────────────────────────────────

    #[test]
    fn cosmos_spawns_shooting_stars_within_bounded_ticks() {
        // With SHOOTING_STAR_SPAWN_PROB_PER_MILLE=4, P(no spawn in N ticks)
        // = (1 - 4/1000)^N. For N=2000 that's ~0.03%, so this should always
        // pass in practice.
        let area = default_area();
        let cfg = cosmos_cfg();
        let mut state = FxState::with_seed(42);
        state.tick(area, &cfg); // initial spawn
        let mut seen = false;
        for _ in 0..2000 {
            state.tick(area, &cfg);
            if !state.shooting_stars.is_empty() {
                seen = true;
                break;
            }
        }
        assert!(seen, "Cosmos must spawn at least one shooting star within 2000 ticks");
    }

    #[test]
    fn shooting_stars_expire_and_queue_does_not_grow_unbounded() {
        // After the shooting star lifetime has elapsed it must be dropped.
        // Run 2000 ticks; the queue must stay bounded (never > some small cap).
        let area = default_area();
        let cfg = cosmos_cfg();
        let mut state = FxState::with_seed(99);
        let max_expected = 20usize; // extremely conservative ceiling
        for _ in 0..2000 {
            state.tick(area, &cfg);
            assert!(
                state.shooting_stars.len() <= max_expected,
                "shooting star queue too large: {}",
                state.shooting_stars.len()
            );
        }
    }

    #[test]
    fn shooting_stars_not_spawned_in_twinkle_or_drift() {
        // Shooting stars are Cosmos-only.
        let area = default_area();
        for cfg in [twinkle_cfg(), drift_cfg()] {
            let mut state = FxState::with_seed(42);
            for _ in 0..2000 {
                state.tick(area, &cfg);
            }
            assert!(
                state.shooting_stars.is_empty(),
                "shooting stars must not appear in non-Cosmos mode"
            );
        }
    }

    // ── render fns: no panic on edge-case areas ───────────────────────────────

    fn assert_render_no_panic(area: Rect, cfg: &AnimationConfig) {
        let theme = cosmos();
        let mut state = FxState::with_seed(1);
        // Ensure state has been ticked at least once so there are stars.
        state.tick(area, cfg);
        let mut buf = Buffer::empty(if area.width == 0 || area.height == 0 {
            Rect::new(0, 0, 1, 1)
        } else {
            area
        });
        render_starfield(&mut buf, area, &state, cfg, &theme);
        render_supernovae(&mut buf, area, &state, cfg, &theme);
        render_shooting_stars(&mut buf, area, &state, cfg, &theme);
    }

    #[test]
    fn render_no_panic_zero_area() {
        for cfg in [cosmos_cfg(), twinkle_cfg(), drift_cfg()] {
            assert_render_no_panic(Rect::new(0, 0, 0, 0), &cfg);
            assert_render_no_panic(Rect::new(0, 0, 80, 0), &cfg);
            assert_render_no_panic(Rect::new(0, 0, 0, 24), &cfg);
        }
    }

    #[test]
    fn render_no_panic_one_by_one_area() {
        for cfg in [cosmos_cfg(), twinkle_cfg(), drift_cfg()] {
            assert_render_no_panic(Rect::new(0, 0, 1, 1), &cfg);
        }
    }

    #[test]
    fn render_no_panic_large_area() {
        let area = Rect::new(0, 0, 500, 200);
        for cfg in [cosmos_cfg(), twinkle_cfg(), drift_cfg()] {
            assert_render_no_panic(area, &cfg);
        }
    }

    #[test]
    fn render_no_op_when_disabled() {
        let area = default_area();
        let cfg = AnimationConfig {
            enabled: false,
            ..AnimationConfig::default()
        };
        let theme = cosmos();
        let mut state = FxState::with_seed(1);
        state.tick(area, &cfg);
        let mut buf = Buffer::empty(area);
        // Fill buffer with a sentinel glyph; render must not touch it.
        for cell in buf.content.iter_mut() {
            cell.set_char('X');
        }
        render_starfield(&mut buf, area, &state, &cfg, &theme);
        render_supernovae(&mut buf, area, &state, &cfg, &theme);
        render_shooting_stars(&mut buf, area, &state, &cfg, &theme);
        for cell in buf.content.iter() {
            assert_eq!(
                cell.symbol(),
                "X",
                "disabled render must not touch buffer cells"
            );
        }
    }

    // ── adversarial: density 0 and huge density ───────────────────────────────

    #[test]
    fn density_zero_yields_no_stars_and_renders_cleanly() {
        let area = default_area();
        let cfg = AnimationConfig {
            density: 0,
            ..AnimationConfig::default()
        };
        let mut state = FxState::with_seed(1);
        state.tick(area, &cfg);
        assert_eq!(state.stars().len(), 0);
        let theme = cosmos();
        let mut buf = Buffer::empty(area);
        render_starfield(&mut buf, area, &state, &cfg, &theme);
    }

    #[test]
    fn huge_density_caps_at_1000() {
        let area = Rect::new(0, 0, 200, 100);
        let cfg = AnimationConfig {
            density: 100,
            ..AnimationConfig::default()
        };
        let mut state = FxState::with_seed(2);
        state.tick(area, &cfg);
        assert!(
            state.stars().len() <= 1000,
            "star count must be capped: {}",
            state.stars().len()
        );
    }

    // ── render_shooting_stars is a no-op in non-Cosmos modes ─────────────────

    #[test]
    fn render_shooting_stars_noop_in_non_cosmos() {
        let area = default_area();
        let theme = cosmos();
        for cfg in [twinkle_cfg(), drift_cfg()] {
            let mut state = FxState::with_seed(1);
            state.tick(area, &cfg);
            // Inject a fake shooting star directly to verify the render guard.
            state.shooting_stars.push_back(ShootingStar {
                xq: 10 << 8,
                yq: 5 << 8,
                vxq: 256,
                vyq: 128,
                age: 0,
                lifetime: 8,
            });
            let mut buf = Buffer::empty(area);
            for cell in buf.content.iter_mut() {
                cell.set_char('Z');
            }
            render_shooting_stars(&mut buf, area, &state, &cfg, &theme);
            // Buffer must be untouched.
            for cell in buf.content.iter() {
                assert_eq!(
                    cell.symbol(),
                    "Z",
                    "render_shooting_stars must be no-op outside Cosmos"
                );
            }
        }
    }

    // ── fixed-point cell derivation ──────────────────────────────────────────

    #[test]
    fn star_cell_matches_fixed_point_integer_part() {
        // After any tick the rendered cell must equal xq>>8, yq>>8.
        let area = default_area();
        let cfg = drift_cfg();
        let mut state = FxState::with_seed(55);
        for _ in 0..100 {
            state.tick(area, &cfg);
            for s in state.stars() {
                assert_eq!(
                    s.x,
                    (s.xq >> 8) as u16,
                    "star.x must equal xq>>8"
                );
                assert_eq!(
                    s.y,
                    (s.yq >> 8) as u16,
                    "star.y must equal yq>>8"
                );
            }
        }
    }

    // ── cosmos determinism ────────────────────────────────────────────────────

    #[test]
    fn cosmos_is_deterministic() {
        let area = default_area();
        let cfg = cosmos_cfg();
        let mut a = FxState::with_seed(7);
        let mut b = FxState::with_seed(7);
        for _ in 0..100 {
            a.tick(area, &cfg);
            b.tick(area, &cfg);
        }
        let pa: Vec<(u16, u16)> = a.stars().iter().map(|s| (s.x, s.y)).collect();
        let pb: Vec<(u16, u16)> = b.stars().iter().map(|s| (s.x, s.y)).collect();
        assert_eq!(pa, pb);
        assert_eq!(a.shooting_stars.len(), b.shooting_stars.len());
    }
}
