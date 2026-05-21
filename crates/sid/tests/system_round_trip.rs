//! Plan 6 — end-to-end CLI integration: pinned configs + quick actions persist
//! across separate `sid` invocations against the same redb file.

use std::process::Command;

use tempfile::tempdir;

#[test]
fn pinned_configs_and_quick_actions_round_trip_via_cli() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let target = dir.path().join("a.conf");
    std::fs::write(&target, "x").unwrap();
    let bin = env!("CARGO_BIN_EXE_sid");

    // Pin a config.
    let pin = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "pin",
            target.to_str().unwrap(),
            "--label",
            "L",
        ])
        .output()
        .unwrap();
    assert!(
        pin.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&pin.stderr)
    );

    // Add a quick action.
    let add = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "action",
            "add",
            "test",
            "echo hi",
        ])
        .output()
        .unwrap();
    assert!(add.status.success());

    // List both.
    let pins = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "pins",
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&pins.stdout).contains('L'));

    let actions = Command::new(bin)
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
    assert!(String::from_utf8_lossy(&actions.stdout).contains("test"));

    // Verify the second invocation sees the same state (persistence).
    let pins2 = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "pins",
        ])
        .output()
        .unwrap();
    assert_eq!(pins.stdout, pins2.stdout);
}
