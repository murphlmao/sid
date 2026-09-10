//! Layout and style shorthands — the design system's repeated measurements, typed once.
//!
//! The audit behind `docs/design/2026-07-26-ui-overhaul-plan.md` counted 74 hand-typed
//! `rounded_md()`, 41 hand-typed `px_3()` and 41 hand-written `hover(..)` chains across
//! 17 files. Each of those is a place where a call site could have typed something else,
//! and several did. These helpers give the spec exactly one spelling.

use gpui::{Div, InteractiveElement, Styled, div, rgb, transparent_black};

use crate::elevation::Elevation;
use crate::theme::Theme;
use crate::typography::Typography;

/// A horizontal flex row, vertically centred — the default row shape.
#[inline]
pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

/// A vertical flex column.
#[inline]
pub fn v_flex() -> Div {
    div().flex().flex_col()
}

/// sid's house style shorthands, on every `gpui` element.
pub trait StyledExt: Styled + Sized {
    /// The design system's row box: `px_3 py_2`, `rounded_md`.
    fn row_padding(self) -> Self {
        self.px_3().py_2().rounded_md()
    }

    /// A hairline border on all four edges.
    fn hairline(self, theme: &Theme) -> Self {
        self.border_1().border_color(rgb(theme.border))
    }

    /// A hairline rule along the bottom edge — region separators.
    fn hairline_b(self, theme: &Theme) -> Self {
        self.border_b_1().border_color(rgb(theme.border))
    }

    /// A hairline rule along the top edge.
    fn hairline_t(self, theme: &Theme) -> Self {
        self.border_t_1().border_color(rgb(theme.border))
    }

    /// Sit this element on a rung of the depth ladder: its fill, plus its hairline if
    /// the rung has one. See [`Elevation`].
    fn elevation(self, rung: Elevation, theme: &Theme) -> Self {
        let filled = self.bg(rgb(rung.fill(theme)));
        match rung.border(theme) {
            Some(border) => filled.border_1().border_color(rgb(border)),
            None => filled,
        }
    }

    /// One line of text, cut with a real `…` when it does not fit.
    ///
    /// **Use this, not `gpui`'s `truncate()`.** They mean the same thing and only one of
    /// them works. `truncate()` sets `white_space: Nowrap` alongside the ellipsis, and
    /// `TextElement`'s measured-layout cache (see `elements/text.rs`) keys itself on
    /// `wrap_width`, which is *always* `None` under `Nowrap`. So the first measure pass —
    /// taffy's intrinsic sizing, where the available width is `MaxContent` and there is
    /// nothing to truncate *to* — caches the full-width layout, and the second pass, the
    /// one that finally knows how wide the element is, hits that cache and returns early
    /// without ever truncating. The text then gets hard-clipped by `overflow_hidden`,
    /// mid-glyph, with no ellipsis: an SSH card read `prod-eu-west-1-application-serv`
    /// jammed against its origin chip.
    ///
    /// Clamping to one line instead leaves `white_space` at `Normal`, so the second pass
    /// carries a real `wrap_width`, misses the cache, and truncates with the suffix.
    fn clamp_one_line(self) -> Self {
        self.line_clamp(1).text_ellipsis()
    }

    /// A section header's type. UPPERCASE is the caller's job (the string is theirs).
    ///
    /// Retained as an alias for [`Typography::text_label`] because the tab modules
    /// still in flight during the type-scale sweep call it; wave 2 deletes it. Note
    /// that the two used to be *identical* — `section_label` and `hint_text` both
    /// expanded to `text_xs` + `muted`, so a section header and a footnote were the
    /// same type. The roles they now forward to differ by weight.
    fn section_label(self, theme: &Theme) -> Self {
        self.text_label(theme)
    }

    /// Metadata / hint type — an alias for [`Typography::text_meta`]. See
    /// [`StyledExt::section_label`] for why it still exists.
    fn hint_text(self, theme: &Theme) -> Self {
        self.text_meta(theme)
    }

