//! Cards and sections — the container that gives a screen a reading order.
//!
//! `settings_tab.rs:96`'s `section_header` is the design system's mandated pattern:
//! `text_xs` UPPERCASE `muted`, optionally `· count`. It is also a file-private free
//! function, which is why the System, Network, SSH and Workspaces tabs each reimplement
//! it inline — four copies of a three-line rule, drifting. This is that function, made
//! importable, plus the frame the System tab's unframed meter cluster is missing.
//!
//! Three shapes, one type:
//!
//! - [`Card::new`] — a `surface` fill with a hairline. Data clusters, forms, grouped
//!   controls: anything that should read as *raised off* the canvas.
//! - [`Card::section`] — header and content, no chrome. A titled block inside a reading
//!   column, where a second border would only add noise.
//! - [`Card::panel`] — a card sized by its container rather than by its contents, whose
//!   body a scrolling list can actually fill. See below.
//!
//! All three take optional header actions, which is what keeps a section's controls
//! anchored to the section instead of drifting to the far edge of an invisible 880px
//! column (the System tab's orphaned `COMMON` list).
//!
//! # Why `panel` is a third shape and not a flag
//!
//! The first two wrap their children in a `v_flex().gap_2()` with padding, which is
//! right for a stack of rows and wrong for exactly one thing: a child that wants to
//! *scroll*. An `overflow_y_scroll` child needs a definite height to scroll **within**,
//! and it gets one only if every ancestor between it and the sized container passes the
//! height down — `flex_1` plus `min_h_0`, all the way. A gap-2 content wrapper with no
//! flex sizing is a hard stop: the list inside it resolves to its content height, grows
//! past the card, and nothing scrolls. `db_tab.rs` worked around this with a local
//! `panel_header()` and a hand-built body; [`Card::panel`] is that arrangement, once.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, Refineable as _, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::elevation::Elevation;
use crate::styled::{StyledExt as _, h_flex, v_flex};
use crate::theme;
use crate::typography::Typography;

/// The header line: UPPERCASE title, and the count appended with the design system's
/// middot separator when there is one.
///
/// `Some(0)` still prints: "0 hosts" is information, and a header that silently drops
/// its count when a list empties reads as a rendering bug.
pub fn header_text(title: &str, count: Option<usize>) -> String {
    match count {
        Some(count) => format!("{} · {count}", title.to_uppercase()),
        None => title.to_uppercase(),
    }
}

/// Whether a card draws chrome of its own, and how its body is sized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardChrome {
    /// `surface` fill + hairline: raised off the canvas. Body sized by its contents.
    Raised,
    /// Header and content only: a titled block on whatever it sits on.
    Flat,
    /// `surface` fill + hairline, a ruled header, and a body sized by the *container*
    /// — see the module docs.
    Panel,
}

impl CardChrome {
    /// Whether this shape paints a fill and a hairline.
    const fn is_raised(self) -> bool {
        matches!(self, CardChrome::Raised | CardChrome::Panel)
    }

    /// Whether this shape's body takes its height from the container instead of from
    /// its own contents. Only a body that does can hold something that scrolls.
    const fn body_fills_container(self) -> bool {
        matches!(self, CardChrome::Panel)
    }
}

/// A panel's body: the wrapper a scrolling child needs above it.
///
/// `flex_1` so it takes the height the panel has left after the header, and `min_h_0`
/// so it is allowed to be *shorter* than its contents — without that second call a flex
/// item's minimum is its content height, the body grows to fit the whole list, and the
/// `overflow_y_scroll` inside it never has a smaller box to scroll within. No gap and
/// no padding: a scrolling list draws its own rows to the panel's edges, and a gap here
/// would show through under the last visible row.
fn panel_body() -> gpui::Div {
    v_flex().flex_1().min_h_0()
}

