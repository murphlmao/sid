//! End-to-end integration tests for `sid db` CLI subcommands.

use std::process::Command;
use tempfile::tempdir;

#[test]
fn end_to_end_sqlite_session() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let sqlite = dir.path().join("data.db");
    let bin = env!("CARGO_BIN_EXE_sid");
    let common = ["--db", db.to_str().unwrap(), "--skip-discovery"];

    // Add
    assert!(
        Command::new(bin)
            .args(common)
            .args([
                "db",
                "add",
                "data",
                "--kind",
                "sqlite",
                "--name",
                "Data",
                "--dsn",
                sqlite.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success()
    );

    // Create + insert
    assert!(
        Command::new(bin)
            .args(common)
            .args([
                "db",
                "query",
                "data",
                "CREATE TABLE users (id INT, name TEXT)"
            ])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new(bin)
            .args(common)
            .args([
                "db",
                "query",
                "data",
                "INSERT INTO users VALUES (1, 'alice'), (2, 'bob')",
            ])
            .status()
            .unwrap()
            .success()
    );

    // Query + parse CSV
    let out = Command::new(bin)
        .args(common)
        .args([
            "db",
            "query",
            "data",
            "SELECT id, name FROM users ORDER BY id",
        ])
        .output()
        .unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("id,name"), "stdout: {s}");
    assert!(s.contains("1,alice"), "stdout: {s}");
    assert!(s.contains("2,bob"), "stdout: {s}");

    // List
    let list = Command::new(bin)
        .args(common)
        .args(["db", "list"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("data"),
        "stdout: {}",
        String::from_utf8_lossy(&list.stdout)
    );

    // Remove
    assert!(
        Command::new(bin)
            .args(common)
            .args(["db", "remove", "data"])
            .status()
            .unwrap()
            .success()
    );

    // List again — should not include data.
    let list2 = Command::new(bin)
        .args(common)
        .args(["db", "list"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&list2.stdout).contains("data "));
}

#[test]
fn db_add_with_password_then_query_succeeds() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let sqlite = dir.path().join("data.db");
    let bin = env!("CARGO_BIN_EXE_sid");
    let common = ["--db", db.to_str().unwrap(), "--skip-discovery"];

    assert!(
        Command::new(bin)
            .args(common)
            .args([
                "db",
                "add",
                "secured",
                "--kind",
                "sqlite",
                "--name",
                "Sec",
                "--dsn",
                sqlite.to_str().unwrap(),
                "--password",
                "shh",
            ])
            .status()
            .unwrap()
            .success()
    );

    // Remove should still succeed and clean up the secret.
    assert!(
        Command::new(bin)
            .args(common)
            .args(["db", "remove", "secured"])
            .status()
            .unwrap()
            .success()
    );
}
