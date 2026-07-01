//! P2.3 — WorkspaceStore (`.sid/config.toml`): round-trip, missing = empty, malformed = error.

use sid_store::entities::{DbConnection, Host, QuickAction};
use sid_store::workspace::{WorkspaceConfig, WorkspaceStore};

fn host(alias: &str, secret: Option<&str>) -> Host {
    Host {
        alias: alias.into(),
        user: "deploy".into(),
        host: "prod.acme-api.internal".into(),
        port: 22,
        secret_ref: secret.map(Into::into),
    }
}

#[test]
fn missing_file_is_empty_default_not_error() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WorkspaceStore::new(dir.path());
    let cfg = ws.load().unwrap();
    assert_eq!(cfg.version, 1);
    assert!(cfg.ssh.host.is_empty());
    assert!(cfg.db.connection.is_empty());
    assert!(!ws.config_path().exists(), "load must not create the file");
}

#[test]
fn save_then_load_roundtrips_everything() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WorkspaceStore::new(dir.path());
    let mut cfg = WorkspaceConfig::default();
    cfg.ssh.host.push(host("prod", Some("ssh.prod.key")));
    cfg.ssh.host.push(host("staging", None));
    cfg.db.connection.push(DbConnection {
        id: "acme-pg".into(),
        dsn: "postgres://acme@db.acme.internal/acme".into(),
        secret_ref: Some("db.acme-pg.pw".into()),
    });
    cfg.quick_action.push(QuickAction {
        label: "tail app log".into(),
        cmd: "journalctl -u acme-api -f".into(),
    });

    ws.save(&cfg).unwrap();
    assert!(ws.config_path().exists());

    let got = ws.load().unwrap();
    assert_eq!(got.ssh.host, cfg.ssh.host);
    assert_eq!(got.db.connection, cfg.db.connection);
    assert_eq!(got.quick_action, cfg.quick_action);
}

#[test]
fn secret_ref_is_written_and_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WorkspaceStore::new(dir.path());
    ws.upsert_host(&host("prod", Some("ssh.prod.key"))).unwrap();

    let text = std::fs::read_to_string(ws.config_path()).unwrap();
    assert!(text.contains("secret_ref"), "the ref key is written");
    assert!(text.contains("ssh.prod.key"), "the opaque ref value is written");

    let got = ws.load().unwrap();
    assert_eq!(got.ssh.host[0].secret_ref.as_deref(), Some("ssh.prod.key"));
}

#[test]
fn upsert_replaces_by_identity() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WorkspaceStore::new(dir.path());
    ws.upsert_host(&host("prod", None)).unwrap();
    let mut updated = host("prod", None);
    updated.user = "root".into();
    ws.upsert_host(&updated).unwrap();

    let cfg = ws.load().unwrap();
    assert_eq!(cfg.ssh.host.len(), 1, "same alias replaces, not duplicates");
    assert_eq!(cfg.ssh.host[0].user, "root");
}

#[test]
fn remove_host_reports_presence() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WorkspaceStore::new(dir.path());
    ws.upsert_host(&host("prod", None)).unwrap();
    assert!(ws.remove_host("prod").unwrap());
    assert!(!ws.remove_host("prod").unwrap(), "removing an absent host is Ok(false)");
    assert!(ws.load().unwrap().ssh.host.is_empty());
}

#[test]
fn malformed_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".sid")).unwrap();
    std::fs::write(dir.path().join(".sid").join("config.toml"), "this is [ not valid").unwrap();
    let ws = WorkspaceStore::new(dir.path());
    assert!(ws.load().is_err());
}
