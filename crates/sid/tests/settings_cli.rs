//! Integration tests for `sid settings get/set/list/delete` subcommands.

use std::process::Command;

use tempfile::tempdir;

fn sid() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sid"))
}

#[test]
fn set_then_get_round_trips() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    let set = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "set",
            "theme_name",
            "void",
        ])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "settings set failed: {}",
        String::from_utf8_lossy(&set.stderr)
    );

    let get = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "get",
            "theme_name",
        ])
        .output()
        .unwrap();
    assert!(get.status.success());
    let stdout = String::from_utf8_lossy(&get.stdout);
    assert_eq!(stdout.trim(), "void");
}

#[test]
fn get_unset_key_returns_error_exit_code() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    let get = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "get",
            "never_set",
        ])
        .output()
        .unwrap();
    assert!(!get.status.success(), "expected non-zero exit for missing key");
    let stderr = String::from_utf8_lossy(&get.stderr);
    assert!(stderr.contains("not set"), "stderr: {stderr}");
}

#[test]
fn list_shows_set_keys() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    for (k, v) in [("theme_name", "cosmos"), ("default_tab", "ssh")] {
        let out = sid()
            .args([
                "--db",
                db.to_str().unwrap(),
                "--skip-discovery",
                "settings",
                "set",
                k,
                v,
            ])
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    let list = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "list",
        ])
        .output()
        .unwrap();
    assert!(list.status.success());
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("theme_name = cosmos"), "stdout: {stdout}");
    assert!(stdout.contains("default_tab = ssh"), "stdout: {stdout}");
}

#[test]
fn delete_removes_key() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "set",
            "theme_name",
            "cosmos",
        ])
        .output()
        .unwrap();
    let del = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "delete",
            "theme_name",
        ])
        .output()
        .unwrap();
    assert!(del.status.success());
    assert!(String::from_utf8_lossy(&del.stdout).contains("deleted"));

    // get should now fail.
    let get = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "get",
            "theme_name",
        ])
        .output()
        .unwrap();
    assert!(!get.status.success());
}

#[test]
fn delete_missing_key_is_noop_with_zero_exit() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    let del = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "delete",
            "never_set",
        ])
        .output()
        .unwrap();
    assert!(del.status.success());
    assert!(String::from_utf8_lossy(&del.stdout).contains("not set"));
}

#[test]
fn set_overwrites_existing_value() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    for v in ["a", "b", "c"] {
        sid()
            .args([
                "--db",
                db.to_str().unwrap(),
                "--skip-discovery",
                "settings",
                "set",
                "k",
                v,
            ])
            .output()
            .unwrap();
    }
    let get = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "get",
            "k",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "c");
}

#[test]
fn empty_value_round_trips() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "set",
            "k",
            "",
        ])
        .output()
        .unwrap();
    let get = sid()
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "get",
            "k",
        ])
        .output()
        .unwrap();
    assert!(get.status.success());
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "");
}
