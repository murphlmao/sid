//! Shared "coming soon" body used by all six tab widget stubs.

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget};

/// Reusable body for stub widgets that are not yet implemented.
///
/// Each concrete widget owns a `ComingSoonBody` and delegates `title()`,
/// `subtitle()`, `render()`, and `handle_event()` to it.
pub struct ComingSoonBody {
    title: String,
    subtitle: String,
}

impl ComingSoonBody {
    /// Create a new stub body with the given title and subtitle.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::stub::ComingSoonBody;
    ///
    /// let body = ComingSoonBody::new("Workspaces", "git operations — coming in Plan 2");
    /// assert_eq!(body.title(), "Workspaces");
    /// ```
    pub fn new(title: impl Into<String>, subtitle: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: subtitle.into(),
        }
    }

    /// Returns the widget title string.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::stub::ComingSoonBody;
    ///
    /// let body = ComingSoonBody::new("SSH", "embedded terminal — coming in Plan 3");
    /// assert_eq!(body.title(), "SSH");
    /// ```
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the widget subtitle / coming-soon description.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::stub::ComingSoonBody;
    ///
    /// let body = ComingSoonBody::new("Database", "query runner — coming in Plan 4");
    /// assert_eq!(body.subtitle(), "query runner — coming in Plan 4");
    /// ```
    pub fn subtitle(&self) -> &str {
        &self.subtitle
    }

    /// Render the body into a `RenderTarget`.
    ///
    /// In the current stub implementation rendering is a no-op; the `sid-ui`
    /// layer draws the active tab title and "(coming soon)" text directly via
    /// the Ratatui `Frame` in the binary's `draw()` function. The target
    /// parameter is consumed to satisfy the trait signature.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::stub::ComingSoonBody;
    /// use sid_core::widget::RenderTarget;
    ///
    /// struct FakeTarget;
    /// impl RenderTarget for FakeTarget {
    ///     fn width(&self) -> u16 { 80 }
    ///     fn height(&self) -> u16 { 24 }
    /// }
    ///
    /// let body = ComingSoonBody::new("Network", "ports — coming in Plan 5");
    /// let mut target = FakeTarget;
    /// body.render(&mut target); // no-op, must not panic
    /// ```
    pub fn render(&self, target: &mut dyn RenderTarget) {
        let _ = target;
    }

    /// Handle an event. Always returns `EventOutcome::Bubble` because stub
    /// widgets do not consume any events.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::mpsc;
    /// use sid_core::context::WidgetCtx;
    /// use sid_core::event::Event;
    /// use sid_core::widget::EventOutcome;
    /// use sid_widgets::stub::ComingSoonBody;
    ///
    /// let (tx, _rx) = mpsc::channel();
    /// let mut ctx = WidgetCtx::new(tx);
    /// let mut body = ComingSoonBody::new("System", "quick-actions — coming in Plan 6");
    /// let result = body.handle_event(&Event::Tick, &mut ctx);
    /// assert_eq!(result, EventOutcome::Bubble);
    /// ```
    pub fn handle_event(&mut self, _ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        EventOutcome::Bubble
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use sid_core::context::WidgetCtx;
    use sid_core::event::Event;
    use sid_core::widget::{EventOutcome, RenderTarget};

    use super::ComingSoonBody;

    struct FakeTarget;
    impl RenderTarget for FakeTarget {
        fn width(&self) -> u16 {
            80
        }
        fn height(&self) -> u16 {
            24
        }
    }

    // --- happy path ---

    #[test]
    fn new_stores_title_and_subtitle() {
        let b = ComingSoonBody::new("Workspaces", "git operations across registered repos");
        assert_eq!(b.title(), "Workspaces");
        assert_eq!(b.subtitle(), "git operations across registered repos");
    }

    #[test]
    fn render_is_noop() {
        let b = ComingSoonBody::new("SSH", "terminal — Plan 3");
        let mut target = FakeTarget;
        b.render(&mut target); // must not panic
    }

    #[test]
    fn handle_event_always_bubbles() {
        let (tx, _rx) = mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let mut b = ComingSoonBody::new("Database", "query runner");
        assert_eq!(b.handle_event(&Event::Tick, &mut ctx), EventOutcome::Bubble);
    }

    // --- adversarial ---

    #[test]
    fn empty_title_and_subtitle() {
        let b = ComingSoonBody::new("", "");
        assert_eq!(b.title(), "");
        assert_eq!(b.subtitle(), "");
    }

    #[test]
    fn very_long_title_and_subtitle() {
        let long = "x".repeat(100_000);
        let b = ComingSoonBody::new(long.clone(), long.clone());
        assert_eq!(b.title().len(), 100_000);
        assert_eq!(b.subtitle().len(), 100_000);
    }

    #[test]
    fn unicode_title_and_subtitle() {
        let title = "Рабочие пространства 🪐";
        let subtitle = "日本語 — 한국어 — العربية";
        let b = ComingSoonBody::new(title, subtitle);
        assert_eq!(b.title(), title);
        assert_eq!(b.subtitle(), subtitle);
    }

    #[test]
    fn multiple_handle_event_calls_all_bubble() {
        let (tx, _rx) = mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let mut b = ComingSoonBody::new("Network", "ports");
        for _ in 0..100 {
            assert_eq!(b.handle_event(&Event::Tick, &mut ctx), EventOutcome::Bubble);
        }
    }
}
