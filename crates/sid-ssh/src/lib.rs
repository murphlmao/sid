//! `RusshClient` — russh-backed `SshClient` implementation.
//!
//! `RusshClientFactory` is a stateless factory used by the connect flow to
//! produce per-host `RusshClient` instances via `new_client()`.
//!
//! All russh-specific types are confined to this crate; the rest of sid talks
//! to the `SshClient` trait from `sid_core::ssh`.

pub mod auth;
pub mod client;
pub mod known_hosts;
pub mod sftp;
pub mod shell;

pub use client::{RusshClient, RusshClientFactory};
