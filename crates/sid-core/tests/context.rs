use std::sync::mpsc;

use sid_core::context::WidgetCtx;

#[test]
fn ctx_emit_action_pushes_to_channel() {
    let (tx, rx) = mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    ctx.emit_action("quit");
    let id = rx.try_recv().unwrap();
    assert_eq!(id, "quit");
}

#[test]
fn ctx_redraw_flag_persists() {
    let (tx, _rx) = mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    assert!(!ctx.needs_redraw());
    ctx.request_redraw();
    assert!(ctx.needs_redraw());
}
