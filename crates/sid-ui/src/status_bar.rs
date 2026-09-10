//! The app-wide bottom status bar — the strip that says what sid is holding.
//!
//! Every real ops tool has one, and sid did not, so three facts had no home or the
//! wrong one: the secrets backend was a bare `!` pill in the top-right that needed a
//! click to explain itself, the number of open SSH sessions and the live DB connection
//! were only visible from *inside* those tabs, and the UI zoom had no readout at all.
//! A bottom bar is where an ops tool puts ambient truth: never the thing you look at,
//! always the thing you can look at.
//!
//! # The decisions
//!
//! - **Hierarchy.** This is the lowest tier in the app and it must stay there — it has
//!   to fail the squint test. Meta type throughout (12px, normal, `muted`), a `faint`
//!   `·` between items, no boxes. The only element allowed to raise its voice is a
//!   *degraded* state, which spends `warning` and a glyph, because a warning that
//!   whispers was not delivered.
//! - **Depth.** `surface` fill and a hairline **top** border, the mirror of the top
//!   chrome bar's fill and hairline bottom. The app is bracketed by chrome at the same
//!   elevation; the tab between them is the canvas. Borders and surface shifts only —
//!   no shadow (`.interface-design/system.md`).
//! - **Spacing.** [`BAR_H`] tall, `px_2`, `gap_1p5` between items — the tightest rhythm
//!   in the app, on purpose: a status strip that breathes like a toolbar reads as a
//!   second toolbar.
//! - **Zoom.** Every length goes through [`scaled`], so the strip grows with the rest of
//!   the UI instead of becoming a hairline at 200%.
//!
//! There are exactly two types here — a bar and an item — and no third. An item is a
//! word, optionally with a [`StatusDot`] or an [`Icon`], optionally clickable. Anything
//! that wants more than that wants a different surface.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, InteractiveElement as _, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _, rgb,
};

use crate::badge::{BadgeFill, BadgeTone};
use crate::icon::Icon;
use crate::scale::scaled;
use crate::status_dot::{ConnectionState, StatusDot};
use crate::styled::{StyledExt as _, h_flex};
use crate::theme::{self, Theme};
use crate::typography::Typography as _;

/// The strip's height at 100% zoom.
///
/// Sized off the type, not chosen: a Meta line box is `rems(1.25)` = 20px, and 3px of
/// air above and below is the least that keeps the row from reading as clipped. The top
/// chrome bar is 42px — the bottom bar is deliberately a little over half that, because
/// it is read at a glance and never aimed at.
const BAR_H: f32 = 26.;

/// How wide one item's label may get before it elides. A DB connection name is
/// user-supplied and unbounded (`analytics-replica-eu-west-1-readonly`); left free it
/// takes the bar's whole width and pushes the right-hand group off the window, which is
/// the same defect the top bar's scope chips were fixed for.
const LABEL_MAX_W: f32 = 240.;

/// Boxed so an item can carry a `cx.listener(..)` closure — the same shape
/// [`crate::Button`] and [`crate::list::Row`] use.
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One fact on the status bar: a word, optionally marked and optionally clickable.
///
/// ```ignore
/// StatusItem::new("status-keyring", "keyring").tone(BadgeTone::Success)
/// StatusItem::new("status-db", "db: analytics").dot(ConnectionState::Live)
/// StatusItem::new("status-zoom", "125%").on_click(cx.listener(..))
/// ```
///
/// The `id` is required for the same reason [`StatusDot`]'s is: a dot hangs a tooltip
/// off it, and a clickable strip element with no name cannot be one.
#[derive(IntoElement)]
pub struct StatusItem {
    id: ElementId,
    label: SharedString,
    tone: BadgeTone,
    dot: Option<ConnectionState>,
    icon: Option<Icon>,
    on_click: Option<ClickHandler>,
}

