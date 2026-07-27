//! Layout and style shorthands — the design system's repeated measurements, typed once.
//!
//! The audit behind `docs/design/2026-07-26-ui-overhaul-plan.md` counted 74 hand-typed
//! `rounded_md()`, 41 hand-typed `px_3()` and 41 hand-written `hover(..)` chains across
//! 17 files. Each of those is a place where a call site could have typed something else,
//! and several did. These helpers give the spec exactly one spelling.

use gpui::{Div, InteractiveElement, Styled, div, rgb};

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
    fn a_clamped_line_asks_for_an_ellipsis_and_never_for_nowrap() {
        // The `white_space` assertion is the load-bearing one, and it is the whole
        // difference between this helper and gpui's `truncate()`: `Nowrap` pins
        // `wrap_width` to `None`, which makes `TextElement`'s measured-layout cache hit
        // on the first (MaxContent, nothing-to-truncate-to) pass and return before the
        // ellipsis is ever applied. Anyone "simplifying" this back to `.truncate()`
        // reintroduces text hard-clipped mid-glyph across every card in the app.
        let text = style_of(div().clamp_one_line())
            .text
            .clone()
            .unwrap_or_default();
        assert_eq!(text.line_clamp, Some(1), "clamped to one line");
        assert!(text.text_overflow.is_some(), "and cut with a suffix");
        assert_eq!(
            text.white_space, None,
            "nowrap defeats the truncation it comes with"
        );
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
        assert_eq!(
            s.text.clone().unwrap_or_default().font_size,
            Some(crate::typography::TypeRole::Label.size().into()),
        );
        let h = style_of(div().hint_text(&t));
        assert_eq!(
            h.text.clone().unwrap_or_default().color,
            Some(Hsla::from(rgb(t.muted)))
        );
    }
}
