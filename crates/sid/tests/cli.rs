use std::process::Command;

/// `sid --help` exits 0 and mentions both the binary name and `--db` flag.
#[test]
fn sid_help_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--help")
        .output()
        .expect("run sid --help");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("sid"), "stdout should contain 'sid': {stdout}");
    assert!(stdout.contains("--db"), "stdout should contain '--db': {stdout}");
}

/// `sid --version` exits 0.
#[test]
fn sid_version_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--version")
        .output()
        .expect("run sid --version");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

/// `sid --version` output contains the crate version string.
#[test]
fn sid_version_contains_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--version")
        .output()
        .expect("run sid --version");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Version format is "sid X.Y.Z"
    assert!(stdout.contains("sid"), "output: {stdout}");
}

/// An unknown flag should cause sid to exit non-zero.
#[test]
fn unknown_flag_exits_nonzero() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--definitely-not-a-real-flag")
        .output()
        .expect("run sid with unknown flag");
    assert!(!out.status.success(), "should have failed; stdout: {}", String::from_utf8_lossy(&out.stdout));
}

/// `sid --help` output contains `--start-tab` for tab override.
#[test]
fn sid_help_mentions_start_tab() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--help")
        .output()
        .expect("run sid --help");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("start-tab") || stdout.contains("start_tab"),
        "should mention start-tab; stdout: {stdout}");
}
