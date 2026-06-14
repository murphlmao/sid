use std::process::Command;

use tempfile::tempdir;

fn run(bin: &str, db: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(bin);
    cmd.args(["--db", db, "--skip-discovery"]);
    cmd.args(args);
    cmd.output().unwrap()
}

#[test]
fn ssh_add_list_remove() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let db_s = db.to_str().unwrap();

    let add = run(
        bin,
        db_s,
        &[
            "ssh",
            "add",
            "jp46-dev",
            "10.1.40.102",
            "--user",
            "pi",
            "--port",
            "2222",
        ],
    );
    assert!(
        add.status.success(),
        "add failed: stderr={}",
        String::from_utf8_lossy(&add.stderr)
    );

    let list = run(bin, db_s, &["ssh", "list"]);
    assert!(list.status.success());
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("jp46-dev"), "list output: {out}");
    assert!(out.contains("10.1.40.102"), "list output: {out}");

    let remove = run(bin, db_s, &["ssh", "remove", "jp46-dev"]);
    assert!(remove.status.success());

    let list2 = run(bin, db_s, &["ssh", "list"]);
    let out2 = String::from_utf8_lossy(&list2.stdout);
    assert!(!out2.contains("jp46-dev"));
}

#[test]
fn ssh_add_minimal_args_defaults_to_root_22() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let db_s = db.to_str().unwrap();

    let r = run(bin, db_s, &["ssh", "add", "x", "example.com"]);
    assert!(
        r.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let list = run(bin, db_s, &["ssh", "list"]);
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("root@example.com:22"), "list: {out}");
}

#[test]
fn ssh_registry_round_trips_across_invocations() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let db_s = db.to_str().unwrap();

    run(bin, db_s, &["ssh", "add", "a", "ahost"]);
    run(bin, db_s, &["ssh", "add", "b", "bhost", "--port", "2222"]);

    let list = run(bin, db_s, &["ssh", "list"]);
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("a") && out.contains("ahost"), "list: {out}");
    assert!(out.contains("b") && out.contains("bhost"), "list: {out}");
    assert!(out.contains("2222"));

    run(bin, db_s, &["ssh", "remove", "a"]);
    let list2 = run(bin, db_s, &["ssh", "list"]);
    let out2 = String::from_utf8_lossy(&list2.stdout);
    assert!(!out2.contains("ahost"));
    assert!(out2.contains("bhost"));
}
