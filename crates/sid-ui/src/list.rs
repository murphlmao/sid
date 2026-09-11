//! Lists and rows — the shape 41 hand-written `hover()` chains were each guessing at.
//!
//! `.interface-design/system.md` specifies the row in one line: *"Rows: `px_3 py_2`,
//! `rounded_md`, hover = `selection` fill; primary action inline, everything else in the
//! right-click menu."* The audit found that spelling retyped across 17 files — 74 literal
//! `rounded_md()`, 41 literal `px_3()`, 41 hand-written `hover(..)` — which is 41 chances
//! to type something slightly different, and several did.
//!
//! [`Row`] is that line, once. It also fixes the part the rule leaves implicit: a row has
//! **four slots in a fixed order** — a leading mark, the content, orientation metadata,
//! then engagement — so a screen full of rows cannot drift into "whatever order this call
//! site added children in". The SSH row shipped `global » ✎ folder ×`: five affordances in
//! 150px, orientation and engagement interleaved, in five visual languages.
//!
//! # Where the pixels come from
//!
//! [`RowPaint::resolve`] is the whole colour decision, as a pure function of (selected,
//! actionable, palette), unit-tested below. Everything else here is declarative glue.
//!
//! # Context menus
//!
//! [`Row::on_secondary_mouse_down`] registers the right-click, but a row cannot *own* its
//! menu: `gpui-component` 0.5.1 only exposes a menu builder through
//! `ContextMenuExt::context_menu`, which hardcodes the wrapper's element id to the literal
//! `"context-menu"` (`ContextMenu::menu` itself is private), so every row's wrapper would
//! collide on one `GlobalElementId` and share one menu state. The container therefore
//! attaches a single menu and each row reports itself as the target — the same pattern
//! `gpui-component`'s own `Table` uses.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Refineable as _, RenderOnce, Stateful,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};

use crate::bridge::{hover_of, pressed_of};
use crate::scale::scaled;
use crate::styled::{StyledExt as _, h_flex, v_flex};
use crate::theme::{self, Theme};
use crate::typography::{TypeRole, Typography};

/// gpui's default line height as a multiple of the font size: `Style::default()` sets
/// `line_height: phi()` (`gpui/src/style.rs`), and nothing in sid overrides it. A Body
/// line box is therefore 14 * 1.618 ~= 22.7px, not 14.
const LINE_HEIGHT: f32 = 1.618;

/// The box a [`Row`]'s leading mark is centred in — exactly one Body line tall, at 100%
/// zoom.
///
/// A row's content is a *stack*: an alias over a host, a workspace over a branch and a
/// dirty count. Centring the mark against that stack put a two-line row's dot between
/// its lines and a three-line row's dot on line two, so the one element whose whole job
/// is to mark the title pointed at the subtitle instead. Give the slot the height of the
/// first line and centre in that, and the offset falls out as half the line minus half
/// the mark — for any mark, without the row having to know how tall this one is.
fn leading_slot_height() -> f32 {
    f32::from(TypeRole::Body.size()) * LINE_HEIGHT
}

/// A click handler, shared so the builder can move it into gpui's own slot.
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A mouse-down handler, for the right-click that opens the container's menu.
type MouseDownHandler = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// The vertical stack [`Row`]s live in.
///
/// Two shapes, because a list is either the whole scrolling body of a pane or a short
/// block inside one, and the two need different geometry.
pub struct List;

impl List {
    /// A plain stack of rows, sized by its content. For short lists inside a card or a
    /// reading column.
    pub fn stack() -> Div {
        v_flex().gap_0p5()
    }

    /// A scrolling list body: takes the pane's free height and scrolls its rows.
    ///
    /// `min_h(0)` is load-bearing — a flex child's default minimum is content-sized, so
    /// without it the list grows past its parent instead of scrolling inside it.
    ///
    /// Returns the raw `Stateful<Div>` rather than a wrapper type so the container can
    /// still attach the things only it can own: the single `context_menu`, a
    /// capture-phase mouse handler, a key context.
    pub fn scrolling(id: impl Into<ElementId>) -> Stateful<Div> {
        List::stack()
            .id(id)
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .py_1()
    }
}

