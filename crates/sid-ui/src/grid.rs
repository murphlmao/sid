//! The card grid — the layout for a screen whose items are *objects*, not lines.
//!
//! `.interface-design/system.md`'s reading-column rule caps a list at 880px so a row's
//! actions never live a screen-width from its name. That rule is right about rows and
//! wrong about a 2000px window: the SSH home shipped 880px of list against 1120px of
//! nothing, and the verdict on it was *"a shitload of empty space where there shouldn't
//! be"*.
//!
//! A grid answers both halves at once. Each card is its own 300–420px reading column —
//! name, address and actions inside one box, never a screen apart — and the row of cards
//! consumes the whole width instead of stranding it. Wide windows get more columns,
//! narrow ones fewer, down to one, with no breakpoint list: `flex_wrap` plus a per-cell
//! flex-basis is the entire mechanism. [`CardGrid::columns`] is the *contract* that
//! mechanism implements, pinned as a pure function so the responsive claim is tested
//! rather than asserted in a comment.
//!
//! # Where the pixels come from
//!
//! [`CardPaint::resolve`] is the whole colour decision, as a pure function of (selected,
//! actionable, palette) — the [`crate::RowPaint`] of this module. The difference is the
//! rest state: a row rests transparent so it doesn't punch a hole through whatever it is
//! listed inside, and a card rests on `surface`, because being raised off the canvas is
//! what makes it a card.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Refineable as _, RenderOnce,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};

use crate::bridge::hover_of;
use crate::styled::v_flex;
use crate::theme::{self, Theme};

/// A click handler, shared so the builder can move it into gpui's own slot.
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A mouse-down handler, for the right-click that opens the container's menu.
type MouseDownHandler = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// The narrowest a card may be laid out at, and therefore the width that decides how
/// many columns a window gets. Below this a card can no longer hold a name, a
/// `user@host:port` and a row of controls without one of them wrapping.
pub const MIN_COL: Pixels = px(300.);

/// The widest a card may stretch to when its line is short.
///
/// Two caps are hiding in this one number. The obvious one: without any cap, two cards
/// in a 2000px window grow to 970px each and the grid degenerates back into the
/// full-bleed rows it replaced. The subtle one, and the reason this sits only ~13% above
/// [`MIN_COL`] rather than at a roomy 420px: flexbox distributes free space **per line**,
/// so a three-card group and a six-card group on the same screen stretch by different
/// amounts. At 420px that showed up as 420px cards above 318px cards — same screen, same
/// object, two sizes, which reads as an accident. Capping close to the basis keeps every
/// card on the screen within a few percent of every other one, at the price of a small
/// trailing gap on short lines. Uniformity is worth more than the last 20px.
pub const MAX_COL: Pixels = px(340.);

/// The gutter between cards, in both axes. Matches the `gap_3` the render path sets —
/// they must agree or [`CardGrid::columns`] is predicting a layout that isn't happening.
pub const GAP: Pixels = px(12.);

/// The resolved fills for one (selected, actionable, palette).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardPaint {
    /// Rest fill. Always a colour: a card is a raised object, never a transparent one.
    pub fill: u32,
    /// Fill under the pointer, or `None` for a card nothing can be done to.
    pub hover_fill: Option<u32>,
    /// The card's hairline.
    pub border: u32,
}

impl CardPaint {
    /// The colour decision. Pure: palette in, tokens out.
    ///
    /// - A **resting** card sits on `surface` inside a `border` hairline — the design
    ///   system's raised-object spelling.
    /// - A **selected** card takes the `selection` fill *and* a brighter outline. The
    ///   two fills are one rung apart by design (they are neighbours on the depth
    ///   ladder), so the fill alone is too quiet to carry "this is the one you picked";
    ///   the outline is what actually says it, and it is `muted` rather than `accent`
    ///   because the SSH home spends its single accent on quick-connect.
    /// - An **actionable** card moves under the pointer. An inert one does not: a hover
    ///   fill on a card with no behaviour is a promise the UI cannot keep.
    pub fn resolve(selected: bool, actionable: bool, theme: &Theme) -> Self {
        let fill = match selected {
            true => theme.selection,
            false => theme.surface,
        };
        CardPaint {
            fill,
            hover_fill: actionable.then(|| hover_of(theme, fill)),
            border: match selected {
                true => theme.muted,
                false => theme.border,
            },
        }
    }
}

