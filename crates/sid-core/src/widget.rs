use std::fmt;

use serde::{Deserialize, Serialize};

use crate::context::WidgetCtx;
use crate::event::Event;

/// Stable identity of a widget instance. Used for state restoration and keybind scope.
///
/// A `WidgetId` wraps a `String` and is `Clone`, `Hash`, `Eq`, `Serialize`, and
/// `Deserialize`. Ids are compared by their string content.
///
/// # Examples
///
/// ```
/// use sid_core::WidgetId;
///
/// let a = WidgetId::new("git-log");
/// let b = WidgetId::new("git-log");
/// assert_eq!(a, b);
/// assert_eq!(a.as_str(), "git-log");
/// ```
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct WidgetId(String);

impl WidgetId {
    /// Create a new `WidgetId` from any string-like value.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::WidgetId;
    ///
    /// let id = WidgetId::new("my-widget");
    /// assert_eq!(id.as_str(), "my-widget");
    ///
    /// // Works with owned String too
    /// let id2 = WidgetId::new(String::from("owned"));
    /// assert_eq!(id2.as_str(), "owned");
    /// ```
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Return the underlying string slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::WidgetId;
    ///
    /// let id = WidgetId::new("terminal");
    /// assert_eq!(id.as_str(), "terminal");
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Displays as the raw string id, without quotes or decoration.
///
/// # Examples
///
/// ```
/// use sid_core::WidgetId;
///
/// let id = WidgetId::new("git-diff");
/// assert_eq!(format!("{id}"), "git-diff");
/// ```
impl fmt::Display for WidgetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Outcome of an event passed to a widget.
///
/// # Examples
///
/// ```
/// use sid_core::EventOutcome;
///
/// let outcome = EventOutcome::Consumed;
/// assert_eq!(outcome, EventOutcome::Consumed);
/// assert_ne!(outcome, EventOutcome::Bubble);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventOutcome {
    /// Widget handled the event; the event loop stops propagation.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::EventOutcome;
    ///
    /// let outcome = EventOutcome::Consumed;
    /// assert_eq!(outcome, EventOutcome::Consumed);
    /// ```
    Consumed,

    /// Widget did not handle the event; let parent / global handlers see it.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::EventOutcome;
    ///
    /// let outcome = EventOutcome::Bubble;
    /// assert_eq!(outcome, EventOutcome::Bubble);
    /// ```
    Bubble,
}

/// Render target abstraction. `sid-ui` provides the only impl for now (over Ratatui).
/// Keeping the trait here means widgets don't depend on Ratatui directly.
///
/// # Examples
///
/// ```
/// use sid_core::widget::RenderTarget;
///
/// struct FixedArea { w: u16, h: u16 }
///
/// impl RenderTarget for FixedArea {
///     fn width(&self) -> u16 { self.w }
///     fn height(&self) -> u16 { self.h }
/// }
///
/// let area = FixedArea { w: 80, h: 24 };
/// assert_eq!(area.width(), 80);
/// assert_eq!(area.height(), 24);
/// ```
pub trait RenderTarget {
    /// Width of the area the widget should render into, in cells.
    fn width(&self) -> u16;
    /// Height of the area, in cells.
    fn height(&self) -> u16;
}

/// A focused, self-contained UI module. In v1 each tab contains exactly one Widget.
///
/// # Examples
///
/// ```
/// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
/// use sid_core::event::Event;
/// use sid_core::context::WidgetCtx;
///
/// struct MyWidget { id: WidgetId }
///
/// impl Widget for MyWidget {
///     fn id(&self) -> &WidgetId { &self.id }
///     fn title(&self) -> &str { "My Widget" }
///     fn render(&self, _target: &mut dyn RenderTarget) {}
///     fn handle_event(&mut self, _ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
///         EventOutcome::Bubble
///     }
/// }
///
/// let w = MyWidget { id: WidgetId::new("my-widget") };
/// assert_eq!(w.id().as_str(), "my-widget");
/// assert_eq!(w.title(), "My Widget");
/// ```
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