/// The resolved fills for one (selected, actionable, palette).
///
/// `None` means genuinely transparent — a resting row must not paint `bg`, or it punches
/// a canvas-coloured hole through any card it is listed inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowPaint {
    /// Rest fill.
    pub fill: Option<u32>,
    /// Fill under the pointer, or `None` for a row nothing can be done to.
    pub hover_fill: Option<u32>,
    /// Fill while held down, or `None` for an inert row. One rung *past* the hover in
    /// the same direction [`crate::Button`] takes: hover steps toward the palette's
    /// lightest ink, pressed toward its darkest, so "raised" and "pushed in" read the
    /// same way on a row as on a button.
    pub pressed_fill: Option<u32>,
}

impl RowPaint {
    /// The colour decision. Pure: palette in, tokens out.
    ///
    /// - A **selected** row is filled with `selection` — the design system's "active row"
    ///   token — and still moves under the pointer, so "selected" and "about to be
    ///   clicked" stay distinguishable.
    /// - An **actionable** row is transparent at rest and fills on hover. That hover is
    ///   the only thing telling a reader the row does anything at all.
    /// - An **inert** row does not move, under the pointer or under a press. A hover fill
    ///   on a row with no behaviour is a promise the UI cannot keep.
    pub fn resolve(selected: bool, actionable: bool, theme: &Theme) -> Self {
        match (selected, actionable) {
            (true, true) => RowPaint {
                fill: Some(theme.selection),
                hover_fill: Some(hover_of(theme, theme.selection)),
                pressed_fill: Some(pressed_of(theme, theme.selection)),
            },
            (true, false) => RowPaint {
                fill: Some(theme.selection),
                hover_fill: None,
                pressed_fill: None,
            },
            (false, true) => RowPaint {
                fill: None,
                hover_fill: Some(theme.selection),
                pressed_fill: Some(pressed_of(theme, theme.selection)),
            },
            (false, false) => RowPaint {
                fill: None,
                hover_fill: None,
                pressed_fill: None,
            },
        }
    }
}

/// One list row.
///
/// ```ignore
/// Row::new(("ssh-host", row_id))
///     .leading(StatusDot::new(("dot", row_id), state))
///     .child(v_flex().child(alias).child(address))
///     .meta(ScopeChip::global())
///     .action(Button::new(("connect", row_id), "connect").primary().small())
///     .on_secondary_mouse_down(cx.listener(..))
/// ```
///
/// Slots render left to right in a fixed order — leading mark, content, orientation,
/// engagement — and only the content slot grows.
#[derive(IntoElement)]
pub struct Row {
    id: ElementId,
    selected: bool,
    tab_index: Option<isize>,
    /// A group name, so descendants can style themselves against this row's hover.
    group: Option<gpui::SharedString>,
    leading: Option<AnyElement>,
    children: Vec<AnyElement>,
    meta: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
    on_secondary_mouse_down: Option<MouseDownHandler>,
    style: StyleRefinement,
}

