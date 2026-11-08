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

mod v2;
pub use v2::{
    Diff, DiffHunk, DiffLimits, DiffLine, DiffLineKind, DiffStatus, DiffTarget, FetchReport,
    FileDiff, FileStatus, LogEntry, LogPage, LogQuery, PushReport, RefDecoration, RefKind, StashEntry,
    StatusCounts, StatusOptions, WorktreeInfo,
};

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
    /// The provider does not implement this operation.
    #[error("not supported by this git provider: {0}")]
    Unsupported(&'static str),
    #[error("commit message is empty")]
    EmptyMessage,
    /// `user.name` / `user.email` missing from git config; the payload says how to fix it.
    #[error("git identity not configured: {0}")]
    MissingIdentity(String),
    #[error("nothing to commit — no staged changes")]
    NothingToCommit,
    #[error("nothing to stash — no local changes")]
    NothingToStash,
    /// Unresolved merge conflicts block the operation.
    #[error("{0} conflicted path(s) — resolve them first")]
    Conflicts(usize),
    /// An apply/merge-style operation hit conflicting local changes.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("branch '{0}' already exists")]
    BranchExists(String),
    #[error("cannot delete '{0}': it is the current branch")]
    BranchIsCurrent(String),
    #[error("branch '{0}' is not fully merged — use force to delete it")]
    BranchNotMerged(String),
    #[error("stash entry {0} not found")]
    StashNotFound(usize),
    #[error("remote '{0}' not found")]
    RemoteNotFound(String),
    /// Authentication failed; sid never prompts, so only ssh-agent / default
    /// credentials are tried.
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    #[error("timed out: {0}")]
    TimedOut(String),
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
    Typechange,
    /// Only reported when [`StatusOptions::include_ignored`] is set.
    Ignored,
}

/// Aggregate status for a repo, plus a quick `is_clean` flag.
///
/// `entries` is the per-side view (a path changed in both index and worktree yields
/// two rows); `files` folds each path into one [`FileStatus`] with both sides;
/// `counts` rolls the buckets up.
///
/// # Examples
///
/// ```
/// use sid_core::git::GitStatus;
/// let s = GitStatus::default();
/// assert!(s.is_clean);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    pub entries: Vec<StatusEntry>,
    pub is_clean: bool,
    /// One row per path, both sides folded. Sorted by path.
    pub files: Vec<FileStatus>,
    pub counts: StatusCounts,
}

