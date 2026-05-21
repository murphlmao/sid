//! Tests for the Status sub-view (Task 24).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sid_core::adapters::git::{
    Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit, StatusEntry,
    StatusKind,
};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspaces::{RightPane, StatusListState, WorkspacesState};

// ─── Mock git provider ───────────────────────────────────────────────────────

struct StatusMockGit {
    status: GitStatus,
    status_calls: Arc<Mutex<u32>>,
    status_should_err: bool,
}

impl StatusMockGit {
    fn new(status: GitStatus) -> Self {
        Self { status, status_calls: Arc::new(Mutex::new(0)), status_should_err: false }
    }

    fn with_error(mut self) -> Self {
        self.status_should_err = true;
        self
    }

    fn call_count(&self) -> Arc<Mutex<u32>> {
        Arc::clone(&self.status_calls)
    }
}

impl GitProvider for StatusMockGit {
    fn open(&self, _path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        Ok(Box::new(StatusMockGit::new(self.status.clone())))
    }
    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        Ok(vec![])
    }
    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        Ok(None)
    }
    fn status(&self) -> Result<GitStatus, GitError> {
        *self.status_calls.lock().unwrap() += 1;
        if self.status_should_err {
            return Err(GitError::Other("status failed".into()));
        }
        Ok(self.status.clone())
    }
    fn commit_log(&self, _max: usize, _from: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
        Ok(vec![])
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

fn entry(path: &str, kind: StatusKind, staged: bool) -> StatusEntry {
    StatusEntry { path: path.into(), kind, staged, old_path: None }
}

fn dirty_status(entries: Vec<StatusEntry>) -> GitStatus {
    GitStatus { entries, is_clean: false }
}

fn clean_status() -> GitStatus {
    GitStatus { entries: vec![], is_clean: true }
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

// ─── StatusListState tests ───────────────────────────────────────────────────

#[test]
fn status_list_state_clean_repo() {
    let s = StatusListState::new(clean_status());
    assert!(s.status().is_clean);
    assert!(s.status().entries.is_empty());
}

#[test]
fn status_list_state_dirty_repo_has_entries() {
    let s = StatusListState::new(dirty_status(vec![
        entry("src/main.rs", StatusKind::Modified, false),
        entry("Cargo.toml", StatusKind::Modified, true),
    ]));
    assert!(!s.status().is_clean);
    assert_eq!(s.status().entries.len(), 2);
}

#[test]
fn status_list_state_navigation() {
    let mut s = StatusListState::new(dirty_status(vec![
        entry("a.rs", StatusKind::Modified, false),
        entry("b.rs", StatusKind::Added, true),
        entry("c.rs", StatusKind::Deleted, false),
    ]));
    assert_eq!(s.selected_idx(), 0);
    s.select_next();
    assert_eq!(s.selected_idx(), 1);
    s.select_next();
    assert_eq!(s.selected_idx(), 2);
    s.select_next(); // wrap
    assert_eq!(s.selected_idx(), 0);
    s.select_prev(); // wrap backward
    assert_eq!(s.selected_idx(), 2);
}

#[test]
fn status_list_state_empty_navigation_is_safe() {
    let mut s = StatusListState::new(clean_status());
    s.select_next();
    s.select_prev();
    assert_eq!(s.selected_idx(), 0);
}

#[test]
fn status_list_set_status_clamps_selection_on_shorter_list() {
    let mut s = StatusListState::new(dirty_status(
        (0..5).map(|i| entry(&format!("f{i}.rs"), StatusKind::Modified, false)).collect(),
    ));
    s.select_next();
    s.select_next();
    s.select_next(); // idx = 3
    assert_eq!(s.selected_idx(), 3);

    s.set_status(dirty_status(vec![
        entry("only.rs", StatusKind::Modified, false),
    ]));
    assert_eq!(s.selected_idx(), 0);
}

#[test]
fn status_list_set_status_preserves_selection_within_bounds() {
    let mut s = StatusListState::new(dirty_status(vec![
        entry("a.rs", StatusKind::Modified, false),
        entry("b.rs", StatusKind::Added, true),
    ]));
    s.select_next(); // idx = 1
    s.set_status(dirty_status(vec![
        entry("a.rs", StatusKind::Modified, false),
        entry("b.rs", StatusKind::Added, true),
        entry("c.rs", StatusKind::Deleted, false),
    ]));
    assert_eq!(s.selected_idx(), 1);
}

// ─── Git provider refresh tests ───────────────────────────────────────────────

#[test]
fn status_provider_call_returns_correct_data() {
    let status = dirty_status(vec![
        entry("modified.rs", StatusKind::Modified, false),
        entry("new.txt", StatusKind::Untracked, false),
    ]);
    let mock = StatusMockGit::new(status.clone());
    let calls = mock.call_count();
    let provider: Box<dyn GitProvider> = Box::new(mock);
    let result = provider.status().unwrap();
    assert_eq!(result.entries.len(), 2);
    assert_eq!(*calls.lock().unwrap(), 1);
}

#[test]
fn status_provider_error_is_propagated() {
    let mock = StatusMockGit::new(clean_status()).with_error();
    let provider: Box<dyn GitProvider> = Box::new(mock);
    let err = provider.status().unwrap_err();
    assert!(matches!(err, GitError::Other(_)));
}

// ─── WorkspacesState Status pane ─────────────────────────────────────────────

#[test]
fn switching_to_status_pane_works() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    s.cycle_pane_next(); // Branches -> Status
    assert!(matches!(s.right_pane(), RightPane::Status(_)));
}

#[test]
fn status_pane_can_receive_refreshed_data() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    s.cycle_pane_next(); // Status pane
    if let RightPane::Status(st) = s.right_pane_mut() {
        st.set_status(dirty_status(vec![
            entry("a.rs", StatusKind::Modified, false),
        ]));
        assert!(!st.status().is_clean);
        assert_eq!(st.status().entries.len(), 1);
    }
}

