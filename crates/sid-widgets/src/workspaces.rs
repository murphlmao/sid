//! Workspaces tab widget — stub for Plan 1; full implementation in Plan 2.

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

/// Tab widget for the Workspaces tab.
///
/// In Plan 1 this renders a "coming soon" placeholder. Real git workspace
/// operations (clone, status, branch management) arrive in Plan 2.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::WorkspacesWidget;
///
/// let w = WorkspacesWidget::new();
/// assert_eq!(w.id().as_str(), "workspaces.root");
/// assert_eq!(w.title(), "Workspaces");
/// ```
pub struct WorkspacesWidget {
    body: ComingSoonBody,
    id: WidgetId,
}

impl WorkspacesWidget {
    /// Create a new `WorkspacesWidget` stub.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::WorkspacesWidget;
    ///
    /// let w = WorkspacesWidget::new();
    /// assert_eq!(w.id().as_str(), "workspaces.root");
    /// assert_eq!(w.title(), "Workspaces");
    /// ```
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "Workspaces",
                "git operations across your registered code workspaces — coming in Plan 2",
            ),
            id: WidgetId::new("workspaces.root"),
        }
    }
}

impl Default for WorkspacesWidget {
    /// Returns a `WorkspacesWidget` identical to [`WorkspacesWidget::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::WorkspacesWidget;
    ///
    /// let w = WorkspacesWidget::default();
    /// assert_eq!(w.id().as_str(), "workspaces.root");
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for WorkspacesWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }

    fn title(&self) -> &str {
        self.body.title()
    }

    fn render(&self, target: &mut dyn RenderTarget) {
        self.body.render(target);
    }

    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        self.body.handle_event(ev, ctx)
    }
}

#[cfg(test)]
mod tests {
    use sid_core::widget::Widget;

    use super::WorkspacesWidget;

    #[test]
    fn id_and_title_correct() {
        let w = WorkspacesWidget::new();
        assert_eq!(w.id().as_str(), "workspaces.root");
        assert_eq!(w.title(), "Workspaces");
    }

    #[test]
    fn save_state_is_empty() {
        let w = WorkspacesWidget::new();
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn load_state_is_noop() {
        let mut w = WorkspacesWidget::new();
        w.load_state(&[0xFF, 0x00]); // must not panic
        assert_eq!(w.id().as_str(), "workspaces.root");
    }
}
