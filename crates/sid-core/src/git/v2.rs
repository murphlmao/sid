//! Workspaces v2 git model: rich status, structured diffs, stash, remotes,
//! worktrees and the paginated decorated log. Pure data — no library types; the
//! `git2` conversion lives in `sid-git`. The methods that produce/consume these are
//! on [`super::GitProvider`].

use serde::{Deserialize, Serialize};

use super::{CommitInfo, StatusKind};

// ─── Status ─────────────────────────────────────────────────────────────────

/// One path with BOTH of its sides folded together (the per-side rows stay in
/// [`super::GitStatus::entries`]).
///
/// # Examples
///
/// ```
/// use sid_core::git::{FileStatus, StatusKind};
/// let f = FileStatus {
///     path: "a.rs".into(),
///     old_path: None,
///     index: Some(StatusKind::Added),
///     worktree: Some(StatusKind::Modified),
/// };
/// assert!(f.is_staged() && f.is_unstaged() && !f.is_conflicted());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    /// Original path when either side is a rename.
    pub old_path: Option<String>,
    /// State in the index relative to HEAD; `None` = unchanged there.
    pub index: Option<StatusKind>,
    /// State in the working tree relative to the index; `None` = unchanged there.
    /// `Some(Conflicted)` marks an unmerged path.
    pub worktree: Option<StatusKind>,
}

impl FileStatus {
    /// Has a change recorded in the index.
    pub fn is_staged(&self) -> bool {
        self.index.is_some()
    }
    /// Has a tracked working-tree change (untracked/ignored/conflicted excluded).
    pub fn is_unstaged(&self) -> bool {
        matches!(
            self.worktree,
            Some(
                StatusKind::Modified
                    | StatusKind::Deleted
                    | StatusKind::Renamed
                    | StatusKind::Typechange
            )
        )
    }
    pub fn is_untracked(&self) -> bool {
        self.worktree == Some(StatusKind::Untracked)
    }
    pub fn is_conflicted(&self) -> bool {
        self.worktree == Some(StatusKind::Conflicted)
    }
}

/// Per-bucket counts over [`FileStatus`] rows (a path in two buckets counts in both).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StatusCounts {
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
    pub ignored: usize,
}

/// What [`super::GitProvider::status_with`] includes. The default (untracked on,
/// ignored off) is what [`super::GitProvider::status`] uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StatusOptions {
    pub include_untracked: bool,
    pub include_ignored: bool,
}

impl Default for StatusOptions {
    fn default() -> Self {
        Self {
            include_untracked: true,
            include_ignored: false,
        }
    }
}

// ─── Diff ───────────────────────────────────────────────────────────────────

/// What to diff.
///
/// # Examples
///
/// ```
/// use sid_core::git::DiffTarget;
/// let t = DiffTarget::Refs { from: "main".into(), to: "feature".into() };
/// assert_ne!(t, DiffTarget::IndexVsHead);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiffTarget {
    /// Unstaged: index to working tree (untracked files included as additions).
    WorkdirVsIndex,
    /// Staged: HEAD to index (an unborn HEAD diffs against the empty tree).
    IndexVsHead,
    /// A commit against its first parent (a root commit against the empty tree).
    Commit(String),
    /// `from` to `to`; any rev-parse expression.
    Refs { from: String, to: String },
}

/// Hard caps so a huge change can never wedge the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffLimits {
    /// Files kept; the rest only set [`Diff::truncated`].
    pub max_files: usize,
    /// Text bytes of line content kept per file; beyond it [`FileDiff::truncated`].
    pub max_bytes_per_file: usize,
    /// Text bytes kept over the whole diff; beyond it [`Diff::truncated`].
    pub max_total_bytes: usize,
}

impl Default for DiffLimits {
    fn default() -> Self {
        Self {
            max_files: 300,
            max_bytes_per_file: 256 * 1024,
            max_total_bytes: 2 * 1024 * 1024,
        }
    }
}

/// How a file changed in a diff.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Typechange,
    Untracked,
    Conflicted,
}

/// Line kind within a hunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// One diff line. Line numbers are 1-based; `None` on the side the line is absent from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    /// Line text without the trailing newline (lossy UTF-8).
    pub text: String,
}

/// One `@@ -a,b +c,d @@` hunk.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// The raw `@@ ... @@ section` header line.
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

/// One changed file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileDiff {
    /// `None` for an added file.
    pub old_path: Option<String>,
    /// `None` for a deleted file.
    pub new_path: Option<String>,
    pub status: DiffStatus,
    /// Binary content: `hunks` is empty.
    pub binary: bool,
    /// Line content was cut at [`DiffLimits::max_bytes_per_file`] / the total cap.
    pub truncated: bool,
    /// Totals for the file (counted even when lines were cut).
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<DiffHunk>,
}

impl FileDiff {
    /// The path to show: new path, else old.
    pub fn display_path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("")
    }
}

/// A structured diff.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    pub files: Vec<FileDiff>,
    /// Number of files in the full diff (>= `files.len()` when truncated).
    pub total_files: usize,
    /// Any cap hit (files dropped, or a file/total byte cap cut lines).
    pub truncated: bool,
}

// ─── Stash ──────────────────────────────────────────────────────────────────

/// One stash entry; `index` 0 is the newest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StashEntry {
    pub index: usize,
    pub message: String,
    pub oid: String,
    /// Branch the stash was made on, parsed from the message ("WIP on x: ..." /
    /// "On x: ..."); `None` when unparseable.
    pub branch: Option<String>,
}

// ─── Remote ─────────────────────────────────────────────────────────────────

/// Outcome of [`super::GitProvider::fetch`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FetchReport {
    pub remote: String,
    pub received_objects: usize,
    pub received_bytes: usize,
    /// Remote-tracking refs that moved (short names, e.g. `origin/main`).
    pub updated_refs: Vec<String>,
    /// Current branch vs its upstream after the fetch; `None` without an upstream.
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
}

// ─── Worktrees ──────────────────────────────────────────────────────────────

/// One linked (or the main) worktree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// `None` for the main worktree.
    pub name: Option<String>,
    pub path: String,
    /// Checked-out branch; `None` when detached/unborn/unreadable.
    pub branch: Option<String>,
    pub is_main: bool,
    pub locked: bool,
    /// Gitdir or working directory is gone and the entry can be pruned.
    pub prunable: bool,
}

// ─── Log ────────────────────────────────────────────────────────────────────

/// What a [`RefDecoration`] names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RefKind {
    Head,
    Branch,
    RemoteBranch,
    Tag,
}

/// A ref pointing at a logged commit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefDecoration {
    /// Short name (`main`, `origin/main`, `v1.0`, `HEAD`).
    pub name: String,
    pub kind: RefKind,
}

/// A page request for [`super::GitProvider::log_page`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LogQuery {
    pub skip: usize,
    pub limit: usize,
    /// Follow only first parents (hides merged-in history).
    pub first_parent: bool,
    /// Start from this rev instead of HEAD.
    pub rev: Option<String>,
}

impl Default for LogQuery {
    fn default() -> Self {
        Self {
            skip: 0,
            limit: 50,
            first_parent: false,
            rev: None,
        }
    }
}

/// A log row: the commit, its parents and the refs that decorate it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub commit: CommitInfo,
    pub parents: Vec<String>,
    pub refs: Vec<RefDecoration>,
}

/// One page of the log.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LogPage {
    pub entries: Vec<LogEntry>,
    /// More commits exist past this page.
    pub has_more: bool,
}