/// One card in a [`CardGrid`]: a raised, selectable, clickable box.
///
/// The *contents* are the caller's — a host card, a workspace card and a saved-query
/// card share this container and nothing else. What this owns is the part every grid of
/// cards gets wrong independently: the fill ladder, the selected outline, the hover, the
/// pointer cursor, and the right-click hand-off.
///
/// ```ignore
/// GridCard::new(("ssh-card", row_id))
///     .selected(is_selected)
///     .on_click(cx.listener(..))          // `ev.click_count()` separates select from open
///     .on_secondary_mouse_down(cx.listener(..))
///     .child(header_line)
///     .child(address_line)
///     .child(action_row)
/// ```
///
/// # Context menus
///
/// Same constraint as [`crate::Row`], same shape: a card cannot own its menu
/// (`gpui-component` hardcodes one element id for every `context_menu` wrapper), so the
/// grid's container attaches a single menu and each card reports itself as the target
/// through [`Self::on_secondary_mouse_down`].
#[derive(IntoElement)]
pub struct GridCard {
    id: ElementId,
    selected: bool,
    children: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
    on_secondary_mouse_down: Option<MouseDownHandler>,
    style: StyleRefinement,
}

impl GridCard {
    /// A resting, inert card. `id` must be unique within the window.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            selected: false,
            children: Vec::new(),
            on_click: None,
            on_secondary_mouse_down: None,
            style: StyleRefinement::default(),
        }
    }

    /// Mark this card as the picked one — `selection` fill plus the brighter outline.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The card body's click. Nested [`crate::Button`]s consume their own clicks, so a
    /// control inside a card does not also fire this — which is what lets the body be a
    /// big soft target without a misclick ever reaching a control's verb.
    ///
    /// The handler receives the raw [`ClickEvent`]: `ev.click_count()` is how a screen
    /// separates "select" from "open".
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// The right-click. See the type docs for why the menu belongs to the container.
    pub fn on_secondary_mouse_down(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_secondary_mouse_down = Some(Rc::new(handler));
        self
    }

    /// Whether anything can be done to the card *itself* — which decides the hover
    /// affordance and the pointer cursor. Containing a button is not enough; the button
    /// has its own hover.
    pub fn is_actionable(&self) -> bool {
        self.on_click.is_some() || self.on_secondary_mouse_down.is_some()
    }
}

impl ParentElement for GridCard {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for GridCard {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for GridCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let actionable = self.is_actionable();
        let paint = CardPaint::resolve(self.selected, actionable, &theme);

        let mut card = v_flex()
            .id(self.id)
            // The cell stretches; the card fills it, so every card on a line ends level
            // with its neighbours however tall the tallest one is.
            .w_full()
            .h_full()
            .gap_1p5()
            .p_3()
            .rounded_md()
            .bg(rgb(paint.fill))
            .border_1()
            .border_color(rgb(paint.border))
            .text_color(rgb(theme.fg))
            .when(actionable, |this| this.cursor_pointer())
            // gpui permits exactly one hover style per element (it debug-asserts on a
            // second call), which is why this is the only place a card declares one.
            .when_some(paint.hover_fill, |this, fill| {
                let fill = rgb(fill);
                this.hover(move |s| s.bg(fill))
            })
            .children(self.children)
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |ev, window, cx| on_click(ev, window, cx))
            })
            .when_some(self.on_secondary_mouse_down, |this, handler| {
                this.on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                    handler(ev, window, cx)
                })
            });
        // The caller's own refinement lands last, so a call-site `.min_h()` wins over
        // the card's box.
        card.style().refine(&self.style);
        card
    }
}

