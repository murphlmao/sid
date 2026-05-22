//! The `App` struct: the top-level owner of all sid state, minus the runtime
//! (Tokio) and the terminal backend (Ratatui). Those are wired in the binary.

use std::sync::mpsc::{Receiver, Sender, channel};

use crate::action::{ActionId, ActionRegistry};
use crate::keybind::KeybindMap;
use crate::palette::CommandPalette;
use crate::tab::TabManager;

/// Top-level application state for sid.
///
/// Owns the tab manager, keybind map, action registry, and command palette.
/// Does not own the Tokio runtime or Ratatui terminal — those are wired in the
/// binary crate so that `sid-core` remains free of async/rendering dependencies.
///
/// # Examples
///
/// ```
/// use sid_core::action::ActionRegistry;
/// use sid_core::app::App;
/// use sid_core::keybind::KeybindMap;
/// use sid_core::layout::Layout;
/// use sid_core::tab::{Tab, TabId, TabManager};
/// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
/// use sid_core::context::WidgetCtx;
/// use sid_core::event::Event;
///
/// struct Stub { id: WidgetId }
/// impl Widget for Stub {
///     fn id(&self) -> &WidgetId { &self.id }
///     fn title(&self) -> &str { "Stub" }
///     fn render(&self, _: &mut dyn RenderTarget) {}
///     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
///     fn as_any(&self) -> &dyn std::any::Any { self }
/// }
///
/// let tabs = TabManager::new(vec![Tab {
///     id: TabId::new("a"),
///     title: "A".into(),
///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
///     hotkey: None,
///     kind: sid_core::tab::TabKind::Core,
/// }]);
/// let app = App::new(tabs, KeybindMap::new(), ActionRegistry::new());
/// assert!(!app.is_quitting());
/// ```
pub struct App {
    tabs: TabManager,
    keybinds: KeybindMap,
    actions: ActionRegistry,
    palette: CommandPalette,
    action_tx: Sender<String>,
    action_rx: Receiver<String>,
    quit: bool,
}

impl App {
    /// Create a new `App` with the given tabs, keybinds, and action registry.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::app::App;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::layout::Layout;
    /// use sid_core::tab::{Tab, TabId, TabManager};
    /// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// use sid_core::context::WidgetCtx;
    /// use sid_core::event::Event;
    ///
    /// struct Stub { id: WidgetId }
    /// impl Widget for Stub {
    ///     fn id(&self) -> &WidgetId { &self.id }
    ///     fn title(&self) -> &str { "Stub" }
    ///     fn render(&self, _: &mut dyn RenderTarget) {}
    ///     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    ///     fn as_any(&self) -> &dyn std::any::Any { self }
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
    ///     kind: sid_core::tab::TabKind::Core,
    /// }]);
    /// let app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    /// assert_eq!(app.tabs().active().id.as_str(), "a");
    /// ```
    pub fn new(tabs: TabManager, keybinds: KeybindMap, actions: ActionRegistry) -> Self {
        let (action_tx, action_rx) = channel();
        Self {
            tabs,
            keybinds,
            actions,
            palette: CommandPalette::new(),
            action_tx,
            action_rx,
            quit: false,
        }
    }

    /// Return a reference to the tab manager.
    pub fn tabs(&self) -> &TabManager {
        &self.tabs
    }

    /// Return a mutable reference to the tab manager.
    pub fn tabs_mut(&mut self) -> &mut TabManager {
        &mut self.tabs
    }

    /// Return a reference to the keybind map.
    pub fn keybinds(&self) -> &KeybindMap {
        &self.keybinds
    }

    /// Return a reference to the action registry.
    pub fn actions(&self) -> &ActionRegistry {
        &self.actions
    }

    /// Return a mutable reference to the action registry. Used by Plan 6 to
    /// hydrate global quick-actions from the store at startup and on CRUD.
    pub fn actions_mut(&mut self) -> &mut ActionRegistry {
        &mut self.actions
    }

    /// Return a reference to the command palette.
    pub fn palette(&self) -> &CommandPalette {
        &self.palette
    }

    /// Return a mutable reference to the command palette.
    pub fn palette_mut(&mut self) -> &mut CommandPalette {
        &mut self.palette
    }

    /// Return `true` if a quit has been requested.
    pub fn is_quitting(&self) -> bool {
        self.quit
    }

