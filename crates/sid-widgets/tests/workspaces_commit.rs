//! Tests for the Commit drafter (Task 27).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use sid_core::{
    adapters::git::{Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit},
    workspace_metadata::WorkspaceKind,
};
use sid_store::Workspace;
use sid_widgets::workspaces::{
    CommitDraftPhase, CommitDraftState, EditorRunner, MockEditorRunner, RightPane, WorkspacesState,
};

// ─── Mock git provider with commit tracking ──────────────────────────────────

struct CommitMockGit {
    committed: Arc<Mutex<Vec<(String, bool)>>>, // (message, stage_all)
    commit_should_fail: bool,
}

impl CommitMockGit {
    fn new() -> Self {
        Self {
            committed: Arc::new(Mutex::new(Vec::new())),
            commit_should_fail: false,
        }
    }

    fn with_commit_failure(mut self) -> Self {
        self.commit_should_fail = true;
        self
    }

    fn committed_messages(&self) -> Arc<Mutex<Vec<(String, bool)>>> {
        Arc::clone(&self.committed)
    }
}

impl GitProvider for CommitMockGit {
    fn open(&self, _path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        Ok(Box::new(CommitMockGit::new()))
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
    fn diff(&self, _staged: bool) -> Result<Vec<DiffEntry>, GitError> {
        Ok(vec![])
    }
    fn checkout_branch(&mut self, _name: &str) -> Result<(), GitError> {
        Ok(())
    }
    fn commit(&mut self, new: NewCommit<'_>) -> Result<String, GitError> {
        if self.commit_should_fail {
            return Err(GitError::Other("commit failed".into()));
        }
        self.committed
            .lock()
            .unwrap()
            .push((new.message.to_string(), new.stage_all));
        Ok("a".repeat(40))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

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

// ─── CommitDraftState tests ───────────────────────────────────────────────────

#[test]
fn commit_draft_state_machine_idle_is_default() {
    let s = CommitDraftState::default();
    assert!(s.is_idle());
    assert_eq!(s.phase(), &CommitDraftPhase::Idle);
}

#[test]
fn commit_draft_start_editing_clears_previous_state() {
    let mut s = CommitDraftState::default();
    s.start_editing();
    s.finish_editing("old message".into());
    s.mark_done("abc123".into());
    // Reset and start again
    s.reset();
    s.start_editing();
    assert_eq!(s.phase(), &CommitDraftPhase::EditingMessage);
    assert!(s.draft_message().is_empty());
    assert!(s.committed_oid().is_none());
    assert!(s.error().is_none());
}

#[test]
fn commit_draft_full_success_flow() {
    let mut s = CommitDraftState::default();
    s.start_editing();
    assert_eq!(s.phase(), &CommitDraftPhase::EditingMessage);
    s.finish_editing("feat: add widget".into());
    assert_eq!(s.phase(), &CommitDraftPhase::Committing);
    assert_eq!(s.draft_message(), "feat: add widget");
    s.mark_done("a".repeat(40));
    assert_eq!(s.phase(), &CommitDraftPhase::Done);
    assert!(s.committed_oid().is_some());
}

#[test]
fn commit_draft_failure_flow() {
    let mut s = CommitDraftState::default();
    s.start_editing();
    s.finish_editing("fix: broken thing".into());
    s.mark_failed("working tree is dirty".into());
    assert_eq!(s.phase(), &CommitDraftPhase::Failed);
    assert_eq!(s.error(), Some("working tree is dirty"));
    assert!(s.committed_oid().is_none());
}

#[test]
fn commit_draft_reset_from_done() {
    let mut s = CommitDraftState::default();
    s.start_editing();
    s.finish_editing("msg".into());
    s.mark_done("abc".into());
    s.reset();
    assert!(s.is_idle());
    assert!(s.committed_oid().is_none());
    assert!(s.error().is_none());
}

#[test]
fn commit_draft_reset_from_failed() {
    let mut s = CommitDraftState::default();
    s.start_editing();
    s.finish_editing("msg".into());
    s.mark_failed("err".into());
    s.reset();
    assert!(s.is_idle());
    assert!(s.error().is_none());
}

// ─── MockEditorRunner tests ────────────────────────────────────────────────────

#[test]
fn mock_editor_runner_success() {
    let runner = MockEditorRunner::new("feat: add tests".into());
    assert!(!runner.will_fail());
    let msg = runner.run_editor().unwrap();
    assert_eq!(msg, "feat: add tests");
}

#[test]
fn mock_editor_runner_failure() {
    let runner = MockEditorRunner::failing("editor not found".into());
    assert!(runner.will_fail());
    let err = runner.run_editor().unwrap_err();
    assert_eq!(err, "editor not found");
}

#[test]
fn mock_editor_runner_empty_message() {
    let runner = MockEditorRunner::new(String::new());
    let msg = runner.run_editor().unwrap();
    assert!(msg.is_empty());
}

#[test]
fn editor_runner_is_dyn_compatible() {
    let runner: Box<dyn EditorRunner> = Box::new(MockEditorRunner::new("hi".into()));
    assert!(runner.run_editor().is_ok());
}

// ─── Integration: editor + commit state machine ──────────────────────────────

/// Simulates the full flow: editor runs -> state transitions -> commit
#[test]
fn simulate_editor_to_commit_flow_via_mock() {
    let message = "feat: add new feature\n\nThis adds a new feature.\n";
    let runner = MockEditorRunner::new(message.into());

    // 1. Enter editing phase
    let mut draft = CommitDraftState::default();
    draft.start_editing();
    assert_eq!(draft.phase(), &CommitDraftPhase::EditingMessage);

    // 2. Run editor (simulated)
    let editor_result = runner.run_editor().unwrap();
    assert_eq!(editor_result, message);

    // 3. Transition to committing
    draft.finish_editing(editor_result.clone());
    assert_eq!(draft.phase(), &CommitDraftPhase::Committing);
    assert_eq!(draft.draft_message(), message);

    // 4. Simulate git commit
    let mut git = CommitMockGit::new();
    let recorded = git.committed_messages();
    let nc = NewCommit {
        message: draft.draft_message(),
        author_name: None,
        author_email: None,
        stage_all: true,
    };
    let oid = git.commit(nc).unwrap();
    assert_eq!(oid.len(), 40);

    let msgs = recorded.lock().unwrap();
    assert_eq!(msgs[0].0, message);
    assert!(msgs[0].1); // stage_all = true

    // 5. Mark done
    draft.mark_done(oid);
    assert_eq!(draft.phase(), &CommitDraftPhase::Done);
}

/// Simulates the abort flow: editor fails -> state stays at editing
#[test]
fn simulate_editor_failure_cancels_commit() {
    let runner = MockEditorRunner::failing("user pressed Ctrl+C".into());

    let mut draft = CommitDraftState::default();
    draft.start_editing();

    let editor_result = runner.run_editor();
    assert!(editor_result.is_err());

    // State should still be in EditingMessage; the caller handles abort
    // by resetting or staying (per UX design)
    assert_eq!(draft.phase(), &CommitDraftPhase::EditingMessage);
    // Reset on abort
    draft.reset();
    assert!(draft.is_idle());
}

/// Simulates commit failure after editor succeeds
#[test]
fn simulate_commit_failure_after_editor_success() {
    let runner = MockEditorRunner::new("fix: something".into());
    let mut git = CommitMockGit::new().with_commit_failure();

    let mut draft = CommitDraftState::default();
    draft.start_editing();
    let msg = runner.run_editor().unwrap();
    draft.finish_editing(msg.clone());

    let nc = NewCommit {
        message: &msg,
        author_name: None,
        author_email: None,
        stage_all: false,
    };
    let result = git.commit(nc);
    assert!(result.is_err());

    draft.mark_failed(result.unwrap_err().to_string());
    assert_eq!(draft.phase(), &CommitDraftPhase::Failed);
    assert!(draft.error().is_some());
}

// ─── WorkspacesState Commit pane integration ─────────────────────────────────

#[test]
fn switching_to_commit_pane_works() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    s.cycle_pane_next(); // Status
    s.cycle_pane_next(); // Log
    s.cycle_pane_next(); // Diff
    s.cycle_pane_next(); // Commit
    assert!(matches!(s.right_pane(), RightPane::Commit(_)));
}

#[test]
fn commit_pane_state_transitions_via_right_pane() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    // Navigate to commit pane
    for _ in 0..4 {
        s.cycle_pane_next();
    }
    if let RightPane::Commit(draft) = s.right_pane_mut() {
        draft.start_editing();
        assert_eq!(draft.phase(), &CommitDraftPhase::EditingMessage);
        draft.finish_editing("docs: update README".into());
        draft.mark_done("b".repeat(40));
        assert_eq!(draft.phase(), &CommitDraftPhase::Done);
    }
}

// ─── Adversarial tests ───────────────────────────────────────────────────────

#[test]
fn commit_message_with_unicode_is_preserved() {
    let msg = "feat: add 工作区 support 🐕\n\nCo-authored-by: test@example.com";
    let runner = MockEditorRunner::new(msg.into());
    let result = runner.run_editor().unwrap();
    assert_eq!(result, msg);
}

#[test]
fn commit_message_very_long_does_not_panic() {
    let msg = "x".repeat(100_000);
    let runner = MockEditorRunner::new(msg.clone());
    let result = runner.run_editor().unwrap();
    assert_eq!(result.len(), 100_000);
}

#[test]
fn commit_message_only_whitespace_is_preserved() {
    // git would reject this, but state should store it faithfully
    let runner = MockEditorRunner::new("   \n\t\n".into());
    let mut draft = CommitDraftState::default();
    draft.start_editing();
    draft.finish_editing(runner.run_editor().unwrap());
    assert_eq!(draft.draft_message(), "   \n\t\n");
}

#[test]
fn many_reset_cycles_do_not_corrupt_state() {
    let mut s = CommitDraftState::default();
    for _ in 0..1000 {
        s.start_editing();
        s.finish_editing("msg".into());
        s.mark_done("oid".into());
        s.reset();
    }
    assert!(s.is_idle());
}

// ─── Property tests ──────────────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_mock_editor_returns_message_intact(msg in "[a-zA-Z0-9 _.-]{0,200}") {
        let runner = MockEditorRunner::new(msg.clone());
        let result = runner.run_editor().unwrap();
        prop_assert_eq!(result, msg);
    }

    #[test]
    fn prop_commit_draft_finish_editing_preserves_message(msg in "[a-zA-Z0-9 _.-]{0,200}") {
        let mut s = CommitDraftState::default();
        s.start_editing();
        s.finish_editing(msg.clone());
        prop_assert_eq!(s.draft_message(), msg.as_str());
        prop_assert_eq!(s.phase(), &CommitDraftPhase::Committing);
    }
}
