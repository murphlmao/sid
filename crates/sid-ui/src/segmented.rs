//! The segmented control — one row of mutually exclusive sub-views.
//!
//! The UI audit called the Network tab's `[Ports] [Services] [Interfaces] [Docker]
//! [Kubernetes]` strip *"the best-looking control in the app — bordered, padded, with an
//! active fill. It proves the codebase can do this and simply does not, anywhere else."*
//! This is that control, promoted out of `network_tab.rs` so every screen can have it,
//! with one defect corrected on the way in.
//!
//! # The defect
//!
//! The original painted the *unselected* segments with `theme.border` and the selected
//! one with `theme.selection`. On cosmos those are `0x1f1f2e` and `0x1c1c2c` — the
//! inactive chips are very slightly **brighter** than the active one, so the control
//! reads as four raised chips with one dent in it. The fix here is structural rather
//! than a swapped constant: the strip is a recessed [`Elevation::Well`] track, the
//! unselected segments are transparent, and only the selected one is a filled, bounded
//! chip. That gives the same read on all four palettes without a per-theme special case
//! — see [`segment_paint`], which is where the whole colour decision lives and is
//! exhaustively tested.
//!
//! # Using it
//!
//! ```ignore
//! SegmentedControl::new("system-subview")
//!     .segments(SystemSubView::ALL.map(|v| Segment::new(v.label()).icon(v.icon())))
//!     .selected(active as usize)
//!     .on_select(cx.listener(|this, ev: &SegmentSelect, _window, cx| { .. }))
//! ```

use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled, Window,
    prelude::FluentBuilder as _, rgb, transparent_black,
};

use crate::elevation::Elevation;
use crate::icon::Icon;
use crate::styled::{StyledExt as _, h_flex};
use crate::theme::{self, Theme};

/// A selection handler, shared so the render path can clone one per segment without
/// re-boxing it.
type SelectHandler = Rc<dyn Fn(&SegmentSelect, &mut Window, &mut App)>;

/// The rung the strip's track sits on: recessed, so the active chip reads as raised out
/// of it. Named here rather than inlined so a call site can match a surrounding surface.
pub const TRACK: Elevation = Elevation::Well;

/// The resolved colours for one segment. `None` means "genuinely transparent" — an
/// unselected segment must not paint a fill, or the track disappears behind the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentPaint {
    /// Rest fill. Only the selected segment has one.
    pub fill: Option<u32>,
    /// Label and icon colour.
    pub ink: u32,
    /// Hairline colour. Only the selected segment is bounded; the element still draws a
    /// transparent 1px border on the others so the two states are the same size.
    pub border: Option<u32>,
    /// Fill under the pointer. `None` for the selected segment, which is already filled
    /// and is not going anywhere when you point at it.
    pub hover_fill: Option<u32>,
}

/// The colour decision, as a pure function of (selected, palette).
///
/// The selected chip is `selection`-filled with the palette's strongest ink and a
/// hairline; everything else is transparent with `muted` ink. One rule, four palettes,
/// no special cases — see the module docs for the inversion this replaces.
pub fn segment_paint(selected: bool, theme: &Theme) -> SegmentPaint {
    if selected {
        SegmentPaint {
            fill: Some(theme.selection),
            ink: theme.fg_strong,
            border: Some(theme.border),
            hover_fill: None,
        }
    } else {
        SegmentPaint {
            fill: None,
            ink: theme.muted,
            border: None,
            hover_fill: Some(theme.selection),
        }
    }
}

/// Clamp a caller's selected index against the segments it actually supplied.
///
/// A control with no segments selects nothing; an out-of-range index selects the last
/// segment rather than nothing. The failure this prevents is a strip that renders with
/// **no** chip lit — indistinguishable from "the app forgot which view you are on" —
/// after a caller's enum and its label list drift apart.
pub fn resolve_selected(selected: usize, len: usize) -> Option<usize> {
    (len > 0).then(|| selected.min(len - 1))
}

/// Which segment was clicked.
///
/// A named event rather than a bare `usize` so a handler reads as an event handler and
/// composes with `Context::listener`, which is the only way a view-owned callback can
/// reach `&mut Self` — the same shape `Button::on_click` takes a `ClickEvent` in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentSelect {
    /// The clicked segment's index, into the order segments were added.
    pub index: usize,
}

/// One segment: a label, and optionally a leading icon.
pub struct Segment {
    label: SharedString,
    icon: Option<Icon>,
}

impl Segment {
    /// A labelled segment.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
        }
    }

    /// A leading icon, from the registry.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl<T: Into<SharedString>> From<T> for Segment {
    fn from(label: T) -> Self {
        Segment::new(label)
    }
}

/// A row of mutually exclusive sub-view choices. See the module docs.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: SharedString,
    segments: Vec<Segment>,
    selected: usize,
    on_select: Option<SelectHandler>,
}

