use sid::wire::build_app_full;
use sid_store::{SshAuthKind, SshHost, SshHostSource};

fn host(alias: &str) -> SshHost {
    SshHost {
        alias: alias.into(),
        host: format!("{alias}.example"),
        port: 22,
        user: "pi".into(),
        identity_file: None,
        source: SshHostSource::Manual,
        last_connected: 0,
        command_history: Vec::new(),
        last_sftp_path: None,
        auth_kind: SshAuthKind::Agent,
    }
}

#[test]
fn build_app_full_with_no_ssh_state_returns_app() {
    let _app = build_app_full(None, vec![], vec![], vec![], None);
}

#[test]
fn build_app_full_with_unknown_alias_returns_app() {
    let _app = build_app_full(None, vec![], vec![], vec![], Some("nonexistent".into()));
}

#[test]
fn build_app_full_with_start_alias_selects_ssh_tab() {
    let app = build_app_full(
        None,
        vec![],
        vec![host("jp46-dev")],
        vec![],
        Some("jp46-dev".into()),
    );
    // The active tab should be "ssh" because start_ssh_alias was Some.
    assert_eq!(app.tabs().active().id.as_str(), "ssh");
}

#[test]
fn build_app_full_does_not_panic_when_ssh_config_missing() {
    let _app = build_app_full(None, vec![], vec![], vec![], None);
}