    /// The house hover affordance for an actionable row: a `selection` fill.
    ///
    /// `gpui` allows exactly one hover style per element (it `debug_assert!`s on a
    /// second call), so this is the *only* hover an element using it may declare.
    fn hover_fill(self, theme: &Theme) -> Self
    where
        Self: InteractiveElement,
    {
        let fill = rgb(theme.selection);
        self.hover(move |s| s.bg(fill))
    }

    /// The keyboard focus ring — the one shape a focused control takes in sid.
    ///
    /// An `accent` hairline while the element holds focus, over a **transparent hairline
    /// at rest**. Both halves matter:
    ///
    /// - The colour is [`focus_ring_color`] and nothing else. `bridge` already maps the
    ///   borrowed `gpui-component` widgets' `ring` token to the same value, so the ring
    ///   `Button` gets from the library and the ring an element gets from here cannot
    ///   drift apart.
    /// - The resting border is what makes the ring free: without it a focused row would
    ///   grow 2px on both axes the instant Tab reached it and shove its neighbours,
    ///   which reads as a rendering bug rather than as focus.
    ///
    /// Call it **before** any border colour the element draws for itself — a rest colour
    /// is an ordinary style field and overwrites this one, while the ring is a focus
    /// refinement gpui applies on top of whatever the rest style resolved to.
    ///
    /// The element still has to be focusable for a ring to ever show:
    /// `InteractiveElement::tab_index` (which also enrols it as a tab stop) or
    /// `track_focus`.
    fn focus_ring(self, theme: &Theme) -> Self
    where
        Self: InteractiveElement,
    {
        let ring = rgb(focus_ring_color(theme));
        self.border_1()
            .border_color(transparent_black())
            .focus(move |s| s.border_color(ring))
    }
}

/// The ink every focus ring in sid is drawn in: the `accent` token.
///
/// A function rather than an inline field read so "the focus ring is the accent" is one
/// statement with one test, and so the borrowed widgets' ring (`bridge`'s
/// `ThemeColor::ring`) has something to be checked against.
pub fn focus_ring_color(theme: &Theme) -> u32 {
    theme.accent
}

