//! Integration test for Task 7: applying a theme via the picker and persisting
//! the resulting name into the `theme_name` setting round-trips through
//! `RedbStore`.

use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::event::{Event, KeyChord};
use sid_store::{OpenStore, RedbStore, TypedSettings, settings_keys};
use sid_ui::theme_registry::ThemeRegistry;
use sid_widgets::settings::theme_picker::{ThemePickerOutcome, ThemePickerView};
use tempfile::tempdir;

#[test]
fn apply_persists_theme_name_to_store() {
    let d = tempdir().unwrap();
    let store = RedbStore::open(&d.path().join("s.redb")).unwrap();
    let registry = ThemeRegistry::with_builtins();
    let mut view = ThemePickerView::new(&registry, "cosmos");
    // Built-in lex order: cosmos, cosmos-light, dusk, void.
    view.next();
    view.next();
    // Focused theme is now "dusk".
    let focused_name = view.focused().name.clone();
    let name = view.apply_focused().to_string();
    assert_eq!(name, focused_name);
    store.put_string(settings_keys::THEME_NAME, &name).unwrap();
    assert_eq!(
        store
            .get_string(settings_keys::THEME_NAME)
            .unwrap()
            .as_deref(),
        Some(focused_name.as_str()),
    );
}

#[test]
fn handle_event_enter_yields_applied_and_we_can_persist() {
    let d = tempdir().unwrap();
    let store = RedbStore::open(&d.path().join("s.redb")).unwrap();
    let registry = ThemeRegistry::with_builtins();
    let mut view = ThemePickerView::new(&registry, "cosmos");
    view.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Down,
        KeyModifiers::empty(),
    )));
    let outcome = view.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    )));
    match outcome {
        ThemePickerOutcome::Applied { name } => {
            store.put_string(settings_keys::THEME_NAME, &name).unwrap();
            assert_eq!(
                store
                    .get_string(settings_keys::THEME_NAME)
                    .unwrap()
                    .as_deref(),
                Some(name.as_str()),
            );
        }
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[test]
fn reapply_is_idempotent() {
    let registry = ThemeRegistry::with_builtins();
    let mut view = ThemePickerView::new(&registry, "cosmos");
    view.next();
    let n1 = view.apply_focused().to_string();
    let n2 = view.apply_focused().to_string();
    assert_eq!(n1, n2);
    assert_eq!(view.applied_name(), n1);
}

#[test]
fn reapply_after_navigation_updates_applied() {
    let registry = ThemeRegistry::with_builtins();
    let mut view = ThemePickerView::new(&registry, "cosmos");
    let first = view.apply_focused().to_string();
    view.next();
    let second = view.apply_focused().to_string();
    assert_ne!(first, second);
    assert_eq!(view.applied_name(), second);
}

#[test]
fn unrelated_key_yields_none() {
    let registry = ThemeRegistry::with_builtins();
    let mut view = ThemePickerView::new(&registry, "cosmos");
    let before = view.focused_index();
    let outcome = view.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    )));
    assert_eq!(outcome, ThemePickerOutcome::None);
    assert_eq!(view.focused_index(), before);
}
