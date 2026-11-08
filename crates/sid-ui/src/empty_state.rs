//! The empty state — what a screen says when it has nothing to show.
//!
//! An empty screen is the first thing a new user sees and the most common thing a
//! filtered one shows, so it carries more of an app's personality than its size
//! suggests. v1 already had the right *parts* (a headline, one line of guidance, a real
//! [`crate::Button`] to take the way out, no box); what made it read clinical was the
//! volume: a 28px faint glyph, a 14px lowercase headline, 12px guidance, a 32px button,
//! centred in 1600px of panel.
//!
//! v2 (`docs/design/2026-11-07-friendly-ui-v2.md`) keeps the parts, illustration-free,
//! and turns them up:
//!
//! - a 22px icon on a soft `selection` disc, so the glyph is an object, not a smudge;
//! - the headline in the **Display** role (22px semibold), sentence case — the component
//!   owns the casing (`"no saved connections"` renders `No saved connections`);
//! - the guidance in **Body** (15px), still one line's worth;
//! - the action as given — the convention is a `Lg` primary button with an icon — plus an
//!   optional secondary beside it.
//!
//! [`EmptyState::compact`] is the same shape at panel scale (Body headline, Meta
//! guidance, a 32px disc) for the small boxes inside a sidebar — the schema list, the
//! history list — where a Display headline would shout.
//!
//! Display here is deliberate (Murphy, 2026-11-07, decision D1): data tabs get no page
//! heading, because the tab strip already names the screen — Display is spent where it
//! adds information, and an empty pane's one sentence is exactly that.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder as _, rgb,
};

use crate::copy::sentence_case;
use crate::icon::Icon;
use crate::metrics::{CONTROL_GAP, Space};
use crate::scale::scaled;
use crate::styled::{h_flex, v_flex};
use crate::theme;
use crate::typography::Typography;

/// A centred "nothing here yet" panel.
///
/// ```ignore
/// EmptyState::new("no workspaces yet")
///     .guidance("Add a git repository to track its branches, hosts and connections.")
///     .icon(Icon::Folder)
///     .action(Button::new("add-workspace", "add workspace").primary().icon(Icon::Add)
///         .size(ButtonSize::Lg))
/// ```
#[derive(IntoElement)]
pub struct EmptyState {
    headline: SharedString,
    guidance: Option<SharedString>,
    icon: Option<Icon>,
    action: Option<AnyElement>,
    secondary: Option<AnyElement>,
    compact: bool,
}

impl EmptyState {
    /// The headline: what is missing, in the screen's own words.
    pub fn new(headline: impl Into<SharedString>) -> Self {
        Self {
            headline: headline.into(),
            guidance: None,
            icon: None,
            action: None,
            secondary: None,
            compact: false,
        }
    }

    /// One line saying what to do next. One — if it needs two, the screen needs a
    /// different design, not a longer paragraph.
    pub fn guidance(mut self, guidance: impl Into<SharedString>) -> Self {
        self.guidance = Some(guidance.into());
        self
    }

    /// A glyph on a soft disc above the headline.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The primary action. Usually a `Primary` [`crate::Button`] with an icon — the
    /// whole point is that the way out of the empty state is a control, not prose.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// A second, quieter way out beside the primary one ("Import from ~/.ssh/config").
    pub fn secondary(mut self, action: impl IntoElement) -> Self {
        self.secondary = Some(action.into_any_element());
        self
    }

    /// The panel-scale shape: Body headline, Meta guidance, smaller disc. For the empty
    /// box inside a sidebar or a small panel.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

/// The disc's diameter and the glyph's size, in design pixels: (disc, glyph).
const fn disc_metrics(compact: bool) -> (f32, f32) {
    if compact { (32., 16.) } else { (48., 22.) }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let compact = self.compact;
        let (disc, glyph) = disc_metrics(compact);
        let has_actions = self.action.is_some() || self.secondary.is_some();

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(if compact {
                Space::X2.len()
            } else {
                Space::X3.len()
            })
            .p(if compact {
                Space::X4.len()
            } else {
                Space::X8.len()
            })
            .when_some(self.icon, |this, icon| {
                this.child(
                    div()
                        // `flex_none`: in a short panel the column would otherwise shrink
                        // the disc's height and squash the circle into a pill.
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(scaled(disc))
                        .rounded_full()
                        .bg(rgb(theme.selection))
                        .child(icon.el().size(scaled(glyph)).text_color(rgb(theme.muted)))
                        .into_any_element(),
                )
            })
            // `max_w_96` + centred, the same box the guidance gets: a headline is a
            // caller's sentence, and an unbounded one centred in a 2000px pane spills out
            // of *both* edges rather than wrapping. Wrapping — not clamping — is right
            // here: this is the one line on an empty screen.
            .child(
                div()
                    .max_w_96()
                    .text_center()
                    .map(|this| {
                        if compact {
                            this.text_body(&theme)
                        } else {
                            this.text_display(&theme)
                        }
                    })
                    .child(sentence_case(&self.headline)),
            )
            .when_some(self.guidance, |this, guidance| {
                this.child(
                    div()
                        .max_w(scaled(440.))
                        .text_center()
                        .map(|this| {
                            if compact {
                                this.text_meta(&theme)
                            } else {
                                this.text_body(&theme).text_color(rgb(theme.muted))
                            }
                        })
                        .child(guidance),
                )
            })
            .when(has_actions, |this| {
                this.child(
                    h_flex()
                        .mt(if compact {
                            Space::X1.len()
                        } else {
                            Space::X2.len()
                        })
                        .gap(CONTROL_GAP.len())
                        .children(self.action)
                        .children(self.secondary),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compact_shape_is_smaller_in_every_measure() {
        let (big_disc, big_glyph) = disc_metrics(false);
        let (small_disc, small_glyph) = disc_metrics(true);
        assert!(small_disc < big_disc);
        assert!(small_glyph < big_glyph);
        // The glyph sits inside its disc with room to spare.
        assert!(big_glyph * 1.5 < big_disc);
        assert!(small_glyph * 1.5 < small_disc);
    }

    #[test]
    fn builders_land_where_they_claim() {
        let e = EmptyState::new("no hosts")
            .guidance("add one")
            .icon(Icon::Globe)
            .compact();
        assert_eq!(
            e.headline.as_ref(),
            "no hosts",
            "casing is applied at render, not stored"
        );
        assert_eq!(e.guidance.as_deref(), Some("add one"));
        assert_eq!(e.icon, Some(Icon::Globe));
        assert!(e.compact);
        assert!(e.action.is_none() && e.secondary.is_none());
    }
}
