//! Tests for `App::handle_event` — global dispatch, palette, keybinds, widget
//! forwarding, and the `run_action` built-in handlers.

use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::action::{Action, ActionRegistry};
use sid_core::app::{App, Dispatch};
use sid_core::context::WidgetCtx;
use sid_core::event::{Event, KeyChord};
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabManager};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

// ---------------------------------------------------------------------------
// Test widget stub
// ---------------------------------------------------------------------------

struct W {
    id: WidgetId,
}

impl Widget for W {
    fn id(&self) -> &WidgetId {
        &self.id
    }
    fn title(&self) -> &str {
        "W"
    }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome {
        EventOutcome::Bubble
    }
}

fn t(id: &'static str) -> Tab {
    Tab {
        id: TabId::new(id),
        title: id.into(),
        layout: Layout::Single(Box::new(W {
            id: WidgetId::new(id),
        })),
        hotkey: None,
    }
}

fn three_tab_app() -> App {
    App::new(
        TabManager::new(vec![t("a"), t("b"), t("c")]),
        KeybindMap::cosmos_default(),
        ActionRegistry::new(),
    )
}

fn two_tab_app() -> App {
    App::new(
        TabManager::new(vec![t("a"), t("b")]),
        KeybindMap::cosmos_default(),
        ActionRegistry::new(),
    )
}

// ---------------------------------------------------------------------------
// Plan-specified tests
// ---------------------------------------------------------------------------

#[test]
fn ctrl_right_advances_tab() {
    let mut app = two_tab_app();
    let chord = KeyChord::new(KeyCode::Right, KeyModifiers::CONTROL);
    let dispatch = app.handle_event(&Event::Key(chord));
    assert_eq!(app.tabs().active().id.as_str(), "b");
    assert_eq!(dispatch, Dispatch::Continue);
}

#[test]
fn ctrl_q_sets_quit_flag() {
    let mut app = two_tab_app();
    let dispatch = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('q'),
        KeyModifiers::CONTROL,
    )));
    assert!(app.is_quitting());
    assert_eq!(dispatch, Dispatch::Quit);
}

#[test]
fn ctrl_f_opens_palette() {
    let mut app = two_tab_app();
    assert!(!app.palette().is_open());
    let dispatch = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    assert!(app.palette().is_open());
    assert_eq!(dispatch, Dispatch::Continue);
}

#[test]
fn ctrl_2_jumps_to_second_tab() {
    let mut app = three_tab_app();
    let dispatch = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('2'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.tabs().active().id.as_str(), "b");
    assert_eq!(dispatch, Dispatch::Continue);
}

// ---------------------------------------------------------------------------
// Adversarial tests
// ---------------------------------------------------------------------------

/// An unbound keystroke must be a no-op — no panic, no state change.
#[test]
fn unbound_keystroke_is_noop() {
    let mut app = two_tab_app();
    let starting_tab = app.tabs().active().id.as_str().to_string();
    let dispatch = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('z'),
        KeyModifiers::NONE,
    )));
    assert_eq!(app.tabs().active().id.as_str(), starting_tab);
    assert!(!app.is_quitting());
    assert!(!app.palette().is_open());
    assert_eq!(dispatch, Dispatch::Continue);
}

/// Palette open then Esc closes it without executing anything.
#[test]
fn palette_open_then_esc_closes() {
    let mut app = two_tab_app();
    // Open the palette.
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    assert!(app.palette().is_open());
    // Esc should close it.
    let dispatch = app.handle_event(&Event::Key(KeyChord::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(!app.palette().is_open());
    assert!(!app.is_quitting());
    assert_eq!(dispatch, Dispatch::Continue);
}

/// Unknown action ID must warn-and-continue, not panic.
#[test]
fn unknown_action_id_is_warn_and_continue() {
    // Inject an unknown action via a direct keybind binding.
    use sid_core::keybind::{KeyBinding, KeybindMap};
    let mut kb = KeybindMap::new();
    kb.bind(KeyBinding {
        chord: KeyChord::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        action: sid_core::action::ActionId::new("totally.unknown.action"),
    });
    let mut app = App::new(TabManager::new(vec![t("a")]), kb, ActionRegistry::new());
    let dispatch = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('x'),
        KeyModifiers::CONTROL,
    )));
    // Must not panic; must continue.
    assert_eq!(dispatch, Dispatch::Continue);
    assert!(!app.is_quitting());
}

