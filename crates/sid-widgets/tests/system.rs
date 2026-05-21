use sid_core::widget::Widget;
use sid_widgets::SystemWidget;

#[test]
fn system_widget_has_expected_id_and_title() {
    let w = SystemWidget::new();
    assert_eq!(w.id().as_str(), "system.root");
    assert_eq!(w.title(), "System");
}

#[test]
fn system_widget_default_matches_new() {
    let a = SystemWidget::new();
    let b = SystemWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn system_save_state_returns_empty() {
    let w = SystemWidget::new();
    assert!(w.save_state().is_empty());
}

#[test]
fn system_load_state_is_noop() {
    let mut w = SystemWidget::new();
    w.load_state(&[0xFF, 0xFE, 0xFD]);
    assert_eq!(w.id().as_str(), "system.root");
}

#[test]
fn system_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(SystemWidget::new());
    assert_eq!(w.id().as_str(), "system.root");
    assert_eq!(w.title(), "System");
}

#[test]
fn system_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SystemWidget>();
}
