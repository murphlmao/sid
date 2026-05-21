//! SSH tab widget — stub for Plan 1; full implementation in Plan 3.

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

/// Tab widget for the SSH tab.
///
/// In Plan 1 this renders a "coming soon" placeholder. Real SSH host
/// management, embedded terminal, and SFTP features arrive in Plan 3.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::SshWidget;
///
/// let w = SshWidget::new();
/// assert_eq!(w.id().as_str(), "ssh.root");
/// assert_eq!(w.title(), "SSH");
/// ```
pub struct SshWidget {
    body: ComingSoonBody,
    id: WidgetId,
}

impl SshWidget {
    /// Create a new `SshWidget` stub.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::SshWidget;
    ///
    /// let w = SshWidget::new();
    /// assert_eq!(w.id().as_str(), "ssh.root");
    /// assert_eq!(w.title(), "SSH");
    /// ```
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "SSH",
                "SSH host list + embedded terminal + SFTP — coming in Plan 3",
            ),
            id: WidgetId::new("ssh.root"),
        }
    }
}

impl Default for SshWidget {
    /// Returns an `SshWidget` identical to [`SshWidget::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::SshWidget;
    ///
    /// let w = SshWidget::default();
    /// assert_eq!(w.id().as_str(), "ssh.root");
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for SshWidget {
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

    use super::SshWidget;

    #[test]
    fn id_and_title_correct() {
        let w = SshWidget::new();
        assert_eq!(w.id().as_str(), "ssh.root");
        assert_eq!(w.title(), "SSH");
    }

    #[test]
    fn save_state_is_empty() {
        let w = SshWidget::new();
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn load_state_is_noop() {
        let mut w = SshWidget::new();
        w.load_state(&[0xFF, 0x00, 0xAB]);
        assert_eq!(w.id().as_str(), "ssh.root");
    }
}
