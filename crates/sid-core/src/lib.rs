//! Core abstractions for sid.

pub mod action;
pub mod context;
pub mod error;
pub mod event;
pub mod layout;
pub mod tab;
pub mod widget;

pub use action::{Action, ActionId, ActionRegistry, ActionScope};
pub use error::{Result, SidError};
pub use layout::{Dir, Layout};
pub use tab::{Tab, TabId, TabManager};
pub use widget::{EventOutcome, RenderTarget, Widget, WidgetId};
