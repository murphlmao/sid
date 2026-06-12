//! Pure state for the Workspaces *detail* tab (UX-v2 rework).
//!
//! The detail tab shows an umbrella row plus its satellites in a left list,
//! each carrying a [`RepoGit`] snapshot loaded off-thread by the binary (so
//! widget code never names `git2`). The right side is a `SplitView` drill-in
//! stack over [`DetailView`]; see `workspace_detail.rs` for the widget that
//! drives these types.

use std::path::PathBuf;

use sid_core::adapters::git::{Branch, CommitInfo, DiffEntry};

/// A git snapshot for one repo row. `None`-ish defaults mean "not loaded yet";
/// the binary replaces this wholesale via the widget's `apply_*` setters once
/// the off-thread load completes.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail_state::RepoGit;
///
/// let g = RepoGit::loading();
/// assert!(g.is_loading());
/// assert_eq!(g.branch, "…");
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepoGit {
    /// Current branch name, or `"…"` while loading, `"?"` if detached/failed.
    pub branch: String,
    /// Files with uncommitted changes.
    pub dirty: u32,
    /// Commits ahead of upstream (the "outgoing" count shown in the header).
    pub outgoing: u32,
    /// Commits behind upstream.
    pub behind: u32,
    /// True until the off-thread load lands.
    loading: bool,
}

impl RepoGit {
    /// The pre-load placeholder snapshot.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspace_detail_state::RepoGit;
    /// assert!(RepoGit::loading().is_loading());
    /// ```
    pub fn loading() -> Self {
        Self {
            branch: "…".to_string(),
            dirty: 0,
            outgoing: 0,
            behind: 0,
            loading: true,
        }
    }

    /// Build a loaded snapshot.
    pub fn loaded(branch: String, dirty: u32, outgoing: u32, behind: u32) -> Self {
        Self {
            branch,
            dirty,
            outgoing,
            behind,
            loading: false,
        }
    }

    /// Whether the off-thread load is still pending.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// One-line header summary: `main · 3 dirty · ↑2`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspace_detail_state::RepoGit;
    /// let g = RepoGit::loaded("main".into(), 3, 2, 0);
    /// assert_eq!(g.header_summary(), "main · 3 dirty · ↑2");
    /// let clean = RepoGit::loaded("main".into(), 0, 0, 0);
    /// assert_eq!(clean.header_summary(), "main · clean");
    /// ```
    pub fn header_summary(&self) -> String {
        let dirty = if self.dirty == 0 {
            "clean".to_string()
        } else {
            format!("{} dirty", self.dirty)
        };
        let mut s = format!("{} · {dirty}", self.branch);
        if self.outgoing > 0 {
            s.push_str(&format!(" · ↑{}", self.outgoing));
        }
        if self.behind > 0 {
            s.push_str(&format!(" · ↓{}", self.behind));
        }
        s
    }
}

/// One row in the detail tab's left list: a repo (umbrella or satellite) plus
/// its git snapshot.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_widgets::workspace_detail_state::{RepoGit, SatelliteRow};
///
/// let row = SatelliteRow {
///     name: "api".into(),
///     path: PathBuf::from("/stack/api"),
///     is_umbrella: false,
///     git: RepoGit::loading(),
/// };
/// assert_eq!(row.name, "api");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SatelliteRow {
    /// Display name (umbrella name or satellite basename).
    pub name: String,
    /// Absolute repo path — the key the binary loads git data against.
    pub path: PathBuf,
    /// True for the single umbrella row that heads the list.
    pub is_umbrella: bool,
    /// Off-thread git snapshot for this row.
    pub git: RepoGit,
}

