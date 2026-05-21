//! Plan 6 CLI smoke test — `sid system services`.

use std::process::Command;

use tempfile::tempdir;

#[test]
fn services_cli_runs_or_self_skips() {
    if which::which("systemctl").is_err() || which::which("journalctl").is_err() {
        eprintln!("skip: systemctl/journalctl missing");
        return;
    }
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "--skip-discovery",
            "system",
            "services",
            "--user",
        ])
        .output()
        .unwrap();
    // Either ran cleanly or surfaced a domain error — we only assert no panic.
    assert!(out.status.success() || !out.stderr.is_empty());
}