/// A panel's header: the ruled strip the body scrolls under.
///
/// `flex_none` is the other half of [`panel_body`]'s `flex_1` — a header that could
/// shrink would give the body a height that changes as the list does. `px_3().py_2()`
/// is the same box `Toolbar` draws (`toolbar.rs`) — the two used to disagree (`py(6.)`
/// here, `py_2` there), which is exactly the "different box" a panel's header and a
/// bare `Toolbar` should never have, since the whole point of this shape is that a
/// panel's header *is* the toolbar row. `sid_ui::Toolbar` stays a separate type only
/// because `systems_tab.rs` still renders one outside a `Card::panel`; the box is the
/// single contract either way.
fn panel_header() -> gpui::Div {
    h_flex()
        .flex_none()
        .justify_between()
        .gap_3()
        .px_3()
        .py_2()
}

/// A titled container. See the module docs for the two shapes.
#[derive(IntoElement)]
pub struct Card {
    chrome: CardChrome,
    title: Option<SharedString>,
    count: Option<usize>,
    actions: Vec<AnyElement>,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

impl Card {
    /// A raised card: `surface` fill, hairline border, padded body.
    pub fn new() -> Self {
        Self {
            chrome: CardChrome::Raised,
            title: None,
            count: None,
            actions: Vec::new(),
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    /// A flat section: the same header, no fill and no border. This is the importable
    /// form of `settings_tab::section_header`.
    pub fn section(title: impl Into<SharedString>) -> Self {
        Self {
            chrome: CardChrome::Flat,
            title: Some(title.into()),
            count: None,
            actions: Vec::new(),
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    /// A panel: a raised card whose body is sized by its container, so a `flex_1` +
    /// `overflow_y_scroll` child inside it has a height to scroll within.
    ///
    /// The panel itself still has to be *given* a height by its own parent — `flex_1`
    /// in a column, or an explicit one. What this shape guarantees is that the height
    /// reaches the body instead of stopping at a content wrapper. Note the shape also
    /// drops the body padding: a scrolling list paints its own rows to the edge.
    pub fn panel(title: impl Into<SharedString>) -> Self {
        Self {
            chrome: CardChrome::Panel,
            title: Some(title.into()),
            count: None,
            actions: Vec::new(),
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    /// Set the header title. A card without one renders no header row at all.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Append `· n` to the header.
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// A control on the header row, right-aligned. Repeatable; they render in the order
    /// added. Anchoring actions to the header is what keeps them next to the thing they
    /// act on.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl Default for Card {
    fn default() -> Self {
        Self::new()
    }
}

/// A card is a box a layout has to be able to size: side-by-side cards need `flex_1` to
/// share a row and `h_full` to end level with each other, and neither is expressible
/// through a wrapper (a wrapper stretches, its card child does not follow). The
/// refinement is applied *last*, so a call site's `.flex_1()` wins over the card's own
/// padding and fill.
impl Styled for Card {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Card {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Card {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let chrome = self.chrome;
        let panel = chrome.body_fills_container();
        let header = self.title.map(|title| {
            let bar = if panel {
                panel_header().hairline_b(&theme)
            } else {
                h_flex().justify_between().gap_3()
            };
            bar.child(
                // `flex_1` + `min_w_0` + a clamp: a header is one line, and gpui reports
                // a text element's min-content width as its *whole string*, so without
                // the pair a long title pushes the header's actions off the card's right
                // edge instead of eliding.
                div()
                    .flex_1()
                    .min_w_0()
                    .clamp_one_line()
                    .section_label(&theme)
                    .child(header_text(&title, self.count)),
            )
            .when(!self.actions.is_empty(), |this| {
                this.child(h_flex().flex_none().gap_1().children(self.actions))
            })
        });

        let mut card = v_flex()
            .when(chrome.is_raised(), |this| {
                this.elevation(Elevation::Surface, &theme)
            })
            // A panel does its padding per region (the header's own, the body's none),
            // because a scrolling body has to reach the card's edges.
            .when(!panel, |this| this.p_3().gap_2())
            .when(panel, |this| this.min_h_0())
            .children(header)
            .child(if panel {
                panel_body().children(self.children)
            } else {
                v_flex().gap_2().children(self.children)
            })
            // A card's own type, so its body does not inherit whatever size and colour
            // the surrounding chrome happened to be painted in. It used to set only the
            // colour, which left the same card rendering at 12px inside a `text_xs`
            // panel and 16px at the root.
            .text_body(&theme);
        card.style().refine(&self.style);
        card
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_is_uppercase() {
        // The design system's rule, and the reason four tabs each retyped it.
        assert_eq!(header_text("processes", None), "PROCESSES");
        assert_eq!(header_text("Common", None), "COMMON");
        assert_eq!(header_text("SSH / SFTP", None), "SSH / SFTP");
    }

    #[test]
    fn a_count_is_appended_with_the_middot() {
        assert_eq!(header_text("processes", Some(132)), "PROCESSES · 132");
        assert_eq!(header_text("hosts", Some(1)), "HOSTS · 1");
    }

    #[test]
    fn a_zero_count_still_prints() {
        // "0 hosts" is information. A header that drops its count when the list
        // empties reads as a broken render, which is exactly the complaint the
        // Workspaces empty state drew.
        assert_eq!(header_text("hosts", Some(0)), "HOSTS · 0");
    }

    #[test]
    fn a_card_takes_a_style_refinement() {
        // Side-by-side cards need to be sizeable by their layout; a wrapper div cannot
        // do it for them, because the wrapper stretches and the card inside does not.
        let mut card = Card::new().flex_1().h_full();
        let style = card.style().clone();
        assert!(style.flex_grow.is_some(), "flex_1");
        assert!(style.size.height.is_some(), "h_full");
    }

    #[test]
    fn the_three_shapes_differ_only_in_chrome() {
        assert_eq!(Card::new().chrome, CardChrome::Raised);
        assert_eq!(Card::section("x").chrome, CardChrome::Flat);
        assert_eq!(Card::panel("x").chrome, CardChrome::Panel);
        assert_eq!(Card::section("x").title.expect("titled").as_ref(), "x");
        assert_eq!(Card::panel("x").title.expect("titled").as_ref(), "x");
        assert!(Card::new().title.is_none());
    }

    #[test]
    fn only_a_panel_hands_its_height_to_its_body() {
        // The distinction the third shape exists for. A card sizes its body by the
        // body's contents; a panel sizes it by the container, which is the only way a
        // scrolling child gets a box smaller than its content to scroll within.
        assert!(CardChrome::Panel.body_fills_container());
        assert!(!CardChrome::Raised.body_fills_container());
        assert!(!CardChrome::Flat.body_fills_container());
        // ...and a panel is still a raised card, chrome-wise.
        assert!(CardChrome::Panel.is_raised());
        assert!(CardChrome::Raised.is_raised());
        assert!(!CardChrome::Flat.is_raised());
    }

    #[test]
    fn a_panels_body_can_be_shorter_than_its_contents() {
        // Both halves are load-bearing and only one of them is obvious. `flex_1` gives
        // the body the panel's leftover height; `min_h_0` is what *allows* it to be
        // shorter than the list inside — a flex item's default minimum is its content
        // size, so without it the body grows to fit the whole list and the
        // `overflow_y_scroll` inside never has a smaller box to scroll within.
        let style = style_of(panel_body());
        assert_eq!(style.flex_grow, Some(1.), "the body takes the leftover");
        assert_eq!(
            style.min_size.height,
            Some(gpui::px(0.).into()),
            "without min_h_0 nothing inside a panel can scroll"
        );
        // The header is the other half of the arithmetic: it may not shrink, or the
        // body's height changes as the list does.
        assert_eq!(style_of(panel_header()).flex_grow, Some(0.));
        assert_eq!(style_of(panel_header()).flex_shrink, Some(0.));
    }

    /// Read back a `Div`'s refined style — the trick `styled.rs`'s tests use.
    fn style_of(mut d: gpui::Div) -> StyleRefinement {
        d.style().clone()
    }
}
