use sid_core::widget::Widget;
use sid_widgets::DatabaseWidget;

#[test]
fn database_widget_has_expected_id_and_title() {
    let w = DatabaseWidget::new();
    assert_eq!(w.id().as_str(), "database.root");
    assert_eq!(w.title(), "Database");
}

#[test]
fn database_widget_default_matches_new() {
    let a = DatabaseWidget::new();
    let b = DatabaseWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn database_save_state_returns_empty() {
    let w = DatabaseWidget::new();
    assert!(w.save_state().is_empty());
}

#[test]
fn database_load_state_is_noop() {
    let mut w = DatabaseWidget::new();
    w.load_state(&[0x01, 0x02, 0x03]);
    assert_eq!(w.id().as_str(), "database.root");
}

#[test]
fn database_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(DatabaseWidget::new());
    assert_eq!(w.id().as_str(), "database.root");
    assert_eq!(w.title(), "Database");
}

#[test]
fn database_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DatabaseWidget>();
}
