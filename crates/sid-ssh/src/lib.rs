//! `RusshClient` — russh-backed `SshClient` implementation.
//!
//! `RusshClientFactory` is a stateless factory used by the binary to produce
//! per-host `RusshClient` instances via `connect(host)`.
//!
//! All russh-specific types are confined to this crate; the rest of sid talks
//! to the `SshClient` trait from `sid-core::adapters::ssh`.

pub mod auth;
pub mod client;
pub mod config;
pub mod sftp;
pub mod shell;

pub use client::{RusshClient, RusshClientFactory};
pub use config::{SshConfigEntry, read_ssh_config};
