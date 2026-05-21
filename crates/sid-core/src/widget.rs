use std::fmt;

use serde::{Deserialize, Serialize};

use crate::context::WidgetCtx;
use crate::event::Event;

/// Stable identity of a widget instance. Used for state restoration and keybind scope.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct WidgetId(String);

impl WidgetId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WidgetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Outcome of an event passed to a widget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventOutcome {
    /// Widget handled the event.
    Consumed,
    /// Widget did not handle the event; let parent / global handlers see it.
    Bubble,
}

/// Render target abstraction. `sid-ui` provides the only impl for now (over Ratatui).
/// Keeping the trait here means widgets don't depend on Ratatui directly.
pub trait RenderTarget {
    /// Width of the area the widget should render into, in cells.
    fn width(&self) -> u16;
    /// Height of the area, in cells.
    fn height(&self) -> u16;
}

/// A focused, self-contained UI module. In v1 each tab contains exactly one Widget.
pub trait Widget: Send + Sync {
    /// Stable identity for state restoration. Implementations store this in a field
    /// and return a borrow to avoid per-call allocation.
    fn id(&self) -> &WidgetId;
    fn title(&self) -> &str;
    fn render(&self, target: &mut dyn RenderTarget);
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome;
    /// Serialize widget UI state for restoration. Default: empty.
    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }
    /// Restore widget UI state. Default: no-op.
    fn load_state(&mut self, _bytes: &[u8]) {}
}