    /// Return a cloneable sender for the action channel.
    ///
    /// Hand copies of this to [`crate::context::WidgetCtx::new`] so widgets
    /// can emit actions back to the app.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::app::App;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::layout::Layout;
    /// use sid_core::tab::{Tab, TabId, TabManager};
    /// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// use sid_core::context::WidgetCtx;
    /// use sid_core::event::Event;
    ///
    /// struct Stub { id: WidgetId }
    /// impl Widget for Stub {
    ///     fn id(&self) -> &WidgetId { &self.id }
    ///     fn title(&self) -> &str { "Stub" }
    ///     fn render(&self, _: &mut dyn RenderTarget) {}
    ///     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    ///     fn as_any(&self) -> &dyn std::any::Any { self }
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
    ///     kind: sid_core::tab::TabKind::Core,
    /// }]);
    /// let mut app = App::new(tabs, KeybindMap::new(), ActionRegistry::new());
    /// let tx = app.action_tx();
    /// tx.send("app.quit".into()).unwrap();
    /// let drained = app.drain_pending_actions();
    /// assert_eq!(drained.len(), 1);
    /// ```
    pub fn action_tx(&self) -> Sender<String> {
        self.action_tx.clone()
    }

    /// Drain all action IDs that widgets have emitted since the last call.
    ///
    /// Returns an empty `Vec` if no actions are pending.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::app::App;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::layout::Layout;
    /// use sid_core::tab::{Tab, TabId, TabManager};
    /// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// use sid_core::context::WidgetCtx;
    /// use sid_core::event::Event;
    ///
    /// struct Stub { id: WidgetId }
    /// impl Widget for Stub {
    ///     fn id(&self) -> &WidgetId { &self.id }
    ///     fn title(&self) -> &str { "Stub" }
    ///     fn render(&self, _: &mut dyn RenderTarget) {}
    ///     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    ///     fn as_any(&self) -> &dyn std::any::Any { self }
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
    ///     kind: sid_core::tab::TabKind::Core,
    /// }]);
    /// let mut app = App::new(tabs, KeybindMap::new(), ActionRegistry::new());
    /// assert!(app.drain_pending_actions().is_empty());
    /// ```
    pub fn drain_pending_actions(&mut self) -> Vec<ActionId> {
        let mut out = Vec::new();
        while let Ok(id) = self.action_rx.try_recv() {
            out.push(ActionId::new(id));
        }
        out
    }

    /// Request the application to quit. Sets the quit flag; the binary's event
    /// loop checks `is_quitting()` to clean up and exit.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::app::App;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::layout::Layout;
    /// use sid_core::tab::{Tab, TabId, TabManager};
    /// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// use sid_core::context::WidgetCtx;
    /// use sid_core::event::Event;
    ///
    /// struct Stub { id: WidgetId }
    /// impl Widget for Stub {
    ///     fn id(&self) -> &WidgetId { &self.id }
    ///     fn title(&self) -> &str { "Stub" }
    ///     fn render(&self, _: &mut dyn RenderTarget) {}
    ///     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    ///     fn as_any(&self) -> &dyn std::any::Any { self }
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
    ///     kind: sid_core::tab::TabKind::Core,
    /// }]);
    /// let mut app = App::new(tabs, KeybindMap::new(), ActionRegistry::new());
    /// assert!(!app.is_quitting());
    /// app.request_quit();
    /// assert!(app.is_quitting());
    /// ```
    pub fn request_quit(&mut self) {
        self.quit = true;
    }
}

// ---------------------------------------------------------------------------
// Event dispatch
// ---------------------------------------------------------------------------

/// What the caller (the binary's runtime) should do next.
///
/// Returned from [`App::handle_event`] after every event.
///
/// # Examples
///
/// ```
/// use sid_core::app::Dispatch;
///
/// let d = Dispatch::Continue;
/// assert_eq!(d, Dispatch::Continue);
/// assert_ne!(d, Dispatch::Quit);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Dispatch {
    /// No further action needed by the runtime; redraw if state changed.
    Continue,
    /// The application should exit cleanly.
    Quit,
}

