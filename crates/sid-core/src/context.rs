use std::sync::mpsc::Sender;

/// Context passed to a widget when it handles an event.
///
/// Lets widgets emit actions back to the app, request a redraw, or log.
pub struct WidgetCtx {
    action_tx: Sender<String>,
    redraw: bool,
}

impl WidgetCtx {
    /// Build a context that emits actions on the given channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::mpsc;
    /// use sid_core::context::WidgetCtx;
    /// let (tx, _rx) = mpsc::channel();
    /// let ctx = WidgetCtx::new(tx);
    /// assert!(!ctx.needs_redraw());
    /// ```
    pub fn new(action_tx: Sender<String>) -> Self {
        Self { action_tx, redraw: false }
    }

    /// Emit an action by ID. The App will dispatch it via its ActionRegistry.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::mpsc;
    /// use sid_core::context::WidgetCtx;
    /// let (tx, rx) = mpsc::channel();
    /// let mut ctx = WidgetCtx::new(tx);
    /// ctx.emit_action("quit");
    /// assert_eq!(rx.try_recv().unwrap(), "quit");
    /// ```
    pub fn emit_action(&mut self, id: impl Into<String>) {
        let _ = self.action_tx.send(id.into());
    }

    /// Mark the screen as dirty; the next event-loop iteration redraws.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::mpsc;
    /// use sid_core::context::WidgetCtx;
    /// let (tx, _rx) = mpsc::channel();
    /// let mut ctx = WidgetCtx::new(tx);
    /// ctx.request_redraw();
    /// assert!(ctx.needs_redraw());
    /// ```
    pub fn request_redraw(&mut self) {
        self.redraw = true;
    }

    /// Read the redraw flag without consuming it. Useful for tests; prefer
    /// `take_redraw` in the App's event loop so the flag doesn't latch.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::mpsc;
    /// use sid_core::context::WidgetCtx;
    /// let (tx, _rx) = mpsc::channel();
    /// let mut ctx = WidgetCtx::new(tx);
    /// assert!(!ctx.needs_redraw());
    /// ctx.request_redraw();
    /// assert!(ctx.needs_redraw());
    /// ```
    pub fn needs_redraw(&self) -> bool {
        self.redraw
    }

    /// Consume the redraw flag: returns its value and resets it to false.
    /// The App calls this after each render pass to debounce subsequent frames.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::mpsc;
    /// use sid_core::context::WidgetCtx;
    /// let (tx, _rx) = mpsc::channel();
    /// let mut ctx = WidgetCtx::new(tx);
    /// ctx.request_redraw();
    /// assert!(ctx.take_redraw());   // true — flag was set
    /// assert!(!ctx.take_redraw());  // false — already cleared
    /// ```
    pub fn take_redraw(&mut self) -> bool {
        let v = self.redraw;
        self.redraw = false;
        v
    }
}
