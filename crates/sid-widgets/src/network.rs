//! Network tab widget. Plan 5 builds out the full functionality
//! incrementally: each pane (ports table, processes table, interfaces
//! sidebar, filter input, kill modal) is implemented as a self-contained
//! state type in its own submodule before being assembled into the
//! top-level [`NetworkWidget`].
//!
//! Until [`NetworkWidget`] is rebuilt to consume those pieces (Plan 5
//! Task 17), the widget itself remains the Plan 1 "coming soon" stub.

pub mod filter_input;
pub mod interfaces_sidebar;
pub mod ports_table;
pub mod processes_table;

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

/// Tab widget for the Network tab.
///
/// In Plan 1 this renders a "coming soon" placeholder. Listening ports,
/// processes, interfaces, and kill-PID hotkeys arrive in Plan 5.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::NetworkWidget;
///
/// let w = NetworkWidget::new();
/// assert_eq!(w.id().as_str(), "network.root");
/// assert_eq!(w.title(), "Network");
/// ```
pub struct NetworkWidget {
    body: ComingSoonBody,
    id: WidgetId,
}

impl NetworkWidget {
    /// Create a new `NetworkWidget` stub.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::NetworkWidget;
    ///
    /// let w = NetworkWidget::new();
    /// assert_eq!(w.id().as_str(), "network.root");
    /// assert_eq!(w.title(), "Network");
    /// ```
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "Network",
                "listening ports, processes, interfaces with kill-PID hotkeys — coming in Plan 5",
            ),
            id: WidgetId::new("network.root"),
        }
    }
}

impl Default for NetworkWidget {
    /// Returns a `NetworkWidget` identical to [`NetworkWidget::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::NetworkWidget;
    ///
    /// let w = NetworkWidget::default();
    /// assert_eq!(w.id().as_str(), "network.root");
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for NetworkWidget {
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

    use super::NetworkWidget;

    #[test]
    fn id_and_title_correct() {
        let w = NetworkWidget::new();
        assert_eq!(w.id().as_str(), "network.root");
        assert_eq!(w.title(), "Network");
    }

    #[test]
    fn save_state_is_empty() {
        let w = NetworkWidget::new();
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn load_state_is_noop() {
        let mut w = NetworkWidget::new();
        w.load_state(&[0x00, 0xFF, 0x7F]);
        assert_eq!(w.id().as_str(), "network.root");
    }
}
