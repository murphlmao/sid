//! Tests for the Commit log sub-view (Task 25).

use std::path::{Path, PathBuf};

use sid_core::{
    adapters::git::{Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit},
    workspace_metadata::WorkspaceKind,
};
use sid_store::Workspace;
use sid_widgets::workspaces::{LogListState, RightPane, WorkspacesState};

// ─── Mock ─────────────────────────────────────────────────────────────────────

struct LogMockGit {
    commits: Vec<CommitInfo>,
}

impl LogMockGit {
    fn new(commits: Vec<CommitInfo>) -> Self {
        Self { commits }
    }
}

impl GitProvider for LogMockGit {
    fn open(&self, _path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        Ok(Box::new(LogMockGit::new(self.commits.clone())))
    }
    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        Ok(vec![])
    }
    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        Ok(None)
    }
    fn status(&self) -> Result<GitStatus, GitError> {
        Ok(GitStatus {
            entries: vec![],
            is_clean: true,
        })
    }
    fn commit_log(&self, max: usize, from_oid: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
        if max == 0 {
            return Ok(vec![]);
        }
        let start_idx = match from_oid {
            Some(oid) => match self.commits.iter().position(|c| c.oid == oid) {
                Some(idx) => idx,
                None => {
                    return Err(GitError::InvalidRef(format!("oid not found: {oid}")));
                }
            },
            None => 0,
        };
        Ok(self
            .commits
            .iter()
            .skip(start_idx)
            .take(max)
            .cloned()
            .collect())
    }
    fn diff(&self, _staged: bool) -> Result<Vec<DiffEntry>, GitError> {
        Ok(vec![])
    }
    fn checkout_branch(&mut self, _name: &str) -> Result<(), GitError> {
        Ok(())
    }
    fn commit(&mut self, _new: NewCommit<'_>) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_commit(oid: &str, summary: &str) -> CommitInfo {
    CommitInfo {
        oid: oid.to_string(),
        summary: summary.to_string(),
        author_name: "Test Author".into(),
        author_email: "test@example.com".into(),
        timestamp_secs: 1_700_000_000,
        parents: vec![],
    }
}

fn make_commits(count: usize) -> Vec<CommitInfo> {
    (0..count)
        .map(|i| make_commit(&format!("{:040}", i), &format!("commit {i}")))
        .collect()
}

fn _ws(p: &str) -> Workspace {
    Workspace {
        path: PathBuf::from(p),
        name: p.trim_start_matches('/').to_string(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    }
}

// ─── LogListState tests ───────────────────────────────────────────────────────

#[test]
fn log_list_state_new_empty() {
    let s = LogListState::new(vec![], 50);
    assert!(s.entries().is_empty());
    assert_eq!(s.page_size(), 50);
    assert!(s.next_page_cursor().is_none());
}

#[test]
fn log_list_state_page_size_minimum_is_1() {
    let s = LogListState::new(vec![], 0);
    assert_eq!(s.page_size(), 1);
}

#[test]
fn log_list_state_set_entries_updates_cursor() {
    let mut s = LogListState::new(vec![], 3);
    let commits = make_commits(3);
    let last_oid = commits[2].oid.clone();
    s.set_entries(commits);
    assert_eq!(s.next_page_cursor(), Some(last_oid.as_str()));
}

#[test]
fn log_list_state_navigation() {
    let mut s = LogListState::new(make_commits(5), 50);
    assert_eq!(s.selected_idx(), 0);
    s.select_next();
    assert_eq!(s.selected_idx(), 1);
    s.select_next();
    s.select_next();
    assert_eq!(s.selected_idx(), 3);
    s.select_prev();
    assert_eq!(s.selected_idx(), 2);
}

#[test]
fn log_list_state_navigation_wraps() {
    let mut s = LogListState::new(make_commits(3), 50);
    s.select_prev(); // wraps to last
    assert_eq!(s.selected_idx(), 2);
    s.select_next(); // wraps to first
    assert_eq!(s.selected_idx(), 0);
}

#[test]
fn log_list_state_empty_navigation_is_safe() {
    let mut s = LogListState::new(vec![], 50);
    s.select_next();
    s.select_prev();
    assert_eq!(s.selected_idx(), 0);
}

// ─── Pagination cursor stack ─────────────────────────────────────────────────

#[test]
fn pagination_forward_and_back_restores_cursor() {
    let mut s = LogListState::new(vec![], 3);
    let page0 = make_commits(3); // commits 0, 1, 2

    // Load page 0 (from HEAD = None)
    s.push_cursor(None); // remember cursor before page 0
    s.set_entries(page0.clone());
    let cursor_for_page1 = s.next_page_cursor().map(String::from);
    assert!(cursor_for_page1.is_some()); // cursor = last OID of page 0

    // Simulate going to next page
    s.push_cursor(cursor_for_page1.clone()); // push current page's ending cursor
    let page1 = make_commits(3); // commits 3, 4, 5
    s.set_entries(page1);

    // Go back: pop cursor to restore to page 0's starting cursor
    let prev_cursor = s.pop_cursor();
    assert!(prev_cursor.is_some());

    // Pop again for original None cursor
    let orig_cursor = s.pop_cursor();
    assert_eq!(orig_cursor, Some(None)); // the initial None we pushed
}

#[test]
fn pop_cursor_from_empty_stack_returns_none() {
    let mut s = LogListState::new(vec![], 50);
    let result = s.pop_cursor();
    assert!(result.is_none());
}

// ─── Provider interaction tests ──────────────────────────────────────────────

#[test]
fn commit_log_provider_returns_correct_count() {
    let commits = make_commits(10);
    let provider = LogMockGit::new(commits.clone());
    let result = provider.commit_log(5, None).unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(result[0].summary, "commit 0");
}

#[test]
fn commit_log_provider_respects_from_oid() {
    let commits = make_commits(5);
    let from_oid = commits[2].oid.clone();
    let provider = LogMockGit::new(commits);
    let result = provider.commit_log(10, Some(&from_oid)).unwrap();
    assert_eq!(result[0].summary, "commit 2");
    assert_eq!(result.len(), 3); // commits 2, 3, 4
}

#[test]
fn commit_log_zero_max_returns_empty() {
    let commits = make_commits(5);
    let provider = LogMockGit::new(commits);
    let result = provider.commit_log(0, None).unwrap();
    assert!(result.is_empty());
}

#[test]
fn commit_log_invalid_oid_returns_error() {
    let commits = make_commits(3);
    let provider = LogMockGit::new(commits);
    let err = provider.commit_log(5, Some("bad-oid")).unwrap_err();
    assert!(matches!(err, GitError::InvalidRef(_)));
}

// ─── WorkspacesState Log pane integration ─────────────────────────────────────

#[test]
fn switching_to_log_pane_works() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    s.cycle_pane_next(); // Status
    s.cycle_pane_next(); // Log
    assert!(matches!(s.right_pane(), RightPane::Log(_)));
}

