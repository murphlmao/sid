//! Tests for workspace_discovery — Tasks 19, 20, 21.

use std::{fs, path::Path};

use sid_core::{
    workspace_discovery::{WorkspaceUpserter, merge_discoveries_into, scan_workspace_root},
    workspace_metadata::WorkspaceKind,
};
use tempfile::tempdir;

/// Create a minimal fake git repo at `path` (just .git/HEAD is enough to trigger detection).
fn init_git_at(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::create_dir_all(path.join(".git")).unwrap();
    fs::write(path.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
}

// ── Task 19: scan_workspace_root ──────────────────────────────────────────────

#[test]
fn scan_finds_a_single_git_repo() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("repo-a"));
    let found = scan_workspace_root(root.path(), 2).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, WorkspaceKind::Repo);
    assert!(found[0].path.ends_with("repo-a"));
}

#[test]
fn scan_finds_two_repos_at_same_level() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("a"));
    init_git_at(&root.path().join("b"));
    let found = scan_workspace_root(root.path(), 2).unwrap();
    // 2 repos
    let repo_count = found
        .iter()
        .filter(|w| w.kind == WorkspaceKind::Repo)
        .count();
    assert_eq!(repo_count, 2);
}

#[test]
fn scan_respects_depth_limit() {
    let root = tempdir().unwrap();
    // 5 levels deep — won't be found at depth 2
    init_git_at(&root.path().join("a/b/c/d/e"));
    let found = scan_workspace_root(root.path(), 2).unwrap();
    assert!(found.is_empty());
}

#[test]
fn scan_skips_target_node_modules_dot_dirs() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("real"));
    init_git_at(&root.path().join("target/junk"));
    init_git_at(&root.path().join("node_modules/lib"));
    init_git_at(&root.path().join(".cache/x"));
    let found = scan_workspace_root(root.path(), 4).unwrap();
    let repo_paths: Vec<_> = found
        .iter()
        .filter(|w| w.kind == WorkspaceKind::Repo)
        .map(|w| w.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(
        repo_paths.contains(&"real".to_string()),
        "expected 'real' in {repo_paths:?}"
    );
    assert!(
        !repo_paths.contains(&"junk".to_string()),
        "should skip 'target' subdir"
    );
    assert!(
        !repo_paths.contains(&"lib".to_string()),
        "should skip 'node_modules' subdir"
    );
}

#[test]
fn scan_handles_symlinks_safely() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("real"));
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path().join("real"), root.path().join("link")).unwrap();
    // No assertion on count — just verifying no panic or infinite loop
    let _ = scan_workspace_root(root.path(), 2).unwrap();
}

#[test]
fn scan_empty_root_returns_empty() {
    let root = tempdir().unwrap();
    let found = scan_workspace_root(root.path(), 4).unwrap();
    assert!(found.is_empty());
}

#[test]
fn scan_result_has_valid_metadata_name() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("my-project"));
    let found = scan_workspace_root(root.path(), 2).unwrap();
    let repo = found
        .iter()
        .find(|w| w.path.ends_with("my-project"))
        .unwrap();
    assert!(!repo.metadata.name.is_empty());
}

#[test]
fn scan_picks_up_metadata_sid_when_present() {
    let root = tempdir().unwrap();
    let proj = root.path().join("annotated-project");
    init_git_at(&proj);
    fs::create_dir_all(proj.join(".sid")).unwrap();
    fs::write(
        proj.join(".sid/_metadata.sid"),
        r#"{"name":"Custom Name","kind":"Repo"}"#,
    )
    .unwrap();
    let found = scan_workspace_root(root.path(), 2).unwrap();
    let repo = found
        .iter()
        .find(|w| w.path.ends_with("annotated-project"))
        .unwrap();
    assert_eq!(repo.metadata.name, "Custom Name");
}

// ── Task 20: umbrella detection ───────────────────────────────────────────────

#[test]
fn umbrella_dir_with_subrepos_is_detected_as_umbrella() {
    let root = tempdir().unwrap();
    let umbrella = root.path().join("stack");
    fs::create_dir(&umbrella).unwrap();
    fs::write(umbrella.join("CLAUDE.md"), "# stack\n").unwrap();
    init_git_at(&umbrella.join("repo-a"));
    init_git_at(&umbrella.join("repo-b"));
    let found = scan_workspace_root(root.path(), 4).unwrap();
    let umbrella_path = umbrella.to_string_lossy().to_string();
    assert!(
        found.iter().any(|w| w.path.to_string_lossy() == umbrella_path && w.kind == WorkspaceKind::Umbrella),
        "expected umbrella at {umbrella_path}\nfound: {found:?}",
    );
    // Sub-repos still listed as Repo
    assert!(
        found
            .iter()
            .any(|w| w.path.ends_with("repo-a") && w.kind == WorkspaceKind::Repo),
        "expected repo-a as Repo"
    );
}

#[test]
fn directory_with_claude_md_but_no_subrepos_is_not_umbrella() {
    let root = tempdir().unwrap();
    // Just a CLAUDE.md, no git repos under it
    fs::write(root.path().join("CLAUDE.md"), "# docs\n").unwrap();
    let found = scan_workspace_root(root.path(), 2).unwrap();
    // Root itself won't be emitted as umbrella since there are no sub-repos
    assert!(found.iter().all(|w| w.kind != WorkspaceKind::Umbrella));
}

