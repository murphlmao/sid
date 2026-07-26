//! The redistribution core: declared column intent -> resolved pixel widths.
//!
//! This module is the whole reason the fill-width fix is cheap. It is a **pure
//! algorithmic subdomain** — no `gpui`, no `gpui_component`, no `App`, no entity, no
//! window. It takes a list of declared intents and a viewport width in logical pixels
//! and returns the width every column should be. That makes the interesting part of the
//! feature testable in milliseconds instead of behind a rendering harness, which is why
//! the test list below is exhaustive rather than representative.
//!
//! # The model
//!
//! | Intent | Meaning |
//! |---|---|
//! | [`ColumnWidth::Fixed`] | exactly this many pixels, at every viewport width |
//! | [`ColumnWidth::Min`] | never narrower than this; takes leftover only when nothing grows |
//! | [`ColumnWidth::Grow`] | takes `weight / total_weight` of the leftover, never below `min` |
//!
//! # The rules, in order
//!
//! 1. Every column starts at its **floor** — `Fixed`/`Min` at their declared pixels, a
//!    `Grow` at its `min`. Floors are inviolable: a column is never squeezed below the
//!    width its author said it needs, because a squeezed column is a truncated column
//!    and truncation is the bug being fixed (the Network tab's IPv6 addresses at 120px).
//! 2. **Leftover** = viewport - sum(floors). If it is zero or negative the viewport is
//!    too narrow to honour the declaration; every column stays at its floor and the
//!    table overflows into its own horizontal scroll — the same behaviour it has today,
//!    which is the correct degenerate answer.
//! 3. Leftover goes to the **growers**: the `Grow` columns, split by weight.
//! 4. If there is no `Grow` column at all, the `Min` columns share the leftover equally.
//!    Without this rule a table declared entirely in `Min` would leave dead space, which
//!    is the exact complaint this model exists to answer; with it, "declare a sensible
//!    minimum for every column" is a complete answer for simple tables and `Grow` is the
//!    opt-in for tables that want one column to soak the slack.
//! 5. If nothing can grow (all `Fixed`), the leftover is simply not allocated. That is a
//!    deliberate declaration — a table of exact pixel columns asked for exact pixels.
//!
//! # Why floors instead of a global minimum
//!
//! "Never below their own minimum" has to be per column: `PID` is legible at 80px and an
//! IPv6 socket address is not legible at 200px. A single crate-wide minimum would either
//! waste space on the narrow columns or truncate the wide ones.

/// The floor a [`ColumnWidth::grow`] column gets when its author does not name one.
///
/// Wide enough for a short header plus its sort chevron, narrow enough that six of them
/// still fit a laptop window. Name a real floor with [`ColumnWidth::min_width`] whenever
/// the column's content has one.
pub const DEFAULT_GROW_MIN: f32 = 64.0;

/// How wide a table column wants to be, declared once by the delegate and resolved
/// against the live viewport by [`resolve_widths`]. See the module docs for the rules.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    /// Exactly this many logical pixels, always. For columns whose content has a known
    /// bound: a percentage, a pid, an icon-sized action slot.
    Fixed(f32),
    /// At least this many logical pixels. Takes a share of the leftover only when the
    /// table declares no [`ColumnWidth::Grow`] column (module docs, rule 4).
    Min(f32),
    /// Absorbs the leftover, `weight` at a time, but never renders below `min`.
    Grow {
        /// This column's share of the leftover, relative to the other growers. Values
        /// at or below zero are treated as zero (the column sits at its floor) unless
        /// *every* grower is zero, in which case they split the leftover equally.
        weight: f32,
        /// The narrowest this column may render — the point below which its content
        /// stops being readable.
        min: f32,
    },
}

impl ColumnWidth {
    /// A weight-1 grower floored at [`DEFAULT_GROW_MIN`]. Refine with
    /// [`ColumnWidth::min_width`] / [`ColumnWidth::weight`].
    pub const fn grow() -> Self {
        Self::Grow {
            weight: 1.0,
            min: DEFAULT_GROW_MIN,
        }
    }