impl Row {
    /// A resting, inert row. `id` must be unique within the window.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            selected: false,
            tab_index: None,
            group: None,
            leading: None,
            children: Vec::new(),
            meta: Vec::new(),
            actions: Vec::new(),
            on_click: None,
            on_secondary_mouse_down: None,
            style: StyleRefinement::default(),
        }
    }

    /// Fill this row with the `selection` token — the active/current item.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Put this row in the keyboard tab order, at `index`, and give it the focus ring.
    ///
    /// **Opt-in, and deliberately so.** A list is a *surface* in some screens and
    /// furniture in others: the save-to picker in a modal and the SSH connection cards
    /// are the thing the user came to operate, and a keyboard-first app that can only
    /// reach them with the mouse is not one — but a 400-row process table that enrols
    /// every row makes Tab useless everywhere else in the window. The screen decides,
    /// because only the screen knows which of its lists is the point.
    ///
    /// It also costs a hairline: a tab-stop row draws the transparent rest border the
    /// ring needs (see [`crate::StyledExt::focus_ring`]), so it stands 2px taller than
    /// an unenrolled one. Enrol a whole list or none of it, never half.
    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = Some(index);
        self
    }

    /// Name this row as a style group, so a descendant can react to the row's hover with
    /// `group_hover(name, ..)`.
    pub fn group(mut self, group: impl Into<gpui::SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// The fixed-width mark at the row's left edge: a status dot, a disclosure caret, a
    /// type icon. Does not grow.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// Orientation metadata, right-aligned before the actions: origin chips, counts,
    /// timestamps. Repeatable, rendered in the order added.
    ///
    /// Separate from [`Self::action`] on purpose: keeping "what this is" left of "what
    /// you can do to it" is what stops a row reading as five interchangeable widgets.
    pub fn meta(mut self, meta: impl IntoElement) -> Self {
        self.meta.push(meta.into_any_element());
        self
    }

    /// A control at the row's right edge. Repeatable, rendered in the order added.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// The row's own click. Nested [`crate::Button`]s consume their clicks, so a button
    /// inside a clickable row does not also fire this.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// The right-click. See the module docs for why the menu itself belongs to the
    /// container and not to the row.
    pub fn on_secondary_mouse_down(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_secondary_mouse_down = Some(Rc::new(handler));
        self
    }

    /// Whether anything can be done to this row — which decides the hover affordance and
    /// the pointer cursor.
    ///
    /// A row counts as actionable if it responds to the mouse *itself*. Containing a
    /// button is not enough: the button has its own hover, and lighting the whole row up
    /// for it would claim the row is clickable when only one 24px square is.
    pub fn is_actionable(&self) -> bool {
        self.on_click.is_some() || self.on_secondary_mouse_down.is_some()
    }
}