#[test]
fn log_pane_can_receive_entries() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    s.cycle_pane_next();
    s.cycle_pane_next(); // Log
    if let RightPane::Log(log) = s.right_pane_mut() {
        log.set_entries(make_commits(10));
        assert_eq!(log.entries().len(), 10);
    }
}

// ─── Adversarial tests ───────────────────────────────────────────────────────

#[test]
fn very_long_commit_summary_is_stored_correctly() {
    let long_summary = "x".repeat(10_000);
    let commits = vec![make_commit("a".repeat(40).as_str(), &long_summary)];
    let s = LogListState::new(commits, 50);
    assert_eq!(s.entries()[0].summary.len(), 10_000);
}

#[test]
fn unicode_commit_summaries() {
    let commits = vec![
        make_commit(&"a".repeat(40), "feat: 工作区 improvements 🐕"),
        make_commit(&"b".repeat(40), "fix: handle résumé files"),
    ];
    let s = LogListState::new(commits, 50);
    assert!(s.entries()[0].summary.contains('🐕'));
}

#[test]
fn large_page_does_not_panic() {
    let commits = make_commits(100);
    let mut s = LogListState::new(commits, 100);
    for _ in 0..1000 {
        s.select_next();
    }
    assert_eq!(s.selected_idx(), 0); // 1000 % 100 = 0
}

#[test]
fn set_entries_on_empty_page_gives_none_cursor() {
    let mut s = LogListState::new(vec![], 50);
    s.set_entries(vec![]);
    assert!(s.next_page_cursor().is_none());
}

// ─── Property tests ──────────────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_log_nav_next_then_prev_invariant(n in 1usize..100) {
        let commits = make_commits(n);
        let mut s = LogListState::new(commits, 50);
        let start = s.selected_idx();
        s.select_next();
        s.select_prev();
        prop_assert_eq!(s.selected_idx(), start);
    }

    #[test]
    fn prop_cursor_stack_round_trip(depth in 0usize..10) {
        let mut s = LogListState::new(vec![], 50);
        for i in 0..depth {
            s.push_cursor(Some(format!("oid-{i}")));
        }
        let mut popped = Vec::new();
        while let Some(c) = s.pop_cursor() {
            popped.push(c);
        }
        prop_assert_eq!(popped.len(), depth);
        // Popped in LIFO order
        if depth > 0 {
            prop_assert_eq!(popped[0].clone(), Some(format!("oid-{}", depth - 1)));
        }
    }

    /// Invariant: navigating to page N and back to page N-1 gives same position
    #[test]
    fn prop_page_n_and_back_yields_same_page(page_size in 1usize..20, n_pages in 1usize..5) {
        let total = page_size * n_pages;
        let commits = make_commits(total);
        let mut s = LogListState::new(vec![], page_size);

        // Simulate loading page 0
        s.push_cursor(None);
        let page_0_entries: Vec<CommitInfo> = commits.iter().take(page_size).cloned().collect();
        let cursor_after_page0 = page_0_entries.last().map(|c| c.oid.clone());
        s.set_entries(page_0_entries.clone());

        // Go to page 1
        s.push_cursor(cursor_after_page0.clone());
        let page_1_entries: Vec<CommitInfo> = commits.iter().skip(page_size).take(page_size).cloned().collect();
        s.set_entries(page_1_entries);

        // Go back: pop, reload page 0
        let _ = s.pop_cursor(); // pops cursor_after_page0
        let back_cursor = s.pop_cursor(); // pops None (original)
        prop_assert_eq!(back_cursor, Some(None));

        // Reload page 0 with None cursor (from HEAD)
        s.set_entries(page_0_entries.clone());
        prop_assert_eq!(s.entries().len(), page_0_entries.len());
    }
}
