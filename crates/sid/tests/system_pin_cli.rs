//! Plan 6 CLI smoke tests — `sid system pin / unpin / pins`.

use std::process::Command;

use tempfile::tempdir;

#[test]
fn system_pin_then_pins_lists_it() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let target = dir.path().join("my.conf");
    std::fs::write(&target, "x").unwrap();

    let bin = env!("CARGO_BIN_EXE_sid");
    let pin = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "pin",
            target.to_str().unwrap(),
            "--label",
            "test cfg",
        ])
        .output()
        .unwrap();
    assert!(
        pin.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&pin.stderr)
    );

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
    assert!(pins.status.success());
    let out = String::from_utf8_lossy(&pins.stdout);
    assert!(out.contains("test cfg"), "stdout: {out}");
}

#[test]
fn full_pin_unpin_round_trip() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let target = dir.path().join("my.conf");
    std::fs::write(&target, "x").unwrap();
    let bin = env!("CARGO_BIN_EXE_sid");

    let _ = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "pin",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let _ = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "unpin",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
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
    let out = String::from_utf8_lossy(&pins.stdout);
    assert!(!out.contains(target.to_str().unwrap()));
}

#[test]
fn unpin_nonexistent_is_noop_returns_zero() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "unpin",
            "/never",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
}
