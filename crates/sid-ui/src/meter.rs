//! Meters and stat clusters — the instrument panel.
//!
//! The audit's verdict on the System tab's overview strip: *"CPU/Memory/Swap meters have
//! no container. Three thin unframed bars with labels floating on the window background
//! at the very top of the canvas… they read as debug output, not as an instrument
//! cluster. The per-core mini-bar strip underneath is a row of ~20 tiny red ticks with no
//! axis, no label, and no explanation of what it is."*
//!
//! Two components answer that, and they answer the design system's amendment with it
//! (`docs/design/2026-07-26-ui-overhaul-plan.md` §5): *"require data/meter clusters to sit
//! on a `surface` card with a section header — the System tab's unframed meters are what
//! 'hairlines only' degrades into."*
//!
//! - [`Meter`] — one labelled bar with a right-aligned readout, and an optional strip of
//!   sub-bars underneath **with its own label**, which is what the unexplained per-core
//!   ticks were missing.
//! - [`StatCluster`] — the frame: a [`crate::Card`] with a section header and a summary
//!   line, laying its meters out side by side.
//!
//! # Where the thinking is
//!
//! Two pure functions, both unit-tested, and neither of them as obvious as it looks:
//!
//! - [`ratio`] — `used / total` without the divide-by-zero (an unconfigured swap device
//!   is `0 / 0`, and the System tab reaches that on any machine without a swapfile).
//! - [`bar_fraction`] — declared fraction to painted width. Clamps, rejects non-finite
//!   input **before** clamping (`f32::clamp` propagates `NaN`, and a `NaN` width does not
//!   lay out), and floors any strictly-positive reading at a visible sliver so a live
//!   0.2% never paints as a flat empty track. Empty stays exactly empty.
//!
//! [`MeterTone`] carries the third: the calm/caution/critical threshold ladder, which was
//! a free `bar_color` function private to `systems_tab.rs`.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder as _, px, relative, rgb,
};

use crate::card::Card;
use crate::styled::{StyledExt as _, h_flex, v_flex};
use crate::theme::{self, Theme};

/// At or above this load, a meter is [`MeterTone::Caution`].
pub const CAUTION_AT: f32 = 0.70;

/// At or above this load, a meter is [`MeterTone::Critical`].
pub const CRITICAL_AT: f32 = 0.90;

/// The narrowest a non-empty bar is ever painted, as a fraction of its track.
///
/// A meter reading 0.2% with a mathematically proportional fill paints nothing at all,
/// which is the same picture as 0% — so a machine under light load and a machine with a
/// broken probe look identical. Anything strictly positive gets at least this much.
pub const MIN_VISIBLE: f32 = 0.02;

/// The track's height, and the bar's.
const BAR_HEIGHT: f32 = 6.0;

/// One sub-bar in a [`Meter::segments`] strip.
const SEGMENT_WIDTH: f32 = 5.0;

/// The height of a [`Meter::segments`] sub-bar.
const SEGMENT_HEIGHT: f32 = 20.0;

/// How loaded a meter is. Derived from the value by [`MeterTone::of`] unless a caller
/// overrides it with [`Meter::tone`] — a meter whose high end is *good* (free disk,
/// battery) wants to say so itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterTone {
    /// Nominal. The accent.
    Calm,
    /// Getting full.
    Caution,
    /// Nearly out.
    Critical,
}

/// Every tone, for the gallery and for exhaustive tests.
pub const ALL_METER_TONES: &[MeterTone] =
    &[MeterTone::Calm, MeterTone::Caution, MeterTone::Critical];

impl MeterTone {
    /// The tone a `0.0..=1.0` load reads as. Same three-tier ladder the rest of the app
    /// uses for status colour; non-finite input is treated as nominal rather than
    /// alarming, because a broken probe is not a full disk.
    pub fn of(fraction: f32) -> Self {
        if !fraction.is_finite() {
            MeterTone::Calm
        } else if fraction >= CRITICAL_AT {
            MeterTone::Critical
        } else if fraction >= CAUTION_AT {
            MeterTone::Caution
        } else {
            MeterTone::Calm
        }
    }

    /// The fill token for this tone.
    pub fn color(self, theme: &Theme) -> u32 {
        match self {
            MeterTone::Calm => theme.accent,
            MeterTone::Caution => theme.warning,
            MeterTone::Critical => theme.danger,
        }
    }

