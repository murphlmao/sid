//! CLI argument smoke / adversarial tests.
//!
//! These tests run the sid binary as a subprocess with flag combinations and
//! verify exit codes and stderr output.  Tests that require a real TTY (raw
//! mode) are gated behind `std::io::IsTerminal` and skipped in CI/no-TTY
//! environments with a clear message.

use std::process::Command;

// ---------------------------------------------------------------------------
// Original tests (kept verbatim)
// ---------------------------------------------------------------------------

/// `sid --help` exits 0 and mentions both the binary name and `--db` flag.
#[test]
fn sid_help_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--help")
        .output()
        .expect("run sid --help");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("sid"),
        "stdout should contain 'sid': {stdout}"
    );
    assert!(
        stdout.contains("--db"),
        "stdout should contain '--db': {stdout}"
    );
}

/// `sid --version` exits 0.
#[test]
fn sid_version_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--version")
        .output()
        .expect("run sid --version");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert!(
        !out.status.success(),
        "should have failed; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
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
    assert!(
        stdout.contains("start-tab") || stdout.contains("start_tab"),
        "should mention start-tab; stdout: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Adversarial: error paths that must exit non-zero with useful output
// ---------------------------------------------------------------------------

/// `sid --db <unreadable-permissions-file>` must exit non-zero and emit a
/// useful error to stderr.
///
/// We create a file, strip all permissions, then pass it as `--db`.  On
/// Linux/macOS `chmod 000` produces a file that is not readable/writable by
/// the owning process (unless running as root).
#[cfg(unix)]
#[test]
fn db_unreadable_file_exits_nonzero_with_stderr() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    // Skip if running as root — root can read/write any file regardless of
    // permissions, so this test is meaningless.  We detect root by checking
    // whether `id -u` prints "0".
    let uid_out = Command::new("id").arg("-u").output().expect("run id -u");
    let uid_str = String::from_utf8_lossy(&uid_out.stdout);
    if uid_str.trim() == "0" {
        eprintln!("SKIP: running as root; permission test is meaningless");
        return;
    }

    let dir = tempdir().unwrap();
    let path = dir.path().join("noaccess.redb");
    fs::write(&path, b"placeholder").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--db")
        .arg(&path)
        .output()
        .expect("run sid with unreadable db");

    // Restore permissions so tempdir cleanup can remove the file.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        !out.status.success(),
        "sid should fail on an unreadable db; exit={:?}",
        out.status.code()
    );

    // There must be *something* on stderr — the error must not be silent.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.is_empty(),
        "stderr must be non-empty on unreadable db error"
    );
}

/// `sid --db /dev/null` — /dev/null is not a valid redb file; sid must exit
/// non-zero with a clear error on stderr.
#[cfg(unix)]
#[test]
fn db_dev_null_exits_nonzero_with_stderr() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--db")
        .arg("/dev/null")
        .output()
        .expect("run sid --db /dev/null");

    assert!(
        !out.status.success(),
        "sid should fail when --db is /dev/null; exit={:?}",
        out.status.code()
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.is_empty(),
        "stderr should be non-empty when --db /dev/null fails"
    );
}

/// `sid --start-tab nonexistent-tab` — an unknown tab id must not crash sid.
///
/// Specifically:
///   - If a TTY is available: sid should launch and eventually exit 0 (the
///     unknown tab falls back to the default, as verified by `build_app`'s unit
///     tests).
///   - If no TTY: sid exits non-zero because `enable_raw_mode` fails; but
///     crucially it must not panic (no Rust backtrace in stderr).
///
/// This test verifies the "no panic" property regardless of TTY availability.
#[test]
fn start_tab_nonexistent_does_not_panic() {
    use std::io::IsTerminal;

    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--start-tab")
        .arg("nonexistent-tab-xyzzy")
        .output()
        .expect("run sid --start-tab nonexistent-tab-xyzzy");

    let stderr = String::from_utf8_lossy(&out.stderr);

    // Regardless of exit code, there must be no Rust panic backtrace.
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "sid must not panic on unknown --start-tab; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("stack backtrace"),
        "sid must not emit a stack backtrace; stderr: {stderr}"
    );

    if std::io::stdout().is_terminal() {
        // When a TTY is present sid should launch and exit 0 (unknown tab
        // falls back to default).  We can't drive it to quit here, so we just
        // verify it started at all — a panic would have exited non-zero.
        eprintln!("INFO: TTY present; sid launched (exit={:?})", out.status);
    } else {
        // In a non-TTY environment sid fails at enable_raw_mode (expected),
        // but it must not panic.
        eprintln!("INFO: non-TTY; exit={:?} (expected non-zero)", out.status);
    }
}

/// `sid --start-tab ""` — an empty string tab id must not panic; sid may exit
/// non-zero in non-TTY environments but never with a Rust panic.
#[test]
fn start_tab_empty_string_does_not_panic() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--start-tab")
        .arg("")
        .output()
        .expect("run sid --start-tab ''");

    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stderr.contains("thread 'main' panicked"),
        "sid must not panic on empty --start-tab; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("stack backtrace"),
        "sid must not emit a backtrace on empty --start-tab; stderr: {stderr}"
    );
}

/// `sid --start-tab settings` — a valid tab id must not crash sid.
/// Skipped when no TTY is available (raw mode would fail).
#[test]
fn start_tab_valid_settings_does_not_panic() {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        eprintln!("SKIP: not a TTY — sid requires a terminal for raw mode");
        return;
    }

    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--start-tab")
        .arg("settings")
        .output()
        .expect("run sid --start-tab settings");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "sid must not panic on --start-tab settings; stderr: {stderr}"
    );
}

/// Combine `--db <temp>` and `--start-tab database` — both flags together
/// must not crash sid.
///
/// In a non-TTY environment we expect a non-zero exit (raw mode fails) but no
/// panic.  In a TTY environment we skip the launch to avoid blocking.
#[test]
fn combined_db_and_start_tab_does_not_panic() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db = dir.path().join("combined.redb");

    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--db")
        .arg(&db)
        .arg("--start-tab")
        .arg("database")
        .output()
        .expect("run sid --db <temp> --start-tab database");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "sid must not panic with combined flags; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("stack backtrace"),
        "sid must not emit a stack backtrace with combined flags; stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Adversarial: additional flag validation
// ---------------------------------------------------------------------------

/// Supplying a value that looks like another flag to `--db` should be handled
/// gracefully by clap (either parse error or the value is used as-is).  Either
/// way sid must not panic.
#[test]
fn db_flag_with_value_that_looks_like_a_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--db")
        .arg("--start-tab")
        .output()
        .expect("run sid --db --start-tab");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "sid must not panic; stderr: {stderr}"
    );
}

/// Passing `--start-tab` without a value should produce a clap parse error
/// (exit non-zero) — not a panic.
#[test]
fn start_tab_without_value_exits_nonzero() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--start-tab")
        .output()
        .expect("run sid --start-tab (no value)");

    // clap requires a value for --start-tab; missing value → parse error → non-zero exit.
    assert!(
        !out.status.success(),
        "sid --start-tab (no value) should exit non-zero; got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "must not panic; stderr: {stderr}"
    );
}