    /// Set a grower's floor. A no-op on `Fixed`/`Min`, which already *are* their floor —
    /// so the builder can be applied uniformly without a match at the call site.
    pub const fn min_width(self, min: f32) -> Self {
        match self {
            Self::Grow { weight, .. } => Self::Grow { weight, min },
            other => other,
        }
    }

    /// Set a grower's share of the leftover. A no-op on `Fixed`/`Min`, which take none.
    pub const fn weight(self, weight: f32) -> Self {
        match self {
            Self::Grow { min, .. } => Self::Grow { weight, min },
            other => other,
        }
    }

    /// The narrowest this column may ever render.
    pub const fn floor(self) -> f32 {
        match self {
            Self::Fixed(px) | Self::Min(px) => px,
            Self::Grow { min, .. } => min,
        }
    }
}

/// Resolve declared intents against a viewport width, in logical pixels.
///
/// The returned vector is parallel to `specs`. When anything can grow and the viewport
/// is wide enough, the widths sum to `viewport` — that "no dead space at the right edge"
/// property is the whole point, and it is asserted across a width sweep in the tests.
pub fn resolve_widths(specs: &[ColumnWidth], viewport: f32) -> Vec<f32> {
    // 1. Everyone starts at their floor.
    let mut widths: Vec<f32> = specs.iter().map(|spec| spec.floor()).collect();

    // 2. What is left over, if anything. A non-finite viewport (an unmeasured first
    //    frame, a degenerate window) lands here too and keeps the floors.
    let leftover = viewport - widths.iter().sum::<f32>();
    if !leftover.is_finite() || leftover <= 0.0 {
        return widths;
    }

    // 3. Who takes it, and in what proportion. Growers by weight; failing that (rule 4)
    //    the `Min` columns equally; failing that (rule 5) nobody.
    let mut shares: Vec<f32> = specs
        .iter()
        .map(|spec| match spec {
            ColumnWidth::Grow { weight, .. } => weight.max(0.0),
            _ => 0.0,
        })
        .collect();
    if !specs.iter().any(|s| matches!(s, ColumnWidth::Grow { .. })) {
        // Rule 4: with no grower declared, the `Min` columns are the fallback growers.
        for (share, spec) in shares.iter_mut().zip(specs) {
            *share = f32::from(matches!(spec, ColumnWidth::Min(_)));
        }
    } else if shares.iter().sum::<f32>() <= 0.0 {
        // Growers exist but every one of them asked for zero. A ratio that cannot be
        // honoured becomes equal shares — anything else leaves dead space in a table
        // that explicitly declared itself flexible.
        for (share, spec) in shares.iter_mut().zip(specs) {
            *share = f32::from(matches!(spec, ColumnWidth::Grow { .. }));
        }
    }
    // 4. Hand it out, proportionally. A zero total means every taker asked for nothing
    //    and neither fallback applied — all `Fixed`, so the leftover is not ours to give.
    let total: f32 = shares.iter().sum();
    if total <= 0.0 {
        return widths;
    }
    for (width, share) in widths.iter_mut().zip(&shares) {
        *width += leftover * share / total;
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widths are compared at 1/100 px: the resolver works in f32 and the assertions
    /// care about layout, not about the last bit of the mantissa.
    fn assert_widths(got: &[f32], want: &[f32]) {
        assert_eq!(got.len(), want.len(), "column count: {got:?} vs {want:?}");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert!(
                (g - w).abs() < 0.01,
                "column {i}: got {g}, want {w} (all: {got:?} vs {want:?})"
            );
        }
    }

    #[test]
    fn a_grower_takes_the_leftover_and_the_row_fills_exactly() {
        // 70 + 90 + grow, in 1000px: the grower gets everything the fixed pair leaves.
        let specs = [
            ColumnWidth::Fixed(70.),
            ColumnWidth::Fixed(90.),
            ColumnWidth::grow(),
        ];
        let got = resolve_widths(&specs, 1000.);
        assert_widths(&got, &[70., 90., 840.]);
        assert!((got.iter().sum::<f32>() - 1000.).abs() < 0.01, "{got:?}");
    }

    #[test]
    fn fixed_columns_never_move() {
        // Same declaration at three viewports: the fixed pair is identical at all three.
        for viewport in [400., 1000., 2000.] {
            let specs = [
                ColumnWidth::Fixed(70.),
                ColumnWidth::grow(),
                ColumnWidth::Fixed(72.),
            ];
            let got = resolve_widths(&specs, viewport);
            assert_widths(&[got[0], got[2]], &[70., 72.]);
        }
    }

    #[test]
    fn two_growers_split_the_leftover_by_weight() {
        // 100px fixed, 900px leftover, weights 1:3 -> 225 / 675.
        let specs = [
            ColumnWidth::Fixed(100.),
            ColumnWidth::grow().min_width(0.),
            ColumnWidth::grow().min_width(0.).weight(3.),
        ];
        assert_widths(&resolve_widths(&specs, 1000.), &[100., 225., 675.]);
    }

    #[test]
    fn weights_split_the_leftover_not_the_total_width() {
        // The distinction that keeps floors inviolable. A 10:1 weight ratio applied to
        // the *total* would give the wide-floored column 91px and truncate it; applied
        // to the leftover above the floors it gets its 500px floor plus its small share.
        // So the rendered ratio is not 10:1, and that is correct.
        let specs = [
            ColumnWidth::grow().min_width(0.).weight(10.),
            ColumnWidth::grow().min_width(500.).weight(1.),
        ];
        // floors 500, leftover 500, split 10:1 -> 454.54 / 45.45.
        assert_widths(&resolve_widths(&specs, 1000.), &[454.545, 545.454]);
    }

    #[test]
    fn a_single_grower_takes_all_of_the_leftover() {
        let specs = [
            ColumnWidth::Fixed(80.),
            ColumnWidth::grow(),
            ColumnWidth::Min(120.),
        ];
        assert_widths(&resolve_widths(&specs, 1000.), &[80., 800., 120.]);
    }

    #[test]
    fn min_columns_hold_their_floor_while_a_grower_soaks_the_slack() {
        // The System tab's declaration: User is Min(120) and must stay 120 no matter how
        // wide the window gets, because Name is the column that should absorb the space.
        let specs = [
            ColumnWidth::Fixed(70.),
            ColumnWidth::grow().min_width(220.),
            ColumnWidth::Min(120.),
        ];
        assert_widths(&resolve_widths(&specs, 2000.), &[70., 1810., 120.]);
    }

    #[test]
    fn min_columns_share_the_leftover_when_nothing_grows() {
        // Rule 4: a table declared entirely in Min still fills, rather than leaving the
        // dead space this whole model exists to delete.
        let specs = [
            ColumnWidth::Fixed(100.),
            ColumnWidth::Min(100.),
            ColumnWidth::Min(200.),
        ];
        assert_widths(&resolve_widths(&specs, 1000.), &[100., 400., 500.]);
    }

    #[test]
    fn all_fixed_columns_leave_the_leftover_unallocated() {
        // Rule 5: exact pixels were asked for, exact pixels are given. The caller who
        // wants the row filled has to say so with a Grow or a Min.
        let specs = [ColumnWidth::Fixed(100.), ColumnWidth::Fixed(200.)];
        assert_widths(&resolve_widths(&specs, 1000.), &[100., 200.]);
    }

    #[test]
    fn a_viewport_narrower_than_the_floors_leaves_every_column_at_its_floor() {
        // Degenerate: 500px of declaration in a 300px viewport. Nothing is squeezed; the
        // table overflows into its own horizontal scroll, as it does today.
        let specs = [
            ColumnWidth::Fixed(200.),
            ColumnWidth::Min(100.),
            ColumnWidth::grow().min_width(200.),
        ];
        let got = resolve_widths(&specs, 300.);
        assert_widths(&got, &[200., 100., 200.]);
        assert!(got.iter().sum::<f32>() > 300., "{got:?}");
    }

    #[test]
    fn an_exactly_full_viewport_adds_nothing() {
        let specs = [
            ColumnWidth::Fixed(200.),
            ColumnWidth::grow().min_width(100.),
        ];
        assert_widths(&resolve_widths(&specs, 300.), &[200., 100.]);
    }

    #[test]
    fn a_zero_weight_grower_stays_at_its_floor() {
        // Weight 0 next to a real weight means "I am flexible in principle, but this
        // round is not mine" — it sits at its floor and the other grower takes it all.
        let specs = [
            ColumnWidth::grow().min_width(100.).weight(0.),
            ColumnWidth::grow().min_width(100.),
        ];
        assert_widths(&resolve_widths(&specs, 1000.), &[100., 900.]);
    }

    #[test]
    fn growers_that_are_all_zero_weight_split_the_leftover_equally() {
        // Every grower asked for nothing, which cannot be honoured as a ratio. Equal
        // shares beat leaving 800px of dead space in a table that declared it flexible.
        let specs = [
            ColumnWidth::grow().min_width(100.).weight(0.),
            ColumnWidth::grow().min_width(100.).weight(0.),
        ];
        assert_widths(&resolve_widths(&specs, 1000.), &[500., 500.]);
    }

    #[test]
    fn a_negative_weight_is_treated_as_zero() {
        let specs = [
            ColumnWidth::grow().min_width(100.).weight(-5.),
            ColumnWidth::grow().min_width(100.),
        ];
        assert_widths(&resolve_widths(&specs, 1000.), &[100., 900.]);
    }

    #[test]
    fn no_columns_resolves_to_no_widths() {
        assert!(resolve_widths(&[], 1000.).is_empty());
    }

    #[test]
    fn a_zero_or_negative_viewport_falls_back_to_the_floors() {
        // The first frame measures before layout: bounds are 0x0. Rendering the floors
        // beats rendering a row of zero-width columns.
        let specs = [ColumnWidth::Fixed(70.), ColumnWidth::grow().min_width(220.)];
        assert_widths(&resolve_widths(&specs, 0.), &[70., 220.]);
        assert_widths(&resolve_widths(&specs, -100.), &[70., 220.]);
    }

    #[test]
    fn a_non_finite_viewport_falls_back_to_the_floors() {
        let specs = [ColumnWidth::Fixed(70.), ColumnWidth::grow().min_width(220.)];
        assert_widths(&resolve_widths(&specs, f32::NAN), &[70., 220.]);
        assert_widths(&resolve_widths(&specs, f32::INFINITY), &[70., 220.]);
    }

    #[test]
    fn the_widths_sum_to_the_viewport_across_a_sweep_of_widths() {
        // Vector-style sweep: the "zero dead space" promise is the point of the commit,
        // so it is asserted at every width from cramped to ultrawide rather than at one.
        let specs = [
            ColumnWidth::Fixed(70.),
            ColumnWidth::Fixed(90.),
            ColumnWidth::Fixed(80.),
            ColumnWidth::grow().min_width(220.),
            ColumnWidth::Min(120.),
            ColumnWidth::Fixed(72.),
        ];
        let floors: f32 = specs.iter().map(|s| s.floor()).sum();
        for viewport in (100..=3000).step_by(37).map(|w| w as f32) {
            let got = resolve_widths(&specs, viewport);
            let total: f32 = got.iter().sum();
            if viewport <= floors {
                assert!((total - floors).abs() < 0.01, "{viewport}px: {got:?}");
            } else {
                assert!((total - viewport).abs() < 0.01, "{viewport}px: {got:?}");
            }
        }
    }

    #[test]
    fn no_column_is_ever_resolved_below_its_floor() {
        let specs = [
            ColumnWidth::Fixed(70.),
            ColumnWidth::Min(120.),
            ColumnWidth::grow().min_width(220.).weight(3.),
            ColumnWidth::grow().min_width(64.),
        ];
        for viewport in (0..=3000).step_by(23).map(|w| w as f32) {
            for (i, (got, spec)) in resolve_widths(&specs, viewport)
                .iter()
                .zip(&specs)
                .enumerate()
            {
                assert!(
                    *got >= spec.floor() - 0.01,
                    "{viewport}px, column {i}: {got} < floor {}",
                    spec.floor()
                );
            }
        }
    }
}
