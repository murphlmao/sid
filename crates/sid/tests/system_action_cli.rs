//! Plan 6 CLI smoke tests — `sid system action add/list/remove/run`.

use std::process::Command;

use tempfile::tempdir;

#[test]
fn action_add_list_run_remove_round_trip() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");

    let add = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "action",
            "add",
            "echo greeting",
            "echo hello",
        ])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let list = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "action",
            "list",
        ])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("echo greeting"), "stdout: {out}");

    let id = out
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    let run = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "action",
            "run",
            &id,
        ])
        .output()
        .unwrap();
    assert!(run.status.success());
    assert!(String::from_utf8_lossy(&run.stdout).contains("hello"));

    let _ = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "action",
            "remove",
            &id,
        ])
        .output()
        .unwrap();
    let list2 = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "action",
            "list",
        ])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&list2.stdout).contains("echo greeting"));
}
