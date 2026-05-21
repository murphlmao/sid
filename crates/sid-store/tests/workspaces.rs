//! Integration tests for workspace registry — Tasks 15–18.

use std::path::PathBuf;

use sid_store::{OpenStore, RedbStore, Store, Workspace, WorkspaceKind, now_epoch};
use tempfile::tempdir;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn ws(path: &str, name: &str, kind: WorkspaceKind, parent: Option<&str>) -> Workspace {
    Workspace {
        path: PathBuf::from(path),
        name: name.into(),
        kind,
        manifest_hash: 0,
        last_seen: now_epoch(),
        parent: parent.map(PathBuf::from),
    }
}

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
    let w = ws("/stack/child", "child", WorkspaceKind::Repo, Some("/stack"));
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
    let w = ws("/tmp/foo", "foo", WorkspaceKind::Umbrella, None);
    let bytes = postcard::to_allocvec(&w).unwrap();
    let back: Workspace = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(w, back);
}

#[test]
fn workspace_postcard_roundtrip_with_parent() {
    let w = ws(
        "/home/user/vcs/stack/child",
        "child",
        WorkspaceKind::Repo,
        Some("/home/user/vcs/stack"),
    );
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

// ─── Task 17 + 18: Store trait + RedbStore implementation ────────────────────

#[test]
fn upsert_then_list_returns_workspace() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let w = ws("/a", "alpha", WorkspaceKind::Repo, None);
    store.upsert_workspace(&w).unwrap();
    let all = store.list_workspaces().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "alpha");
}

#[test]
fn get_workspace_returns_existing_and_none_for_missing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let w = ws("/a", "alpha", WorkspaceKind::Repo, None);
    store.upsert_workspace(&w).unwrap();
    assert!(store.get_workspace(&PathBuf::from("/a")).unwrap().is_some());
    assert!(
        store
            .get_workspace(&PathBuf::from("/missing"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn remove_workspace_drops_it() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let w = ws("/a", "alpha", WorkspaceKind::Repo, None);
    store.upsert_workspace(&w).unwrap();
    store.remove_workspace(&PathBuf::from("/a")).unwrap();
    assert!(store.list_workspaces().unwrap().is_empty());
}

#[test]
fn upsert_replaces_existing_record() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .upsert_workspace(&ws("/a", "v1", WorkspaceKind::Repo, None))
        .unwrap();
    store
        .upsert_workspace(&ws("/a", "v2", WorkspaceKind::Repo, None))
        .unwrap();
    let found = store.get_workspace(&PathBuf::from("/a")).unwrap().unwrap();
    assert_eq!(found.name, "v2");
}

proptest! {
    #[test]
    fn prop_upsert_get_round_trip(name in "[a-zA-Z0-9_-]{1,16}") {
        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        let w = ws(&format!("/tmp/{name}"), &name, WorkspaceKind::Repo, None);
        store.upsert_workspace(&w).unwrap();
        let back = store.get_workspace(&w.path).unwrap().unwrap();
        prop_assert_eq!(w, back);
    }
}

#[test]
fn remove_nonexistent_workspace_is_noop() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    // Removing a path never added should succeed silently.
    store
        .remove_workspace(&PathBuf::from("/never-added"))
        .unwrap();
}

#[test]
fn list_with_100_workspaces_returns_all() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    for i in 0..100 {
        store
            .upsert_workspace(&ws(
                &format!("/w{i}"),
                &format!("n{i}"),
                WorkspaceKind::Repo,
                None,
            ))
            .unwrap();
    }
    let all = store.list_workspaces().unwrap();
    assert_eq!(all.len(), 100);
}

#[test]
fn very_long_path_is_stored_and_retrieved() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    // 240-character path component (large but stored as key string)
    let segment = "a".repeat(240);
    let long_path = format!("/home/user/{segment}");
    let w = ws(&long_path, "long", WorkspaceKind::Repo, None);
    store.upsert_workspace(&w).unwrap();
    let back = store
        .get_workspace(&PathBuf::from(&long_path))
        .unwrap()
        .unwrap();
    assert_eq!(back.path, PathBuf::from(&long_path));
}

#[test]
fn very_long_name_roundtrips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let long_name = "x".repeat(10_000);
    let w = ws("/tmp/named", &long_name, WorkspaceKind::Repo, None);
    store.upsert_workspace(&w).unwrap();
    let back = store
        .get_workspace(&PathBuf::from("/tmp/named"))
        .unwrap()
        .unwrap();
    assert_eq!(back.name.len(), 10_000);
}

#[test]
fn workspace_with_spaces_in_path_roundtrips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let w = ws(
        "/home/user/my projects/with spaces",
        "spaces",
        WorkspaceKind::Repo,
        None,
    );
    store.upsert_workspace(&w).unwrap();
    let back = store
        .get_workspace(&PathBuf::from("/home/user/my projects/with spaces"))
        .unwrap()
        .unwrap();
    assert_eq!(back.name, "spaces");
}

#[test]
fn umbrella_and_children_roundtrip() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let umbrella = ws("/stack", "stack", WorkspaceKind::Umbrella, None);
    let child_a = ws("/stack/a", "a", WorkspaceKind::Repo, Some("/stack"));
    let child_b = ws("/stack/b", "b", WorkspaceKind::Repo, Some("/stack"));
    store.upsert_workspace(&umbrella).unwrap();
    store.upsert_workspace(&child_a).unwrap();
    store.upsert_workspace(&child_b).unwrap();

    let all = store.list_workspaces().unwrap();
    assert_eq!(all.len(), 3);

    let got_umbrella = store
        .get_workspace(&PathBuf::from("/stack"))
        .unwrap()
        .unwrap();
    assert_eq!(got_umbrella.kind, WorkspaceKind::Umbrella);

    let got_child = store
        .get_workspace(&PathBuf::from("/stack/a"))
        .unwrap()
        .unwrap();
    assert_eq!(got_child.parent, Some(PathBuf::from("/stack")));
}

#[test]
fn upsert_updates_last_seen() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let mut w = ws("/a", "alpha", WorkspaceKind::Repo, None);
    w.last_seen = 1000;
    store.upsert_workspace(&w).unwrap();
    w.last_seen = 9999;
    store.upsert_workspace(&w).unwrap();
    let back = store.get_workspace(&PathBuf::from("/a")).unwrap().unwrap();
    assert_eq!(back.last_seen, 9999);
}

#[test]
fn multiple_removes_are_idempotent() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let w = ws("/a", "alpha", WorkspaceKind::Repo, None);
    store.upsert_workspace(&w).unwrap();
    store.remove_workspace(&PathBuf::from("/a")).unwrap();
    // Second remove on already-removed key is a no-op
    store.remove_workspace(&PathBuf::from("/a")).unwrap();
    assert!(store.list_workspaces().unwrap().is_empty());
}
