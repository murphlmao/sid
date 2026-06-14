//! Smoke tests on the sid binary build artifact itself.
//!
//! These tests verify the binary was built, has the correct version output, has
//! the expected help sections, and is a valid ELF binary on Linux.  They are
//! intentionally lightweight — they catch "the binary is missing / was
//! corrupted / has regressed its CLI surface" before any functional test runs.

use std::{
    process::Command,
    time::{Duration, SystemTime},
};

/// `sid --version` output must match the exact format `sid <CARGO_PKG_VERSION>`.
///
/// This pins the version string format so a rename or semver change is caught
/// before any integration test runs.
#[test]
fn version_output_matches_cargo_pkg_version() {
    let expected_version = env!("CARGO_PKG_VERSION");
    let expected = format!("sid {expected_version}");

    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--version")
        .output()
        .expect("run sid --version");

    assert!(
        out.status.success(),
        "sid --version exited non-zero; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Trim trailing whitespace/newline for a clean comparison.
    assert_eq!(
        stdout.trim(),
        expected,
        "version output mismatch: got '{}'",
        stdout.trim()
    );
}

/// `sid --help` must contain USAGE (or Usage), OPTIONS (or Options), and the
/// two documented flags `--db` and `--start-tab`.
///
/// Clap 4 emits lowercase headings; this test accepts both cases.
#[test]
fn help_output_contains_expected_sections() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--help")
        .output()
        .expect("run sid --help");

    assert!(
        out.status.success(),
        "sid --help exited non-zero; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lower = stdout.to_lowercase();

    // Heading sections (case-insensitive).
    assert!(
        lower.contains("usage"),
        "help should contain 'usage': {stdout}"
    );
    assert!(
        lower.contains("options") || lower.contains("arguments"),
        "help should contain 'options' or 'arguments': {stdout}"
    );

    // Documented flags.
    assert!(
        stdout.contains("--db"),
        "help should mention --db: {stdout}"
    );
    assert!(
        stdout.contains("--start-tab") || stdout.contains("start-tab"),
        "help should mention --start-tab: {stdout}"
    );
    assert!(
        stdout.contains("--help"),
        "help should mention --help: {stdout}"
    );
    assert!(
        stdout.contains("--version"),
        "help should mention --version: {stdout}"
    );
}

/// The binary's modification time must be within the last 24 hours —
/// confirming it was rebuilt during this session and not stale.
///
/// This is a build-sanity check: if the binary is missing or very old, the
/// test fails loudly rather than silently running against the wrong artefact.
#[test]
fn binary_mtime_is_recent() {
    let bin = env!("CARGO_BIN_EXE_sid");
    let meta = std::fs::metadata(bin)
        .unwrap_or_else(|e| panic!("could not stat sid binary at '{bin}': {e}"));

    let mtime = meta
        .modified()
        .expect("platform supports mtime (Linux/macOS)");

    let age = SystemTime::now()
        .duration_since(mtime)
        .expect("binary mtime is in the past");

    // Allow up to 24 hours — a reasonable CI window.
    assert!(
        age < Duration::from_secs(24 * 3600),
        "sid binary is too old ({age:?}); expected a freshly built binary at '{bin}'"
    );
}

/// On Linux the sid binary must start with the ELF magic bytes `\x7fELF`.
///
/// This catches accidental replacement of the binary with a script or
/// zero-byte file that would silently pass process-launch smoke tests.
#[cfg(target_os = "linux")]
#[test]
fn binary_is_elf_on_linux() {
    let bin = env!("CARGO_BIN_EXE_sid");
    let bytes = std::fs::read(bin).unwrap_or_else(|e| panic!("cannot read '{bin}': {e}"));

    // ELF magic: 0x7F 'E' 'L' 'F'
    assert!(
        bytes.len() >= 4,
        "sid binary is too small to be an ELF ({} bytes)",
        bytes.len()
    );
    assert_eq!(
        &bytes[..4],
        b"\x7fELF",
        "sid binary does not start with ELF magic (got {:02x?})",
        &bytes[..4]
    );
}

/// `sid --help` and `sid --version` both exit 0 — a quick combined smoke check
/// that clap is wired up correctly and the binary launches at all.
#[test]
fn help_and_version_both_exit_zero() {
    for flag in ["--help", "--version"] {
        let out = Command::new(env!("CARGO_BIN_EXE_sid"))
            .arg(flag)
            .output()
            .unwrap_or_else(|e| panic!("run sid {flag}: {e}"));
        assert!(
            out.status.success(),
            "sid {flag} should exit 0; code={:?}, stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
