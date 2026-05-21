//! Database tab widget — stub for Plan 1; full implementation in Plan 4.

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

/// Tab widget for the Database tab.
///
/// In Plan 1 this renders a "coming soon" placeholder. A Postgres + SQLite
/// query runner with paginated results arrives in Plan 4.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::DatabaseWidget;
///
/// let w = DatabaseWidget::new();
/// assert_eq!(w.id().as_str(), "database.root");
/// assert_eq!(w.title(), "Database");
/// ```
pub struct DatabaseWidget {
    body: ComingSoonBody,
    id: WidgetId,
}

impl DatabaseWidget {
    /// Create a new `DatabaseWidget` stub.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::DatabaseWidget;
    ///
    /// let w = DatabaseWidget::new();
    /// assert_eq!(w.id().as_str(), "database.root");
    /// assert_eq!(w.title(), "Database");
    /// ```
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "Database",
                "Postgres + SQLite query runner with paginated results — coming in Plan 4",
            ),
            id: WidgetId::new("database.root"),
        }
    }
}

impl Default for DatabaseWidget {
    /// Returns a `DatabaseWidget` identical to [`DatabaseWidget::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::DatabaseWidget;
    ///
    /// let w = DatabaseWidget::default();
    /// assert_eq!(w.id().as_str(), "database.root");
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for DatabaseWidget {
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

    use super::DatabaseWidget;

    #[test]
    fn id_and_title_correct() {
        let w = DatabaseWidget::new();
        assert_eq!(w.id().as_str(), "database.root");
        assert_eq!(w.title(), "Database");
    }

    #[test]
    fn save_state_is_empty() {
        let w = DatabaseWidget::new();
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn load_state_is_noop() {
        let mut w = DatabaseWidget::new();
        w.load_state(&[0x42]);
        assert_eq!(w.id().as_str(), "database.root");
    }
}
