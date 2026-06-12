//! Integration test: S-key in AnimationView emits AnimationChanged outcome.
//!
//! Verifies that the full dispatch chain — SettingsWidget → SettingsCategory::Animation
//! → AnimationView::handle_event → PendingSettingsOutcome::AnimationChanged —
//! is wired correctly end-to-end.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::animation::AnimationConfig;
use sid_core::context::WidgetCtx;
use sid_core::event::{Event, KeyChord};
use sid_core::Widget;
use sid_store::{OpenStore, RedbStore, Store};
use sid_widgets::settings::animation::AnimationView;
use sid_widgets::settings::PendingSettingsOutcome;
use sid_widgets::{SettingsCategory, SettingsWidget};
use tempfile::tempdir;

fn key(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyChord::new(code, mods))
}

/// Build a `SettingsWidget` backed by a real store, containing only the
/// `Animation` category, and pre-focused on the SubView pane so that `S`
/// routes directly into `AnimationView::handle_event`.
fn settings_with_animation(store: Arc<dyn Store>) -> SettingsWidget {
    let cfg = AnimationConfig {
        density: 10,
        ..AnimationConfig::default()
    };
    let anim_view = AnimationView::with_store(cfg, Arc::clone(&store));
    let mut w = SettingsWidget::with_categories(vec![SettingsCategory::Animation(anim_view)]);
    // Send Tab to move focus from the category list to the sub-view pane.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut ctx);
    w
}

#[test]
fn s_key_emits_animation_changed_outcome() {
    let d = tempdir().unwrap();
    let store: Arc<dyn Store> =
        Arc::new(RedbStore::open(&d.path().join("anim_outcome.redb")).unwrap());

    let mut settings = settings_with_animation(Arc::clone(&store));

    // Press `S` (uppercase) — should trigger a save and push AnimationChanged.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    settings.handle_event(&key(KeyCode::Char('S'), KeyModifiers::NONE), &mut ctx);

    // The pending queue must contain exactly one AnimationChanged outcome.
    let outcomes = settings.take_pending_outcomes();
    assert_eq!(outcomes.len(), 1, "expected one pending outcome after S, got {}", outcomes.len());
    assert!(
        matches!(outcomes[0], PendingSettingsOutcome::AnimationChanged(_)),
        "outcome must be AnimationChanged, got {:?}",
        outcomes[0]
    );
}

#[test]
fn ctrl_s_emits_animation_changed_outcome() {
    let d = tempdir().unwrap();
    let store: Arc<dyn Store> =
        Arc::new(RedbStore::open(&d.path().join("anim_ctrl_s.redb")).unwrap());

    let mut settings = settings_with_animation(Arc::clone(&store));

    // Press `Ctrl+S` — same dispatch path as `S`.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    settings.handle_event(&key(KeyCode::Char('s'), KeyModifiers::CONTROL), &mut ctx);

    let outcomes = settings.take_pending_outcomes();
    assert_eq!(outcomes.len(), 1, "expected one pending outcome after Ctrl+S");
    assert!(
        matches!(outcomes[0], PendingSettingsOutcome::AnimationChanged(_)),
        "outcome must be AnimationChanged"
    );
}

#[test]
fn non_save_key_emits_no_outcome() {
    let d = tempdir().unwrap();
    let store: Arc<dyn Store> =
        Arc::new(RedbStore::open(&d.path().join("anim_no_outcome.redb")).unwrap());

    let mut settings = settings_with_animation(Arc::clone(&store));

    // Press `j` — moves focus, no save.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    settings.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut ctx);

    let outcomes = settings.take_pending_outcomes();
    assert!(
        outcomes.is_empty(),
        "j key must not emit any outcome, got {:?}",
        outcomes
    );
}
