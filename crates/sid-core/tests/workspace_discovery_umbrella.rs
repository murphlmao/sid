//! Additional umbrella detection tests — Task 20.

use std::fs;
use std::path::Path;

use sid_core::workspace_discovery::scan_workspace_root;
use sid_core::workspace_metadata::WorkspaceKind;
use tempfile::tempdir;

fn init_git_at(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::create_dir_all(path.join(".git")).unwrap();
    fs::write(path.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
}

#[test]
fn nested_umbrella_inside_another_umbrella() {
    let root = tempdir().unwrap();
    let outer = root.path().join("outer");
    let inner = outer.join("inner");
    fs::create_dir_all(&inner).unwrap();
    fs::write(outer.join("CLAUDE.md"), "# outer").unwrap();
    fs::write(inner.join("CLAUDE.md"), "# inner").unwrap();
    init_git_at(&inner.join("repo-a"));
    init_git_at(&inner.join("repo-b"));
    let found = scan_workspace_root(root.path(), 6).unwrap();
    // Inner umbrella should be detected
    let inner_path = inner.to_string_lossy().to_string();
    assert!(
        found.iter().any(|w| w.path.to_string_lossy() == inner_path && w.kind == WorkspaceKind::Umbrella),
        "inner umbrella not detected\nfound: {found:?}",
    );
}

#[test]
fn workspace_deps_yaml_triggers_umbrella() {
    let root = tempdir().unwrap();
    let stack = root.path().join("stack");
    fs::create_dir(&stack).unwrap();
    fs::write(stack.join("workspace.deps.yaml"), "deps: []\n").unwrap();
    init_git_at(&stack.join("service-a"));
    init_git_at(&stack.join("service-b"));
    let found = scan_workspace_root(root.path(), 4).unwrap();
    let stack_path = stack.to_string_lossy().to_string();
    assert!(
        found.iter().any(|w| w.path.to_string_lossy() == stack_path && w.kind == WorkspaceKind::Umbrella),
        "workspace.deps.yaml did not trigger umbrella\nfound: {found:?}",
    );
}

#[test]
fn umbrella_metadata_children_match_sub_repo_paths() {
    let root = tempdir().unwrap();
    let umbrella = root.path().join("mono");
    fs::create_dir(&umbrella).unwrap();
    fs::write(umbrella.join("CLAUDE.md"), "# mono").unwrap();
    init_git_at(&umbrella.join("alpha"));
    init_git_at(&umbrella.join("beta"));
    let found = scan_workspace_root(root.path(), 4).unwrap();
    let umb = found.iter().find(|w| w.kind == WorkspaceKind::Umbrella && w.path.ends_with("mono")).unwrap();
    assert_eq!(umb.metadata.children.len(), 2);
    let child_names: Vec<_> = umb.metadata.children.iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .collect();
    assert!(child_names.contains(&"alpha".to_string()));
    assert!(child_names.contains(&"beta".to_string()));
}

#[test]
fn umbrella_with_single_subrepo_is_still_umbrella() {
    let root = tempdir().unwrap();
    let stack = root.path().join("stack");
    fs::create_dir(&stack).unwrap();
    fs::write(stack.join("CLAUDE.md"), "# stack").unwrap();
    init_git_at(&stack.join("only-child"));
    let found = scan_workspace_root(root.path(), 4).unwrap();
    let stack_path = stack.to_string_lossy().to_string();
    assert!(
        found.iter().any(|w| w.path.to_string_lossy() == stack_path && w.kind == WorkspaceKind::Umbrella),
    );
}

#[test]
fn umbrella_with_cargo_workspace_and_claude_md() {
    let root = tempdir().unwrap();
    let umbrella = root.path().join("workspace");
    fs::create_dir(&umbrella).unwrap();
    fs::write(umbrella.join("CLAUDE.md"), "# workspace").unwrap();
    // Also has a Cargo.toml workspace
    fs::write(
        umbrella.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/a\"]\n",
    ).unwrap();
    init_git_at(&umbrella.join("crates/a"));
    let found = scan_workspace_root(root.path(), 5).unwrap();
    let umb_path = umbrella.to_string_lossy().to_string();
    // Should be detected as umbrella
    assert!(
        found.iter().any(|w| w.path.to_string_lossy() == umb_path && w.kind == WorkspaceKind::Umbrella),
        "expected umbrella at workspace\nfound: {found:?}",
    );
}

#[test]
fn scan_does_not_emit_duplicate_umbrellas() {
    let root = tempdir().unwrap();
    let stack = root.path().join("stack");
    fs::create_dir(&stack).unwrap();
    // Both CLAUDE.md and .code-workspace present — should emit one umbrella, not two
    fs::write(stack.join("CLAUDE.md"), "# stack").unwrap();
    fs::write(stack.join("stack.code-workspace"), r#"{"folders":[]}"#).unwrap();
    init_git_at(&stack.join("repo-x"));
    let found = scan_workspace_root(root.path(), 4).unwrap();
    let umbrella_count = found.iter().filter(|w| {
        w.kind == WorkspaceKind::Umbrella && w.path.to_string_lossy().contains("stack")
    }).count();
    // Exactly one umbrella per directory, even if multiple signals are present
    assert_eq!(umbrella_count, 1, "expected exactly 1 umbrella, got {umbrella_count}");
}
