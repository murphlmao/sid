use std::error::Error as StdError;
use std::path::PathBuf;

use sid_core::SidError;

// ── existing test ────────────────────────────────────────────────────────────

#[test]
fn error_display_includes_message() {
    let e = SidError::Other("boom".into());
    let msg = format!("{e}");
    assert!(msg.contains("boom"));
}

// ── per-variant display tests ────────────────────────────────────────────────

#[test]
fn storage_display_contains_prefix_and_message() {
    let e = SidError::Storage("write failed".into());
    let msg = format!("{e}");
    assert!(msg.contains("storage error"), "msg: {msg}");
    assert!(msg.contains("write failed"), "msg: {msg}");
}

#[test]
fn io_display_contains_path_and_kind() {
    let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let e = SidError::Io { path: PathBuf::from("/etc/secret"), source: inner };
    let msg = format!("{e}");
    assert!(msg.contains("io error reading"), "msg: {msg}");
    assert!(msg.contains("/etc/secret") || msg.contains("secret"), "msg: {msg}");
}

#[test]
fn unknown_widget_display_contains_id_and_context() {
    let e = SidError::UnknownWidget("git-log".into());
    let msg = format!("{e}");
    assert!(msg.contains("git-log"), "msg: {msg}");
    assert!(msg.contains("not registered"), "msg: {msg}");
}

#[test]
fn unknown_action_display_contains_id_and_context() {
    let e = SidError::UnknownAction("open-workspace".into());
    let msg = format!("{e}");
    assert!(msg.contains("open-workspace"), "msg: {msg}");
    assert!(msg.contains("not registered"), "msg: {msg}");
}

#[test]
fn invalid_keybind_display_contains_prefix_and_string() {
    let e = SidError::InvalidKeybind("ctrl+???".into());
    let msg = format!("{e}");
    assert!(msg.contains("invalid keybind"), "msg: {msg}");
    assert!(msg.contains("ctrl+???"), "msg: {msg}");
}

#[test]
fn other_display_is_bare_message() {
    let e = SidError::Other("unexpected condition".into());
    let msg = format!("{e}");
    // Other renders as just the inner string with no prefix
    assert_eq!(msg, "unexpected condition");
}

// ── source-chaining test ─────────────────────────────────────────────────────

#[test]
fn io_variant_source_chains_inner_io_error() {
    let inner = std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timed out");
    let e = SidError::Io { path: PathBuf::from("/tmp/sock"), source: inner };
    // The `#[source]` attribute means `std::error::Error::source()` should
    // return `Some(inner_io_error)`.
    let src = StdError::source(&e);
    assert!(src.is_some(), "expected Io source to be chained");
    let src_msg = format!("{}", src.unwrap());
    assert!(src_msg.contains("timed out") || src_msg.contains("connection"), "src_msg: {src_msg}");
}

#[test]
fn non_io_variants_have_no_source() {
    let variants: &[SidError] = &[
        SidError::Storage("x".into()),
        SidError::UnknownWidget("x".into()),
        SidError::UnknownAction("x".into()),
        SidError::InvalidKeybind("x".into()),
        SidError::Other("x".into()),
    ];
    for e in variants {
        assert!(
            StdError::source(e).is_none(),
            "expected no source for {e:?}"
        );
    }
}

// ── adversarial tests ────────────────────────────────────────────────────────

#[test]
fn empty_message_variants_do_not_panic() {
    let variants: Vec<SidError> = vec![
        SidError::Storage(String::new()),
        SidError::UnknownWidget(String::new()),
        SidError::UnknownAction(String::new()),
        SidError::InvalidKeybind(String::new()),
        SidError::Other(String::new()),
    ];
    for e in &variants {
        let msg = format!("{e}");
        // Must not panic; the message is allowed to be empty-ish but must format.
        let _ = msg;
    }
}

#[test]
fn very_long_message_does_not_panic() {
    let long = "x".repeat(100_000);
    let variants: Vec<SidError> = vec![
        SidError::Storage(long.clone()),
        SidError::UnknownWidget(long.clone()),
        SidError::UnknownAction(long.clone()),
        SidError::InvalidKeybind(long.clone()),
        SidError::Other(long.clone()),
    ];
    for e in &variants {
        let msg = format!("{e}");
        // Long message must be preserved somewhere in the output.
        assert!(msg.contains("x"), "msg too short for variant {e:?}");
    }
}

#[test]
fn unicode_and_emoji_in_message() {
    let unicode = "日本語 🦀 café naïve résumé";
    let e = SidError::Other(unicode.into());
    let msg = format!("{e}");
    assert!(msg.contains("🦀"), "emoji should survive formatting");
    assert!(msg.contains("café"), "accented chars should survive");
}

#[test]
fn newlines_and_control_chars_in_message() {
    let s = "line1\nline2\ttabbed\x00null";
    let e = SidError::Other(s.into());
    let msg = format!("{e}");
    // Should not panic; content preserved.
    assert!(msg.contains("line1"), "msg: {msg}");
}

#[test]
fn io_error_with_empty_path_still_displays() {
    let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let e = SidError::Io { path: PathBuf::from(""), source: inner };
    let msg = format!("{e}");
    // Must format without panicking.
    assert!(msg.contains("io error reading"), "msg: {msg}");
}
