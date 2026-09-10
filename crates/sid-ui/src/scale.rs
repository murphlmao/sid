//! App zoom — one scale factor for the whole UI.
//!
//! # The lever
//!
//! `gpui`'s `px()` is already a *logical* pixel: the compositor's HiDPI scale is applied
//! below this layer, so it is not the knob a user's "make everything bigger" means. App
//! zoom is a **second, independent factor**, and `gpui` already has exactly one place to
//! put it — the window's **rem size**.
//!
//! That works because `gpui`'s own spacing shorthands are authored in rems, not pixels:
//! `.p_2()` expands to `rems(0.5)`, `.gap_1()` to `rems(0.25)`, `.h_8()` to `rems(2.0)`,
//! `.rounded_md()` to `rems(0.375)` (gpui-macros-0.2.2 `src/styles.rs`, the
//! `box_style_suffixes()` / `corner_suffixes()` tables). Every one of those resolves
//! against `Window::rem_size()` at layout time. So `set_rem_size(px(16. * factor))`
//! scales all of them at once, for free, with no call-site change anywhere.
//!
//! What it does **not** scale is anything written as an absolute pixel:
//! `.h(px(42.))`, a `ColumnWidth::Fixed(px(96.))` floor, `TypeRole::size()`'s 12/14/16
//! ladder. Those are `AbsoluteLength::Pixels`, and `rem_size` never touches them. Hence
//! [`scaled`]: it takes the size the designer meant at 100% and returns [`Rems`], so the
//! authored number stays readable at the call site while the resolution rides the same
//! single lever. `scaled(42.)` is 42px at 100% and 63px at 150%.
//!
//! # Why a ladder and not a multiplier
//!
//! [`UiScale`] is a rung on a fixed ladder of whole percents, not an `f32` the app
//! multiplies by 1.1 each press. Repeated float multiplication does not come home:
//! `1.0 * 1.1 / 1.1` is not `1.0` in binary floating point, so "zoom in, zoom out" would
//! leave the UI at 99.999994% forever and the persisted value would drift a little every
//! session. Stepping an index into [`LADDER`] makes "in then out is exactly 100%" true by
//! construction rather than by tolerance, and makes the persisted form a `u16` that means
//! precisely what it says. Browsers pick their rungs the same way, and for the same
//! reason.
//!
//! # Where the terminal fits
//!
//! The PTY grid is deliberately outside the type scale (see [`crate::typography`]) — it
//! is an instrument with kitty cell metrics, not UI text. It is *not* outside zoom: the
//! session multiplies its own font size by [`UiScale::factor`] and recomputes the cell
//! grid, so the terminal reflows to fewer/more cells instead of stretching. That call
//! site lives in `sid`, which owns the PTY; this module owns only the number.

use gpui::{Pixels, Rems, px, rems};

/// `gpui`'s default rem size, and therefore the pixel value of `rems(1.)` at 100% zoom.
///
/// Not a preference — it is the constant every `gpui` spacing shorthand was authored
/// against (`.p_4()` means 16px because it is `rems(1.)`), so [`scaled`] divides by it to
/// convert an authored pixel into the same currency.
pub const BASE_REM_PX: f32 = 16.0;

/// The zoom rungs, in whole percent, ascending.
///
/// Browser-shaped: dense near 100% where users actually live, coarse at the ends. The
/// bounds are the clamp — there is no rung below 50% or above 200%, so
/// [`UiScale::zoom_out`] and [`UiScale::zoom_in`] saturate rather than wrap or run away.
pub const LADDER: &[u16] = &[50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200];

/// The rung everything starts on, and the one `ctrl+0` returns to.
pub const DEFAULT_PERCENT: u16 = 100;

/// A zoom level: one rung of [`LADDER`], as whole percent.
///
/// Constructible only through [`UiScale::from_percent`] (which snaps and clamps) or the
/// step methods, so an off-ladder or out-of-range value cannot exist — including one read
/// back from a store written by a different build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UiScale {
    percent: u16,
}

