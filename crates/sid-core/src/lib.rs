//! Core abstractions for sid.

pub mod context;
pub mod error;
pub mod event;
pub mod layout;
pub mod widget;

pub use error::{Result, SidError};
pub use layout::{Dir, Layout};
pub use widget::{EventOutcome, RenderTarget, Widget, WidgetId};
