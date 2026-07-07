//! Git provider trait + domain types — ported from the POC
//! (`sid-poc/crates/sid-core/src/adapters/git.rs`), trimmed to the Workspaces v1
//! surface (read-everything + `checkout_branch`; commit/diff/stash/worktree stay in
//! the POC as the reference for later slices). Implementations live in `sid-git`
//! (the one crate allowed to name `git2` — see `CLAUDE.md`'s adapter rule).
//!
//! One deliberate addition over the POC: [`GitProvider::summary`] — the single cheap
//! call the umbrella fleet dashboard makes per repo (branch + dirty counts +
//! ahead/behind + last commit), instead of four separate calls per row.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Domain-shaped git error. Concrete impls map their library errors into this.
///
/// # Examples
///
/// ```
/// use sid_core::git::GitError;
/// let e = GitError::NotARepo("/tmp/not-a-repo".into());
/// assert!(!format!("{e}").is_empty());
/// let dirty = GitError::DirtyWorkingTree(5);
/// assert!(!format!("{dirty}").is_empty());
/// ```
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("repository not found at {0}")]
    NotARepo(String),
    #[error("working tree is dirty: {0} uncommitted change(s) — refusing")]
    DirtyWorkingTree(usize),
    #[error("branch '{0}' not found")]
    BranchNotFound(String),
    #[error("invalid reference: {0}")]
    InvalidRef(String),
    #[error("git operation failed: {0}")]
    Other(String),
}

/// One local branch.
///
/// # Examples
///
/// ```
/// use sid_core::git::Branch;
/// let b = Branch {
///     name: "main".into(),
///     head_oid: "a".repeat(40),
///     upstream: Some("origin/main".into()),
///     is_current: true,
/// };
/// assert!(b.is_current);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    /// Last commit OID (40-char hex).
    pub head_oid: String,
    /// Upstream tracking branch, if any (e.g. "origin/main").
    pub upstream: Option<String>,
    pub is_current: bool,
}

/// One entry in a repo's status.
///
/// # Examples
///
/// ```
/// use sid_core::git::{StatusEntry, StatusKind};
/// let e = StatusEntry {
///     path: "src/lib.rs".into(),
///     kind: StatusKind::Modified,
///     staged: false,
///     old_path: None,
/// };
/// assert!(!e.staged);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StatusEntry {
    pub path: String,
    pub kind: StatusKind,
    /// `true` = in the index (staged); `false` = working-tree-only change.
    pub staged: bool,
    /// For renames, the original path.
    pub old_path: Option<String>,
}

/// Kind of change represented by a [`StatusEntry`].
///
/// # Examples
///
/// ```
/// use sid_core::git::StatusKind;
/// assert_eq!(StatusKind::Added, StatusKind::Added);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StatusKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// Aggregate status for a repo, plus a quick `is_clean` flag.
///
/// # Examples
///
/// ```
/// use sid_core::git::GitStatus;
/// let s = GitStatus { entries: vec![], is_clean: true };
/// assert!(s.is_clean);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    pub entries: Vec<StatusEntry>,
    pub is_clean: bool,
}

/// One commit, condensed for log display.
///
/// # Examples
///
/// ```
/// use sid_core::git::CommitInfo;
/// let c = CommitInfo {
///     oid: "a".repeat(40),
///     summary: "feat: add thing".into(),
///     author_name: "m".into(),
///     author_email: "m@x".into(),
///     timestamp_secs: 0,
/// };
/// assert_eq!(c.summary, "feat: add thing");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub oid: String,
    /// First line of the commit message.
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since UNIX epoch.
    pub timestamp_secs: i64,
}