impl SegmentedControl {
    /// An empty control. `id` must be unique within the window — each segment derives
    /// its own element id from it.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            segments: Vec::new(),
            selected: 0,
            on_select: None,
        }
    }

    /// Append one segment.
    pub fn segment(mut self, segment: impl Into<Segment>) -> Self {
        self.segments.push(segment.into());
        self
    }

    /// Append several segments, in order.
    pub fn segments(mut self, segments: impl IntoIterator<Item = impl Into<Segment>>) -> Self {
        self.segments
            .extend(segments.into_iter().map(std::convert::Into::into));
        self
    }

    /// Which segment is lit. Clamped by [`resolve_selected`].
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Called with the clicked segment — including the already-selected one, so a caller
    /// may treat a re-click as "reload this view" if it wants to.
    pub fn on_select(
        mut self,
        handler: impl Fn(&SegmentSelect, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let selected = resolve_selected(self.selected, self.segments.len());
        let on_select = self.on_select;
        let base = self.id;

        h_flex()
            .flex_none()
            .gap_1()
            .p_1()
            .rounded_md()
            .elevation(TRACK, &theme)
            .children(self.segments.into_iter().enumerate().map(|(ix, segment)| {
                let paint = segment_paint(selected == Some(ix), &theme);
                let on_select = on_select.clone();
                let id: ElementId = SharedString::from(format!("{base}-{ix}")).into();
                h_flex()
                    .id(id)
                    .gap_1p5()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    // Always a 1px border, transparent when unselected: without it the
                    // chips would resize by 2px as the selection moves.
                    .border_1()
                    .border_color(
                        paint
                            .border
                            .map_or_else(transparent_black, |b| rgb(b).into()),
                    )
                    .when_some(paint.fill, |this, fill| this.bg(rgb(fill)))
                    .when_some(paint.hover_fill, |this, fill| {
                        this.hover(move |s| s.bg(rgb(fill)))
                    })
                    .text_color(rgb(paint.ink))
                    .when_some(segment.icon, |this, icon| {
                        this.child(icon.small().text_color(rgb(paint.ink)))
                    })
                    .child(segment.label)
                    .when_some(on_select, |this, on_select| {
                        this.on_click(move |_ev: &ClickEvent, window, cx| {
                            on_select(&SegmentSelect { index: ix }, window, cx);
                        })
                    })
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::brightness;
    use crate::theme::{cosmos, cosmos_light, dusk, void};

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    /// How far a segment's ink stands off whatever it is painted on.
    fn contrast(ink: u32, backdrop: u32) -> f32 {
        (brightness(ink) - brightness(backdrop)).abs()
    }

    #[test]
    fn only_the_selected_segment_is_filled_and_bounded() {
        // The whole read of the control: one chip, on a track. If the unselected
        // segments also paint a fill, the track vanishes and it is five buttons again.
        for t in palettes() {
            let on = segment_paint(true, &t);
            let off = segment_paint(false, &t);
            assert_eq!(on.fill, Some(t.selection), "{}: selected fill", t.name);
            assert_eq!(off.fill, None, "{}: unselected is transparent", t.name);
            assert_eq!(on.border, Some(t.border), "{}: selected hairline", t.name);
            assert_eq!(off.border, None, "{}: unselected is unbounded", t.name);
        }
    }

    #[test]
    fn the_selected_chip_is_visible_against_its_own_track() {
        // The defect this control was promoted to fix, pinned: on cosmos the original
        // painted inactive chips `border` (0x1f1f2e) and the active one `selection`
        // (0x1c1c2c) — the *inactive* ones came out brighter. The chip has to separate
        // from the track it sits in, in every palette.
        for t in palettes() {
            let track = TRACK.fill(&t);
            let chip = segment_paint(true, &t).fill.expect("selected is filled");
            assert!(
                contrast(chip, track) > 0.02,
                "{}: chip {chip:06x} is invisible on track {track:06x}",
                t.name
            );
        }
    }

    #[test]
    fn the_active_label_reads_louder_than_the_inactive_ones() {
        // "Which view am I on" must be answerable from the type alone, at a glance,
        // even before the fill registers.
        for t in palettes() {
            let on = segment_paint(true, &t);
            let off = segment_paint(false, &t);
            assert_ne!(on.ink, off.ink, "{}: same ink both ways", t.name);
            let on_contrast = contrast(on.ink, on.fill.expect("filled"));
            let off_contrast = contrast(off.ink, TRACK.fill(&t));
            assert!(
                on_contrast > off_contrast,
                "{}: active label ({on_contrast:.2}) is quieter than inactive \
                 ({off_contrast:.2})",
                t.name
            );
        }
    }

    #[test]
    fn every_label_stays_readable_in_every_palette() {
        for t in palettes() {
            let on = segment_paint(true, &t);
            let off = segment_paint(false, &t);
            assert!(
                contrast(on.ink, on.fill.expect("filled")) > 0.3,
                "{}: active label",
                t.name
            );
            assert!(
                contrast(off.ink, TRACK.fill(&t)) > 0.3,
                "{}: inactive label",
                t.name
            );
        }
    }

    #[test]
    fn only_the_unselected_segments_respond_to_hover() {
        for t in palettes() {
            assert_eq!(
                segment_paint(false, &t).hover_fill,
                Some(t.selection),
                "{}: an unselected segment is a control",
                t.name
            );
            assert_eq!(
                segment_paint(true, &t).hover_fill,
                None,
                "{}: the selected segment has nowhere to go",
                t.name
            );
        }
    }

    #[test]
    fn an_out_of_range_selection_still_lights_a_segment() {
        // A strip with no chip lit is indistinguishable from "the app lost track of
        // which view you are on" — clamp instead.
        assert_eq!(resolve_selected(0, 3), Some(0));
        assert_eq!(resolve_selected(2, 3), Some(2));
        assert_eq!(resolve_selected(3, 3), Some(2));
        assert_eq!(resolve_selected(usize::MAX, 3), Some(2));
    }

    #[test]
    fn a_control_with_no_segments_selects_nothing() {
        assert_eq!(resolve_selected(0, 0), None);
        assert_eq!(resolve_selected(7, 0), None);
    }

    #[test]
    fn segments_can_be_declared_as_bare_strings() {
        let control = SegmentedControl::new("t").segments(["Processes", "Config files"]);
        assert_eq!(control.segments.len(), 2);
        assert_eq!(control.segments[0].label.as_ref(), "Processes");
        assert!(control.segments[0].icon.is_none());
    }
}
