//! Core abstractions for sid: the Widget trait, App, tabs, keybinds, actions.
//! No knowledge of Ratatui, Tokio, or storage backends lives here.

pub mod error;

pub use error::{Result, SidError};
