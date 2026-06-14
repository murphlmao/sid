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

// ---------------------------------------------------------------------------
// focus_at — mouse-click pane routing
// ---------------------------------------------------------------------------

#[test]
fn focus_at_top_left_focuses_categories() {
    use ratatui::layout::Rect;
    use sid_ui::theme_registry::ThemeRegistry;
    use sid_widgets::{
        SettingsCategory,
        settings::{SettingsFocus, reset::ResetView, theme_picker::ThemePickerView},
    };

    let r = ThemeRegistry::with_builtins();
    let mut w = SettingsWidget::with_categories(vec![
        SettingsCategory::Theme(ThemePickerView::new(&r, "cosmos")),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    // Pre-flip focus so the assertion proves the click mutated.
    w.toggle_focused_pane();
    assert_eq!(w.focused_pane(), SettingsFocus::SubView);
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 24,
    };
    // col 5 is inside the 25%-wide left pane.
    w.focus_at(area, 5, 5);
    assert_eq!(w.focused_pane(), SettingsFocus::Categories);
}

#[test]
fn focus_at_top_right_focuses_subview() {
    use ratatui::layout::Rect;
    use sid_ui::theme_registry::ThemeRegistry;
    use sid_widgets::{
        SettingsCategory,
        settings::{SettingsFocus, reset::ResetView, theme_picker::ThemePickerView},
    };

    let r = ThemeRegistry::with_builtins();
    let mut w = SettingsWidget::with_categories(vec![
        SettingsCategory::Theme(ThemePickerView::new(&r, "cosmos")),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    assert_eq!(w.focused_pane(), SettingsFocus::Categories);
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 24,
    };
    // col 80 is well into the right 75%.
    w.focus_at(area, 80, 5);
    assert_eq!(w.focused_pane(), SettingsFocus::SubView);
}
