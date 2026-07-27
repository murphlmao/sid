//! The empty state — what a screen says when it has nothing to show.
//!
//! Today Workspaces renders `select a workspace` centred in 1700px of black, and SSH's
//! zero-host case renders nothing at all. `.interface-design/system.md` says empty
//! states "say what to do next, in muted text, without a box", which is right about the
//! box and incomplete about the rest: at 2000px a single muted line reads as a broken
//! render, and telling the user what to do next while giving them no way to do it is
//! the same failure as documenting the SSH tab's primary verb in a hint string.
//!
//! So: a headline they can see, one line of guidance, and — where an action exists — a
//! real [`crate::Button`] to take it. Still no box.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder as _, rgb,
};

use crate::icon::Icon;
use crate::styled::v_flex;
use crate::theme;
use crate::typography::Typography;

/// A centred "nothing here yet" panel.
///
/// ```ignore
/// EmptyState::new("no workspaces yet")
///     .guidance("add a git repository to track its branches, hosts and connections")
///     .icon(Icon::Folder)
///     .action(Button::new("add-workspace", "add workspace").primary().icon(Icon::Add))
/// ```
#[derive(IntoElement)]
pub struct EmptyState {
    headline: SharedString,
    guidance: Option<SharedString>,
    icon: Option<Icon>,
    action: Option<AnyElement>,
}

impl EmptyState {
    /// The headline: what is missing, in the screen's own words.
    pub fn new(headline: impl Into<SharedString>) -> Self {
        Self {
            headline: headline.into(),
            guidance: None,
            icon: None,
            action: None,
        }
    }

    /// One line saying what to do next. One — if it needs two, the screen needs a
    /// different design, not a longer paragraph.
    pub fn guidance(mut self, guidance: impl Into<SharedString>) -> Self {
        self.guidance = Some(guidance.into());
        self
    }

    /// A large faint glyph above the headline, for orientation on a very empty screen.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The primary action. Usually a [`crate::Button`] in its `Primary` variant — the
    /// whole point is that the way out of the empty state is a control, not prose.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .when_some(self.icon, |this, icon| {
                this.child(
                    icon.el()
                        .size(gpui::px(28.))
                        .text_color(rgb(theme.faint))
                        .into_any_element(),
                )
            })
            .child(div().text_body(&theme).child(self.headline))
            .when_some(self.guidance, |this, guidance| {
                this.child(
                    div()
                        .max_w_96()
                        .text_meta(&theme)
                        .text_center()
                        .child(guidance),
                )
            })
            .when_some(self.action, |this, action| {
                this.child(div().mt_1().child(action))
            })
    }
}
