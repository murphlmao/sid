//! Widget implementations for sid — one per tab.
//!
//! All six concrete widgets are "coming soon" stubs in Plan 1, backed by
//! [`stub::ComingSoonBody`]. Real content arrives in Plans 2-7.
//!
//! The Workspaces widget is fully implemented in Plan 2; see [`workspaces`]
//! for the full tree view and git sub-view details.

pub mod csv_export;
pub mod database;
pub mod form;
pub mod list_cursor;
pub mod modal;
pub mod network;
pub mod settings;
pub mod split_view;
pub mod ssh;
pub mod stub;
pub mod system;
pub mod workspace_detail;
pub mod workspace_detail_state;
pub mod workspaces;

pub use database::DatabaseWidget;
pub use modal::*;
pub use network::NetworkWidget;
pub use settings::{SettingsCategory, SettingsWidget};
pub use ssh::{SshInspector, SshWidget, ssh_add_form_spec, ssh_edit_form_spec};
pub use system::SystemWidget;
pub use workspace_detail::{CiStatus, RepoSummary, WorkspaceDetailWidget};
pub use workspace_detail_state::{DetailOp, DetailView, RepoDetail, RepoGit, SatelliteRow};
pub use workspaces::WorkspacesWidget;
