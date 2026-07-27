//! Cards and sections — the container that gives a screen a reading order.
//!
//! `settings_tab.rs:96`'s `section_header` is the design system's mandated pattern:
//! `text_xs` UPPERCASE `muted`, optionally `· count`. It is also a file-private free
//! function, which is why the System, Network, SSH and Workspaces tabs each reimplement
//! it inline — four copies of a three-line rule, drifting. This is that function, made
//! importable, plus the frame the System tab's unframed meter cluster is missing.
//!
//! Two shapes, one type:
//!
//! - [`Card::new`] — a `surface` fill with a hairline. Data clusters, forms, grouped
//!   controls: anything that should read as *raised off* the canvas.
//! - [`Card::section`] — header and content, no chrome. A titled block inside a reading
//!   column, where a second border would only add noise.
//!
//! Both take optional header actions, which is what keeps a section's controls anchored
//! to the section instead of drifting to the far edge of an invisible 880px column (the
//! System tab's orphaned `COMMON` list).

use gpui::{
    AnyElement, App, IntoElement, ParentElement, Refineable as _, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _, rgb,
};

use crate::elevation::Elevation;
use crate::styled::{StyledExt as _, h_flex, v_flex};
use crate::theme;

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

/// Whether a card draws chrome of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardChrome {
    /// `surface` fill + hairline: raised off the canvas.
    Raised,
    /// Header and content only: a titled block on whatever it sits on.
    Flat,
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
        let raised = self.chrome == CardChrome::Raised;
        let header = self.title.map(|title| {
            h_flex()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .section_label(&theme)
                        .child(header_text(&title, self.count)),
                )
                .when(!self.actions.is_empty(), |this| {
                    this.child(h_flex().gap_1().children(self.actions))
                })
        });

        let mut card = v_flex()
            .when(raised, |this| {
                this.elevation(Elevation::Surface, &theme).p_3()
            })
            .gap_2()
            .children(header)
            .child(v_flex().gap_2().children(self.children))
            // A flat section's own text colour, so its body does not inherit whatever
            // the surrounding chrome happened to be painted in.
            .text_color(rgb(theme.fg));
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
    fn the_two_shapes_differ_only_in_chrome() {
        assert_eq!(Card::new().chrome, CardChrome::Raised);
        assert_eq!(Card::section("x").chrome, CardChrome::Flat);
        assert_eq!(Card::section("x").title.expect("titled").as_ref(), "x");
        assert!(Card::new().title.is_none());
    }
}
