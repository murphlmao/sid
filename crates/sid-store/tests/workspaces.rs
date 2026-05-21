//! Integration tests for workspace registry — Tasks 15–18.

use std::path::PathBuf;

use sid_store::{now_epoch, Workspace, WorkspaceKind};

// ─── Task 15: Workspace domain type ──────────────────────────────────────────

#[test]
fn workspace_construction() {
    let w = Workspace {
        path: PathBuf::from("/home/u/vcs/foo"),
        name: "foo".into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: now_epoch(),
        parent: None,
    };
    assert_eq!(w.name, "foo");
    assert_eq!(w.kind, WorkspaceKind::Repo);
}

#[test]
fn workspace_with_parent() {
    let w = Workspace {
        path: PathBuf::from("/stack/child"),
        name: "child".into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: now_epoch(),
        parent: Some(PathBuf::from("/stack")),
    };
    assert_eq!(w.parent, Some(PathBuf::from("/stack")));
    assert_eq!(w.kind, WorkspaceKind::Repo);
}

#[test]
fn workspace_kind_variants_exist() {
    let _repo = WorkspaceKind::Repo;
    let _umbrella = WorkspaceKind::Umbrella;
}

#[test]
fn workspace_kind_equality() {
    assert_eq!(WorkspaceKind::Repo, WorkspaceKind::Repo);
    assert_ne!(WorkspaceKind::Repo, WorkspaceKind::Umbrella);
}

#[test]
fn workspace_postcard_roundtrip() {
    let w = Workspace {
        path: PathBuf::from("/tmp/foo"),
        name: "foo".into(),
        kind: WorkspaceKind::Umbrella,
        manifest_hash: 0,
        last_seen: now_epoch(),
        parent: None,
    };
    let bytes = postcard::to_allocvec(&w).unwrap();
    let back: Workspace = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(w, back);
}

#[test]
fn workspace_postcard_roundtrip_with_parent() {
    let w = Workspace {
        path: PathBuf::from("/home/user/vcs/stack/child"),
        name: "child".into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: now_epoch(),
        parent: Some(PathBuf::from("/home/user/vcs/stack")),
    };
    let bytes = postcard::to_allocvec(&w).unwrap();
    let back: Workspace = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(w, back);
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_workspace_postcard_roundtrip(name in "[a-zA-Z0-9 _-]{1,40}") {
        let w = Workspace {
            path: PathBuf::from(format!("/tmp/{name}")),
            name: name.clone(),
            kind: WorkspaceKind::Repo,
            manifest_hash: 0,
            last_seen: now_epoch(),
            parent: None,
        };
        let bytes = postcard::to_allocvec(&w).unwrap();
        let back: Workspace = postcard::from_bytes(&bytes).unwrap();
        prop_assert_eq!(w, back);
    }
}
