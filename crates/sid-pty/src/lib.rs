//! `PortablePtyProvider` — portable-pty-backed `PtyProvider` implementation,
//! plus a `vt100`-backed screen for ANSI rendering.
//!
//! portable-pty types do not appear in `sid-core` or `sid-widgets`; those
//! crates name only the `PtyProvider` trait.

pub mod provider;
pub mod screen;

pub use provider::PortablePtyProvider;
pub use screen::Vt100Screen;
