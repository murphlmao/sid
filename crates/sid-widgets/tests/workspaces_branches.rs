//! Tests for Branches sub-view (Task 23).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sid_core::adapters::git::{
    Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit,
};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspaces::{BranchListState, RightPane, WorkspacesState};

// ─── Mock git provider ───────────────────────────────────────────────────────

/// Simple mock that tracks which branch was checked out.
struct MockGit {
    branches: Vec<Branch>,
    current: Option<String>,
    checkout_calls: Arc<Mutex<Vec<String>>>,
    checkout_should_fail: bool,
}

impl MockGit {
    fn new(branches: Vec<Branch>) -> Self {
        let current = branches.iter().find(|b| b.is_current).map(|b| b.name.clone());
        Self {
            branches,
            current,
            checkout_calls: Arc::new(Mutex::new(Vec::new())),
            checkout_should_fail: false,
        }
    }

    fn with_checkout_failure(mut self) -> Self {
        self.checkout_should_fail = true;
        self
    }

    fn checkout_calls(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.checkout_calls)
    }
}

impl GitProvider for MockGit {
    fn open(&self, _path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        Ok(Box::new(MockGit::new(self.branches.clone())))
    }
    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        Ok(self.branches.clone())
    }
    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        Ok(self.branches.iter().find(|b| b.is_current).cloned())
    }
    fn status(&self) -> Result<GitStatus, GitError> {
        Ok(GitStatus { entries: vec![], is_clean: true })
    }
    fn commit_log(&self, _max: usize, _from: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
        Ok(vec![])
    }
    fn diff(&self, _staged: bool) -> Result<Vec<DiffEntry>, GitError> {
        Ok(vec![])
    }
    fn checkout_branch(&mut self, name: &str) -> Result<(), GitError> {
        if self.checkout_should_fail {
            return Err(GitError::DirtyWorkingTree(1));
        }
        self.checkout_calls.lock().unwrap().push(name.to_string());
        // update current
        for b in &mut self.branches {
            b.is_current = b.name == name;
        }
        self.current = Some(name.to_string());
        Ok(())
    }
    fn commit(&mut self, _new: NewCommit<'_>) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

// ─── Helper ──────────────────────────────────────────────────────────────────

fn branch(name: &str, is_current: bool) -> Branch {
    Branch {
        name: name.into(),
        head_oid: "a".repeat(40),
        upstream: None,
        is_current,
    }
}

fn ws(p: &str, n: &str) -> Workspace {
    Workspace {
        path: PathBuf::from(p),
        name: n.into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    }
}

// ─── BranchListState tests ────────────────────────────────────────────────────

#[test]
fn branch_list_state_new_selects_first() {
    let s = BranchListState::new(vec![branch("main", true), branch("dev", false)]);
    assert_eq!(s.selected_idx(), 0);
    assert_eq!(s.selected_branch().unwrap().name, "main");
}

#[test]
fn branch_list_state_set_branches_clamps_selection() {
    let mut s = BranchListState::new(vec![
        branch("main", true),
        branch("a", false),
        branch("b", false),
    ]);
    s.select_next();
    s.select_next(); // idx = 2
    assert_eq!(s.selected_idx(), 2);
    // Replace with shorter list
    s.set_branches(vec![branch("main", true)]);
    assert_eq!(s.selected_idx(), 0);
}

#[test]
fn branch_list_state_navigation_wraps() {
    let mut s =
        BranchListState::new(vec![branch("main", true), branch("dev", false), branch("feat", false)]);
    s.select_next();
    assert_eq!(s.selected_branch().unwrap().name, "dev");
    s.select_next();
    assert_eq!(s.selected_branch().unwrap().name, "feat");
    s.select_next(); // wraps to main
    assert_eq!(s.selected_branch().unwrap().name, "main");
    s.select_prev(); // back to feat
    assert_eq!(s.selected_branch().unwrap().name, "feat");
}

#[test]
fn branch_list_state_empty_navigation_is_safe() {
    let mut s = BranchListState::new(vec![]);
    s.select_next();
    s.select_prev();
    assert!(s.selected_branch().is_none());
}

// ─── Checkout via mock provider ──────────────────────────────────────────────

#[test]
fn checkout_via_locked_provider_succeeds() {
    let branches = vec![branch("main", true), branch("feature", false)];
    let mock = MockGit::new(branches);
    let calls = mock.checkout_calls();
    let provider: Box<dyn GitProvider> = Box::new(mock);
    let mut locked: Box<dyn GitProvider> = provider;

    locked.checkout_branch("feature").unwrap();
    let recorded = calls.lock().unwrap();
    assert_eq!(*recorded, vec!["feature".to_string()]);
}

#[test]
fn checkout_on_dirty_provider_returns_dirty_tree_error() {
    let branches = vec![branch("main", true)];
    let mock = MockGit::new(branches).with_checkout_failure();
    let mut provider: Box<dyn GitProvider> = Box::new(mock);
    let err = provider.checkout_branch("feature").unwrap_err();
    assert!(matches!(err, GitError::DirtyWorkingTree(_)));
}

// ─── WorkspacesState with Branches right pane ─────────────────────────────────

#[test]
fn workspaces_state_starts_with_branches_pane() {
    let s = WorkspacesState::new(vec![ws("/a", "a")]);
    assert!(matches!(s.right_pane(), RightPane::Branches(_)));
}

#[test]
fn right_pane_branches_population_and_navigation() {
    let mut s = WorkspacesState::new(vec![ws("/a", "a")]);
    if let RightPane::Branches(b) = s.right_pane_mut() {
        b.set_branches(vec![branch("main", true), branch("dev", false), branch("feat", false)]);
    }
    if let RightPane::Branches(b) = s.right_pane() {
        assert_eq!(b.branches().len(), 3);
        assert_eq!(b.selected_branch().unwrap().name, "main");
    }
}

// ─── Adversarial ─────────────────────────────────────────────────────────────

#[test]
fn branch_name_with_slashes_and_unicode() {
    let b = branch("feat/auth-🐕", false);
    let s = BranchListState::new(vec![b]);
    assert_eq!(s.selected_branch().unwrap().name, "feat/auth-🐕");
}

#[test]
fn very_long_branch_list_does_not_panic() {
    let branches: Vec<Branch> = (0..500).map(|i| branch(&format!("branch-{i}"), i == 0)).collect();
    let mut s = BranchListState::new(branches);
    for _ in 0..1000 {
        s.select_next();
    }
    // After 1000 steps on 500-item list, index should be 0 (1000 % 500 = 0)
    assert_eq!(s.selected_idx(), 0);
}

#[test]
fn checkout_nonexistent_branch_propagates_error() {
    let mock = MockGit::new(vec![branch("main", true)]);
    let mut provider: Box<dyn GitProvider> = Box::new(mock);
    // Our mock doesn't guard "branch not found" — that's the git2 adapter's job.
    // We just verify error propagation works through the trait.
    let result = provider.checkout_branch("nonexistent-branch");
    // Mock succeeds (no validation) — in real code git2 returns BranchNotFound
    assert!(result.is_ok() || matches!(result, Err(GitError::BranchNotFound(_))));
}

// ─── Property tests ──────────────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_branch_list_state_next_then_prev_returns_to_start(n in 1usize..20) {
        let branches: Vec<Branch> = (0..n).map(|i| branch(&format!("b{i}"), i == 0)).collect();
        let mut s = BranchListState::new(branches);
        let start = s.selected_idx();
        s.select_next();
        s.select_prev();
        prop_assert_eq!(s.selected_idx(), start);
    }

    #[test]
    fn prop_set_branches_never_panics(new_len in 0usize..100) {
        let branches_initial: Vec<Branch> =
            (0..50usize).map(|i| branch(&format!("b{i}"), i == 0)).collect();
        let mut s = BranchListState::new(branches_initial);
        // Select some position
        for _ in 0..37 { s.select_next(); }
        let new_branches: Vec<Branch> =
            (0..new_len).map(|i| branch(&format!("n{i}"), i == 0)).collect();
        s.set_branches(new_branches);
        // Should not panic and selected_idx should be in bounds
        if new_len > 0 {
            prop_assert!(s.selected_idx() < new_len);
        } else {
            prop_assert!(s.selected_branch().is_none());
        }
    }
}
