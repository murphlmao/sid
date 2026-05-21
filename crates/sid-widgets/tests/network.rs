use sid_core::widget::Widget;
use sid_widgets::NetworkWidget;

#[test]
fn network_widget_has_expected_id_and_title() {
    let w = NetworkWidget::new();
    assert_eq!(w.id().as_str(), "network.root");
    assert_eq!(w.title(), "Network");
}

#[test]
fn network_widget_default_matches_new() {
    let a = NetworkWidget::new();
    let b = NetworkWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn network_save_state_returns_empty() {
    let w = NetworkWidget::new();
    assert!(w.save_state().is_empty());
}

#[test]
fn network_load_state_is_noop() {
    let mut w = NetworkWidget::new();
    w.load_state(&[0xDE, 0xAD]);
    assert_eq!(w.id().as_str(), "network.root");
}

#[test]
fn network_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(NetworkWidget::new());
    assert_eq!(w.id().as_str(), "network.root");
    assert_eq!(w.title(), "Network");
}

#[test]
fn network_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NetworkWidget>();
}
