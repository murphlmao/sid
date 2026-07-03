//! Service (systemd unit) provider trait + supporting domain types. Implementations
//! live in `sid-svcctl`.
//!
//! Mirrors `sys.rs`'s shape — domain types ahead of the trait, a dedicated `SvcError`,
//! no `serde` (the Services sub-view is live/ephemeral, same as the rest of the
//! Network tab: nothing here is ever persisted). It deliberately diverges from
//! `SysProvider` in one way: `ServiceProvider` is `async` rather than `&mut self`
//! sync. `SysinfoProvider` caches a `sysinfo::System` handle, so its trait takes
//! `&mut self` and callers serialize access behind `Arc<Mutex<_>>`; a systemctl call
//! has no such handle to cache — the concrete impl (`sid-svcctl::SvcctlProvider`) is
//! stateless and shells out fresh every call — so it takes `&self` and is `async` so
//! it can `.await` `tokio::process::Command` directly instead of blocking a thread on
//! `std::process::Command`. This lets callers hold it as a plain `Arc<dyn
//! ServiceProvider>` with no mutex, and matches the shape `sid_core::ssh`'s
//! `SshClient` already uses for the same "real async I/O" reason.

use async_trait::async_trait;

/// Which systemd service manager to query/act on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SvcScope {
    /// The system-wide manager (PID 1) — `systemctl --system` (also systemctl's
    /// implicit default). Actions here commonly need root; an unprivileged caller gets
    /// [`SvcError::PermissionDenied`] rather than an escalation prompt.
    #[default]
    System,
    /// The calling user's session manager — `systemctl --user`.
    User,
}

/// Action to perform on a unit via [`ServiceProvider::control`].
///
/// # Examples
///
/// ```
/// use sid_core::svc::SvcAction;
/// assert_ne!(SvcAction::Start, SvcAction::Stop);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SvcAction {
    Start,
    Stop,
    Restart,
    /// Send a signal directly to the unit's processes (`systemctl kill`), bypassing
    /// `ExecStop=`.
    Kill,
}

/// Coarse-grained active state — systemd's `ActiveState`, folded to the four values
/// the Services table renders as a badge (green active / red failed / dim inactive /
/// dim other).
///
/// # Examples
///
/// ```
/// use sid_core::svc::SvcActiveState;
/// assert_eq!(SvcActiveState::Active, SvcActiveState::Active);
/// assert_ne!(SvcActiveState::Active, SvcActiveState::Failed);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SvcActiveState {
    Active,
    Inactive,
    Failed,
    /// `activating`, `deactivating`, `reloading`, or anything else systemd reports.
    Other,
}

/// One service (systemd unit) row.
///
/// # Examples
///
/// ```
/// use sid_core::svc::{ServiceInfo, SvcActiveState};
/// let s = ServiceInfo {
///     name: "nginx.service".into(),
///     description: "A high performance web server".into(),
///     active: SvcActiveState::Active,
///     sub_state: "running".into(),
/// };
/// assert_eq!(s.name, "nginx.service");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceInfo {
    /// Full unit name, e.g. `"nginx.service"`.
    pub name: String,
    pub description: String,
    pub active: SvcActiveState,
    /// systemd's `SubState`, e.g. `"running"`, `"dead"`, `"exited"`.
    pub sub_state: String,
}

/// Domain-shaped service-control error. The concrete impl maps subprocess exit codes
/// and stderr text into this — see `sid_svcctl`'s stderr classifier.
///
/// # Examples
///
/// ```
/// use sid_core::svc::SvcError;
/// let e = SvcError::PermissionDenied("Access denied".into());
/// assert!(format!("{e}").contains("permission denied"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum SvcError {
    /// A system-scope action was attempted without root (or the user declined
    /// polkit). Surfaced to the user; sid never auto-escalates (no sudo/polkit prompt
    /// is spawned on the caller's behalf).
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// The unit name isn't known to systemd.
    #[error("not found: {0}")]
    NotFound(String),
    /// Anything else: subprocess spawn failure, unparseable output, ...
    #[error("service control error: {0}")]
    Other(String),
}

/// Service manager probe/control trait needed by the Network tab's Services
/// sub-view. Implementations live in `sid-svcctl`.
///
/// # Object safety
///
/// `#[async_trait]` boxes the returned futures, so `Box<dyn ServiceProvider>` /
/// `Arc<dyn ServiceProvider>` both work despite the `async fn`s below.
#[async_trait]
pub trait ServiceProvider: Send + Sync {
    /// List service units in `scope`. Maps to `systemctl [--user|--system]
    /// list-units --type=service --all --output=json`.
    async fn list_services(&self, scope: SvcScope) -> Result<Vec<ServiceInfo>, SvcError>;

    /// Perform `action` on unit `name` in `scope`. May return
    /// [`SvcError::PermissionDenied`] for a system-scope action attempted without
    /// root — the caller surfaces this in the status line; it never auto-escalates.
    async fn control(&self, name: &str, action: SvcAction, scope: SvcScope)
    -> Result<(), SvcError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety: the Network tab holds this behind `Arc<dyn ServiceProvider>` —
    // a compile-only check that a non-dispatchable method never sneaks in.
    #[allow(dead_code)]
    fn assert_object_safe(_p: &dyn ServiceProvider) {}

    #[test]
    fn boxed_trait_object_constructs() {
        fn takes_provider(_: Box<dyn ServiceProvider>) {}
        let _ = takes_provider;
    }

    #[test]
    fn default_scope_is_system() {
        assert_eq!(SvcScope::default(), SvcScope::System);
    }

    #[test]
    fn permission_denied_message_contains_detail() {
        let e = SvcError::PermissionDenied("Access denied".into());
        assert!(e.to_string().contains("Access denied"));
    }
}
