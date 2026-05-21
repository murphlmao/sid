//! The `App` struct: the top-level owner of all sid state, minus the runtime
//! (Tokio) and the terminal backend (Ratatui). Those are wired in the binary.

use std::sync::mpsc::{channel, Receiver, Sender};

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
/// }
///
/// let tabs = TabManager::new(vec![Tab {
///     id: TabId::new("a"),
///     title: "A".into(),
///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
///     hotkey: None,
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
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
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
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
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
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
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
    /// }
    ///
    /// let tabs = TabManager::new(vec![Tab {
    ///     id: TabId::new("a"),
    ///     title: "A".into(),
    ///     layout: Layout::Single(Box::new(Stub { id: WidgetId::new("w") })),
    ///     hotkey: None,
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
