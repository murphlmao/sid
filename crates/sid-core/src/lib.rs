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
pub mod tab;
pub mod widget;

pub use action::{Action, ActionId, ActionRegistry, ActionScope};
pub use app::App;
pub use error::{Result, SidError};
pub use keybind::{KeyBinding, KeybindMap};
pub use layout::{Dir, Layout};
pub use palette::CommandPalette;
pub use tab::{Tab, TabId, TabManager};
pub use widget::{EventOutcome, RenderTarget, Widget, WidgetId};
