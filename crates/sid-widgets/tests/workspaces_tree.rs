//! Tests for WorkspacesState tree view (Task 22).

use std::path::PathBuf;

use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspaces::WorkspacesState;

fn ws(p: &str, n: &str, parent: Option<&str>, kind: WorkspaceKind) -> Workspace {
    Workspace {
        path: PathBuf::from(p),
        name: n.into(),
        kind,
        manifest_hash: 0,
        last_seen: 0,
        parent: parent.map(PathBuf::from),
    }
}

fn repo(p: &str, n: &str) -> Workspace {
    ws(p, n, None, WorkspaceKind::Repo)
}

fn child(p: &str, n: &str, parent: &str) -> Workspace {
    ws(p, n, Some(parent), WorkspaceKind::Repo)
}

fn umbrella(p: &str, n: &str) -> Workspace {
    ws(p, n, None, WorkspaceKind::Umbrella)
}

// ─── Happy-path tests ───────────────────────────────────────────────────────

#[test]
fn state_holds_workspaces_and_selects_first() {
    let s = WorkspacesState::new(vec![repo("/a", "alpha"), repo("/b", "beta")]);
    assert_eq!(s.selected_path().unwrap().to_string_lossy(), "/a");
}

#[test]
fn next_and_prev_cycle_selection() {
    let mut s = WorkspacesState::new(vec![repo("/a", "a"), repo("/b", "b")]);
    s.select_next();
    assert_eq!(s.selected_path().unwrap().to_string_lossy(), "/b");
    s.select_next();
    // wraps back to /a
    assert_eq!(s.selected_path().unwrap().to_string_lossy(), "/a");
    s.select_prev();
    // wraps back to /b
    assert_eq!(s.selected_path().unwrap().to_string_lossy(), "/b");
}

#[test]
fn empty_state_has_no_selection() {
    let s = WorkspacesState::new(Vec::new());
    assert!(s.selected_path().is_none());
    assert!(s.selected_workspace().is_none());
}

#[test]
fn umbrella_expand_toggles_children_visibility() {
    let ws_list = vec![
        umbrella("/stack", "stack"),
        child("/stack/a", "a", "/stack"),
        child("/stack/b", "b", "/stack"),
        repo("/other", "other"),
    ];
    let mut s = WorkspacesState::new(ws_list);
    // Default: umbrellas collapsed — only top-level visible
    assert_eq!(s.visible_count(), 2); // /stack and /other
    s.toggle_expand_selected(); // expand /stack (currently selected = first visible = /stack)
    assert_eq!(s.visible_count(), 4);
    s.toggle_expand_selected(); // collapse /stack again
    assert_eq!(s.visible_count(), 2);
}

#[test]
fn toggle_expand_on_repo_is_noop() {
    let mut s = WorkspacesState::new(vec![repo("/a", "a")]);
    let before = s.visible_count();
    s.toggle_expand_selected();
    assert_eq!(s.visible_count(), before);
}

#[test]
fn selected_workspace_returns_correct_workspace() {
    let mut s = WorkspacesState::new(vec![repo("/a", "alpha"), repo("/b", "beta")]);
    assert_eq!(s.selected_workspace().unwrap().name, "alpha");
    s.select_next();
    assert_eq!(s.selected_workspace().unwrap().name, "beta");
}

// ─── Adversarial tests ───────────────────────────────────────────────────────

#[test]
fn very_long_workspace_name_does_not_panic() {
    let long = "x".repeat(10_000);
    let s = WorkspacesState::new(vec![repo("/a", &long)]);
    assert_eq!(s.workspaces()[0].name.len(), 10_000);
}

#[test]
fn select_next_on_empty_is_noop() {
    let mut s = WorkspacesState::new(Vec::new());
    s.select_next(); // must not panic
    s.select_prev(); // must not panic
    assert!(s.selected_path().is_none());
}

#[test]
fn select_next_on_single_element_wraps_to_self() {
    let mut s = WorkspacesState::new(vec![repo("/a", "a")]);
    s.select_next();
    assert_eq!(s.selected_path().unwrap().to_string_lossy(), "/a");
}

#[test]
fn unicode_workspace_path_and_name() {
    let s = WorkspacesState::new(vec![repo("/vcs/工作区-🐕", "工作区-🐕")]);
    assert_eq!(s.selected_workspace().unwrap().name, "工作区-🐕");
}

#[test]
fn expand_collapse_does_not_lose_selection_on_visible_item() {
    let ws_list = vec![
        umbrella("/stack", "stack"),
        child("/stack/a", "a", "/stack"),
        repo("/other", "other"),
    ];
    let mut s = WorkspacesState::new(ws_list);
    // Expand: selection is at index 0 (/stack)
    s.toggle_expand_selected();
    assert_eq!(s.visible_count(), 3);
    // Move to /stack/a (index 1 after expand)
    s.select_next();
    let path = s.selected_path().unwrap().to_string_lossy().to_string();
    assert_eq!(path, "/stack/a");
}

#[test]
fn multiple_umbrellas_independent_expand_collapse() {
    let ws_list = vec![
        umbrella("/a", "a"),
        child("/a/r1", "r1", "/a"),
        umbrella("/b", "b"),
        child("/b/r2", "r2", "/b"),
    ];
    let mut s = WorkspacesState::new(ws_list);
    // Initially only umbrellas visible
    assert_eq!(s.visible_count(), 2);
    s.toggle_expand_selected(); // expand /a
    assert_eq!(s.visible_count(), 3); // /a, /a/r1, /b
    // Navigate to /b (index 2)
    s.select_next();
    s.select_next();
    s.toggle_expand_selected(); // expand /b
    assert_eq!(s.visible_count(), 4); // /a, /a/r1, /b, /b/r2
}

// ─── Property tests ──────────────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_next_then_prev_returns_to_original(n in 1usize..10) {
        let ws_list: Vec<Workspace> = (0..n).map(|i| repo(&format!("/{i}"), &format!("w{i}"))).collect();
        let mut s = WorkspacesState::new(ws_list);
        let original = s.selected_path().unwrap().to_path_buf();
        s.select_next();
        s.select_prev();
        prop_assert_eq!(s.selected_path().unwrap(), original.as_path());
    }

    #[test]
    fn prop_visible_count_is_within_bounds(n_repos in 0usize..20, n_umbrellas in 0usize..5) {
        let mut ws_list: Vec<Workspace> = (0..n_repos)
            .map(|i| repo(&format!("/{i}"), &format!("r{i}")))
            .collect();
        for i in 0..n_umbrellas {
            ws_list.push(umbrella(&format!("/u{i}"), &format!("u{i}")));
        }
        let s = WorkspacesState::new(ws_list.clone());
        // All repos + umbrellas are visible when no children; children added separately
        prop_assert_eq!(s.visible_count(), ws_list.len());
    }
}
