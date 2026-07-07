//! `sid-git` — the `git2`-backed [`sid_core::git::GitProvider`] implementation,
//! ported from the POC's `sid-git` crate. This is the only crate allowed to name
//! `git2` (see `CLAUDE.md`'s adapter rule).

use std::path::Path;

use sid_core::git::{Branch, CommitInfo, GitError, GitProvider, GitStatus, RepoSummary};

/// git2-backed provider. `Git2Provider::factory()` returns a stateless factory;
/// [`GitProvider::open`] binds a per-repo handle.
///
/// # Examples
///
/// ```
/// use sid_git::Git2Provider;
/// let _factory = Git2Provider::factory();
/// ```
pub struct Git2Provider;

impl Git2Provider {
    /// A stateless factory handle — call [`GitProvider::open`] to bind a repo.
    pub fn factory() -> Box<dyn GitProvider> {
        Box::new(Git2Provider)
    }
}

impl GitProvider for Git2Provider {
    fn open(&self, path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        let _ = path;
        Err(GitError::Other("sid-git port in progress".into()))
    }
    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        Err(GitError::Other("sid-git port in progress".into()))
    }
    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        Err(GitError::Other("sid-git port in progress".into()))
    }
    fn status(&self) -> Result<GitStatus, GitError> {
        Err(GitError::Other("sid-git port in progress".into()))
    }
    fn commit_log(&self, _max: usize) -> Result<Vec<CommitInfo>, GitError> {
        Err(GitError::Other("sid-git port in progress".into()))
    }
    fn summary(&self) -> Result<RepoSummary, GitError> {
        Err(GitError::Other("sid-git port in progress".into()))
    }
    fn checkout_branch(&mut self, _name: &str) -> Result<(), GitError> {
        Err(GitError::Other("sid-git port in progress".into()))
    }
}