/// tabs.jump.999 clamps to the last tab, not an out-of-bounds panic.
#[test]
fn tabs_jump_999_clamps_to_last() {
    let app = three_tab_app();
    // Inject a keybind for tabs.jump.999 (valid action ID but large index).
    use sid_core::keybind::{KeyBinding, KeybindMap};
    let mut kb = KeybindMap::new();
    kb.bind(KeyBinding {
        chord: KeyChord::new(KeyCode::Char('9'), KeyModifiers::CONTROL),
        action: sid_core::action::ActionId::new("tabs.jump.999"),
    });
    let mut app2 = App::new(
        TabManager::new(vec![t("a"), t("b"), t("c")]),
        kb,
        ActionRegistry::new(),
    );
    let dispatch = app2.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('9'),
        KeyModifiers::CONTROL,
    )));
    // Should clamp to last tab (index 2 → "c").
    assert_eq!(app2.tabs().active().id.as_str(), "c");
    assert_eq!(dispatch, Dispatch::Continue);
    // The original app is unchanged.
    assert_eq!(app.tabs().active().id.as_str(), "a");
}

// ---------------------------------------------------------------------------
// Additional coverage tests
// ---------------------------------------------------------------------------

/// Ctrl+Left moves to previous tab.
#[test]
fn ctrl_left_goes_to_prev_tab() {
    let mut app = three_tab_app();
    // First advance to tab "b".
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Right,
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.tabs().active().id.as_str(), "b");
    // Now go prev.
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Left,
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.tabs().active().id.as_str(), "a");
}

/// Ctrl+1 jumps to the first tab.
#[test]
fn ctrl_1_jumps_to_first_tab() {
    let mut app = three_tab_app();
    // Move to tab "c" first.
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('3'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.tabs().active().id.as_str(), "c");
    // Jump back.
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('1'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.tabs().active().id.as_str(), "a");
}

/// Typing in an open palette filters via input.
#[test]
fn typing_in_palette_updates_query() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("app.quit", "Quit"));
    let mut app = App::new(
        TabManager::new(vec![t("a")]),
        KeybindMap::cosmos_default(),
        reg,
    );
    // Open palette.
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    // Type "q".
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
    )));
    assert_eq!(app.palette().query(), "q");
}

/// Backspace in an open palette removes the last character.
#[test]
fn backspace_in_palette_removes_char() {
    let mut app = App::new(
        TabManager::new(vec![t("a")]),
        KeybindMap::cosmos_default(),
        ActionRegistry::new(),
    );
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
    )));
    assert_eq!(app.palette().query(), "q");
    app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Backspace,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.palette().query(), "");
}

/// Tick events and Resize events are passed through without error.
#[test]
fn non_key_events_do_not_panic() {
    let mut app = two_tab_app();
    let dispatch = app.handle_event(&Event::Tick);
    assert_eq!(dispatch, Dispatch::Continue);
    let dispatch = app.handle_event(&Event::Resize {
        width: 80,
        height: 24,
    });
    assert_eq!(dispatch, Dispatch::Continue);
}

/// tab.detach / tab.attach / tab.reload are no-ops (plan 8 stubs).
#[test]
fn plan8_actions_are_noop() {
    use sid_core::keybind::{KeyBinding, KeybindMap};
    let mut kb = KeybindMap::new();
    kb.bind(KeyBinding {
        chord: KeyChord::new(KeyCode::F(1), KeyModifiers::NONE),
        action: sid_core::action::ActionId::new("tab.detach"),
    });
    kb.bind(KeyBinding {
        chord: KeyChord::new(KeyCode::F(2), KeyModifiers::NONE),
        action: sid_core::action::ActionId::new("tab.attach"),
    });
    kb.bind(KeyBinding {
        chord: KeyChord::new(KeyCode::F(3), KeyModifiers::NONE),
        action: sid_core::action::ActionId::new("tab.reload"),
    });
    let mut app = App::new(TabManager::new(vec![t("a")]), kb, ActionRegistry::new());
    for key in [KeyCode::F(1), KeyCode::F(2), KeyCode::F(3)] {
        let d = app.handle_event(&Event::Key(KeyChord::new(key, KeyModifiers::NONE)));
        assert_eq!(d, Dispatch::Continue);
        assert!(!app.is_quitting());
    }
}