impl StatusItem {
    /// A plain item: `muted` text, no mark, not clickable.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            tone: BadgeTone::Neutral,
            dot: None,
            icon: None,
            on_click: None,
        }
    }

    /// Spend a semantic ink on this item. [`BadgeTone::Neutral`] (the default) is
    /// `muted` furniture; the others are the palette's success/warning/danger/accent.
    ///
    /// Reuses the badge vocabulary rather than growing a second one — a status word and
    /// a status chip are the same statement at two sizes, and they must not be able to
    /// disagree about what "degraded" is coloured.
    pub fn tone(mut self, tone: BadgeTone) -> Self {
        self.tone = tone;
        self
    }

    /// Lead with a connection-state mark. For facts that *are* a connection.
    pub fn dot(mut self, state: ConnectionState) -> Self {
        self.dot = Some(state);
        self
    }

    /// Lead with a glyph from the registry. For facts that are not a connection but
    /// still need to catch the eye — a degraded backend, chiefly.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Make the item actionable: hover fill, pointer cursor, click handler.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for StatusItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        // `Outline` is the fill that is only ink and edge — exactly the tone's own
        // colour, with `Neutral` resolving to `muted`.
        let ink = self.tone.paint(BadgeFill::Outline, &theme).ink;
        let dot = self
            .dot
            .map(|state| StatusDot::new(self.id.clone(), state).into_any_element());
        h_flex()
            .id(self.id)
            .flex_none()
            .gap_1p5()
            .px_1p5()
            .rounded_md()
            // The role sets the measurement; the tone sets the ink, after it.
            .text_meta(&theme)
            .text_color(rgb(ink))
            .children(dot)
            .children(self.icon.map(|icon| icon.small().text_color(rgb(ink))))
            .child(
                // gpui reports a text element's min-content width as its whole string,
                // so an unbounded label cannot shrink — it walks out of the bar and
                // takes the right-hand group with it. Cap and clamp, same as the top
                // bar's scope chips.
                div()
                    .min_w_0()
                    .max_w(scaled(LABEL_MAX_W))
                    .clamp_one_line()
                    .child(self.label),
            )
            .when_some(self.on_click, |this, on_click| {
                this.cursor_pointer()
                    .hover_fill(&theme)
                    .on_click(move |ev, window, cx| on_click(ev, window, cx))
            })
    }
}

/// The full-width strip at the bottom of the window: a left group of facts about what
/// sid is holding, a right group of facts about the view.
///
/// ```ignore
/// StatusBar::new()
///     .left(StatusItem::new("status-keyring", "keyring").tone(BadgeTone::Success))
///     .right(StatusItem::new("status-zoom", "125%").on_click(reset))
/// ```
#[derive(IntoElement)]
pub struct StatusBar {
    left: Vec<AnyElement>,
    right: Vec<AnyElement>,
}

impl StatusBar {
    /// An empty bar. Renders as a bare strip — which is correct: the strip is chrome,
    /// and chrome does not come and go with its contents.
    pub fn new() -> Self {
        Self {
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    /// Append to the left group — what sid is holding. Repeatable, in order.
    pub fn left(mut self, item: impl IntoElement) -> Self {
        self.left.push(item.into_any_element());
        self
    }

    /// Append to the right group — what the *view* is doing. Repeatable, in order.
    pub fn right(mut self, item: impl IntoElement) -> Self {
        self.right.push(item.into_any_element());
        self
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        h_flex()
            .w_full()
            // The bar's height is not negotiable, for the same reason the top bar's is
            // not: a tab whose content reports a taller intrinsic height than the window
            // has left would otherwise take the difference out of the chrome.
            .flex_shrink_0()
            .h(scaled(BAR_H))
            .px_2()
            .justify_between()
            .bg(rgb(theme.surface))
            .hairline_t(&theme)
            .text_meta(&theme)
            // The left group is the one allowed to give: it holds the unbounded strings
            // (a DB connection's name), and losing the tail of one of those is better
            // than losing the zoom readout off the right edge.
            .child(
                group(self.left, &theme)
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden(),
            )
            .child(group(self.right, &theme).flex_none())
    }
}

/// One side of the bar: the items, with a `faint` `·` between each pair.
///
/// A middle dot rather than a hairline rule: at 26px a vertical rule is a third of the
/// bar's height and reads as a table gridline, which is a different (and heavier) depth
/// vocabulary than this strip is allowed to spend.
fn group(items: Vec<AnyElement>, theme: &Theme) -> Div {
    let mut row = h_flex().gap_1p5();
    for (ix, item) in items.into_iter().enumerate() {
        if ix > 0 {
            row = row.child(div().flex_none().text_color(rgb(theme.faint)).child("·"));
        }
        row = row.child(item);
    }
    row
}
