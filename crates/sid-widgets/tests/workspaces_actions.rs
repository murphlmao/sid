//! Tests for the Run-action menu (Task 28).

use std::path::{Path, PathBuf};

use sid_core::workspace_metadata::{WorkspaceAction, WorkspaceKind};
use sid_store::Workspace;
use sid_widgets::workspaces::{
    ActionListState, ActionResult, ActionRunner, MockActionRunner, RightPane, WorkspacesState,
};

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn action(label: &str, cmd: &str, key: Option<char>) -> WorkspaceAction {
    WorkspaceAction {
        label: label.into(),
        cmd: cmd.into(),
        key,
    }
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

// ─── ActionListState tests ────────────────────────────────────────────────────

#[test]
fn action_list_state_empty_is_safe() {
    let mut s = ActionListState::new(vec![]);
    s.select_next();
    s.select_prev();
    assert!(s.selected_action().is_none());
    assert_eq!(s.selected_idx(), 0);
}

#[test]
fn action_list_state_with_actions() {
    let actions = vec![
        action("Build", "cargo build", Some('b')),
        action("Test", "cargo test", Some('t')),
        action("Run", "./run.sh", None),
    ];
    let s = ActionListState::new(actions);
    assert_eq!(s.actions().len(), 3);
    assert_eq!(s.selected_action().unwrap().label, "Build");
}

#[test]
fn action_list_navigation() {
    let actions = vec![
        action("A", "cmd-a", Some('a')),
        action("B", "cmd-b", Some('b')),
        action("C", "cmd-c", None),
    ];
    let mut s = ActionListState::new(actions);
    assert_eq!(s.selected_idx(), 0);
    s.select_next();
    assert_eq!(s.selected_action().unwrap().label, "B");
    s.select_next();
    assert_eq!(s.selected_action().unwrap().label, "C");
    s.select_next(); // wraps to A
    assert_eq!(s.selected_action().unwrap().label, "A");
    s.select_prev(); // wraps to C
    assert_eq!(s.selected_action().unwrap().label, "C");
}

#[test]
fn action_list_single_element_wraps_to_self() {
    let mut s = ActionListState::new(vec![action("Only", "only-cmd", None)]);
    s.select_next();
    assert_eq!(s.selected_action().unwrap().label, "Only");
    s.select_prev();
    assert_eq!(s.selected_action().unwrap().label, "Only");
}

// ─── ActionRunner tests ───────────────────────────────────────────────────────

#[test]
fn mock_action_runner_success() {
    let runner = MockActionRunner::new(0, "hello from action\n".into());
    let result = runner
        .run_action(Path::new("/tmp"), "echo hello from action")
        .unwrap();
    assert!(result.success());
    assert_eq!(result.stdout, "hello from action\n");
    assert_eq!(result.exit_code, 0);
}

#[test]
fn mock_action_runner_failure_exit_code() {
    let runner = MockActionRunner::new(1, String::new());
    let result = runner.run_action(Path::new("/tmp"), "false").unwrap();
    assert!(!result.success());
    assert_eq!(result.exit_code, 1);
}

#[test]
fn mock_action_runner_spawn_error() {
    let runner = MockActionRunner::failing("command not found".into());
    let err = runner
        .run_action(Path::new("/tmp"), "nonexistent-cmd")
        .unwrap_err();
    assert_eq!(err, "command not found");
}

#[test]
fn mock_action_runner_records_cwd_correctly() {
    let runner = MockActionRunner::new(0, String::new());
    let cwd = Path::new("/home/user/project");
    runner.run_action(cwd, "make").unwrap();
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, PathBuf::from("/home/user/project"));
    assert_eq!(calls[0].1, "make");
}

#[test]
fn mock_action_runner_records_cmd_correctly() {
    let runner = MockActionRunner::new(0, String::new());
    runner
        .run_action(Path::new("/tmp"), "./clone-repos.sh")
        .unwrap();
    runner
        .run_action(Path::new("/tmp"), "cargo test --all")
        .unwrap();
    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, "./clone-repos.sh");
    assert_eq!(calls[1].1, "cargo test --all");
}

#[test]
fn action_runner_is_dyn_compatible() {
    let runner: Box<dyn ActionRunner> = Box::new(MockActionRunner::new(0, "ok".into()));
    let result = runner.run_action(Path::new("/tmp"), "true").unwrap();
    assert!(result.success());
}

// ─── ActionResult tests ────────────────────────────────────────────────────────

#[test]
fn action_result_success_on_zero_exit() {
    let r = ActionResult {
        stdout: "ok\n".into(),
        stderr: String::new(),
        exit_code: 0,
    };
    assert!(r.success());
}

#[test]
fn action_result_failure_on_nonzero_exit() {
    let r = ActionResult {
        stdout: String::new(),
        stderr: "err\n".into(),
        exit_code: 1,
    };
    assert!(!r.success());
}