/// Loaded git detail for the currently-drilled repo (Outgoing/Log/Branches/etc.).
/// The binary fills these via the widget's `apply_*` setters as the user drills.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail_state::RepoDetail;
/// let d = RepoDetail::default();
/// assert!(d.branches.is_empty());
/// assert!(d.commits.is_empty());
/// ```
#[derive(Clone, Debug, Default)]
pub struct RepoDetail {
    /// Branches (Branches view).
    pub branches: Vec<Branch>,
    /// Commits — populated for both Outgoing and Log views.
    pub commits: Vec<CommitInfo>,
    /// Per-file diff entries for the diff view.
    pub diff: Vec<DiffEntry>,
}

/// The fixed set of git operations the detail drill-in offers, in render order.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail_state::DetailOp;
/// assert_eq!(DetailOp::Branches.label(), "Branches");
/// assert_eq!(DetailOp::ALL.len(), 5);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailOp {
    /// Commits ahead of upstream (the "outgoing" list).
    Outgoing,
    /// Full commit log (paginated by the binary).
    Log,
    /// Branch list.
    Branches,
    /// Stash entries (read-only list in v1).
    Stash,
    /// Linked worktrees (read-only list in v1).
    Worktrees,
}

impl DetailOp {
    /// Every op, in the order the ops menu renders them.
    pub const ALL: [DetailOp; 5] = [
        DetailOp::Outgoing,
        DetailOp::Log,
        DetailOp::Branches,
        DetailOp::Stash,
        DetailOp::Worktrees,
    ];

    /// Human-readable menu label.
    pub fn label(self) -> &'static str {
        match self {
            DetailOp::Outgoing => "Outgoing",
            DetailOp::Log => "Log",
            DetailOp::Branches => "Branches",
            DetailOp::Stash => "Stash",
            DetailOp::Worktrees => "Worktrees",
        }
    }
}

/// One level of the right-pane drill-in stack (held by `SplitView<DetailView>`).
///
/// Stack shapes the detail tab pushes:
/// - `[Op(op)]` — the op's primary list (commits for Outgoing/Log, branches, …).
/// - `[Op(Outgoing|Log), Commits, Diff(idx)]` — drilled into a commit's diff.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail_state::{DetailOp, DetailView};
/// let v = DetailView::Op(DetailOp::Outgoing);
/// assert!(matches!(v, DetailView::Op(_)));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailView {
    /// An op's top-level list.
    Op(DetailOp),
    /// A scrollable diff for the commit at this index into `RepoDetail::commits`.
    Diff(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_default_renders_ellipsis_branch() {
        let g = RepoGit::loading();
        assert!(g.is_loading());
        assert_eq!(g.branch, "…");
        assert_eq!(g.header_summary(), "… · clean");
    }

    #[test]
    fn header_summary_covers_all_arms() {
        assert_eq!(
            RepoGit::loaded("dev".into(), 0, 0, 0).header_summary(),
            "dev · clean"
        );
        assert_eq!(
            RepoGit::loaded("dev".into(), 1, 0, 0).header_summary(),
            "dev · 1 dirty"
        );
        assert_eq!(
            RepoGit::loaded("dev".into(), 0, 5, 2).header_summary(),
            "dev · clean · ↑5 · ↓2"
        );
    }

    #[test]
    fn default_repogit_is_not_loading() {
        // Default (derive) leaves loading=false; only `loading()` sets it true.
        assert!(!RepoGit::default().is_loading());
    }

    #[test]
    fn detail_op_cycles_and_labels() {
        assert_eq!(DetailOp::ALL.len(), 5);
        assert_eq!(DetailOp::Outgoing.label(), "Outgoing");
        assert_eq!(DetailOp::Worktrees.label(), "Worktrees");
        // ALL is the stable render order
        assert_eq!(DetailOp::ALL[0], DetailOp::Outgoing);
        assert_eq!(DetailOp::ALL[2], DetailOp::Branches);
    }

    #[test]
    fn detail_view_from_op_wraps_the_op() {
        let v = DetailView::Op(DetailOp::Log);
        assert!(matches!(v, DetailView::Op(DetailOp::Log)));
    }
}
