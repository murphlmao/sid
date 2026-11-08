//! [`CellText`] — a table cell's text with no layout node of its own.
//!
//! Every sid table cell used to be `div().px_2().text_mono(..).text_color(..).child(text)`:
//! a whole `Div` (a taffy node, an `Interactivity`, three element phases) wrapped around one
//! text node, once per visible cell per frame. Priced at ~18.5K instructions per node per
//! frame (`docs/design/2026-07-27-table-frame-cost.md` §6), with 160-190 cells on screen.
//!
//! The div only ever carried three things, and none of them needs a node:
//!
//! * **padding** (`px_2`) moves to `Column::paddings`, which upstream's `render_cell`
//!   already applies to the cell it wraps around every `render_td` result — see
//!   [`FillColumns::inset_cells`](super::FillColumns::inset_cells);
//! * **font, size, colour, wrap, ellipsis** are *text style*, which gpui cascades through
//!   `Window::with_text_style` — exactly what a `Div` does around its children. `CellText`
//!   pushes the same refinement around a bare [`SharedString`] element, in all three
//!   phases, and so costs the text node and nothing else;
//! * `w_full`/`min_w(0)` were there to size the div; a bare text node in the (block) cell
//!   already takes the cell's width.

use gpui::{
    App, Bounds, Div, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, SharedString, Styled as _, TextLayout, TextStyleRefinement, Window, div, rgb,
};

use crate::styled::StyledExt as _;
use crate::theme::Theme;
use crate::typography::Typography as _;

/// One cell's text, styled on the body mono rung, in `ink`.
pub struct CellText {
    text: SharedString,
    style: TextStyleRefinement,
}

impl CellText {
    /// Mono body text inked `ink`. The cell it lands in is `whitespace: nowrap` and clips
    /// (`overflow_hidden`), as upstream's cell wrapper always is.
    pub fn new(theme: &Theme, ink: u32, label: impl Into<SharedString>) -> Self {
        Self::from_div(div().text_mono(theme).text_color(rgb(ink)), label.into())
    }

    /// Mono body text that wraps to the column and is cut with `…` instead of clipping
    /// mid-glyph.
    ///
    /// Upstream's cell sets `whitespace_nowrap`, and `Nowrap` pins gpui's text wrap width
    /// to `None`, so the ellipsis pass never gets a width to truncate to. Restoring
    /// `Normal` on the text style is what lets it (see `StyledExt::clamp_one_line`).
    pub fn ellipsized(theme: &Theme, ink: u32, label: impl Into<SharedString>) -> Self {
        Self::from_div(
            div()
                .whitespace_normal()
                .clamp_one_line()
                .text_mono(theme)
                .text_color(rgb(ink)),
            label.into(),
        )
    }

    fn from_div(mut styled: Div, text: SharedString) -> Self {
        Self {
            text,
            style: styled.style().text.clone(),
        }
    }

    /// The text style this cell pushes. Exposed for tests.
    pub fn text_style(&self) -> &TextStyleRefinement {
        &self.style
    }

    /// The label.
    pub fn text(&self) -> &SharedString {
        &self.text
    }
}

impl IntoElement for CellText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for CellText {
    type RequestLayoutState = TextLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, TextLayout) {
        let style = self.style.clone();
        let text = &mut self.text;
        window.with_text_style(Some(style), |window| {
            text.request_layout(id, inspector_id, window, cx)
        })
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut TextLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = self.style.clone();
        let text = &mut self.text;
        window.with_text_style(Some(style), |window| {
            text.prepaint(id, inspector_id, bounds, layout, window, cx)
        })
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut TextLayout,
        prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = self.style.clone();
        let text = &mut self.text;
        window.with_text_style(Some(style), |window| {
            text.paint(id, inspector_id, bounds, layout, prepaint, window, cx)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::cosmos;

    #[test]
    fn a_cell_is_mono_on_the_body_rung_and_inked_by_its_caller() {
        let t = cosmos();
        let cell = CellText::new(&t, t.muted, "0.0.0.0:22");
        let style = cell.text_style();
        assert_eq!(style.font_family.as_deref(), Some(crate::UI_MONO));
        assert_eq!(style.color, Some(gpui::Hsla::from(rgb(t.muted))));
        // Plain cells inherit upstream nowrap/clip: no clamp, no overflow suffix.
        assert_eq!(style.line_clamp, None);
        assert!(style.text_overflow.is_none());
        assert_eq!(cell.text().as_ref(), "0.0.0.0:22");
    }

    #[test]
    fn an_ellipsized_cell_undoes_the_tables_nowrap_and_cuts_with_a_suffix() {
        let t = cosmos();
        let cell = CellText::ellipsized(&t, t.fg, "ghcr.io/acme/a-very-long-image:latest");
        let style = cell.text_style();
        assert_eq!(style.line_clamp, Some(1));
        assert!(style.text_overflow.is_some());
        assert_eq!(style.white_space, Some(gpui::WhiteSpace::Normal));
    }
}
