use sid_core::widget::Widget;
use sid_widgets::SshWidget;

#[test]
fn ssh_widget_has_expected_id_and_title() {
    let w = SshWidget::new();
    assert_eq!(w.id().as_str(), "ssh.root");
    assert_eq!(w.title(), "SSH");
}

#[test]
fn ssh_widget_default_matches_new() {
    let a = SshWidget::new();
    let b = SshWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn ssh_save_state_returns_empty() {
    let w = SshWidget::new();
    assert!(w.save_state().is_empty());
}

#[test]
fn ssh_load_state_is_noop() {
    let mut w = SshWidget::new();
    w.load_state(&[0xCA, 0xFE, 0xBA, 0xBE]);
    assert_eq!(w.id().as_str(), "ssh.root");
}

#[test]
fn ssh_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(SshWidget::new());
    assert_eq!(w.id().as_str(), "ssh.root");
    assert_eq!(w.title(), "SSH");
}

#[test]
fn ssh_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SshWidget>();
}