#[test]
fn code_workspace_file_triggers_umbrella_detection() {
    let root = tempdir().unwrap();
    let stack = root.path().join("stack");
    fs::create_dir(&stack).unwrap();
    fs::write(stack.join("stack.code-workspace"), r#"{"folders":[]}"#).unwrap();
    init_git_at(&stack.join("frontend"));
    init_git_at(&stack.join("backend"));
    let found = scan_workspace_root(root.path(), 4).unwrap();
    let stack_path = stack.to_string_lossy().to_string();
    assert!(
        found
            .iter()
            .any(|w| w.path.to_string_lossy() == stack_path && w.kind == WorkspaceKind::Umbrella),
        "expected .code-workspace to trigger umbrella detection\nfound: {found:?}"
    );
}

#[test]
fn umbrella_children_are_all_listed_as_repos() {
    let root = tempdir().unwrap();
    let umbrella = root.path().join("stack");
    fs::create_dir(&umbrella).unwrap();
    fs::write(umbrella.join("CLAUDE.md"), "# stack\n").unwrap();
    for name in &["repo-a", "repo-b", "repo-c"] {
        init_git_at(&umbrella.join(name));
    }
    let found = scan_workspace_root(root.path(), 4).unwrap();
    let repo_count = found
        .iter()
        .filter(|w| w.kind == WorkspaceKind::Repo && w.path.starts_with(&umbrella))
        .count();
    assert_eq!(repo_count, 3, "all 3 sub-repos should be listed as Repo");
}

// ── Task 21: WorkspaceUpserter + merge_discoveries_into ──────────────────────

#[test]
fn merge_into_store_persists_each_discovery() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("a"));
    init_git_at(&root.path().join("b"));
    let discoveries = scan_workspace_root(root.path(), 2).unwrap();

    use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

    struct MemStore {
        ws: Mutex<BTreeMap<PathBuf, ()>>,
    }
    impl WorkspaceUpserter for MemStore {
        fn upsert(&self, path: &Path, _kind: WorkspaceKind, _name: &str) -> Result<(), String> {
            self.ws.lock().unwrap().insert(path.to_path_buf(), ());
            Ok(())
        }
    }
    let store = MemStore {
        ws: Mutex::new(BTreeMap::new()),
    };
    let n = merge_discoveries_into(&store, &discoveries).unwrap();
    assert_eq!(n, discoveries.len());
    assert_eq!(store.ws.lock().unwrap().len(), discoveries.len());
}

#[test]
fn merge_with_empty_discoveries_returns_zero() {
    struct NoopStore;
    impl WorkspaceUpserter for NoopStore {
        fn upsert(&self, _: &Path, _: WorkspaceKind, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
    let n = merge_discoveries_into(&NoopStore, &[]).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn merge_stops_on_first_upsert_error() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("a"));
    init_git_at(&root.path().join("b"));
    let discoveries = scan_workspace_root(root.path(), 2).unwrap();

    struct FailingStore;
    impl WorkspaceUpserter for FailingStore {
        fn upsert(&self, _: &Path, _: WorkspaceKind, _: &str) -> Result<(), String> {
            Err("storage full".into())
        }
    }
    let result = merge_discoveries_into(&FailingStore, &discoveries);
    assert!(result.is_err());
}

#[test]
fn workspace_upserter_receives_correct_metadata() {
    let root = tempdir().unwrap();
    let proj = root.path().join("my-repo");
    init_git_at(&proj);
    let discoveries = scan_workspace_root(root.path(), 2).unwrap();

    use std::sync::Mutex;
    struct CapturingStore {
        captured: Mutex<Vec<(std::path::PathBuf, WorkspaceKind, String)>>,
    }
    impl WorkspaceUpserter for CapturingStore {
        fn upsert(&self, path: &Path, kind: WorkspaceKind, name: &str) -> Result<(), String> {
            self.captured
                .lock()
                .unwrap()
                .push((path.to_path_buf(), kind, name.to_string()));
            Ok(())
        }
    }
    let store = CapturingStore {
        captured: Mutex::new(vec![]),
    };
    merge_discoveries_into(&store, &discoveries).unwrap();
    let captured = store.captured.lock().unwrap();
    let repo = captured
        .iter()
        .find(|(p, _, _)| p.ends_with("my-repo"))
        .unwrap();
    assert_eq!(repo.2, "my-repo");
}

#[test]
fn discovered_workspace_eq_impl() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("x"));
    let found1 = scan_workspace_root(root.path(), 2).unwrap();
    let found2 = scan_workspace_root(root.path(), 2).unwrap();
    assert_eq!(found1[0], found2[0]);
}

#[test]
fn scan_with_vendor_dir_skipped() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("vendor/dep"));
    init_git_at(&root.path().join("real"));
    let found = scan_workspace_root(root.path(), 4).unwrap();
    assert!(
        found
            .iter()
            .all(|w| !w.path.to_string_lossy().contains("vendor"))
    );
    assert!(found.iter().any(|w| w.path.ends_with("real")));
}
