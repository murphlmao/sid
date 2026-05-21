use std::sync::mpsc::Sender;

/// Context passed to a widget when it handles an event.
///
/// Lets widgets emit actions back to the app, request a redraw, or log.
pub struct WidgetCtx {
    action_tx: Sender<String>,
    redraw: bool,
}

impl WidgetCtx {
    pub fn new(action_tx: Sender<String>) -> Self {
        Self { action_tx, redraw: false }
    }

    /// Emit an action by ID. The App will dispatch it via its ActionRegistry.
    pub fn emit_action(&mut self, id: impl Into<String>) {
        let _ = self.action_tx.send(id.into());
    }

    /// Mark the screen as dirty; the next event-loop iteration redraws.
    pub fn request_redraw(&mut self) {
        self.redraw = true;
    }

    /// Consumed by the App after each event to decide whether to call `render`.
    pub fn needs_redraw(&self) -> bool {
        self.redraw
    }
}