impl Default for GitStatus {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            is_clean: true,
            files: Vec::new(),
            counts: StatusCounts::default(),
        }
    }
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
///         Ok(GitStatus::default())
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

    // ─── v2 surface ─────────────────────────────────────────────────────────
    // Every method below has a default returning [`GitError::Unsupported`] so
    // minimal providers (and test doubles) keep compiling; `sid-git` implements all.

    /// Status with explicit inclusion options (e.g. ignored files). [`Self::status`]
    /// is this with [`StatusOptions::default`].
    fn status_with(&self, _opts: &StatusOptions) -> Result<GitStatus, GitError> {
        Err(GitError::Unsupported("status_with"))
    }

    /// Stage `paths` (repo-relative; files, or a directory = everything under it).
    /// A path that no longer exists stages its deletion. Paths escaping the repo
    /// (absolute or `..`) are [`GitError::InvalidRef`].
    fn stage_paths(&mut self, _paths: &[String]) -> Result<(), GitError> {
        Err(GitError::Unsupported("stage_paths"))
    }

    /// Stage every change, untracked files and deletions included (`git add -A`).
    fn stage_all(&mut self) -> Result<(), GitError> {
        Err(GitError::Unsupported("stage_all"))
    }

    /// Unstage `paths`: reset their index entries to HEAD (working tree untouched).
    /// Works on an unborn HEAD (the entries are just removed from the index).
    fn unstage_paths(&mut self, _paths: &[String]) -> Result<(), GitError> {
        Err(GitError::Unsupported("unstage_paths"))
    }

    /// Unstage everything.
    fn unstage_all(&mut self) -> Result<(), GitError> {
        Err(GitError::Unsupported("unstage_all"))
    }

    /// DESTRUCTIVE: overwrite the working-tree copy of tracked `paths` with the
    /// index version (restoring deleted files). Unsaved edits are unrecoverable, so
    /// the UI MUST confirm (two-click arm) first. Untracked paths are refused
    /// ([`GitError::InvalidRef`]); use [`Self::remove_untracked`] for those.
    fn discard_paths(&mut self, _paths: &[String]) -> Result<(), GitError> {
        Err(GitError::Unsupported("discard_paths"))
    }

    /// DESTRUCTIVE: delete untracked files/directories at `paths` from disk. Tracked
    /// paths are refused ([`GitError::InvalidRef`]). The UI MUST confirm first.
    fn remove_untracked(&mut self, _paths: &[String]) -> Result<(), GitError> {
        Err(GitError::Unsupported("remove_untracked"))
    }

    /// Structured, size-capped diff — see [`DiffTarget`] and [`DiffLimits`].
    fn diff(&self, _target: &DiffTarget, _limits: &DiffLimits) -> Result<Diff, GitError> {
        Err(GitError::Unsupported("diff"))
    }

    /// Commit the index. `message` is refused when blank ([`GitError::EmptyMessage`]);
    /// identity comes from git config ([`GitError::MissingIdentity`] when unset);
    /// no staged change is [`GitError::NothingToCommit`] (except `amend`, which may
    /// just reword); unresolved conflicts are [`GitError::Conflicts`]. `amend`
    /// rewrites HEAD (keeping its author). Works for the first commit on an unborn HEAD.
    fn commit(&mut self, _message: &str, _amend: bool) -> Result<CommitInfo, GitError> {
        Err(GitError::Unsupported("commit"))
    }

    /// The stash stack, newest (index 0) first.
    fn stash_list(&self) -> Result<Vec<StashEntry>, GitError> {
        Err(GitError::Unsupported("stash_list"))
    }

    /// Stash local changes, optionally including untracked files. Returns the new
    /// entry. [`GitError::NothingToStash`] when there is nothing to save.
    fn stash_push(
        &mut self,
        _message: Option<&str>,
        _include_untracked: bool,
    ) -> Result<StashEntry, GitError> {
        Err(GitError::Unsupported("stash_push"))
    }

    /// Apply stash `index`, keeping it. Local edits that collide with it are
    /// [`GitError::Conflict`] (working tree untouched); a content conflict is applied
    /// with markers and reported as [`GitError::Conflicts`].
    fn stash_apply(&mut self, _index: usize) -> Result<(), GitError> {
        Err(GitError::Unsupported("stash_apply"))
    }

    /// Apply then drop stash `index`; the entry is kept when the apply fails or conflicts.
    fn stash_pop(&mut self, _index: usize) -> Result<(), GitError> {
        Err(GitError::Unsupported("stash_pop"))
    }

    /// Drop stash `index` without applying it.
    fn stash_drop(&mut self, _index: usize) -> Result<(), GitError> {
        Err(GitError::Unsupported("stash_drop"))
    }

    /// Create a local branch at `from` (any rev; `None` = HEAD). Does not switch to
    /// it. [`GitError::BranchExists`] on a name clash.
    fn create_branch(&mut self, _name: &str, _from: Option<&str>) -> Result<Branch, GitError> {
        Err(GitError::Unsupported("create_branch"))
    }

    /// Delete a local branch. Refuses the current branch
    /// ([`GitError::BranchIsCurrent`]) and, unless `force`, one not merged into HEAD
    /// ([`GitError::BranchNotMerged`]).
    fn delete_branch(&mut self, _name: &str, _force: bool) -> Result<(), GitError> {
        Err(GitError::Unsupported("delete_branch"))
    }

    /// Rename a local branch (HEAD follows when it is the current one).
    fn rename_branch(&mut self, _old: &str, _new: &str) -> Result<(), GitError> {
        Err(GitError::Unsupported("rename_branch"))
    }

    /// Configured remote names.
    fn remotes(&self) -> Result<Vec<String>, GitError> {
        Err(GitError::Unsupported("remotes"))
    }

    /// Fetch `remote`'s configured refspecs. Credentials: ssh-agent, then default
    /// credentials — never a prompt; exhaustion is [`GitError::AuthFailed`]. The call
    /// is time-bounded ([`GitError::TimedOut`]). Reports ahead/behind afterwards.
    fn fetch(&self, _remote: &str) -> Result<FetchReport, GitError> {
        Err(GitError::Unsupported("fetch"))
    }

    /// Push the current branch to its configured upstream. Never prompts
    /// ([`GitError::AuthFailed`]); time-bounded ([`GitError::TimedOut`]); a rejected
    /// non-fast-forward is [`GitError::Conflict`].
    fn push(&self) -> Result<PushReport, GitError> {
        Err(GitError::Unsupported("push"))
    }

    /// The main worktree plus linked ones (read-only).
    fn worktrees(&self) -> Result<Vec<WorktreeInfo>, GitError> {
        Err(GitError::Unsupported("worktrees"))
    }

    /// A page of history with branch/tag/HEAD decorations — see [`LogQuery`].
    fn log_page(&self, _query: &LogQuery) -> Result<LogPage, GitError> {
        Err(GitError::Unsupported("log_page"))
    }
}
