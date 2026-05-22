//! Insta snapshot tests for [`SshWidget::render_into_frame`].
//!
//! Each test builds a deterministic [`SshState`], optionally drives the
//! connection state machine and SFTP panel, and renders the widget into a
//! fixed `TestBackend`. The text body is then snapshotted via insta so that
//! future changes to the SSH tab layout surface as a visible diff.

use sid_core::adapters::ssh::SftpEntry;
use sid_pty::Vt100Screen;
use sid_store::{SshHost, SshHostSource};
use sid_widgets::SshWidget;
use sid_widgets::ssh::{
    PtyPane, SshConfigEntryLite, SshState, render_to_string, render_to_string_with_resize,
};

fn host(alias: &str, source: SshHostSource) -> SshHost {
    SshHost {
        alias: alias.into(),
        host: format!("{alias}.example"),
        port: 22,
        user: "u".into(),
        identity_file: None,
        source,
        last_connected: 0,
        command_history: Vec::new(),
        last_sftp_path: None,
        auth_kind: sid_store::SshAuthKind::Agent,
    }
}

fn cfg(alias: &str) -> SshConfigEntryLite {
    SshConfigEntryLite {
        alias: alias.into(),
        host: format!("{alias}.cfg"),
        port: 22,
        user: "u".into(),
        identity_file: None,
    }
}

#[test]
fn snapshot_empty_host_list() {
    let w = SshWidget::new();
    let s = render_to_string(&w, 80, 16);
    insta::assert_snapshot!("ssh_empty_host_list", s);
}

#[test]
fn snapshot_three_hosts_default_selection() {
    // SshState selects index 0 by default — but we want a "none selected"
    // look. The state machine always has *some* index selected when the list
    // is non-empty; this case captures the default-on-first-host appearance.
    let state = SshState::new(
        vec![
            host("my-prod-server", SshHostSource::Manual),
            host("staging-bastion", SshHostSource::Manual),
        ],
        vec![cfg("github.com")],
    );
    let w = SshWidget::with_state(state);
    let s = render_to_string(&w, 80, 16);
    insta::assert_snapshot!("ssh_three_hosts_default", s);
}

#[test]
fn snapshot_three_hosts_second_selected() {
    let state = SshState::new(
        vec![
            host("my-prod-server", SshHostSource::Manual),
            host("staging-bastion", SshHostSource::Manual),
        ],
        vec![cfg("github.com")],
    );
    let mut w = SshWidget::with_state(state);
    w.state_mut().select_next();
    let s = render_to_string(&w, 80, 16);
    insta::assert_snapshot!("ssh_three_hosts_second_selected", s);
}

#[test]
fn snapshot_connecting_state() {
    let state = SshState::new(vec![host("my-prod-server", SshHostSource::Manual)], vec![]);
    let mut w = SshWidget::with_state(state);
    w.connection_mut().begin_connecting("my-prod-server".into());
    let s = render_to_string(&w, 80, 16);
    insta::assert_snapshot!("ssh_connecting", s);
}

#[test]
fn snapshot_connected_with_sftp_toggled() {
    let state = SshState::new(vec![host("my-prod-server", SshHostSource::Manual)], vec![]);
    let mut w = SshWidget::with_state(state);
    w.connection_mut().begin_connecting("my-prod-server".into());
    w.connection_mut().mark_connected();
    // Toggle the SFTP panel and populate it with deterministic entries.
    w.sftp_panel_mut().toggle();
    w.sftp_panel_mut().set_cwd("/var/log".into());
    w.sftp_panel_mut().set_entries(vec![
        SftpEntry {
            name: "syslog".into(),
            is_dir: false,
            size: 4096,
            mtime_secs: 0,
            mode: 0o644,
        },
        SftpEntry {
            name: "nginx".into(),
            is_dir: true,
            size: 0,
            mtime_secs: 0,
            mode: 0o755,
        },
    ]);
    let s = render_to_string(&w, 80, 16);
    insta::assert_snapshot!("ssh_connected_sftp", s);
}

// ---------------------------------------------------------------------------
// PTY body — live vt100 buffer render path
// ---------------------------------------------------------------------------
//
// Each test below brings the widget into the `Connected` phase, attaches a
// `PtyPane` wrapping a real `Vt100Screen`, optionally feeds it bytes, and
// snapshots the rendered output. The `render_to_string_with_resize` helper
// resizes the pane to match the body rect before drawing so the screen and
// the bordered area agree on `(rows, cols)`.

/// Build a Connected widget with a single host and an attached `PtyPane`
/// wrapping a fresh `Vt100Screen` sized `rows x cols`.
fn connected_widget_with_pane(rows: u16, cols: u16) -> SshWidget {
    let state = SshState::new(vec![host("my-prod-server", SshHostSource::Manual)], vec![]);
    let mut w = SshWidget::with_state(state);
    w.connection_mut().begin_connecting("my-prod-server".into());
    w.connection_mut().mark_connected();
    let screen = Vt100Screen::new(rows, cols);
    w.set_pty_pane(PtyPane::new(Box::new(screen)));
    w
}

