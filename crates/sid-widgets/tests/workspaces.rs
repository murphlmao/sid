use sid_core::widget::Widget;
use sid_widgets::WorkspacesWidget;

#[test]
fn workspaces_widget_has_expected_id_and_title() {
    let w = WorkspacesWidget::new(vec![], None);
    assert_eq!(w.id().as_str(), "workspaces.root");
    assert_eq!(w.title(), "Workspaces");
}

#[test]
fn workspaces_widget_default_matches_new() {
    let a = WorkspacesWidget::new(vec![], None);
    let b = WorkspacesWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn workspaces_save_state_returns_empty() {
    let w = WorkspacesWidget::new(vec![], None);
    assert!(w.save_state().is_empty());
}

#[test]
fn workspaces_load_state_is_noop() {
    let mut w = WorkspacesWidget::new(vec![], None);
    w.load_state(&[0xDE, 0xAD, 0xBE, 0xEF]); // arbitrary bytes — must not panic
    assert_eq!(w.id().as_str(), "workspaces.root");
}

#[test]
fn workspaces_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(WorkspacesWidget::new(vec![], None));
    assert_eq!(w.id().as_str(), "workspaces.root");
    assert_eq!(w.title(), "Workspaces");
}

/// Compile-time assertion: WorkspacesWidget implements Send + Sync.
#[test]
fn workspaces_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WorkspacesWidget>();
}