impl<T: Styled + Sized> StyledExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::cosmos;
    use gpui::{AlignSelf, Hsla, StyleRefinement};

    /// Read back a `Div`'s refined style — enough to assert that a helper set the
    /// fields it claims to, without a renderer.
    fn style_of(mut d: Div) -> StyleRefinement {
        d.style().clone()
    }

    #[test]
    fn row_padding_sets_the_documented_box() {
        let s = style_of(div().row_padding());
        assert!(s.padding.left.is_some(), "px");
        assert!(s.padding.right.is_some(), "px");
        assert!(s.padding.top.is_some(), "py");
        assert!(s.padding.bottom.is_some(), "py");
        assert!(s.corner_radii.top_left.is_some(), "rounded");
    }

    #[test]
    fn hairline_uses_the_border_token() {
        let t = cosmos();
        let s = style_of(div().hairline(&t));
        assert_eq!(s.border_color, Some(Hsla::from(rgb(t.border))));
        assert!(s.border_widths.top.is_some());
        assert!(s.border_widths.bottom.is_some());

        let b = style_of(div().hairline_b(&t));
        assert!(b.border_widths.bottom.is_some());
        assert!(b.border_widths.top.is_none(), "bottom edge only");
    }

    #[test]
    fn elevation_fills_from_the_ladder_and_skips_the_canvas_border() {
        let t = cosmos();
        let surface = style_of(div().elevation(Elevation::Surface, &t));
        assert_eq!(surface.background, Some(gpui::rgb(t.surface).into()));
        assert!(surface.border_widths.top.is_some(), "surface is bounded");

        let canvas = style_of(div().elevation(Elevation::Bg, &t));
        assert_eq!(canvas.background, Some(gpui::rgb(t.bg).into()));
        assert!(canvas.border_widths.top.is_none(), "canvas is unbounded");

        let well = style_of(div().elevation(Elevation::Well, &t));
        assert_eq!(well.background, Some(gpui::rgb(t.well).into()));
    }

    #[test]
    fn a_clamped_line_asks_for_an_ellipsis_and_never_for_nowrap() {
        // The `white_space` assertion is the load-bearing one, and it is the whole
        // difference between this helper and gpui's `truncate()`: `Nowrap` pins
        // `wrap_width` to `None`, which makes `TextElement`'s measured-layout cache hit
        // on the first (MaxContent, nothing-to-truncate-to) pass and return before the
        // ellipsis is ever applied. Anyone "simplifying" this back to `.truncate()`
        // reintroduces text hard-clipped mid-glyph across every card in the app.
        let text = style_of(div().clamp_one_line()).text.clone();
        assert_eq!(text.line_clamp, Some(1), "clamped to one line");
        assert!(text.text_overflow.is_some(), "and cut with a suffix");
        assert_eq!(
            text.white_space, None,
            "nowrap defeats the truncation it comes with"
        );
    }

    #[test]
    fn self_start_opts_out_of_the_parents_stretch() {
        // The defect this exists for: `flex_none` governs the main axis only, so in a
        // `v_flex` parent it does nothing about width and `align-items: stretch` pulls
        // the element to the column's full width. Both calls have to be present, and
        // they have to land in different fields.
        let s = style_of(div().flex_none().self_start());
        assert_eq!(s.align_self, Some(AlignSelf::Start), "the cross axis");
        assert_eq!(s.flex_grow, Some(0.), "and the main axis is still pinned");
        // `flex_none` alone is exactly the bug — assert it does *not* set align_self, so
        // nobody "simplifies" the pair back down to one call.
        assert_eq!(style_of(div().flex_none()).align_self, None);
    }

    #[test]
    fn the_focus_ring_costs_no_layout_until_it_appears() {
        // The whole reason the helper draws a *transparent* hairline at rest: a ring
        // that materialises a border only when focused grows the element by 2px on both
        // axes the moment Tab reaches it, and every neighbour jumps. The rest border has
        // to be there, and it has to be invisible.
        let t = cosmos();
        let s = style_of(div().focus_ring(&t));
        assert!(s.border_widths.top.is_some(), "a hairline at rest");
        assert!(s.border_widths.left.is_some());
        assert_eq!(
            s.border_color,
            Some(transparent_black()),
            "invisible until focused"
        );
    }

    #[test]
    fn an_elements_own_rest_border_wins_over_the_rings() {
        // The documented call order — `.focus_ring(theme)` first, the element's own
        // border colour after — has to actually resolve that way, or a segmented
        // control's chip loses its hairline to the ring's transparent rest colour.
        let t = cosmos();
        let s = style_of(div().focus_ring(&t).border_color(rgb(t.border)));
        assert_eq!(s.border_color, Some(Hsla::from(rgb(t.border))));
    }

    #[test]
    fn every_ring_is_the_accent_and_separates_from_the_chrome_it_rings() {
        // One ring, one token, four palettes. A ring the same colour as the border it
        // replaces is not a ring.
        for t in [
            cosmos(),
            crate::theme::void(),
            crate::theme::dusk(),
            crate::theme::cosmos_light(),
        ] {
            assert_eq!(focus_ring_color(&t), t.accent, "{}", t.name);
            for backdrop in [t.bg, t.surface, t.selection, t.border] {
                assert_ne!(
                    focus_ring_color(&t),
                    backdrop,
                    "{}: the ring dissolves into {backdrop:06x}",
                    t.name
                );
            }
        }
    }

    #[test]
    fn flex_helpers_set_their_axis() {
        let h = style_of(h_flex());
        assert_eq!(h.flex_direction, Some(gpui::FlexDirection::Row));
        let v = style_of(v_flex());
        assert_eq!(v.flex_direction, Some(gpui::FlexDirection::Column));
    }

    #[test]
    fn label_helpers_use_the_muted_token() {
        let t = cosmos();
        let s = style_of(div().section_label(&t));
        assert_eq!(s.text.clone().color, Some(Hsla::from(rgb(t.muted))));
        assert_eq!(
            s.text.clone().font_size,
            Some(crate::typography::TypeRole::Label.length().into()),
        );
        let h = style_of(div().hint_text(&t));
        assert_eq!(h.text.clone().color, Some(Hsla::from(rgb(t.muted))));
    }
}
