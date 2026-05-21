use std::path::PathBuf;

use sid_core::workspace_metadata::{WorkspaceAction, WorkspaceKind, WorkspaceMetadata};

#[test]
fn metadata_construction() {
    let m = WorkspaceMetadata {
        name: "eggsight-stack".into(),
        kind: WorkspaceKind::Umbrella,
        actions: vec![WorkspaceAction {
            label: "Clone all".into(),
            cmd: "./clone-repos.sh".into(),
            key: Some('c'),
        }],
        children: vec![PathBuf::from("../eggsight-core")],
    };
    assert_eq!(m.kind, WorkspaceKind::Umbrella);
    assert_eq!(m.actions[0].key, Some('c'));
}

#[test]
fn workspace_kind_variants() {
    let _ = WorkspaceKind::Repo;
    let _ = WorkspaceKind::Umbrella;
}

#[test]
fn from_basename_uses_directory_name() {
    let m = WorkspaceMetadata::from_basename(
        std::path::Path::new("/home/user/vcs/my-project"),
        WorkspaceKind::Repo,
    );
    assert_eq!(m.name, "my-project");
    assert_eq!(m.kind, WorkspaceKind::Repo);
    assert!(m.actions.is_empty());
    assert!(m.children.is_empty());
}

#[test]
fn from_basename_with_empty_path_uses_fallback() {
    let m = WorkspaceMetadata::from_basename(std::path::Path::new(""), WorkspaceKind::Repo);
    assert_eq!(m.name, "workspace");
}

#[test]
fn workspace_action_fields() {
    let a = WorkspaceAction {
        label: "Build".into(),
        cmd: "cargo build".into(),
        key: Some('b'),
    };
    assert_eq!(a.label, "Build");
    assert_eq!(a.cmd, "cargo build");
    assert_eq!(a.key, Some('b'));
}

#[test]
fn workspace_action_key_optional() {
    let a = WorkspaceAction {
        label: "Deploy".into(),
        cmd: "./deploy.sh".into(),
        key: None,
    };
    assert!(a.key.is_none());
}
