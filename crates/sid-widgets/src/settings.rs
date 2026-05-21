//! Settings tab widget.
//!
//! Plan 7 fills this out with sub-views (theme picker, keybind editor, etc.).
//! For now the top-level widget is still a stub; the sub-modules expose the
//! state types that the composer (Task 20) will glue together.

pub mod behavior_toggles;
pub mod keybind_editor;
pub mod live_preview;
pub mod theme_picker;

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

/// Tab widget for the Settings tab.
///
/// In Plan 1 this renders a "coming soon" placeholder. A theme picker,
/// keybind editor, and behavior toggles arrive in Plan 7.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::SettingsWidget;
///
/// let w = SettingsWidget::new();
/// assert_eq!(w.id().as_str(), "settings.root");
/// assert_eq!(w.title(), "Settings");
/// ```
pub struct SettingsWidget {
    body: ComingSoonBody,
    id: WidgetId,
}

impl SettingsWidget {
    /// Create a new `SettingsWidget` stub.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::SettingsWidget;
    ///
    /// let w = SettingsWidget::new();
    /// assert_eq!(w.id().as_str(), "settings.root");
    /// assert_eq!(w.title(), "Settings");
    /// ```
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "Settings",
                "theme picker, keybind editor, behavior toggles — coming in Plan 7",
            ),
            id: WidgetId::new("settings.root"),
        }
    }
}

impl Default for SettingsWidget {
    /// Returns a `SettingsWidget` identical to [`SettingsWidget::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::SettingsWidget;
    ///
    /// let w = SettingsWidget::default();
    /// assert_eq!(w.id().as_str(), "settings.root");
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for SettingsWidget {
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

    use super::SettingsWidget;

    #[test]
    fn id_and_title_correct() {
        let w = SettingsWidget::new();
        assert_eq!(w.id().as_str(), "settings.root");
        assert_eq!(w.title(), "Settings");
    }

    #[test]
    fn save_state_is_empty() {
        let w = SettingsWidget::new();
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn load_state_is_noop() {
        let mut w = SettingsWidget::new();
        w.load_state(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00]);
        assert_eq!(w.id().as_str(), "settings.root");
    }
}