impl Default for UiScale {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl UiScale {
    /// 100% — no zoom.
    pub const DEFAULT: UiScale = UiScale {
        percent: DEFAULT_PERCENT,
    };

    /// The nearest rung to `percent`, clamped into [`LADDER`]'s range.
    ///
    /// Total by design: this is the front door for persisted values, so a store row
    /// holding `0`, `37`, or `60000` resolves to a usable rung instead of an error.
    pub fn from_percent(percent: u16) -> Self {
        Self {
            percent: LADDER[Self::rung_index(percent)],
        }
    }

    /// The index in [`LADDER`] of the rung nearest `percent`.
    ///
    /// `LADDER` ascends, so "nearest" is a single pass: take the first rung at or above
    /// `percent`, then keep whichever of it and its predecessor is closer. A `percent`
    /// past the top has no rung at or above it and lands on the last one — that is the
    /// upper clamp, and the `unwrap_or` below is where it happens.
    fn rung_index(percent: u16) -> usize {
        let above = LADDER
            .iter()
            .position(|&rung| rung >= percent)
            .unwrap_or(LADDER.len() - 1);
        let Some(below) = above.checked_sub(1) else {
            // Nothing below the first rung: that is the lower clamp.
            return above;
        };
        // Signed distances: when `percent` is past the top rung, `above` is the last one
        // and `LADDER[above] - percent` is negative — a `u16` subtraction would panic
        // there instead of clamping, which is exactly what the out-of-range test found.
        let (percent, below_rung, above_rung) = (
            i32::from(percent),
            i32::from(LADDER[below]),
            i32::from(LADDER[above]),
        );
        if percent - below_rung <= above_rung - percent {
            below
        } else {
            above
        }
    }

    /// This rung, as whole percent — the persisted form and the label's number.
    pub fn percent(self) -> u16 {
        self.percent
    }

    /// The multiplier: `1.0` at 100%, `1.5` at 150%.
    pub fn factor(self) -> f32 {
        f32::from(self.percent) / 100.0
    }

    /// Whether this is 100% — the UI hides the zoom indicator when it is.
    pub fn is_default(self) -> bool {
        self.percent == DEFAULT_PERCENT
    }

    /// The next rung up, saturating at the top of [`LADDER`].
    pub fn zoom_in(self) -> Self {
        self.step(1)
    }

    /// The next rung down, saturating at the bottom of [`LADDER`].
    pub fn zoom_out(self) -> Self {
        self.step(-1)
    }

    /// Move `by` rungs, saturating at both ends.
    ///
    /// Index arithmetic, never float arithmetic — see the module docs on why "in then
    /// out" has to land on 100% exactly rather than nearly.
    fn step(self, by: isize) -> Self {
        let at = Self::rung_index(self.percent) as isize;
        let to = at.saturating_add(by).clamp(0, LADDER.len() as isize - 1);
        Self {
            percent: LADDER[to as usize],
        }
    }

    /// Back to 100%.
    pub fn reset(self) -> Self {
        Self::DEFAULT
    }

    /// The rem size to hand `Window::set_rem_size` — the single lever (see module docs).
    pub fn rem_size(self) -> Pixels {
        px(BASE_REM_PX * self.factor())
    }

    /// Recover the zoom level in force for a window, from its rem size.
    ///
    /// The inverse of [`UiScale::rem_size`], and the reason code that has to do its own
    /// pixel arithmetic (table column layout, the terminal's cell grid) needs no second
    /// channel: the window already carries the factor, so there is exactly one place the
    /// current zoom can be read from and nothing to keep in sync.
    pub fn from_rem_size(rem_size: Pixels) -> Self {
        Self::from_percent((f32::from(rem_size) / BASE_REM_PX * 100.0).round() as u16)
    }

