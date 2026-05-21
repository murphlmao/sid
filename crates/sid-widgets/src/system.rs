//! System tab widget — stub for Plan 1; full implementation in Plan 6.

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

/// Tab widget for the System tab.
///
/// In Plan 1 this renders a "coming soon" placeholder. Pinned configs,
/// systemctl integration, and custom quick-actions arrive in Plan 6.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::SystemWidget;
///
/// let w = SystemWidget::new();
/// assert_eq!(w.id().as_str(), "system.root");
/// assert_eq!(w.title(), "System");
/// ```
pub struct SystemWidget {
    body: ComingSoonBody,
    id: WidgetId,
}

impl SystemWidget {
    /// Create a new `SystemWidget` stub.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::SystemWidget;
    ///
    /// let w = SystemWidget::new();
    /// assert_eq!(w.id().as_str(), "system.root");
    /// assert_eq!(w.title(), "System");
    /// ```
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "System",
                "pinned configs, systemctl, custom quick-actions — coming in Plan 6",
            ),
            id: WidgetId::new("system.root"),
        }
    }
}

impl Default for SystemWidget {
    /// Returns a `SystemWidget` identical to [`SystemWidget::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::SystemWidget;
    ///
    /// let w = SystemWidget::default();
    /// assert_eq!(w.id().as_str(), "system.root");
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for SystemWidget {
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

    use super::SystemWidget;

    #[test]
    fn id_and_title_correct() {
        let w = SystemWidget::new();
        assert_eq!(w.id().as_str(), "system.root");
        assert_eq!(w.title(), "System");
    }

    #[test]
    fn save_state_is_empty() {
        let w = SystemWidget::new();
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn load_state_is_noop() {
        let mut w = SystemWidget::new();
        w.load_state(&[0x01, 0x02, 0x03, 0x04, 0x05]);
        assert_eq!(w.id().as_str(), "system.root");
    }
}