// ─── Adversarial tests ───────────────────────────────────────────────────────

#[test]
fn unicode_paths_in_status_entries() {
    let s = StatusListState::new(dirty_status(vec![
        entry("src/工作区/main.rs", StatusKind::Modified, false),
        entry("hello-🐕.txt", StatusKind::Untracked, false),
    ]));
    assert_eq!(s.status().entries[0].path, "src/工作区/main.rs");
    assert_eq!(s.status().entries[1].path, "hello-🐕.txt");
}

#[test]
fn very_large_status_list_does_not_panic() {
    let entries: Vec<StatusEntry> = (0..1000)
        .map(|i| entry(&format!("file-{i}.rs"), StatusKind::Modified, i % 2 == 0))
        .collect();
    let mut s = StatusListState::new(dirty_status(entries));
    // Navigate through all entries
    for _ in 0..1000 {
        s.select_next();
    }
    // 1000 steps on 1000-item list = back to start
    assert_eq!(s.selected_idx(), 0);
}

#[test]
fn all_status_kinds_are_constructible() {
    let all_kinds = [
        StatusKind::Modified,
        StatusKind::Added,
        StatusKind::Deleted,
        StatusKind::Renamed,
        StatusKind::Untracked,
        StatusKind::Conflicted,
    ];
    let entries: Vec<StatusEntry> = all_kinds
        .iter()
        .map(|k| entry("test.rs", *k, false))
        .collect();
    let s = StatusListState::new(dirty_status(entries));
    assert_eq!(s.status().entries.len(), 6);
}

#[test]
fn staged_and_unstaged_mixed_status() {
    let s = StatusListState::new(dirty_status(vec![
        entry("a.rs", StatusKind::Modified, true),   // staged
        entry("a.rs", StatusKind::Modified, false),  // also unstaged (both staged and wt changes)
        entry("b.rs", StatusKind::Added, true),      // staged only
    ]));
    let staged: Vec<_> = s.status().entries.iter().filter(|e| e.staged).collect();
    let unstaged: Vec<_> = s.status().entries.iter().filter(|e| !e.staged).collect();
    assert_eq!(staged.len(), 2);
    assert_eq!(unstaged.len(), 1);
}

// ─── Property tests ──────────────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_status_nav_next_then_prev_returns_to_start(n in 1usize..50) {
        let entries: Vec<StatusEntry> = (0..n)
            .map(|i| entry(&format!("f{i}.rs"), StatusKind::Modified, false))
            .collect();
        let mut s = StatusListState::new(dirty_status(entries));
        let start = s.selected_idx();
        s.select_next();
        s.select_prev();
        prop_assert_eq!(s.selected_idx(), start);
    }

    #[test]
    fn prop_set_status_never_panics_with_any_size(prev_n in 0usize..50, new_n in 0usize..50) {
        let make_entries = |n: usize| -> Vec<StatusEntry> {
            (0..n).map(|i| entry(&format!("f{i}.rs"), StatusKind::Modified, false)).collect()
        };
        let mut s = StatusListState::new(dirty_status(make_entries(prev_n)));
        // Navigate to some position
        for _ in 0..prev_n / 2 { s.select_next(); }
        s.set_status(dirty_status(make_entries(new_n)));
        if new_n > 0 {
            prop_assert!(s.selected_idx() < new_n);
        }
    }
}
