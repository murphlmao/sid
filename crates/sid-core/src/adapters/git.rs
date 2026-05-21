//! Git provider trait + supporting domain types. Implementations live in `sid-git`.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Domain-shaped git error. Concrete impls map their library errors into this.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::git::GitError;
///
/// let e = GitError::NotARepo("/tmp/not-a-repo".into());
/// assert!(!format!("{e}").is_empty());
///
/// let dirty = GitError::DirtyWorkingTree(5);
/// assert!(!format!("{dirty}").is_empty());
/// ```
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("repository not found at {0}")]
    NotARepo(String),
    #[error("working tree is dirty: {0} uncommitted change(s) — refuse to proceed")]
    DirtyWorkingTree(usize),
    #[error("branch '{0}' not found")]
    BranchNotFound(String),
    #[error("invalid reference: {0}")]
    InvalidRef(String),
    #[error("merge conflict in {0} path(s)")]
    Conflict(usize),
    #[error("git operation failed: {0}")]
    Other(String),
}

/// A branch reference plus whether it's the currently checked-out branch.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::git::Branch;
///
/// let b = Branch {
///     name: "main".into(),
///     head_oid: "a".repeat(40),
///     upstream: Some("origin/main".into()),
///     is_current: true,
/// };
/// assert_eq!(b.name, "main");
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

/// One entry in the porcelain v2 status output.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::git::{StatusEntry, StatusKind};
///
/// let e = StatusEntry {
///     path: "src/lib.rs".into(),
///     kind: StatusKind::Modified,
///     staged: false,
///     old_path: None,
/// };
/// assert_eq!(e.path, "src/lib.rs");
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
/// use sid_core::adapters::git::StatusKind;
///
/// let k = StatusKind::Added;
/// assert_eq!(k, StatusKind::Added);
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

/// Aggregate of all status entries for a repo, plus a quick `is_clean` flag.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::git::GitStatus;
///
/// let s = GitStatus { entries: vec![], is_clean: true };
/// assert!(s.is_clean);
/// assert!(s.entries.is_empty());
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
/// use sid_core::adapters::git::CommitInfo;
///
/// let c = CommitInfo {
///     oid: "a".repeat(40),
///     summary: "feat: add thing".into(),
///     author_name: "Alice".into(),
///     author_email: "alice@example.com".into(),
///     timestamp_secs: 1_700_000_000,
///     parents: vec![],
/// };
/// assert_eq!(c.summary, "feat: add thing");
/// assert_eq!(c.timestamp_secs, 1_700_000_000);
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
    /// Parent OIDs (1 for normal commits, 2+ for merges).
    pub parents: Vec<String>,
}

/// One diff hunk pair (per-file).
///
/// # Examples
///
/// ```
/// use sid_core::adapters::git::DiffEntry;
///
/// let d = DiffEntry {
///     path: "src/main.rs".into(),
///     old_path: None,
///     patch: "@@ -1 +1,2 @@\n hello\n+world\n".into(),
///     added: 1,
///     removed: 0,
/// };
/// assert_eq!(d.added, 1);
/// assert!(d.patch.contains("+world"));
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffEntry {
    pub path: String,
    pub old_path: Option<String>,
    /// Unified diff text (`@@ -…,… +…,… @@` blocks).
    pub patch: String,
    pub added: usize,
    pub removed: usize,
}

/// Inputs for a new commit. Author/committer come from repo config if omitted.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::git::NewCommit;
///
/// let nc = NewCommit {
///     message: "fix: correct off-by-one",
///     author_name: Some("Bob"),
///     author_email: Some("bob@example.com"),
///     stage_all: false,
/// };
/// assert_eq!(nc.message, "fix: correct off-by-one");
/// assert!(!nc.stage_all);
/// ```
#[derive(Clone, Debug)]
pub struct NewCommit<'a> {
    pub message: &'a str,
    pub author_name: Option<&'a str>,
    pub author_email: Option<&'a str>,
    /// If true, also commit unstaged changes (stage-all-then-commit).
    pub stage_all: bool,
}

/// Git operations needed by the Workspaces tab. Implementations live in `sid-git`.
///
/// # Object safety
///
/// All methods take `&self`/`&mut self` and use no generics in method position,
/// so `Box<dyn GitProvider>` works.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use sid_core::adapters::git::{Branch, CommitInfo, DiffEntry, GitError, GitProvider,
///     GitStatus, NewCommit, StatusEntry};
///
/// struct NoopGit;
///
/// impl GitProvider for NoopGit {
///     fn open(&self, _p: &Path) -> Result<Box<dyn GitProvider>, GitError> {
///         Ok(Box::new(NoopGit))
///     }
///     fn list_branches(&self) -> Result<Vec<Branch>, GitError> { Ok(vec![]) }
///     fn current_branch(&self) -> Result<Option<Branch>, GitError> { Ok(None) }
///     fn status(&self) -> Result<GitStatus, GitError> {
///         Ok(GitStatus { entries: vec![], is_clean: true })
///     }
///     fn commit_log(&self, _max: usize, _from: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
///         Ok(vec![])
///     }
///     fn diff(&self, _staged: bool) -> Result<Vec<DiffEntry>, GitError> { Ok(vec![]) }
///     fn checkout_branch(&mut self, _name: &str) -> Result<(), GitError> { Ok(()) }
///     fn commit(&mut self, _new: NewCommit<'_>) -> Result<String, GitError> {
///         Ok("0".repeat(40))
///     }
/// }
///
/// let g: Box<dyn GitProvider> = Box::new(NoopGit);
/// assert!(g.list_branches().unwrap().is_empty());
/// ```
pub trait GitProvider: Send + Sync {
    /// Open the repo at `path`. Returns a *new* provider bound to that repo.
    /// (The caller's `self` may be a "factory" provider; the returned one is
    /// the per-repo handle.)
    fn open(&self, path: &Path) -> Result<Box<dyn GitProvider>, GitError>;

    fn list_branches(&self) -> Result<Vec<Branch>, GitError>;
    fn current_branch(&self) -> Result<Option<Branch>, GitError>;
    fn status(&self) -> Result<GitStatus, GitError>;

    /// Walk the commit log starting at `from_oid` (None = HEAD), returning at most `max` commits.
    fn commit_log(&self, max: usize, from_oid: Option<&str>) -> Result<Vec<CommitInfo>, GitError>;

    /// Return per-file diffs. `staged = true` returns index-vs-HEAD; `false` returns working-tree-vs-index.
    fn diff(&self, staged: bool) -> Result<Vec<DiffEntry>, GitError>;

    /// Switch to `name`. Refuses if the working tree is dirty (returns `GitError::DirtyWorkingTree`).
    fn checkout_branch(&mut self, name: &str) -> Result<(), GitError>;

    /// Commit. Returns the new commit OID.
    fn commit(&mut self, new: NewCommit<'_>) -> Result<String, GitError>;
}
