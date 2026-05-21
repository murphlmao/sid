//! Core abstractions for sid.

pub mod context;
pub mod error;
pub mod event;
pub mod widget;

pub use error::{Result, SidError};
pub use widget::{EventOutcome, RenderTarget, Widget, WidgetId};