    /// A human label, for the gallery.
    pub fn label(self) -> &'static str {
        match self {
            MeterTone::Calm => "calm",
            MeterTone::Caution => "caution",
            MeterTone::Critical => "critical",
        }
    }
}

/// `used / total` as a `0.0..=1.0` fraction, with `0 / 0` reading as empty rather than
/// as a panic or a `NaN`.
///
/// An unconfigured swap device is exactly `0 / 0`, and every machine without a swapfile
/// reaches that on the System tab's first paint.
pub fn ratio(used: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    bar_fraction_unfloored(used as f32 / total as f32)
}

/// A declared fraction as a painted width.
///
/// Clamped to `0.0..=1.0`, non-finite input rejected, and any strictly-positive reading
/// floored at [`MIN_VISIBLE`] so a live meter never paints as an empty one. Exact zero
/// stays exactly zero — "nothing" must still be able to look like nothing.
pub fn bar_fraction(fraction: f32) -> f32 {
    let clamped = bar_fraction_unfloored(fraction);
    if clamped > 0.0 {
        clamped.max(MIN_VISIBLE)
    } else {
        0.0
    }
}

/// [`bar_fraction`] without the visible-sliver floor: the honest number, for feeding
/// [`MeterTone::of`] and for [`ratio`]'s return value.
fn bar_fraction_unfloored(fraction: f32) -> f32 {
    // `f32::clamp` propagates NaN, and a NaN width silently breaks taffy's layout
    // rather than erroring — so the finite check has to come first.
    if !fraction.is_finite() {
        return 0.0;
    }
    fraction.clamp(0.0, 1.0)
}

/// A labelled bar with a right-aligned readout.
///
/// ```ignore
/// Meter::new("Memory", Meter::ratio(used, total))
///     .value(format!("{} / {}", humanize(used), humanize(total)))
/// ```
#[derive(IntoElement)]
pub struct Meter {
    label: SharedString,
    fraction: f32,
    value: Option<SharedString>,
    note: Option<SharedString>,
    tone: Option<MeterTone>,
    segments: Vec<f32>,
    segments_label: Option<SharedString>,
}

impl Meter {
    /// A meter at `fraction` of full (`0.0..=1.0`; out-of-range and non-finite values
    /// are handled by [`bar_fraction`]).
    pub fn new(label: impl Into<SharedString>, fraction: f32) -> Self {
        Self {
            label: label.into(),
            fraction,
            value: None,
            note: None,
            tone: None,
            segments: Vec::new(),
            segments_label: None,
        }
    }

    /// `used / total` as a fraction — see [`ratio`]. Re-exported as an associated
    /// function so a call site reads `Meter::ratio(used, total)`.
    pub fn ratio(used: u64, total: u64) -> f32 {
        ratio(used, total)
    }

    /// The readout on the right of the label line — "1.2 / 31.0 GB", "12.4%".
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// A muted line under the bar: why this meter reads the way it does ("none
    /// configured"), not a second value.
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Override the derived [`MeterTone`].
    pub fn tone(mut self, tone: MeterTone) -> Self {
        self.tone = Some(tone);
        self
    }

    /// A strip of sub-bars under the main one — per-core CPU, per-disk usage.
    ///
    /// **The label is required**, because the thing this replaces is a row of twenty
    /// unexplained ticks: *"no axis, no label, and no explanation of what it is."*
    pub fn segments(
        mut self,
        values: impl IntoIterator<Item = f32>,
        label: impl Into<SharedString>,
    ) -> Self {
        self.segments = values.into_iter().collect();
        self.segments_label = Some(label.into());
        self
    }

    /// The tone this meter paints in: the caller's override, else the derived one.
    fn resolved_tone(&self) -> MeterTone {
        self.tone.unwrap_or_else(|| MeterTone::of(self.fraction))
    }
}

impl RenderOnce for Meter {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let fill = self.resolved_tone().color(&theme);
        let width = bar_fraction(self.fraction);
        let segments = self.segments;
        let segments_label = self.segments_label.filter(|_| !segments.is_empty());

