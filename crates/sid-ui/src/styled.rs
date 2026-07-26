//! Layout and style shorthands — the design system's repeated measurements, typed once.
//!
//! The audit behind `docs/design/2026-07-26-ui-overhaul-plan.md` counted 74 hand-typed
//! `rounded_md()`, 41 hand-typed `px_3()` and 41 hand-written `hover(..)` chains across
//! 17 files. Each of those is a place where a call site could have typed something else,
//! and several did. These helpers give the spec exactly one spelling.

use gpui::{Div, InteractiveElement, Styled, div, rgb};

use crate::elevation::Elevation;
use crate::theme::Theme;

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

    /// A section header's type: `text_xs`, UPPERCASE is the caller's job (the string is
    /// theirs), `muted`.
    fn section_label(self, theme: &Theme) -> Self {
        self.text_xs().text_color(rgb(theme.muted))
    }

    /// Metadata / hint type: `text_xs` `muted`.
    fn hint_text(self, theme: &Theme) -> Self {
        self.text_xs().text_color(rgb(theme.muted))
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
}

impl<T: Styled + Sized> StyledExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::cosmos;
    use gpui::{Hsla, StyleRefinement};

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
        assert_eq!(
            s.text.clone().unwrap_or_default().color,
            Some(Hsla::from(rgb(t.muted)))
        );
        assert!(
            s.text.clone().unwrap_or_default().font_size.is_some(),
            "text_xs"
        );
        let h = style_of(div().hint_text(&t));
        assert_eq!(
            h.text.clone().unwrap_or_default().color,
            Some(Hsla::from(rgb(t.muted)))
        );
    }
}
