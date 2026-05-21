//! `sid-system` — adapter crate for systemd-based systems.
//!
//! Exposes parsers (`parse`), `SystemctlCmdClient` (CLI-shelling
//! implementation of [`sid_core::adapters::systemctl::SystemctlClient`]),
//! and `KittyTerminalSpawner` (CLI-shelling implementation of
//! [`sid_core::adapters::terminal_spawner::TerminalSpawner`]).
//!
//! # Examples
//!
//! ```
//! use sid_core::adapters::systemctl::UnitBus;
//! use sid_system::parse::parse_list_units;
//! let units = parse_list_units("", UnitBus::User).unwrap();
//! assert!(units.is_empty());
//! ```

pub mod parse;
// pub mod client;   // Task 7
// pub mod kitty;    // Task 16
// pub mod env;      // Task 16
