//! GitProvider — filled out in Plan 2 (Workspaces + git adapter).

/// Trait for git operations. Concrete impl (`Git2Provider`) lives in `sid-git`.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::git::GitProvider;
///
/// struct NoopGit;
/// impl GitProvider for NoopGit {}
///
/// fn accepts_provider(_g: &dyn GitProvider) {}
/// accepts_provider(&NoopGit);
/// ```
pub trait GitProvider: Send + Sync {}