        v_flex()
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .child(
                // Label and readout sit *together*, left-aligned, rather than pushed to
                // opposite ends of the meter's width. A `justify_between` pair reads fine
                // at 300px and falls apart at 660 — which is what a third of a 2000px
                // card is — leaving "Swap" at one end of the card and "5.5 GB / 34.2 GB"
                // at the other, with nothing tying them together. The bar underneath
                // still spans the full width; only the caption is a unit.
                h_flex()
                    .gap_2()
                    .child(div().hint_text(&theme).child(self.label))
                    .when_some(self.value, |this, value| {
                        this.child(div().text_xs().text_color(rgb(theme.fg)).child(value))
                    }),
            )
            .child(
                track(&theme).child(div().h_full().rounded_sm().bg(rgb(fill)).w(relative(width))),
            )
            .when_some(self.note, |this, note| {
                this.child(div().hint_text(&theme).child(note))
            })
            .when_some(segments_label, |this, label| {
                this.child(
                    v_flex()
                        .gap_1()
                        .pt_1()
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_1()
                                .children(segments.iter().map(|&value| segment_bar(&theme, value))),
                        )
                        .child(div().hint_text(&theme).child(label)),
                )
            })
    }
}

/// The dim track a bar is painted into.
fn track(theme: &Theme) -> gpui::Div {
    div()
        .w_full()
        .h(px(BAR_HEIGHT))
        .rounded_sm()
        .bg(rgb(theme.border))
}

/// One sub-bar of a [`Meter::segments`] strip: a fixed-height track filled from the
/// bottom, so the strip reads as a bar chart rather than as a row of dashes.
fn segment_bar(theme: &Theme, fraction: f32) -> impl IntoElement + use<> {
    let height = bar_fraction(fraction);
    let fill = MeterTone::of(fraction).color(theme);
    v_flex()
        .w(px(SEGMENT_WIDTH))
        .h(px(SEGMENT_HEIGHT))
        .justify_end()
        .rounded_sm()
        .bg(rgb(theme.border))
        .child(
            div()
                .w_full()
                .rounded_sm()
                .bg(rgb(fill))
                .h(relative(height)),
        )
}

/// The frame around a group of meters: a raised [`Card`] with a section header, an
/// optional summary line, and its stats laid out side by side.
///
/// This is the container the System tab's overview never had.
#[derive(IntoElement)]
pub struct StatCluster {
    title: Option<SharedString>,
    summary: Option<SharedString>,
    actions: Vec<AnyElement>,
    stats: Vec<AnyElement>,
}

impl StatCluster {
    /// An unframed-until-titled cluster. Give it a [`StatCluster::title`] — the design
    /// system's amendment asks for the header specifically.
    pub fn new() -> Self {
        Self {
            title: None,
            summary: None,
            actions: Vec::new(),
            stats: Vec::new(),
        }
    }

