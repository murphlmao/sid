//! `sid-system` — adapter crate for systemd-based systems.
//!
//! Exposes:
//! - [`parse`] — pure-Rust parsers for `systemctl` / `journalctl` text output.
//! - [`SystemctlCmdClient`] — CLI-shelling implementation of
//!   [`sid_core::adapters::systemctl::SystemctlClient`].
//! - [`KittyTerminalSpawner`] — CLI-shelling implementation of
//!   [`sid_core::adapters::terminal_spawner::TerminalSpawner`] (Task 16).
//!
//! # Examples
//!
//! ```
//! use sid_core::adapters::systemctl::UnitBus;
//! use sid_system::parse::parse_list_units;
//! let units = parse_list_units("", UnitBus::User).unwrap();
//! assert!(units.is_empty());
//! ```

pub mod client;
pub mod env;
pub mod kitty;
pub mod parse;

pub use client::SystemctlCmdClient;
pub use kitty::KittyTerminalSpawner;