#[test]
fn pty_body_empty_renders_waiting_hint() {
    // Connect, attach an empty pane, feed nothing. Body should show the
    // dim "(waiting for output…)" hint instead of a wall of spaces.
    let mut w = connected_widget_with_pane(10, 40);
    let s = render_to_string_with_resize(&mut w, 80, 16);
    assert!(
        s.contains("(waiting for output"),
        "expected waiting hint, got:\n{s}"
    );
    insta::assert_snapshot!("ssh_pty_body_empty", s);
}

#[test]
fn pty_body_with_feed_renders_lines() {
    // Feed "hello\r\nworld\r\n" — vt100 needs explicit CR to reset column.
    // Both lines must surface in the rendered body.
    let mut w = connected_widget_with_pane(10, 40);
    w.pty_pane_mut().unwrap().feed(b"hello\r\nworld\r\n");
    let s = render_to_string_with_resize(&mut w, 80, 16);
    assert!(s.contains("hello"), "expected 'hello' in:\n{s}");
    assert!(s.contains("world"), "expected 'world' in:\n{s}");
    insta::assert_snapshot!("ssh_pty_body_with_feed", s);
}

#[test]
fn pty_body_cursor_position_is_inverted() {
    // Feed "abc" — cursor lands at (0, 3). Render and confirm:
    // (a) the row "abc" is visible,
    // (b) the cursor cell is drawn (the snapshot pins the exact look).
    // The buffer-level invert (fg<->bg) doesn't show as a glyph in the
    // ASCII dump, but the snapshot still pins the body's visible content,
    // so any regression that *moves* or *drops* the cursor row will diff.
    let mut w = connected_widget_with_pane(10, 40);
    w.pty_pane_mut().unwrap().feed(b"abc");
    let (cur_row, cur_col) = w.pty_pane().unwrap().cursor_position();
    assert_eq!(
        (cur_row, cur_col),
        (0, 3),
        "vt100 advances cursor one column per glyph"
    );
    let s = render_to_string_with_resize(&mut w, 80, 16);
    assert!(s.contains("abc"), "expected 'abc' in:\n{s}");
    insta::assert_snapshot!("ssh_pty_body_cursor", s);
}

#[test]
fn pty_body_handles_wide_terminal() {
    // 200x60 — confirm the render does not panic on a wide area, the pane
    // is resized to fit, and the body truncates / wraps cleanly.
    //
    // Order matters: we resize the pane FIRST (via a tiny pre-render at
    // 200x60), then feed bytes so vt100 wraps at the *post-resize* column
    // count. This proves the resize hook actually changes the inner
    // geometry the parser is using.
    let mut w = connected_widget_with_pane(10, 40);
    // Pre-render once to trigger the resize on the attached pane.
    let _ = render_to_string_with_resize(&mut w, 200, 60);
    let (post_rows, post_cols) = w.pty_pane().unwrap().size();
    assert!(
        post_cols > 40,
        "pane should have widened beyond its 40-col construction \
         size after pty_pane_resize_to_area; got cols={post_cols}, rows={post_rows}"
    );
    // Now feed; vt100 will wrap at `post_cols`, not the original 40.
    let long: Vec<u8> = std::iter::repeat_n(b'X', 250).collect();
    w.pty_pane_mut().unwrap().feed(&long);
    let s = render_to_string_with_resize(&mut w, 200, 60);
    // No line in the output exceeds the terminal width (200 cols).
    for line in s.lines() {
        let count = line.chars().count();
        assert!(count <= 200, "line of {count} chars exceeds 200: {line:?}");
    }
    // The widget still draws its chrome.
    assert!(s.contains("Hosts"), "expected Hosts pane in:\n{s}");
    insta::assert_snapshot!("ssh_pty_body_wide", s);
}

#[test]
fn pty_body_handles_tiny_terminal() {
    // 20x5 — the right pane is genuinely tiny (a couple of cells wide,
    // a single body row). Must not panic. We deliberately do NOT snapshot
    // this case — the chrome dominates and the snapshot would be brittle.
    // Behavioural assertion only.
    let mut w = connected_widget_with_pane(10, 40);
    w.pty_pane_mut().unwrap().feed(b"data");
    let s = render_to_string_with_resize(&mut w, 20, 5);
    // Sanity: output has the expected 5 rows.
    let row_count = s.lines().count();
    assert_eq!(row_count, 5, "expected 5 rows, got {row_count}");
    for line in s.lines() {
        let count = line.chars().count();
        assert!(count <= 20, "line of {count} chars exceeds 20: {line:?}");
    }
}