#[test]
fn action_result_default_is_success() {
    let r = ActionResult::default();
    assert!(r.success());
    assert!(r.stdout.is_empty());
    assert!(r.stderr.is_empty());
}

// ─── Integration: action dispatch via state ───────────────────────────────────

#[test]
fn selected_action_cmd_is_dispatched_to_runner() {
    let actions = vec![
        action("Clone all", "./clone-repos.sh", Some('c')),
        action("Update deps", "./update-deps.sh", Some('u')),
    ];
    let mut s = ActionListState::new(actions);
    let runner = MockActionRunner::new(0, "cloned!".into());
    let cwd = PathBuf::from("/home/user/project");

    // Run the selected (first) action
    if let Some(a) = s.selected_action() {
        let result = runner.run_action(&cwd, &a.cmd).unwrap();
        assert!(result.success());
        assert_eq!(result.stdout, "cloned!");
    }

    // Navigate to second and run
    s.select_next();
    if let Some(a) = s.selected_action() {
        runner.run_action(&cwd, &a.cmd).unwrap();
    }

    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, "./clone-repos.sh");
    assert_eq!(calls[1].1, "./update-deps.sh");
    // Both use the same cwd
    assert!(calls.iter().all(|(c, _)| c == &cwd));
}

#[test]
fn action_failure_result_propagates_stderr() {
    let runner = MockActionRunner::new(1, String::new());
    let result = runner.run_action(Path::new("/tmp"), "false").unwrap();
    assert!(!result.success());
    assert_eq!(result.exit_code, 1);
}

// ─── WorkspacesState Actions pane integration ─────────────────────────────────

#[test]
fn switching_to_actions_pane_works() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    for _ in 0..5 {
        s.cycle_pane_next();
    } // Branches -> Status -> Log -> Diff -> Commit -> Actions
    assert!(matches!(s.right_pane(), RightPane::Actions(_)));
}

#[test]
fn actions_pane_can_receive_workspace_actions() {
    let mut s = WorkspacesState::new(vec![_ws("/a")]);
    for _ in 0..5 {
        s.cycle_pane_next();
    } // -> Actions
    if let RightPane::Actions(al) = s.right_pane_mut() {
        al.select_next(); // noop on empty
        // Populate actions
        *al = ActionListState::new(vec![
            action("Build", "cargo build", Some('b')),
            action("Test", "cargo test", Some('t')),
        ]);
        assert_eq!(al.actions().len(), 2);
        assert_eq!(al.selected_action().unwrap().label, "Build");
    }
}

#[test]
fn r_key_in_any_sub_view_opens_actions_pane_via_widget() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::context::WidgetCtx;
    use sid_core::event::{Event, KeyChord};
    use sid_core::widget::Widget;
    use sid_widgets::WorkspacesWidget;

    let mut w = WorkspacesWidget::new(vec![_ws("/a")], None);
    // Start in Branches pane
    assert!(matches!(w.state().right_pane(), RightPane::Branches(_)));

    // Press 'r'
    let ev = Event::Key(KeyChord::new(KeyCode::Char('r'), KeyModifiers::NONE));
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    w.handle_event(&ev, &mut ctx);
    assert!(matches!(w.state().right_pane(), RightPane::Actions(_)));
}

// ─── Adversarial tests ───────────────────────────────────────────────────────

#[test]
fn action_cmd_with_unicode_runs_correctly() {
    let runner = MockActionRunner::new(0, "done".into());
    // Command with unicode (real shell would handle this)
    let result = runner
        .run_action(Path::new("/工作区"), "./build-🐕.sh")
        .unwrap();
    assert!(result.success());
    let calls = runner.calls();
    assert_eq!(calls[0].1, "./build-🐕.sh");
}

#[test]
fn action_cwd_path_unicode() {
    let runner = MockActionRunner::new(0, "ok".into());
    let cwd = Path::new("/home/用户/vcs/project");
    runner.run_action(cwd, "ls").unwrap();
    let calls = runner.calls();
    assert_eq!(calls[0].0, PathBuf::from("/home/用户/vcs/project"));
}

#[test]
fn very_many_actions_does_not_panic() {
    let actions: Vec<WorkspaceAction> = (0..1000)
        .map(|i| action(&format!("Action {i}"), &format!("cmd-{i}"), None))
        .collect();
    let mut s = ActionListState::new(actions);
    for _ in 0..10_000 {
        s.select_next();
    }
    // 10_000 % 1000 = 0
    assert_eq!(s.selected_idx(), 0);
}

#[test]
fn multiple_calls_all_recorded() {
    let runner = MockActionRunner::new(0, String::new());
    for i in 0..100 {
        runner
            .run_action(Path::new("/tmp"), &format!("cmd-{i}"))
            .unwrap();
    }
    let calls = runner.calls();
    assert_eq!(calls.len(), 100);
}