    /// Scale a length that must be resolved *in Rust* rather than by `gpui`'s layout —
    /// table column arithmetic, terminal cell geometry, anything measured before paint.
    ///
    /// Rounds to a whole logical pixel (fractional device columns are where hairlines go
    /// grey), and never collapses a visible length to nothing: a 1px rule at 50% is still
    /// 1px, because a border that vanishes when you zoom out is a bug, not a small
    /// border. Zero in is zero out — an absent gap stays absent.
    ///
    /// The rounding applies at 100% too, which is why the design system's lengths are
    /// authored as whole pixels: on that vocabulary this is exactly the identity when
    /// zoom is off, and nothing in the app measures differently for having been routed
    /// through here.
    pub fn scale_px(self, length: Pixels) -> Pixels {
        let raw = f32::from(length);
        if raw == 0.0 {
            return px(0.0);
        }
        px((raw * self.factor()).round().max(1.0))
    }

    /// `"150%"` — for the settings row and the zoom toast.
    pub fn label(self) -> String {
        format!("{}%", self.percent)
    }
}

/// A design-system length, authored in pixels at 100% zoom.
///
/// Returns [`Rems`] so `gpui` resolves it against the window's rem size, which is where
/// app zoom lives (see the module docs). Use it anywhere a literal `px(..)` would have
/// gone in a *style* position — `.h(scaled(42.))`, `.w(scaled(220.))`,
/// `.text_size(scaled(14.))`. For a length the code has to compute with, use
/// [`UiScale::scale_px`] instead.
pub fn scaled(px_at_100: f32) -> Rems {
    rems(px_at_100 / BASE_REM_PX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_starts_unzoomed() {
        assert_eq!(UiScale::default(), UiScale::DEFAULT);
        assert_eq!(UiScale::default().percent(), 100);
        assert!(UiScale::default().is_default());
    }

    #[test]
    fn zooming_in_takes_the_next_rung_up() {
        assert_eq!(UiScale::DEFAULT.zoom_in().percent(), 110);
    }

    #[test]
    fn zooming_out_takes_the_next_rung_down() {
        assert_eq!(UiScale::DEFAULT.zoom_out().percent(), 90);
    }

    #[test]
    fn in_then_out_returns_to_exactly_one_hundred() {
        // The whole reason [`LADDER`] is integers. With `f32` multiplication this is
        // 99.999994% and the drift is permanent, because it gets persisted.
        let there_and_back = UiScale::DEFAULT.zoom_in().zoom_out();
        assert_eq!(there_and_back, UiScale::DEFAULT);
        assert_eq!(there_and_back.percent(), 100);
        assert_eq!(there_and_back.factor(), 1.0);
    }

    #[test]
    fn out_then_in_returns_to_exactly_one_hundred() {
        assert_eq!(UiScale::DEFAULT.zoom_out().zoom_in(), UiScale::DEFAULT);
    }

    #[test]
    fn walking_the_whole_ladder_up_and_back_lands_on_the_rung_it_started_from() {
        // Not the same statement as the two above: this proves every *pair* of adjacent
        // rungs is symmetric, so no single step is a one-way door.
        let bottom = UiScale::from_percent(LADDER[0]);
        let mut s = bottom;
        for _ in 0..LADDER.len() * 2 {
            s = s.zoom_in();
        }
        assert_eq!(s.percent(), *LADDER.last().expect("ladder is not empty"));
        for _ in 0..LADDER.len() * 2 {
            s = s.zoom_out();
        }
        assert_eq!(s, bottom);
    }

    #[test]
    fn zooming_in_saturates_at_two_hundred_percent() {
        let mut s = UiScale::DEFAULT;
        for _ in 0..50 {
            s = s.zoom_in();
        }
        assert_eq!(s.percent(), 200);
    }

    #[test]
    fn zooming_out_saturates_at_fifty_percent() {
        let mut s = UiScale::DEFAULT;
        for _ in 0..50 {
            s = s.zoom_out();
        }
        assert_eq!(s.percent(), 50);
    }

    #[test]
    fn reset_returns_to_one_hundred_from_either_end() {
        assert_eq!(UiScale::from_percent(200).reset(), UiScale::DEFAULT);
        assert_eq!(UiScale::from_percent(50).reset(), UiScale::DEFAULT);
        assert_eq!(UiScale::DEFAULT.reset(), UiScale::DEFAULT);
    }

    #[test]
    fn an_off_ladder_percent_snaps_to_the_nearest_rung() {
        // A value from a store written by another build, or a hand-edited one.
        assert_eq!(UiScale::from_percent(101).percent(), 100);
        assert_eq!(UiScale::from_percent(140).percent(), 150);
        assert_eq!(UiScale::from_percent(120).percent(), 125);
        assert_eq!(UiScale::from_percent(70).percent(), 67);
    }

    #[test]
    fn a_percent_outside_the_ladder_clamps_to_its_ends() {
        assert_eq!(UiScale::from_percent(0).percent(), 50);
        assert_eq!(UiScale::from_percent(1).percent(), 50);
        assert_eq!(UiScale::from_percent(u16::MAX).percent(), 200);
        assert_eq!(UiScale::from_percent(201).percent(), 200);
    }

    #[test]
    fn every_rung_survives_the_persisted_round_trip() {
        // `percent()` is what goes into the store and `from_percent` is what comes back;
        // if any rung is not a fixed point of that pair, a restart moves the UI.
        for &rung in LADDER {
            let s = UiScale::from_percent(rung);
            assert_eq!(s.percent(), rung, "rung {rung} did not round-trip");
            assert_eq!(UiScale::from_percent(s.percent()), s);
        }
    }

    #[test]
    fn the_ladder_is_ascending_spans_fifty_to_two_hundred_and_contains_one_hundred() {
        assert!(
            LADDER.windows(2).all(|w| w[0] < w[1]),
            "rungs must ascend — the step methods index this slice"
        );
        assert_eq!(LADDER.first(), Some(&50), "the documented lower clamp");
        assert_eq!(LADDER.last(), Some(&200), "the documented upper clamp");
        assert!(
            LADDER.contains(&DEFAULT_PERCENT),
            "reset must land on a real rung"
        );
    }

    #[test]
    fn the_factor_is_the_percent_over_a_hundred() {
        assert_eq!(UiScale::from_percent(100).factor(), 1.0);
        assert_eq!(UiScale::from_percent(150).factor(), 1.5);
        assert_eq!(UiScale::from_percent(50).factor(), 0.5);
        assert_eq!(UiScale::from_percent(200).factor(), 2.0);
    }

    #[test]
    fn at_one_hundred_percent_the_rem_size_is_gpuis_own_default() {
        // The no-op property: with zoom off, nothing in the app may measure differently
        // than it did before this module existed.
        assert_eq!(UiScale::DEFAULT.rem_size(), px(BASE_REM_PX));
    }

    #[test]
    fn the_rem_size_carries_the_factor() {
        assert_eq!(UiScale::from_percent(150).rem_size(), px(24.));
        assert_eq!(UiScale::from_percent(50).rem_size(), px(8.));
    }

    #[test]
    fn a_windows_rem_size_reports_the_rung_that_set_it() {
        // The round-trip that lets table and terminal geometry read the zoom off the
        // window instead of carrying a duplicate of it.
        for &rung in LADDER {
            let s = UiScale::from_percent(rung);
            assert_eq!(UiScale::from_rem_size(s.rem_size()), s, "rung {rung}");
        }
    }

    #[test]
    fn gpuis_untouched_rem_size_reads_as_unzoomed() {
        // A window nobody has called `set_rem_size` on must not look like 0% zoom.
        assert_eq!(UiScale::from_rem_size(px(BASE_REM_PX)), UiScale::DEFAULT);
    }

    #[test]
    fn scaling_a_pixel_length_is_the_identity_at_one_hundred_percent() {
        // The no-op property, over the design system's actual vocabulary: every authored
        // length in `sid-ui` is a whole pixel. Routing them through `scale_px` with zoom
        // off must not move a single one.
        for length in [1., 6., 7., 12., 14., 42., 64., 96., 220.] {
            assert_eq!(UiScale::DEFAULT.scale_px(px(length)), px(length));
        }
    }

    #[test]
    fn a_fractional_length_is_snapped_to_a_whole_pixel_even_unzoomed() {
        // The other half of "identity at 100%": it holds on whole pixels because
        // `scale_px` is also the rounding gate. A half-pixel width is a grey hairline on
        // a 1x display; the function will not pass one through at any zoom level.
        assert_eq!(UiScale::DEFAULT.scale_px(px(220.5)), px(221.));
        assert_eq!(UiScale::DEFAULT.scale_px(px(6.25)), px(6.));
    }

    #[test]
    fn a_scaled_pixel_length_lands_on_a_whole_pixel() {
        // 96 * 0.67 = 64.32 -> 64. Fractional widths are where a table's hairline
        // separators go grey and its columns stop lining up.
        assert_eq!(UiScale::from_percent(67).scale_px(px(96.)), px(64.));
        assert_eq!(UiScale::from_percent(150).scale_px(px(21.)), px(32.));
    }

    #[test]
    fn a_visible_length_never_scales_away_to_nothing() {
        // The bug this pins: `(1px * 0.5).round()` is 0, and a border, a divider or a
        // minimum column width of zero is invisible — zooming out would delete the
        // furniture instead of shrinking it.
        for percent in LADDER.iter().copied() {
            let s = UiScale::from_percent(percent);
            assert!(
                s.scale_px(px(1.)) >= px(1.),
                "{percent}%: a 1px rule vanished"
            );
            assert!(
                s.scale_px(px(0.4)) >= px(1.),
                "{percent}%: a hairline vanished"
            );
        }
    }

    #[test]
    fn a_zero_length_stays_zero() {
        // The other half of the rule above: "never zero" must not invent a pixel where
        // the design asked for none.
        assert_eq!(UiScale::from_percent(200).scale_px(px(0.)), px(0.));
        assert_eq!(UiScale::from_percent(50).scale_px(px(0.)), px(0.));
    }

    #[test]
    fn scaling_a_pixel_length_is_monotone_across_the_ladder() {
        // Zooming in may never make something smaller — the property that catches a
        // rounding rule applied in the wrong order.
        let mut previous = px(0.);
        for &rung in LADDER {
            let now = UiScale::from_percent(rung).scale_px(px(40.));
            assert!(now >= previous, "{rung}%: {now:?} < {previous:?}");
            previous = now;
        }
    }

    #[test]
    fn an_authored_length_is_its_own_pixel_count_at_the_base_rem() {
        // `scaled` hands gpui the same measurement the designer typed; the zoom arrives
        // later, from the window's rem size. If this is off, every literal in the design
        // system silently moves the moment it is converted.
        for length in [1., 8., 14., 42., 220.] {
            assert_eq!(
                scaled(length).to_pixels(px(BASE_REM_PX)),
                px(length),
                "scaled({length}) must measure {length}px at 100%"
            );
        }
    }

    #[test]
    fn an_authored_length_rides_the_rem_size_lever() {
        assert_eq!(
            scaled(42.).to_pixels(UiScale::from_percent(150).rem_size()),
            px(63.)
        );
        assert_eq!(
            scaled(42.).to_pixels(UiScale::from_percent(50).rem_size()),
            px(21.)
        );
    }

    #[test]
    fn the_label_is_the_percent_with_a_sign() {
        assert_eq!(UiScale::DEFAULT.label(), "100%");
        assert_eq!(UiScale::from_percent(67).label(), "67%");
        assert_eq!(UiScale::from_percent(200).label(), "200%");
    }
}