/// Everything a fleet-table row / overview header needs about one repo, in one call.
///
/// # Examples
///
/// ```
/// use sid_core::git::RepoSummary;
/// let s = RepoSummary {
///     branch: Some("main".into()),
///     detached: false,
///     staged: 0,
///     unstaged: 2,
///     untracked: 1,
///     ahead: Some(3),
///     behind: Some(0),
///     last_commit: None,
/// };
/// assert!(!s.is_clean());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoSummary {
    /// Current branch name; `None` on an unborn HEAD.
    pub branch: Option<String>,
    /// Detached HEAD (`branch` then holds a short OID).
    pub detached: bool,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    /// Commits ahead of upstream; `None` when there is no upstream.
    pub ahead: Option<usize>,
    /// Commits behind upstream; `None` when there is no upstream.
    pub behind: Option<usize>,
    /// HEAD commit, if any.
    pub last_commit: Option<CommitInfo>,
}

impl RepoSummary {
    /// Whether the working tree has no changes of any kind.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::git::RepoSummary;
    /// let s = RepoSummary {
    ///     branch: Some("main".into()),
    ///     detached: false,
    ///     staged: 0,
    ///     unstaged: 0,
    ///     untracked: 0,
    ///     ahead: None,
    ///     behind: None,
    ///     last_commit: None,
    /// };
    /// assert!(s.is_clean());
    /// ```
    pub fn is_clean(&self) -> bool {
        self.staged == 0 && self.unstaged == 0 && self.untracked == 0
    }
}

/// The git adapter seam. `open` returns a per-repo handle; every other method
/// operates on that handle. All methods are synchronous — callers run them on the
/// shared background runtime, never the render thread.
///
/// # Object safety
///
/// No generics in method position; `Box<dyn GitProvider>` works.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use sid_core::git::{
///     Branch, CommitInfo, GitError, GitProvider, GitStatus, RepoSummary,
/// };
///
/// struct Noop;
/// impl GitProvider for Noop {
///     fn open(&self, _path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
///         Ok(Box::new(Noop))
///     }
///     fn list_branches(&self) -> Result<Vec<Branch>, GitError> { Ok(vec![]) }
///     fn current_branch(&self) -> Result<Option<Branch>, GitError> { Ok(None) }
///     fn status(&self) -> Result<GitStatus, GitError> {
///         Ok(GitStatus { entries: vec![], is_clean: true })
///     }
///     fn commit_log(&self, _max: usize) -> Result<Vec<CommitInfo>, GitError> { Ok(vec![]) }
///     fn summary(&self) -> Result<RepoSummary, GitError> {
///         Err(GitError::Other("noop".into()))
///     }
///     fn checkout_branch(&mut self, name: &str) -> Result<(), GitError> {
///         Err(GitError::BranchNotFound(name.into()))
///     }
/// }
///
/// let factory = Noop;
/// assert!(factory.list_branches().unwrap().is_empty());
/// ```
pub trait GitProvider: Send + Sync {
    /// Open the repo at `path`, returning a per-repo handle. The receiver may be a
    /// stateless "factory" provider (mirrors `DbClient::open`).
    ///
    /// A `path` that exists but is not inside a git repository maps to
    /// [`GitError::NotARepo`] — callers use that to distinguish "plain directory"
    /// (fine for a scope-only workspace) from a real failure.
    fn open(&self, path: &Path) -> Result<Box<dyn GitProvider>, GitError>;

    /// Local branches, current first, then alphabetical.
    fn list_branches(&self) -> Result<Vec<Branch>, GitError>;

    /// The current branch, or `None` on an unborn/detached HEAD.
    fn current_branch(&self) -> Result<Option<Branch>, GitError>;

    /// Full working-tree + index status.
    fn status(&self) -> Result<GitStatus, GitError>;

    /// Up to `max` commits walking back from HEAD, newest first.
    fn commit_log(&self, max: usize) -> Result<Vec<CommitInfo>, GitError>;

    /// The one-call rollup for fleet rows / overview headers — see [`RepoSummary`].
    fn summary(&self) -> Result<RepoSummary, GitError>;

    /// Switch the working tree to `name`. MUST refuse with
    /// [`GitError::DirtyWorkingTree`] when any tracked change exists — sid never
    /// destroys uncommitted work.
    fn checkout_branch(&mut self, name: &str) -> Result<(), GitError>;
}