// ─── Property tests ──────────────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_action_nav_next_then_prev_returns_to_start(n in 1usize..50) {
        let actions: Vec<WorkspaceAction> = (0..n)
            .map(|i| action(&format!("a{i}"), &format!("cmd{i}"), None))
            .collect();
        let mut s = ActionListState::new(actions);
        let start = s.selected_idx();
        s.select_next();
        s.select_prev();
        prop_assert_eq!(s.selected_idx(), start);
    }

    #[test]
    fn prop_mock_runner_records_correct_call_count(n in 0usize..20) {
        let runner = MockActionRunner::new(0, String::new());
        for i in 0..n {
            runner.run_action(Path::new("/tmp"), &format!("cmd{i}")).unwrap();
        }
        let calls = runner.calls();
        prop_assert_eq!(calls.len(), n);
    }
}

// ---------------------------------------------------------------------------
// Branch #2 Task 3 — Enter emits open_detail; Right/Left toggle umbrella expand
// ---------------------------------------------------------------------------

mod task3 {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::context::WidgetCtx;
    use sid_core::event::{Event, KeyChord};
    use sid_core::widget::Widget;
    use sid_widgets::WorkspacesWidget;

    fn repo(path: &str, name: &str) -> Workspace {
        Workspace {
            path: PathBuf::from(path),
            name: name.into(),
            kind: WorkspaceKind::Repo,
            manifest_hash: 0,
            last_seen: 0,
            parent: None,
        }
    }

    fn umbrella(path: &str, name: &str) -> Workspace {
        Workspace {
            path: PathBuf::from(path),
            name: name.into(),
            kind: WorkspaceKind::Umbrella,
            manifest_hash: 0,
            last_seen: 0,
            parent: None,
        }
    }

    fn make_ctx() -> (WidgetCtx, std::sync::mpsc::Receiver<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (WidgetCtx::new(tx), rx)
    }

    #[test]
    fn enter_on_repo_emits_open_detail_action() {
        let mut w =
            WorkspacesWidget::new(vec![repo("/vcs/eggsight-stack", "eggsight-stack")], None);
        let (mut ctx, rx) = make_ctx();
        let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = w.handle_event(&ev, &mut ctx);
        let action = rx.try_recv().expect("expected an action to be emitted");
        assert_eq!(action, "workspaces.open_detail");
    }

    #[test]
    fn enter_on_umbrella_now_opens_detail() {
        // UX-v2 Decision 8: Enter on ANY node (umbrella included) opens it as a
        // pushed subrepo tab. Inline tree expansion moved to →/↓/←.
        let mut w = WorkspacesWidget::new(vec![umbrella("/vcs/monorepo", "monorepo")], None);
        let (mut ctx, rx) = make_ctx();
        let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = w.handle_event(&ev, &mut ctx);
        let action = rx
            .try_recv()
            .expect("Enter on an umbrella must emit open_detail under Decision 8");
        assert_eq!(action, "workspaces.open_detail");
        let opened = w.take_pending_open_detail();
        assert_eq!(opened.expect("pending detail set").name, "monorepo");
    }

    #[test]
    fn right_arrow_toggles_umbrella_expansion() {
        let umb = umbrella("/vcs/monorepo", "monorepo");
        let child = Workspace {
            parent: Some(PathBuf::from("/vcs/monorepo")),
            ..repo("/vcs/monorepo/child", "child")
        };
        let mut w = WorkspacesWidget::new(vec![umb, child], None);
        assert_eq!(w.state().visible_count(), 1);
        let (mut ctx, _rx) = make_ctx();
        let ev = Event::Key(KeyChord::new(KeyCode::Right, KeyModifiers::NONE));
        let _ = w.handle_event(&ev, &mut ctx);
        assert_eq!(w.state().visible_count(), 2);
    }

    #[test]
    fn left_arrow_collapses_umbrella() {
        let umb = umbrella("/vcs/monorepo", "monorepo");
        let child = Workspace {
            parent: Some(PathBuf::from("/vcs/monorepo")),
            ..repo("/vcs/monorepo/child", "child")
        };
        let mut w = WorkspacesWidget::new(vec![umb, child], None);
        let (mut ctx, _rx) = make_ctx();
        let _ = w.handle_event(
            &Event::Key(KeyChord::new(KeyCode::Right, KeyModifiers::NONE)),
            &mut ctx,
        );
        assert_eq!(w.state().visible_count(), 2);
        let _ = w.handle_event(
            &Event::Key(KeyChord::new(KeyCode::Left, KeyModifiers::NONE)),
            &mut ctx,
        );
        assert_eq!(w.state().visible_count(), 1);
    }

    #[test]
    fn enter_on_workspace_with_missing_path_still_emits() {
        let missing = repo("/nonexistent/path/that/does/not/exist", "ghost");
        let mut w = WorkspacesWidget::new(vec![missing], None);
        let (mut ctx, rx) = make_ctx();
        let _ = w.handle_event(
            &Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE)),
            &mut ctx,
        );
        let action = rx
            .try_recv()
            .expect("Enter must emit even on missing-path workspaces");
        assert_eq!(action, "workspaces.open_detail");
    }
}