    /// The card's section header. Uppercased by [`Card`].
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A single line of context under the header — hostname, kernel, uptime.
    pub fn summary(mut self, summary: impl Into<SharedString>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// A control on the header row, right-aligned.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// One [`Meter`] (or anything else meter-shaped). Repeatable; they share the row's
    /// width equally.
    pub fn stat(mut self, stat: impl IntoElement) -> Self {
        self.stats.push(stat.into_any_element());
        self
    }
}

impl Default for StatCluster {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for StatCluster {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let mut card = Card::new();
        if let Some(title) = self.title {
            card = card.title(title);
        }
        for action in self.actions {
            card = card.action(action);
        }
        card.when_some(self.summary, |this, summary| {
            this.child(div().text_xs().text_color(rgb(theme.muted)).child(summary))
        })
        .child(h_flex().w_full().items_start().gap_6().children(self.stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{cosmos, cosmos_light, dusk, void};

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    #[test]
    fn an_unconfigured_device_reads_as_empty_not_as_a_divide_by_zero() {
        // Every machine without a swapfile hits `0 / 0` on the System tab's first paint.
        assert_eq!(ratio(0, 0), 0.0);
        assert_eq!(ratio(5, 0), 0.0);
    }

    #[test]
    fn ratio_is_used_over_total() {
        assert_eq!(ratio(0, 10), 0.0);
        assert_eq!(ratio(5, 10), 0.5);
        assert_eq!(ratio(10, 10), 1.0);
    }

    #[test]
    fn ratio_clamps_an_over_full_reading() {
        // `sysinfo` can report used > total across a probe boundary; a 1.4-wide bar
        // paints straight out of its card.
        assert_eq!(ratio(14, 10), 1.0);
    }

    #[test]
    fn empty_paints_as_exactly_empty() {
        // The floor below must not turn "nothing" into "a sliver of something".
        assert_eq!(bar_fraction(0.0), 0.0);
        assert_eq!(bar_fraction(-0.5), 0.0);
    }

    #[test]
    fn a_live_but_tiny_reading_is_never_invisible() {
        // 0.2% CPU proportionally painted is zero pixels — the same picture as a dead
        // probe. Anything strictly positive gets a visible sliver instead.
        assert_eq!(bar_fraction(0.002), MIN_VISIBLE);
        assert_eq!(bar_fraction(0.000_01), MIN_VISIBLE);
        assert!(bar_fraction(0.5) > MIN_VISIBLE);
    }

    #[test]
    fn a_full_bar_fills_its_track_and_no_further() {
        assert_eq!(bar_fraction(1.0), 1.0);
        assert_eq!(bar_fraction(1.5), 1.0);
        assert_eq!(bar_fraction(f32::INFINITY), 0.0);
    }

    #[test]
    fn a_non_finite_reading_cannot_reach_the_layout() {
        // `f32::clamp` propagates NaN, and gpui takes a NaN width without complaint and
        // then lays out nothing. Reject before clamping, not after.
        assert_eq!(bar_fraction(f32::NAN), 0.0);
        assert!(bar_fraction(f32::NAN).is_finite());
        assert_eq!(bar_fraction(f32::NEG_INFINITY), 0.0);
        assert_eq!(MeterTone::of(f32::NAN), MeterTone::Calm);
    }

    #[test]
    fn bar_fraction_is_monotonic() {
        let mut previous = bar_fraction(0.0);
        for step in 0..=100 {
            let width = bar_fraction(step as f32 / 100.0);
            assert!(
                width >= previous,
                "{step}%: {width} went backwards from {previous}"
            );
            previous = width;
        }
    }

    #[test]
    fn the_tone_ladder_switches_at_the_documented_thresholds() {
        // Ported from `systems_tab::bar_color`'s test, in fraction space.
        assert_eq!(MeterTone::of(0.0), MeterTone::Calm);
        assert_eq!(MeterTone::of(0.699), MeterTone::Calm);
        assert_eq!(MeterTone::of(CAUTION_AT), MeterTone::Caution);
        assert_eq!(MeterTone::of(0.899), MeterTone::Caution);
        assert_eq!(MeterTone::of(CRITICAL_AT), MeterTone::Critical);
        assert_eq!(MeterTone::of(1.0), MeterTone::Critical);
    }

    #[test]
    fn each_tone_maps_to_its_semantic_token_in_every_palette() {
        for t in palettes() {
            assert_eq!(MeterTone::Calm.color(&t), t.accent, "{}", t.name);
            assert_eq!(MeterTone::Caution.color(&t), t.warning, "{}", t.name);
            assert_eq!(MeterTone::Critical.color(&t), t.danger, "{}", t.name);
        }
    }

    #[test]
    fn the_three_tones_are_distinguishable_in_every_palette() {
        // A ladder whose rungs share a colour tells you nothing.
        for t in palettes() {
            let colors: Vec<u32> = ALL_METER_TONES.iter().map(|&x| x.color(&t)).collect();
            for (ix, &a) in colors.iter().enumerate() {
                for &b in &colors[ix + 1..] {
                    assert_ne!(a, b, "{}: two tones share {a:06x}", t.name);
                }
            }
        }
    }

    #[test]
    fn a_caller_can_override_the_derived_tone() {
        // A meter whose high end is good (free space, battery) must be able to say so.
        assert_eq!(
            Meter::new("battery", 0.95).resolved_tone(),
            MeterTone::Critical
        );
        assert_eq!(
            Meter::new("battery", 0.95)
                .tone(MeterTone::Calm)
                .resolved_tone(),
            MeterTone::Calm
        );
    }

    #[test]
    fn a_segment_strip_without_values_drops_its_label_too() {
        // An empty strip with a caption underneath is a caption for nothing.
        let meter = Meter::new("CPU", 0.1).segments(Vec::<f32>::new(), "per core");
        assert!(meter.segments.is_empty());
        assert_eq!(
            meter.segments_label.as_ref().map(SharedString::as_ref),
            Some("per core")
        );
        // The render path filters it; this pins the intent alongside.
        let with_values = Meter::new("CPU", 0.1).segments([0.2, 0.4], "per core");
        assert_eq!(with_values.segments.len(), 2);
    }
}
