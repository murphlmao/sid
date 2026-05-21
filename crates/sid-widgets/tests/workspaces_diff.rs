//! Tests for the Diff sub-view (Task 26).

use std::path::{Path, PathBuf};

use sid_core::adapters::git::{
    Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit,
};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspaces::{DiffViewState, RightPane, WorkspacesState};

// ─── Mock ─────────────────────────────────────────────────────────────────────

struct DiffMockGit {
    staged_diff: Vec<DiffEntry>,
    unstaged_diff: Vec<DiffEntry>,
    diff_should_err: bool,
}

impl DiffMockGit {
    fn new(staged: Vec<DiffEntry>, unstaged: Vec<DiffEntry>) -> Self {
        Self {
            staged_diff: staged,
            unstaged_diff: unstaged,
            diff_should_err: false,
        }
    }

    fn with_error(mut self) -> Self {
        self.diff_should_err = true;
        self
    }
}

impl GitProvider for DiffMockGit {
    fn open(&self, _path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        Ok(Box::new(DiffMockGit::new(
            self.staged_diff.clone(),
            self.unstaged_diff.clone(),
        )))
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
    fn commit_log(&self, _max: usize, _from: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
        Ok(vec![])
    }
    fn diff(&self, staged: bool) -> Result<Vec<DiffEntry>, GitError> {
        if self.diff_should_err {
            return Err(GitError::Other("diff failed".into()));
        }
        if staged {
            Ok(self.staged_diff.clone())
        } else {
            Ok(self.unstaged_diff.clone())
        }
    }
    fn checkout_branch(&mut self, _name: &str) -> Result<(), GitError> {
        Ok(())
    }
    fn commit(&mut self, _new: NewCommit<'_>) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn diff_entry(path: &str, patch: &str, added: usize, removed: usize) -> DiffEntry {
    DiffEntry {
        path: path.into(),
        old_path: None,
        patch: patch.into(),
        added,
        removed,
    }
}

fn multi_line_patch(n_lines: usize) -> String {
    (0..n_lines)
        .map(|i| format!("+line {i}\n"))
        .collect::<String>()
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

// ─── DiffViewState tests ──────────────────────────────────────────────────────

#[test]
fn diff_view_state_default_is_unstaged() {
    let s = DiffViewState::default();
    assert!(!s.staged());
    assert!(s.entries().is_empty());
}

#[test]
fn diff_view_state_toggle_staged() {
    let mut s = DiffViewState::new(vec![], false);
    assert!(!s.staged());
    s.toggle_staged();
    assert!(s.staged());
    s.toggle_staged();
    assert!(!s.staged());
}

#[test]
fn diff_view_state_toggle_resets_scroll_and_file() {
    let entries = vec![
        diff_entry("a.rs", "+x\n", 1, 0),
        diff_entry("b.rs", "+y\n", 1, 0),
    ];
    let mut s = DiffViewState::new(entries, false);
    s.next_file(); // selected_file = 1
    s.scroll_down();
    s.scroll_down(); // scroll_offset = 2
    assert_eq!(s.selected_file(), 1);
    assert_eq!(s.scroll_offset(), 2);
    s.toggle_staged();
    assert_eq!(s.selected_file(), 0);
    assert_eq!(s.scroll_offset(), 0);
}

#[test]
fn diff_view_file_navigation_wraps() {
    let entries = vec![
        diff_entry("a.rs", "+x\n", 1, 0),
        diff_entry("b.rs", "+y\n", 1, 0),
        diff_entry("c.rs", "+z\n", 1, 0),
    ];
    let mut s = DiffViewState::new(entries, false);
    assert_eq!(s.selected_file(), 0);
    s.next_file();
    assert_eq!(s.selected_file(), 1);
    s.next_file();
    assert_eq!(s.selected_file(), 2);
    s.next_file(); // wraps to 0
    assert_eq!(s.selected_file(), 0);
    s.prev_file(); // wraps to 2
    assert_eq!(s.selected_file(), 2);
}

#[test]
fn diff_view_file_navigation_empty_is_safe() {
    let mut s = DiffViewState::new(vec![], false);
    s.next_file();
    s.prev_file();
    assert_eq!(s.selected_file(), 0);
}

#[test]
fn diff_view_scroll() {
    let entries = vec![diff_entry("a.rs", &multi_line_patch(50), 50, 0)];
    let mut s = DiffViewState::new(entries, false);
    for _ in 0..10 {
        s.scroll_down();
    }
    assert_eq!(s.scroll_offset(), 10);
    for _ in 0..5 {
        s.scroll_up();
    }
    assert_eq!(s.scroll_offset(), 5);
}

#[test]
fn diff_view_scroll_up_does_not_underflow() {
    let mut s = DiffViewState::default();
    s.scroll_up(); // at 0, should stay 0
    s.scroll_up();
    assert_eq!(s.scroll_offset(), 0);
}

#[test]
fn diff_view_visible_patch_lines_respects_200_line_cap() {
    let patch = multi_line_patch(500); // 500 lines
    let entries = vec![diff_entry("a.rs", &patch, 500, 0)];
    let s = DiffViewState::new(entries, false);
    let visible = s.visible_patch_lines();
    assert!(visible.len() <= 200, "got {} lines", visible.len());
}

#[test]
fn diff_view_visible_patch_lines_respects_scroll_offset() {
    let patch = multi_line_patch(20); // exactly 20 lines
    let entries = vec![diff_entry("a.rs", &patch, 20, 0)];
    let mut s = DiffViewState::new(entries, false);
    s.scroll_down();
    s.scroll_down(); // offset = 2
    let visible = s.visible_patch_lines();
    // First visible line should be line 2
    assert!(visible[0].contains("line 2"));
}

#[test]
fn diff_view_visible_patch_empty_no_entries() {
    let s = DiffViewState::default();
    assert!(s.visible_patch_lines().is_empty());
}

#[test]
fn diff_view_set_entries_resets_scroll() {
    let entries = vec![diff_entry("a.rs", "+x\n", 1, 0)];
    let mut s = DiffViewState::new(entries, false);
    s.scroll_down();
    s.scroll_down();
    assert_eq!(s.scroll_offset(), 2);
    s.set_entries(vec![diff_entry("b.rs", "+y\n", 1, 0)]);
    assert_eq!(s.scroll_offset(), 0);
}

// ─── Provider tests ──────────────────────────────────────────────────────────

#[test]
fn diff_provider_returns_staged_diff() {
    let staged = vec![diff_entry("staged.rs", "+staged\n", 1, 0)];
    let unstaged = vec![diff_entry("unstaged.rs", "+unstaged\n", 1, 0)];
    let provider = DiffMockGit::new(staged, unstaged);
    let result = provider.diff(true).unwrap();
    assert_eq!(result[0].path, "staged.rs");
}

#[test]
fn diff_provider_returns_unstaged_diff() {
    let staged = vec![diff_entry("staged.rs", "+staged\n", 1, 0)];
    let unstaged = vec![diff_entry("unstaged.rs", "+unstaged\n", 1, 0)];
    let provider = DiffMockGit::new(staged, unstaged);
    let result = provider.diff(false).unwrap();
    assert_eq!(result[0].path, "unstaged.rs");
}

#[test]
fn diff_provider_error_propagated() {
    let provider = DiffMockGit::new(vec![], vec![]).with_error();
    let err = provider.diff(false).unwrap_err();
    assert!(matches!(err, GitError::Other(_)));
}

// ─── WorkspacesState Diff pane integration ────────────────────────────────────

#[test]
fn switching_to_diff_pane_works() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    s.cycle_pane_next(); // Status
    s.cycle_pane_next(); // Log
    s.cycle_pane_next(); // Diff
    assert!(matches!(s.right_pane(), RightPane::Diff(_)));
}

#[test]
fn diff_pane_can_receive_entries() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    s.cycle_pane_next();
    s.cycle_pane_next();
    s.cycle_pane_next(); // Diff
    if let RightPane::Diff(d) = s.right_pane_mut() {
        d.set_entries(vec![
            diff_entry("a.rs", "+x\n", 1, 0),
            diff_entry("b.rs", "+y\n", 1, 0),
        ]);
        assert_eq!(d.entries().len(), 2);
    }
}

// ─── Adversarial tests ───────────────────────────────────────────────────────

#[test]
fn binary_like_patch_content_handled() {
    // A patch that looks like binary data (all null bytes replaced with escaped equivalents)
    let patch = "\x00diff --git a/bin\n+\x01\x02\x03\n";
    let entries = vec![diff_entry("binary", patch, 0, 0)];
    let s = DiffViewState::new(entries, false);
    let _ = s.visible_patch_lines(); // must not panic
}

#[test]
fn diff_entry_with_rename_has_old_path() {
    let entry = DiffEntry {
        path: "new_name.rs".into(),
        old_path: Some("old_name.rs".into()),
        patch: "@@ -1 +1 @@\n".into(),
        added: 0,
        removed: 0,
    };
    let s = DiffViewState::new(vec![entry], false);
    assert_eq!(s.entries()[0].old_path, Some("old_name.rs".into()));
}

#[test]
fn unicode_file_paths_in_diff() {
    let entries = vec![
        diff_entry("src/工作区/main.rs", "+x\n", 1, 0),
        diff_entry("hello-🐕.txt", "+y\n", 1, 0),
    ];
    let s = DiffViewState::new(entries, false);
    assert_eq!(s.entries()[0].path, "src/工作区/main.rs");
    assert_eq!(s.entries()[1].path, "hello-🐕.txt");
}

#[test]
fn very_many_diff_files_does_not_panic() {
    let entries: Vec<DiffEntry> = (0..1000)
        .map(|i| diff_entry(&format!("file-{i}.rs"), "+line\n", 1, 0))
        .collect();
    let mut s = DiffViewState::new(entries, false);
    for _ in 0..10_000 {
        s.next_file();
    }
    // 10_000 % 1000 = 0
    assert_eq!(s.selected_file(), 0);
}

// ─── Property tests ──────────────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_next_file_then_prev_returns_to_start(n in 1usize..50) {
        let entries: Vec<DiffEntry> = (0..n)
            .map(|i| diff_entry(&format!("f{i}.rs"), "+x\n", 1, 0))
            .collect();
        let mut s = DiffViewState::new(entries, false);
        let start = s.selected_file();
        s.next_file();
        s.prev_file();
        prop_assert_eq!(s.selected_file(), start);
    }

    #[test]
    fn prop_scroll_never_underflows(n in 0usize..100) {
        let mut s = DiffViewState::default();
        for _ in 0..n { s.scroll_up(); }
        prop_assert_eq!(s.scroll_offset(), 0);
    }

    #[test]
    fn prop_visible_patch_lines_never_exceeds_200(n_lines in 0usize..1000) {
        let patch = multi_line_patch(n_lines);
        let entries = vec![diff_entry("a.rs", &patch, n_lines, 0)];
        let s = DiffViewState::new(entries, false);
        let visible = s.visible_patch_lines();
        prop_assert!(visible.len() <= 200);
    }
}
