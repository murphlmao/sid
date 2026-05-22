//! Insta snapshot tests for [`SettingsWidget::render_into_frame`] and each
//! sub-view dispatched from the focused-category right pane.
//!
//! Each test builds a deterministic [`SettingsWidget`] (varying which
//! category is focused), renders it into a fixed `TestBackend` via
//! [`render_to_string`], and pins the ASCII body via insta. Future changes
//! to the layout surface as a visible diff.

use std::path::PathBuf;

use sid_core::action::{Action, ActionRegistry};
use sid_core::keybind::KeybindMap;
use sid_store::{QuickAction, QuickActionScope};
use sid_ui::theme_registry::ThemeRegistry;
use sid_widgets::settings::behavior_toggles::BehaviorTogglesView;
use sid_widgets::settings::db_path::DbPathView;
use sid_widgets::settings::keybind_editor::KeybindEditorView;
use sid_widgets::settings::quick_actions::QuickActionsView;
use sid_widgets::settings::{render_to_string, render_to_string_with_styles};
use sid_widgets::settings::reset::ResetView;
use sid_widgets::settings::theme_picker::ThemePickerView;
use sid_widgets::settings::workspace_roots::WorkspaceRootsView;
use sid_widgets::{SettingsCategory, SettingsWidget};
use tempfile::tempdir;

fn theme_view() -> ThemePickerView {
    let r = ThemeRegistry::with_builtins();
    ThemePickerView::new(&r, "cosmos")
}

fn keybind_view() -> KeybindEditorView {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("app.quit", "Quit"));
    reg.register(Action::new("app.help", "Help"));
    reg.register(Action::new("workspaces.next", "Next workspace"));
    KeybindEditorView::new(&reg, KeybindMap::cosmos_default())
}

fn workspaces_view() -> WorkspaceRootsView {
    WorkspaceRootsView::new(vec![
        PathBuf::from("/home/example/vcs"),
        PathBuf::from("/srv/projects"),
    ])
}

fn quick_actions_view() -> QuickActionsView {
    QuickActionsView::new(vec![
        QuickAction {
            id: "qa.build".into(),
            label: "Build".into(),
            cmd: "cargo build".into(),
            keybind: Some("Char('b')|2".into()),
            scope: QuickActionScope::Global,
        },
        QuickAction {
            id: "qa.test".into(),
            label: "Test".into(),
            cmd: "cargo test".into(),
            keybind: None,
            scope: QuickActionScope::Global,
        },
    ])
}

#[test]
fn snapshot_empty_widget_renders_cleanly() {
    let w = SettingsWidget::with_categories(vec![]);
    let s = render_to_string(&w, 80, 12);
    insta::assert_snapshot!("settings_empty", s);
}

#[test]
fn snapshot_theme_focused() {
    let w = SettingsWidget::with_categories(vec![
        SettingsCategory::Theme(theme_view()),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("settings_theme_focused", s);
}

#[test]
fn snapshot_keybinds_focused() {
    let w = SettingsWidget::with_categories(vec![
        SettingsCategory::Keybinds(keybind_view()),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("settings_keybinds_focused", s);
}

#[test]
fn snapshot_behavior_focused() {
    let w = SettingsWidget::with_categories(vec![
        SettingsCategory::Behavior(BehaviorTogglesView::defaults()),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("settings_behavior_focused", s);
}

#[test]
fn snapshot_workspace_roots_focused() {
    let w = SettingsWidget::with_categories(vec![
        SettingsCategory::WorkspaceRoots(workspaces_view()),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("settings_workspace_roots_focused", s);
}

#[test]
fn snapshot_quick_actions_focused() {
    let w = SettingsWidget::with_categories(vec![
        SettingsCategory::QuickActions(quick_actions_view()),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("settings_quick_actions_focused", s);
}

#[test]
fn snapshot_db_path_focused() {
    // Use a tempdir-rooted sid.toml so the snapshot does not depend on the
    // test runner's HOME, but normalise the *active* path to a constant so
    // the snapshot itself is stable across machines.
    let d = tempdir().unwrap();
    let toml = d.path().join("sid.toml");
    let active = PathBuf::from("/var/lib/sid/sid.redb");
    let view = DbPathView::open(active, toml).unwrap();
    let w = SettingsWidget::with_categories(vec![
        SettingsCategory::DbPath(view),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    let s = render_to_string(&w, 80, 12);
    insta::assert_snapshot!("settings_db_path_focused", s);
}

// ---------------------------------------------------------------------------
// Composer-level focus snapshots — capture fg/bold so the diff between
// "categories focused" and "sub-view focused" is visible in insta output.
// `render_to_string` alone only captures `cell.symbol()` and would not show
// any change when focus flips.
// ---------------------------------------------------------------------------

#[test]
fn snapshot_categories_focused_with_styles() {
    let mut w = SettingsWidget::with_categories(vec![
        SettingsCategory::Theme(theme_view()),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    // Default: categories pane is focused.
    let s = render_to_string_with_styles(&w, 80, 24);
    insta::assert_snapshot!("settings_focus_categories_with_styles", s);
    let _ = &mut w; // silence unused-mut if focus_next API changes
}

#[test]
fn snapshot_subview_focused_with_styles() {
    let mut w = SettingsWidget::with_categories(vec![
        SettingsCategory::Theme(theme_view()),
        SettingsCategory::Reset(ResetView::new()),
    ]);
    // Flip focus to the sub-view pane.
    w.toggle_focused_pane();
    let s = render_to_string_with_styles(&w, 80, 24);
    insta::assert_snapshot!("settings_focus_subview_with_styles", s);
}
