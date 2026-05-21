//! Core abstractions for sid.

pub mod action;
pub mod adapters;
pub mod app;
pub mod context;
pub mod error;
pub mod event;
pub mod keybind;
pub mod layout;
pub mod palette;
pub mod persister;
pub mod restore;
pub mod tab;
pub mod widget;
pub mod workspace_discovery;
pub mod workspace_metadata;

pub use action::{Action, ActionId, ActionRegistry, ActionScope};
pub use app::App;
pub use error::{Result, SidError};
pub use keybind::{KeyBinding, KeybindMap};
pub use layout::{Dir, Layout};
pub use palette::CommandPalette;
pub use persister::StatePersister;
pub use restore::{decide, RestoreDecision, SessionView};
pub use tab::{Tab, TabId, TabManager};
pub use widget::{EventOutcome, RenderTarget, Widget, WidgetId};
