//! Insta snapshot tests for [`SshWidget::render_into_frame`].
//!
//! Each test builds a deterministic [`SshState`], optionally drives the
//! connection state machine and SFTP panel, and renders the widget into a
//! fixed `TestBackend`. The text body is then snapshotted via insta so that
//! future changes to the SSH tab layout surface as a visible diff.

use sid_core::adapters::ssh::SftpEntry;
use sid_store::{SshHost, SshHostSource};
use sid_widgets::SshWidget;
use sid_widgets::ssh::{SshConfigEntryLite, SshState, render_to_string};

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