impl ParentElement for Row {
    /// The content slot — the only one that grows.
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Row {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Row {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let actionable = self.is_actionable();
        let paint = RowPaint::resolve(self.selected, actionable, &theme);
        let hover_fill = paint.hover_fill;

        let mut row = h_flex()
            .id(self.id)
            .w_full()
            .gap_2()
            .row_padding()
            // A row's default type. Without it a row was whatever its ancestor happened
            // to set, so the same `Row` rendered at three sizes in three screens; the
            // children that want the meta rung say so themselves, and win, because a
            // role sets every field it names.
            .text_body(&theme)
            .when_some(self.group, |this, group| this.group(group))
            .when_some(paint.fill, |this, fill| this.bg(rgb(fill)))
            .when(actionable, |this| this.cursor_pointer())
            // gpui permits exactly one hover style per element (it debug-asserts on a
            // second call), which is why this is the only place a row declares one.
            .when_some(hover_fill, |this, fill| {
                let fill = rgb(fill);
                this.hover(move |s| s.bg(fill))
            })
            .when_some(paint.pressed_fill, |this, fill| {
                let fill = rgb(fill);
                this.active(move |s| s.bg(fill))
            })
            // The ring's transparent hairline only appears on rows that can actually be
            // focused: on the other several hundred it would be 2px of height bought
            // for a state they can never enter.
            .when_some(self.tab_index, |this, index| {
                this.tab_index(index).focus_ring(&theme)
            })
            .when_some(self.leading, |this, leading| {
                this.child(
                    // `self_start` + a one-line-tall box: the mark aligns to the row's
                    // first line instead of to the middle of its content stack. See
                    // [`leading_slot_height`].
                    div()
                        .flex_none()
                        .self_start()
                        .h(scaled(leading_slot_height()))
                        .flex()
                        .items_center()
                        .child(leading),
                )
            })
            .child(
                // `min_w(0)` lets the content actually shrink: a flex item's default
                // minimum is content-sized, which would push the actions off the row
                // rather than truncate a long alias.
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .children(self.children),
            )
            .when(!self.meta.is_empty(), |this| {
                this.child(h_flex().flex_none().gap_1().children(self.meta))
            })
            .when(!self.actions.is_empty(), |this| {
                this.child(h_flex().flex_none().gap_1().children(self.actions))
            })
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |ev, window, cx| on_click(ev, window, cx))
            })
            .when_some(self.on_secondary_mouse_down, |this, handler| {
                this.on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                    handler(ev, window, cx)
                })
            });
        // The caller's own refinement lands last, so a `.mt_2()` typed at the call site
        // wins over the row's box.
        row.style().refine(&self.style);
        row
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

    #[test]
    fn a_leading_mark_sits_on_the_first_line_not_the_middle_of_the_stack() {
        // The defect: the leading slot centred against the *whole* content stack, so on
        // a two-line row the dot floated between the lines and on a three-line one it
        // sat level with line two — marking whatever happened to be in the middle
        // instead of the title it is there to mark. The slot is one Body line tall and
        // the mark centres in that, whatever the stack does.
        let slot = leading_slot_height();
        assert!(
            (slot - 22.7).abs() < 0.5,
            "the leading slot is one Body line, got {slot}"
        );
        // Half the line minus half the mark: an 8px StatusDot lands ~7.3px below the
        // top of the content box — on the title's line, not a line under it.
        assert!(((slot - 8.0) / 2.0 - 7.3).abs() < 0.5);
    }

    #[test]
    fn an_actionable_row_always_moves_under_the_pointer() {
        // The hover fill is the whole of a row's affordance — without it a list of
        // clickable rows is indistinguishable from a list of text.
        for t in palettes() {
            let resting = RowPaint::resolve(false, true, &t);
            assert_eq!(
                resting.hover_fill,
                Some(t.selection),
                "{}: hover is the selection fill",
                t.name
            );
            assert_ne!(resting.hover_fill, resting.fill, "{}: no shift", t.name);
        }
    }

    #[test]
    fn an_inert_row_promises_nothing() {
        // A hover fill on a row that does not respond is a lie the UI tells once per
        // row. Containing a button does not make the row itself clickable.
        for t in palettes() {
            let paint = RowPaint::resolve(false, false, &t);
            assert_eq!(paint.fill, None, "{}", t.name);
            assert_eq!(paint.hover_fill, None, "{}", t.name);
        }
        assert!(!Row::new("r").is_actionable());
        assert!(Row::new("r").on_click(|_, _, _| {}).is_actionable());
        assert!(
            Row::new("r")
                .on_secondary_mouse_down(|_, _, _| {})
                .is_actionable()
        );
        // A row whose only interactive content is a child button stays inert itself.
        assert!(!Row::new("r").action(div()).is_actionable());
    }

    #[test]
    fn a_selected_row_is_filled_and_still_reacts() {
        // "Selected" and "about to be clicked" are different facts and need different
        // pixels; collapsing them makes a keyboard-driven list unreadable under a mouse.
        for t in palettes() {
            let paint = RowPaint::resolve(true, true, &t);
            assert_eq!(paint.fill, Some(t.selection), "{}", t.name);
            assert_ne!(
                paint.hover_fill, paint.fill,
                "{}: a selected row must still shift on hover",
                t.name
            );
            // A selected but inert row (a read-only current item) holds still.
            assert_eq!(RowPaint::resolve(true, false, &t).hover_fill, None);
        }
    }

    #[test]
    fn a_resting_row_is_transparent_rather_than_canvas_coloured() {
        // `None`, never `theme.bg`: rows are listed inside cards as often as on the
        // canvas, and a bg-filled row would punch a hole through the card.
        for t in palettes() {
            for actionable in [true, false] {
                assert_eq!(
                    RowPaint::resolve(false, actionable, &t).fill,
                    None,
                    "{}: resting rows are transparent",
                    t.name
                );
            }
        }
    }

    #[test]
    fn every_fill_a_row_paints_is_visible_against_the_canvas() {
        for t in palettes() {
            for selected in [true, false] {
                let paint = RowPaint::resolve(selected, true, &t);
                for fill in [paint.fill, paint.hover_fill].into_iter().flatten() {
                    assert_ne!(fill, t.bg, "{}: invisible fill", t.name);
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
    fn the_slot_builders_land_where_they_claim() {
        let row = Row::new("r")
            .leading(div())
            .meta(div())
            .meta(div())
            .action(div())
            .selected(true);
        assert!(row.leading.is_some());
        assert_eq!(row.meta.len(), 2, "meta is repeatable");
        assert_eq!(row.actions.len(), 1);
        assert!(row.selected);
        assert!(row.children.is_empty(), "content comes from ParentElement");
    }
}
