use sid_core::adapters::ssh::SftpEntry;
use sid_store::{SshHost, SshHostSource};
use sid_widgets::ssh::{
    CommandHistory, ConnectionPhase, ConnectionState, SftpEditPhase, SftpEditState, SftpPanel,
    SftpPanelVisibility, SshConfigEntryLite, SshState, prepare_download, prepare_upload,
};
use tempfile::tempdir;

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

// ---- SshState ----

#[test]
fn state_holds_hosts_and_selects_first() {
    let s = SshState::new(vec![host("a", SshHostSource::Manual)], vec![], false);
    assert_eq!(s.selected_alias().unwrap(), "a");
}

#[test]
fn next_and_prev_move_selection() {
    // ListCursor saturates (no wrap) — at the last item, down() stays put.
    let mut s = SshState::new(
        vec![
            host("a", SshHostSource::Manual),
            host("b", SshHostSource::Manual),
        ],
        vec![],
        false,
    );
    s.select_next();
    assert_eq!(s.selected_alias().unwrap(), "b");
    // at bottom, next saturates (stays on "b")
    s.select_next();
    assert_eq!(s.selected_alias().unwrap(), "b");
    s.select_prev();
    assert_eq!(s.selected_alias().unwrap(), "a");
}

#[test]
fn empty_state_has_no_selection() {
    let s = SshState::new(vec![], vec![], false);
    assert!(s.selected_alias().is_none());
}

#[test]
fn merges_ssh_config_entries_with_store_hosts() {
    let s = SshState::new(
        vec![host("manual-only", SshHostSource::Manual)],
        vec![cfg("config-only"), cfg("manual-only")],
        false,
    );
    let aliases: Vec<_> = s.visible_hosts().iter().map(|h| h.alias.clone()).collect();
    assert!(aliases.contains(&"manual-only".to_string()));
    assert!(aliases.contains(&"config-only".to_string()));
    let mo = s
        .visible_hosts()
        .iter()
        .find(|h| h.alias == "manual-only")
        .unwrap();
    assert_eq!(mo.host, "manual-only.example");
    let co = s
        .visible_hosts()
        .iter()
        .find(|h| h.alias == "config-only")
        .unwrap();
    assert_eq!(co.source, SshHostSource::SshConfig);
}

#[test]
fn select_next_on_empty_is_noop() {
    let mut s = SshState::new(vec![], vec![], false);
    s.select_next();
    s.select_prev();
    assert!(s.selected_alias().is_none());
}

// ---- ConnectionState ----

#[test]
fn fresh_connection_state_is_idle() {
    let s = ConnectionState::default();
    assert_eq!(s.phase(), ConnectionPhase::Idle);
}

#[test]
fn idle_transitions_through_full_lifecycle() {
    let mut s = ConnectionState::default();
    s.begin_connecting("a".into());
    assert_eq!(s.phase(), ConnectionPhase::Connecting);
    s.mark_connected();
    assert_eq!(s.phase(), ConnectionPhase::Connected);
    s.mark_disconnected();
    assert_eq!(s.phase(), ConnectionPhase::Disconnected);
}

#[test]
fn connecting_can_fail() {
    let mut s = ConnectionState::default();
    s.begin_connecting("a".into());
    s.mark_failed("auth".into());
    assert_eq!(s.phase(), ConnectionPhase::Failed);
    assert_eq!(s.error_message(), Some("auth"));
}

#[test]
fn reset_clears_state() {
    let mut s = ConnectionState::default();
    s.begin_connecting("a".into());
    s.mark_failed("oops".into());
    s.reset();
    assert_eq!(s.phase(), ConnectionPhase::Idle);
    assert!(s.alias().is_none());
    assert!(s.error_message().is_none());
}

// ---- CommandHistory ----

#[test]
fn fresh_history_is_empty() {
    let h = CommandHistory::new(100);
    assert!(h.entries().is_empty());
}

#[test]
fn push_appends_and_caps() {
    let mut h = CommandHistory::new(3);
    h.push("a".into());
    h.push("b".into());
    h.push("c".into());
    h.push("d".into());
    assert_eq!(h.entries(), vec!["b".to_string(), "c".into(), "d".into()]);
}

#[test]
fn duplicate_consecutive_is_collapsed() {
    let mut h = CommandHistory::new(10);
    h.push("ls".into());
    h.push("ls".into());
    h.push("cd".into());
    assert_eq!(h.entries(), vec!["ls".to_string(), "cd".into()]);
}

#[test]
fn empty_commands_are_ignored() {
    let mut h = CommandHistory::new(10);
    h.push("".into());
    h.push("   ".into());
    assert!(h.entries().is_empty());
}

