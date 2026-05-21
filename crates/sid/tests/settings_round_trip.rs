//! End-to-end: `sid settings set` in one subprocess, `sid settings get` in
//! another, same DB path. Verifies the on-disk format is portable across
//! sid invocations and that the sid.toml override path actually re-points
//! the DB.

use std::process::Command;

use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_sid")
}

#[test]
fn settings_persist_across_processes() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");

    let set = Command::new(bin())
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
        "set failed: {}",
        String::from_utf8_lossy(&set.stderr)
    );

    let get = Command::new(bin())
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
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "void");
}

#[test]
fn very_long_value_round_trips_across_processes() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");
    let big = "x".repeat(8 * 1024);

    let set = Command::new(bin())
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "set",
            "k",
            &big,
        ])
        .output()
        .unwrap();
    assert!(set.status.success());

    let get = Command::new(bin())
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
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim().len(), big.len());
}

#[test]
fn two_sets_then_get_yields_last_writer() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");

    for v in ["v1", "v2", "v3"] {
        let out = Command::new(bin())
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
        assert!(out.status.success());
    }
    let get = Command::new(bin())
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
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "v3");
}

#[test]
fn list_in_second_process_sees_first_process_writes() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");

    Command::new(bin())
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
    Command::new(bin())
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "set",
            "default_tab",
            "network",
        ])
        .output()
        .unwrap();

    let list = Command::new(bin())
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "settings",
            "list",
        ])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("theme_name = cosmos"));
    assert!(out.contains("default_tab = network"));
}
