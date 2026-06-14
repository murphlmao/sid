use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{context::WidgetCtx, event::Event};

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

/// One footer hint: a single uppercase letter (or named chord) and its label.
///
/// Rendered as `[ N: new ]` in the per-tab footer of the active widget. Concrete
/// widgets return a `Vec<FooterHint>` from [`Widget::footer_hint`] to advertise
/// their CRUD verbs to the binary's render layer.
///
/// `chord` is the displayed key. It is usually a single uppercase letter
/// (`"N"`, `"E"`, `"D"`) but may be a multi-character named chord such as
/// `"Del"`, `"Enter"`, `"Tab"`, or `"Ctrl+R"`. `label` is a short verb describing
/// the action (`"new"`, `"edit"`, `"remove"`).
///
/// # Examples
///
/// ```
/// use sid_core::widget::FooterHint;
///
/// let h = FooterHint::new("N", "new");
/// assert_eq!(h.chord, "N");
/// assert_eq!(h.label, "new");
/// ```
#[derive(Debug, Clone)]
pub struct FooterHint {
    /// Displayed text, usually one uppercase letter; can be `"Del"`, `"Enter"`, `"Tab"`.
    pub chord: String,
    /// Short description, e.g. `"new"` / `"edit"` / `"help"`.
    pub label: String,
}

impl FooterHint {
    /// Construct a `FooterHint` from any string-like values.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::FooterHint;
    ///
    /// let h = FooterHint::new("Enter", "promote");
    /// assert_eq!(h.chord, "Enter");
    /// assert_eq!(h.label, "promote");
    ///
    /// // Works with owned String too.
    /// let h2 = FooterHint::new(String::from("?"), String::from("help"));
    /// assert_eq!(h2.chord, "?");
    /// assert_eq!(h2.label, "help");
    /// ```
    pub fn new(chord: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            chord: chord.into(),
            label: label.into(),
        }
    }
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
///     fn as_any(&self) -> &dyn std::any::Any { self }
///     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
/// }
///
/// let w = MyWidget { id: WidgetId::new("my-widget") };
/// assert_eq!(w.id().as_str(), "my-widget");
/// assert_eq!(w.title(), "My Widget");
/// // Default `footer_hint` is empty.
/// assert!(w.footer_hint().is_empty());
/// ```
pub trait Widget: std::any::Any + Send + Sync {
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
    /// Capital-letter / named-chord actions for the active tab footer.
    ///
    /// The binary's render layer queries this from the active widget and
    /// renders each entry as `[ <chord>: <label> ]` in the footer strip.
    /// Default: empty. Concrete widgets override to surface their CRUD verbs.
    fn footer_hint(&self) -> Vec<FooterHint> {
        Vec::new()
    }
    /// Downcasting hook so the binary's render layer can call concrete-type
    /// rendering helpers (which take ratatui types, not allowed in this crate).
    /// Each impl is one line: `fn as_any(&self) -> &dyn std::any::Any { self }`.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Mutable downcasting hook. Each impl is one line:
    /// `fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }`.
    ///
    /// Used by the binary's wire layer to mutate a widget's state from a
    /// background job (e.g., applying a completed sub-repo scan to the
    /// matching `WorkspaceDetailWidget`).
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}
