use sid_core::widget::Widget;
use sid_widgets::SettingsWidget;

#[test]
fn settings_widget_has_expected_id_and_title() {
    let w = SettingsWidget::new();
    assert_eq!(w.id().as_str(), "settings.root");
    assert_eq!(w.title(), "Settings");
}

#[test]
fn settings_widget_default_matches_new() {
    let a = SettingsWidget::new();
    let b = SettingsWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn settings_save_state_returns_empty() {
    let w = SettingsWidget::new();
    assert!(w.save_state().is_empty());
}

#[test]
fn settings_load_state_is_noop() {
    let mut w = SettingsWidget::new();
    w.load_state(&[0xBA, 0xD0, 0xFF]);
    assert_eq!(w.id().as_str(), "settings.root");
}

#[test]
fn settings_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(SettingsWidget::new());
    assert_eq!(w.id().as_str(), "settings.root");
    assert_eq!(w.title(), "Settings");
}

#[test]
fn settings_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SettingsWidget>();
}