#[test]
fn cap_of_zero_normalized_to_one() {
    let mut h = CommandHistory::new(0);
    h.push("a".into());
    h.push("b".into());
    assert_eq!(h.entries(), vec!["b".to_string()]);
}

// ---- SftpPanel ----

fn entry(name: &str, is_dir: bool) -> SftpEntry {
    SftpEntry {
        name: name.into(),
        is_dir,
        size: 0,
        mtime_secs: 0,
        mode: 0,
    }
}

#[test]
fn fresh_panel_is_hidden() {
    let p = SftpPanel::new();
    assert_eq!(p.visibility(), SftpPanelVisibility::Hidden);
}

#[test]
fn toggle_makes_visible() {
    let mut p = SftpPanel::new();
    p.toggle();
    assert_eq!(p.visibility(), SftpPanelVisibility::Visible);
    p.toggle();
    assert_eq!(p.visibility(), SftpPanelVisibility::Hidden);
}

#[test]
fn cwd_join_for_drill_in() {
    let mut p = SftpPanel::new();
    p.set_cwd("/home/test".into());
    p.set_entries(vec![entry("subdir", true)]);
    assert_eq!(p.selected_remote_path().unwrap(), "/home/test/subdir");
}

#[test]
fn ascend_drops_last_path_segment() {
    let mut p = SftpPanel::new();
    p.set_cwd("/home/test/sub".into());
    p.ascend();
    assert_eq!(p.cwd(), "/home/test");
    p.ascend();
    assert_eq!(p.cwd(), "/home");
    p.ascend();
    assert_eq!(p.cwd(), "/");
    p.ascend();
    assert_eq!(p.cwd(), "/");
}

#[test]
fn set_cwd_empty_normalizes_to_root() {
    let mut p = SftpPanel::new();
    p.set_cwd(String::new());
    assert_eq!(p.cwd(), "/");
}

// ---- prepare_download / prepare_upload ----

#[test]
fn prepare_download_returns_paths() {
    let mut panel = SftpPanel::new();
    panel.set_cwd("/home/test".into());
    panel.set_entries(vec![entry("foo.txt", false)]);
    let dir = tempdir().unwrap();
    let (remote, local) = prepare_download(&panel, dir.path()).unwrap();
    assert_eq!(remote, "/home/test/foo.txt");
    assert!(local.starts_with(dir.path()));
}

#[test]
fn prepare_download_refuses_directories() {
    let mut panel = SftpPanel::new();
    panel.set_cwd("/h".into());
    panel.set_entries(vec![entry("adir", true)]);
    let dir = tempdir().unwrap();
    assert!(prepare_download(&panel, dir.path()).is_none());
}

#[test]
fn prepare_upload_joins_basename_with_cwd() {
    let mut panel = SftpPanel::new();
    panel.set_cwd("/var/up".into());
    let dir = tempdir().unwrap();
    let local = dir.path().join("report.txt");
    std::fs::write(&local, b"hi").unwrap();
    let (l, r) = prepare_upload(&panel, &local).unwrap();
    assert_eq!(l, local);
    assert_eq!(r, "/var/up/report.txt");
}

#[test]
fn prepare_upload_with_nonexistent_local_returns_none() {
    let panel = SftpPanel::new();
    assert!(prepare_upload(&panel, std::path::Path::new("/never/exists")).is_none());
}

// ---- SftpEditState ----

#[test]
fn fresh_edit_state_is_idle() {
    let s = SftpEditState::default();
    assert_eq!(s.phase(), SftpEditPhase::Idle);
}

#[test]
fn full_edit_lifecycle() {
    let mut s = SftpEditState::default();
    s.begin_download("/r/x".into(), "/tmp/x".into());
    assert_eq!(s.phase(), SftpEditPhase::Downloading);
    s.mark_download_complete();
    assert_eq!(s.phase(), SftpEditPhase::Editing);
    s.mark_editor_done(true);
    assert_eq!(s.phase(), SftpEditPhase::Uploading);
    s.mark_upload_complete();
    assert_eq!(s.phase(), SftpEditPhase::Done);
}

#[test]
fn editor_failure_to_failed() {
    let mut s = SftpEditState::default();
    s.begin_download("/r".into(), "/l".into());
    s.mark_download_complete();
    s.mark_editor_done(false);
    assert_eq!(s.phase(), SftpEditPhase::Failed);
}

#[test]
fn edit_reset_clears_all_fields() {
    let mut s = SftpEditState::default();
    s.begin_download("/r".into(), "/l".into());
    s.mark_failed("oops".into());
    s.reset();
    assert_eq!(s.phase(), SftpEditPhase::Idle);
    assert!(s.remote_path().is_none());
}
