//! Widget implementations for sid — one per tab.
//!
//! All six concrete widgets are "coming soon" stubs in Plan 1, backed by
//! [`stub::ComingSoonBody`]. Real content arrives in Plans 2-7.

pub mod database;
pub mod network;
pub mod settings;
pub mod ssh;
pub mod stub;
pub mod system;
pub mod workspaces;

pub use database::DatabaseWidget;
pub use network::NetworkWidget;
pub use settings::SettingsWidget;
pub use ssh::SshWidget;
pub use system::SystemWidget;
pub use workspaces::WorkspacesWidget;
