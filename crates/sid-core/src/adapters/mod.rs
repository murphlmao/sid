//! Adapter traits. Each external dependency that sid will eventually wrap
//! gets a trait here; concrete impls live in their own crates.

pub mod clipboard;
pub mod db_client;
pub mod git;
pub mod notifier;
pub mod pty;
pub mod secrets;
pub mod ssh;
pub mod sys;
pub mod systemctl;
pub mod terminal_spawner;
