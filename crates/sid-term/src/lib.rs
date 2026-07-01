//! `Vt100Screen` — a `vt100`-backed [`sid_core::term::TerminalScreen`].
//!
//! vt100 types (`Parser`, `Color`, `Cell`) are confined to this crate; the rest
//! of sid names only the `TerminalScreen` trait and its `TermCell` grid.

pub mod screen;

pub use screen::Vt100Screen;
