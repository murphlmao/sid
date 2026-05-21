//! Tests for combined read_workspace_metadata — Task 14.

use std::fs;

use proptest::prelude::*;
use sid_core::workspace_metadata::{read_workspace_metadata, WorkspaceKind};
use tempfile::tempdir;

#[test]
fn read_returns_repo_kind_for_plain_directory() {
    let dir = tempdir().unwrap();
    let m = read_workspace_metadata(dir.path()).unwrap();
    assert_eq!(m.kind, WorkspaceKind::Repo);
}

#[test]
fn read_name_never_empty() {
    let dir = tempdir().unwrap();
    let m = read_workspace_metadata(dir.path()).unwrap();
    assert!(!m.name.is_empty());
}

#[test]
fn read_prefers_explicit_metadata_over_package_json_workspace() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(
        sid_dir.join("_metadata.sid"),
        r#"{"name":"explicit-name","kind":"Repo"}"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"workspaces":["a","b","c"]}"#,
    )
    .unwrap();
    let m = read_workspace_metadata(dir.path()).unwrap();
    assert_eq!(m.name, "explicit-name");
    assert_eq!(m.kind, WorkspaceKind::Repo);
    // Children should come from metadata, not from package.json
    assert!(m.children.is_empty());
}

#[test]
fn read_with_cargo_workspace_produces_children_list() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"[workspace]
resolver = "2"
members = ["crates/x", "crates/y", "crates/z"]
"#,
    )
    .unwrap();
    let m = read_workspace_metadata(dir.path()).unwrap();
    assert_eq!(m.kind, WorkspaceKind::Umbrella);
    assert_eq!(m.children.len(), 3);
    let paths: Vec<_> = m.children.iter().map(|c| c.to_string_lossy().to_string()).collect();
    assert!(paths.contains(&"crates/x".to_string()));
}

#[test]
fn read_malformed_metadata_sid_returns_err() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(sid_dir.join("_metadata.sid"), b"definitely not json").unwrap();
    assert!(read_workspace_metadata(dir.path()).is_err());
}

proptest! {
    #[test]
    fn prop_read_always_returns_something(dir_suffix in "[a-z0-9]{1,12}") {
        let dir = tempdir().unwrap();
        let sub = dir.path().join(&dir_suffix);
        fs::create_dir(&sub).unwrap();
        // Must always succeed
        let m = read_workspace_metadata(&sub).unwrap();
        // Name is non-empty
        prop_assert!(!m.name.is_empty());
    }

    #[test]
    fn prop_metadata_sid_name_survives_roundtrip(name in "[a-zA-Z0-9_-]{1,40}") {
        let dir = tempdir().unwrap();
        let sid_dir = dir.path().join(".sid");
        fs::create_dir(&sid_dir).unwrap();
        let json = format!(r#"{{"name":"{}","kind":"Repo"}}"#, name);
        fs::write(sid_dir.join("_metadata.sid"), json.as_bytes()).unwrap();
        let m = read_workspace_metadata(dir.path()).unwrap();
        prop_assert_eq!(m.name, name);
    }
}