impl App {
    /// Top-level event dispatch.
    ///
    /// Priority order:
    /// 1. If the command palette is open, handle palette-specific keys (Esc,
    ///    Enter, Up/Down, Backspace, printable chars). Other keys are swallowed
    ///    without reaching global keybinds.
    /// 2. Global keybind lookup — if the event is a [`crate::event::Event::Key`]
    ///    and the chord has a binding, the bound action is executed.
    /// 3. Forward to the active widget (via its `handle_event`). Any actions
    ///    the widget emits are drained and executed.
    ///
    /// Returns [`Dispatch::Quit`] when the `app.quit` action fires; otherwise
    /// [`Dispatch::Continue`].
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::app::{App, Dispatch};
    /// use sid_core::event::{Event, KeyChord};
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::layout::Layout;
    /// use sid_core::tab::{Tab, TabId, TabManager};
    /// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// use sid_core::context::WidgetCtx;
    ///
    /// struct Stub { id: WidgetId }
    /// impl Widget for Stub {
    ///     fn id(&self) -> &WidgetId { &self.id }
    ///     fn title(&self) -> &str { "Stub" }
    ///     fn render(&self, _: &mut dyn RenderTarget) {}
    ///     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    ///     fn as_any(&self) -> &dyn std::any::Any { self }
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
    ///     kind: sid_core::tab::TabKind::Core,
    /// }]);
    /// let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    ///
    /// // Ctrl+Q quits.
    /// let d = app.handle_event(&Event::Key(KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL)));
    /// assert_eq!(d, Dispatch::Quit);
    /// assert!(app.is_quitting());
    /// ```
    pub fn handle_event(&mut self, ev: &crate::event::Event) -> Dispatch {
        use crate::event::Event;
        use crossterm::event::{KeyCode, KeyModifiers};

        // ---- 1. Palette intercept ----
        if self.palette.is_open() {
            if let Event::Key(chord) = ev {
                match chord.code {
                    KeyCode::Esc => {
                        self.palette.close();
                        return Dispatch::Continue;
                    }
                    KeyCode::Enter => {
                        if let Some(action) = self.palette.current(&self.actions).cloned() {
                            self.palette.close();
                            return self.run_action(&action.id.clone());
                        }
                        return Dispatch::Continue;
                    }
                    KeyCode::Up => {
                        self.palette.cursor_up(&self.actions);
                        return Dispatch::Continue;
                    }
                    KeyCode::Down => {
                        self.palette.cursor_down(&self.actions);
                        return Dispatch::Continue;
                    }
                    KeyCode::Backspace => {
                        self.palette.backspace();
                        return Dispatch::Continue;
                    }
                    KeyCode::Char(c)
                        if chord.mods == KeyModifiers::NONE
                            || chord.mods == KeyModifiers::SHIFT =>
                    {
                        self.palette.input(&c.to_string());
                        return Dispatch::Continue;
                    }
                    _ => return Dispatch::Continue,
                }
            }
            // Non-key events while palette is open are swallowed.
            return Dispatch::Continue;
        }

        // ---- 2. Global keybind ----
        if let Event::Key(chord) = ev {
            if let Some(action_id) = self.keybinds.lookup(chord).cloned() {
                return self.run_action(&action_id);
            }
        }

        // ---- 3. Forward to active widget ----
        {
            let tx = self.action_tx.clone();
            let mut ctx = crate::context::WidgetCtx::new(tx);
            if let Some(widget) = self.tabs.active_mut().layout.iter_widgets_mut().next() {
                widget.handle_event(ev, &mut ctx);
            }
        }

        // Drain actions the widget emitted and run them.
        let pending = self.drain_pending_actions();
        let mut last_dispatch = Dispatch::Continue;
        for id in pending {
            let d = self.run_action(&id);
            if d == Dispatch::Quit {
                last_dispatch = Dispatch::Quit;
            }
        }

        last_dispatch
    }

    /// Execute a single action by id. Returns [`Dispatch::Quit`] if the action
    /// is `app.quit`, otherwise [`Dispatch::Continue`].
    ///
    /// Unknown action ids log a warning and return `Continue` — they never
    /// panic.
    pub fn run_action(&mut self, id: &ActionId) -> Dispatch {
        match id.as_str() {
            "app.quit" => {
                self.quit = true;
                Dispatch::Quit
            }
            "palette.open" => {
                self.palette.open();
                Dispatch::Continue
            }
            "tabs.next" => {
                self.tabs.next();
                Dispatch::Continue
            }
            "tabs.prev" => {
                self.tabs.prev();
                Dispatch::Continue
            }
            s if s.starts_with("tabs.jump.") => {
                if let Some(n) = s
                    .strip_prefix("tabs.jump.")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    // Human-facing: 1-based. Clamp to last tab via jump().
                    self.tabs.jump(n.saturating_sub(1));
                }
                Dispatch::Continue
            }
            "app.open_settings" => {
                self.tabs.switch_to(&crate::tab::TabId::new("settings"));
                Dispatch::Continue
            }
            "tab.close" => {
                // Close the active tab if it is a Detail; no-op on Core.
                let _ = self.tabs.close_active();
                Dispatch::Continue
            }
            // Branch #2 placeholder: the Workspaces widget emits this when
            // Enter is pressed on a Repo leaf. Branch #3 replaces this arm
            // with the real "build WorkspaceDetailWidget + push as Detail
            // tab" flow.
            "workspaces.open_detail" => Dispatch::Continue,
            // No-ops in Plan 1; implemented in Plan 8.
            "tab.detach" | "tab.attach" | "tab.reload" => Dispatch::Continue,
            _ => {
                tracing::warn!(action = %id, "unknown action id — ignoring");
                Dispatch::Continue
            }
        }
    }
}