/// A responsive, wrapping grid of cards.
///
/// Every child is wrapped in a cell that is `MIN_COL` wide by default, grows into the
/// line's free space and stops at `MAX_COL`. `flex_wrap` then does the responsiveness
/// for free: the line takes as many cells as fit at their basis and breaks. No media
/// queries, no measured width, no column count anywhere in the render path.
///
/// ```ignore
/// CardGrid::new().children(hosts.iter().map(host_card))
/// ```
#[derive(IntoElement)]
pub struct CardGrid {
    min_col: Pixels,
    max_col: Pixels,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

impl CardGrid {
    /// A grid with the house column widths ([`MIN_COL`]–[`MAX_COL`]).
    pub fn new() -> Self {
        Self {
            min_col: MIN_COL,
            max_col: MAX_COL,
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    /// Override the column width the grid wraps at. Narrower means more columns.
    pub fn min_col(mut self, min_col: Pixels) -> Self {
        self.min_col = min_col;
        self
    }

    /// Override the width a card may stretch to on a short line.
    pub fn max_col(mut self, max_col: Pixels) -> Self {
        self.max_col = max_col;
        self
    }

    /// How many columns `available` pixels of content width yields — the contract
    /// `flex_wrap` implements, and the only place the responsive behaviour is written
    /// down in numbers.
    ///
    /// Never zero: one card too wide for the window is laid out anyway (and shrinks),
    /// because the alternative is a screen that renders nothing.
    pub fn columns(available: Pixels, min_col: Pixels, gap: Pixels) -> usize {
        let available = f32::from(available).max(0.);
        let min_col = f32::from(min_col).max(1.);
        let gap = f32::from(gap).max(0.);
        // n columns need n*min_col + (n-1)*gap; solving for n and flooring is the same
        // as flooring (available + gap) / (min_col + gap).
        let fits = ((available + gap) / (min_col + gap)).floor();
        (fits as usize).max(1)
    }
}

impl Default for CardGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for CardGrid {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for CardGrid {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for CardGrid {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (min_col, max_col) = (self.min_col, self.max_col);
        // Deliberately NOT `h_flex()`: that centres its items, and a grid needs the
        // default `stretch` so short cards end level with tall ones on the same line.
        let mut grid = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .w_full()
            .gap_3()
            .children(self.children.into_iter().map(move |child| {
                div()
                    // A flex container, so the card inside stretches to the line height.
                    .flex()
                    .flex_basis(min_col)
                    .flex_grow()
                    // Nothing below `min_w(0)`: a window narrower than one column gets a
                    // squeezed card, never a horizontal scrollbar.
                    .min_w_0()
                    .max_w(max_col)
                    .child(child)
            }));
        grid.style().refine(&self.style);
        grid
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

    /// The content width a window of `window` pixels leaves the grid, after the SSH
    /// home's `px_4` gutters.
    fn content(window: f32) -> Pixels {
        px(window - 32.)
    }

    #[test]
    fn the_grid_fills_a_wide_window_instead_of_stranding_it() {
        // The whole point of the rebuild: at 2000px the rejected layout drew 880px of
        // list and 1120px of nothing. Six columns is the fix, stated as a number.
        assert_eq!(CardGrid::columns(content(2000.), MIN_COL, GAP), 6);
        assert_eq!(CardGrid::columns(content(2560.), MIN_COL, GAP), 8);
    }

    #[test]
    fn a_narrow_window_degrades_column_by_column() {
        // Two columns at a half-screen window, one on a phone-width pane — the
        // degradation `flex_wrap` gives for free, pinned so a change to MIN_COL or the
        // gap cannot silently move the breakpoints.
        assert_eq!(CardGrid::columns(content(900.), MIN_COL, GAP), 2);
        assert_eq!(CardGrid::columns(content(600.), MIN_COL, GAP), 1);
    }

    #[test]
    fn a_window_narrower_than_one_card_still_lays_one_out() {
        // Zero columns is a blank screen. A squeezed card is a readable one.
        assert_eq!(CardGrid::columns(px(120.), MIN_COL, GAP), 1);
        assert_eq!(CardGrid::columns(px(0.), MIN_COL, GAP), 1);
        assert_eq!(CardGrid::columns(px(-50.), MIN_COL, GAP), 1);
    }

    #[test]
    fn the_gap_is_counted_between_cards_and_not_after_the_last_one() {
        // Three 100px cards with a 10px gap need 320px, not 330: the difference is one
        // whole column at the boundary.
        assert_eq!(CardGrid::columns(px(320.), px(100.), px(10.)), 3);
        assert_eq!(CardGrid::columns(px(319.), px(100.), px(10.)), 2);
    }

    #[test]
    fn a_resting_card_is_raised_off_the_canvas() {
        // `surface`, never `bg`: a card that shares the canvas fill is not a card.
        for t in palettes() {
            let paint = CardPaint::resolve(false, true, &t);
            assert_eq!(paint.fill, t.surface, "{}", t.name);
            assert_ne!(paint.fill, t.bg, "{}: invisible card", t.name);
            assert_eq!(paint.border, t.border, "{}", t.name);
        }
    }

    #[test]
    fn a_selected_card_is_told_apart_by_its_outline_as_well_as_its_fill() {
        // `selection` and `surface` are neighbours on the depth ladder — in cosmos they
        // are 0x1c1c2c and 0x13131f, a delta the eye loses across a 6-column grid. The
        // outline is what actually carries the state, so assert it separates.
        for t in palettes() {
            let resting = CardPaint::resolve(false, true, &t);
            let selected = CardPaint::resolve(true, true, &t);
            assert_eq!(selected.fill, t.selection, "{}", t.name);
            assert_ne!(selected.fill, resting.fill, "{}: no fill shift", t.name);
            assert_ne!(
                selected.border, resting.border,
                "{}: a selected card needs an outline of its own",
                t.name
            );
            let delta = (brightness(selected.border) - brightness(resting.border)).abs();
            assert!(
                delta > 0.15,
                "{}: the selected outline separates by only {delta:.3}",
                t.name
            );
        }
    }

    #[test]
    fn the_selected_outline_is_never_the_screens_accent() {
        // One accent, used sparingly: the SSH home spends its only one on quick-connect,
        // so selection has to be told in neutrals.
        for t in palettes() {
            assert_ne!(
                CardPaint::resolve(true, true, &t).border,
                t.accent,
                "{}",
                t.name
            );
        }
    }

    #[test]
    fn an_actionable_card_moves_under_the_pointer_and_an_inert_one_does_not() {
        for t in palettes() {
            for selected in [true, false] {
                let live = CardPaint::resolve(selected, true, &t);
                let hover = live.hover_fill.expect("actionable cards hover");
                assert_ne!(hover, live.fill, "{}: no hover shift", t.name);
                assert_eq!(CardPaint::resolve(selected, false, &t).hover_fill, None);
            }
        }
        assert!(!GridCard::new("c").is_actionable());
        assert!(GridCard::new("c").on_click(|_, _, _| {}).is_actionable());
        assert!(
            GridCard::new("c")
                .on_secondary_mouse_down(|_, _, _| {})
                .is_actionable()
        );
        // A card whose only interactive content is a child button stays inert itself.
        assert!(!GridCard::new("c").child(div()).is_actionable());
    }

    #[test]
    fn every_fill_a_card_paints_is_visible_against_the_canvas() {
        for t in palettes() {
            for selected in [true, false] {
                let paint = CardPaint::resolve(selected, true, &t);
                for fill in [Some(paint.fill), paint.hover_fill].into_iter().flatten() {
                    let delta = (brightness(fill) - brightness(t.bg)).abs();
                    assert!(
                        delta > 0.005,
                        "{}: fill separates from the canvas by only {delta:.4}",
                        t.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_grid_keeps_its_column_bounds_in_the_right_order() {
        let grid = CardGrid::new();
        assert!(
            grid.min_col < grid.max_col,
            "a card cannot cap below its basis"
        );
        let custom = CardGrid::new().min_col(px(160.)).max_col(px(240.));
        assert_eq!(custom.min_col, px(160.));
        assert_eq!(custom.max_col, px(240.));
    }
}
