//! Integration tests for `sid workspace add/remove/list` subcommands.
//!
//! These tests exercise the full binary round-trip using a temp redb file,
//! verifying that workspaces can be added, listed, and removed without
//! launching the TUI.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

// ── helpers ──────────────────────────────────────────────────────────────────

fn sid() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sid"))
}

/// Create a minimal git-like directory (just a `.git` subdir) at `path`.
fn make_fake_repo(path: &std::path::Path) {
    fs::create_dir_all(path).unwrap();
    fs::create_dir(path.join(".git")).unwrap();
}

// ── happy-path round-trip ─────────────────────────────────────────────────────

/// Core round-trip: add → list (shows it) → remove → list (gone).
#[test]
fn workspace_add_list_remove_round_trip() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let repo = dir.path().join("my-repo");
    make_fake_repo(&repo);

    // add
    let add = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "add", repo.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "workspace add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let add_out = String::from_utf8_lossy(&add.stdout);
    assert!(add_out.contains("added"), "expected 'added' in output: {add_out}");

    // list → should show the repo
    let list = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "list"])
        .output()
        .unwrap();
    assert!(list.status.success());
    let list_out = String::from_utf8_lossy(&list.stdout);
    assert!(
        list_out.contains("my-repo"),
        "expected workspace name in list output: {list_out}"
    );

    // remove
    let remove = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "remove", repo.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        remove.status.success(),
        "workspace remove failed: {}",
        String::from_utf8_lossy(&remove.stderr)
    );

    // list again → should no longer show the repo
    let list2 = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "list"])
        .output()
        .unwrap();
    assert!(list2.status.success());
    let list2_out = String::from_utf8_lossy(&list2.stdout);
    assert!(
        !list2_out.contains("my-repo"),
        "workspace should have been removed; output: {list2_out}"
    );
}

/// Adding multiple workspaces and listing them all appears correct.
#[test]
fn multiple_workspaces_all_appear_in_list() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    for name in ["repo-alpha", "repo-beta", "repo-gamma"] {
        let repo = dir.path().join(name);
        make_fake_repo(&repo);
        let out = sid()
            .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "add", repo.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success(), "add {name} failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    let list = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "list"])
        .output()
        .unwrap();
    assert!(list.status.success());
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("repo-alpha"), "missing repo-alpha: {out}");
    assert!(out.contains("repo-beta"), "missing repo-beta: {out}");
    assert!(out.contains("repo-gamma"), "missing repo-gamma: {out}");
}

/// `workspace list` on an empty store emits a friendly message (not an error).
#[test]
fn list_empty_store_exits_zero() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let out = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "list"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "list on empty store should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Removing a path that was never registered is a no-op (exit 0).
#[test]
fn remove_nonexistent_workspace_is_noop() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    // Create the dir so canonicalize doesn't fail
    let repo = dir.path().join("not-registered");
    fs::create_dir(&repo).unwrap();
    let out = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "remove", repo.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "removing an unregistered workspace should succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── adversarial ───────────────────────────────────────────────────────────────

/// Adding a path that does not exist on disk fails gracefully (no panic).
#[test]
fn add_nonexistent_path_exits_nonzero_without_panic() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bogus = "/nonexistent/path/that/does-not-exist-xyzzy-12345";
    let out = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "add", bogus])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "adding a nonexistent path should fail; got exit={:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "must not panic; stderr: {stderr}"
    );
}

/// Double-add of the same workspace is idempotent — no error.
#[test]
fn double_add_same_workspace_is_idempotent() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let repo = dir.path().join("idempotent-repo");
    make_fake_repo(&repo);

    for _ in 0..2 {
        let out = sid()
            .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "add", repo.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "double-add should succeed; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // Only one entry in the list
    let list = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "list"])
        .output()
        .unwrap();
    let list_out = String::from_utf8_lossy(&list.stdout);
    let count = list_out.lines().filter(|l| l.contains("idempotent-repo")).count();
    assert_eq!(count, 1, "should have exactly 1 entry after double-add; output: {list_out}");
}

/// Double-remove of the same workspace is idempotent — no error.
#[test]
fn double_remove_workspace_is_idempotent() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let repo = dir.path().join("remove-twice-repo");
    make_fake_repo(&repo);

    // Add first
    sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "add", repo.to_str().unwrap()])
        .output()
        .unwrap();

    // Remove twice
    for i in 0..2 {
        let out = sid()
            .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "remove", repo.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "remove #{i} should succeed; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// A workspace name with a very long directory name does not cause a panic.
#[test]
fn add_workspace_with_very_long_name_does_not_panic() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let long_name = "a".repeat(200);
    let repo = dir.path().join(&long_name);
    make_fake_repo(&repo);

    let out = sid()
        .args(["--db", db.to_str().unwrap(), "--skip-discovery", "workspace", "add", repo.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "must not panic; stderr: {stderr}"
    );
}

/// `sid --help` now mentions workspace subcommand.
#[test]
fn help_mentions_workspace_subcommand() {
    let out = sid().arg("--help").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("workspace"),
        "help should mention workspace subcommand; stdout: {stdout}"
    );
}

/// `sid workspace --help` exits 0 and mentions add/remove/list.
#[test]
fn workspace_help_mentions_ops() {
    let out = sid().args(["workspace", "--help"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("add"), "should mention 'add': {stdout}");
    assert!(stdout.contains("remove"), "should mention 'remove': {stdout}");
    assert!(stdout.contains("list"), "should mention 'list': {stdout}");
}
