//! Integration tests for `App` initialization.

use sid_core::{
    action::{Action, ActionId, ActionRegistry},
    app::App,
    context::WidgetCtx,
    event::Event,
    keybind::KeybindMap,
    layout::Layout,
    tab::{Tab, TabId, TabKind, TabManager},
    widget::{EventOutcome, RenderTarget, Widget, WidgetId},
};

// ---------------------------------------------------------------------------
// Test widget stub
// ---------------------------------------------------------------------------

struct W {
    id: WidgetId,
    title: &'static str,
}

impl Widget for W {
    fn id(&self) -> &WidgetId {
        &self.id
    }
    fn title(&self) -> &str {
        self.title
    }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome {
        EventOutcome::Bubble
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn tab(id: &'static str, title: &'static str, w: &'static str) -> Tab {
    Tab {
        id: TabId::new(id),
        title: title.into(),
        layout: Layout::Single(Box::new(W {
            id: WidgetId::new(w),
            title: w,
        })),
        hotkey: None,
        kind: TabKind::Core,
    }
}

// ---------------------------------------------------------------------------
// Plan-specified tests
// ---------------------------------------------------------------------------

#[test]
fn app_initializes_with_tabs_keybinds_and_registry() {
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::cosmos_default();
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("app.quit", "Quit"));
    let app = App::new(tabs, kb, reg);
    assert_eq!(app.tabs().active().id.as_str(), "a");
    assert!(app.actions().get(&ActionId::new("app.quit")).is_some());
    assert!(!app.is_quitting());
}

#[test]
fn action_tx_clone_can_send_actions() {
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::new();
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("test.action", "Test"));
    let app = App::new(tabs, kb, reg);

    // Cloned sender should be able to send without error.
    let tx = app.action_tx();
    tx.send("test.action".into()).unwrap();
}

#[test]
fn drain_pending_actions_collects_widget_emissions() {
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::new();
    let reg = ActionRegistry::new();
    let mut app = App::new(tabs, kb, reg);

    // Simulate a widget emitting an action via the shared sender.
    let tx = app.action_tx();
    tx.send("app.quit".into()).unwrap();
    tx.send("palette.open".into()).unwrap();

    let drained = app.drain_pending_actions();
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].as_str(), "app.quit");
    assert_eq!(drained[1].as_str(), "palette.open");
}

#[test]
fn request_quit_sets_quit_flag() {
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::new();
    let reg = ActionRegistry::new();
    let mut app = App::new(tabs, kb, reg);

    assert!(!app.is_quitting());
    app.request_quit();
    assert!(app.is_quitting());
}

// ---------------------------------------------------------------------------
// Adversarial tests
// ---------------------------------------------------------------------------

#[test]
fn empty_action_registry_is_valid() {
    // App must not panic when constructed with an empty registry.
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::new();
    let reg = ActionRegistry::new();
    let app = App::new(tabs, kb, reg);
    assert!(!app.is_quitting());
    assert_eq!(app.tabs().tabs().len(), 1);
}

#[test]
fn single_tab_app_is_valid() {
    let tabs = TabManager::new(vec![tab("only", "Only", "w")]);
    let kb = KeybindMap::cosmos_default();
    let reg = ActionRegistry::new();
    let app = App::new(tabs, kb, reg);
    assert_eq!(app.tabs().active().id.as_str(), "only");
    assert_eq!(app.tabs().tabs().len(), 1);
}

#[test]
fn drain_pending_on_empty_queue_returns_empty_vec() {
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::new();
    let reg = ActionRegistry::new();
    let mut app = App::new(tabs, kb, reg);
    let drained = app.drain_pending_actions();
    assert!(drained.is_empty());
}

#[test]
fn multiple_action_tx_clones_all_feed_same_queue() {
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::new();
    let reg = ActionRegistry::new();
    let mut app = App::new(tabs, kb, reg);

    let tx1 = app.action_tx();
    let tx2 = app.action_tx();
    tx1.send("a1".into()).unwrap();
    tx2.send("a2".into()).unwrap();

    let drained = app.drain_pending_actions();
    assert_eq!(drained.len(), 2);
}

// ---------------------------------------------------------------------------
// WidgetCtx integration: verify App hands out valid WidgetCtx
// ---------------------------------------------------------------------------

#[test]
fn widget_ctx_created_from_app_tx_emits_to_drain() {
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::new();
    let reg = ActionRegistry::new();
    let mut app = App::new(tabs, kb, reg);

    let tx = app.action_tx();
    let mut ctx = WidgetCtx::new(tx);
    ctx.emit_action("some.action");

    let drained = app.drain_pending_actions();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].as_str(), "some.action");
}
