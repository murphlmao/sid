//! P2.2 — GlobalStore (redb) CRUD, persistence, and missing-key behaviour.

use sid_core::db::DbKind;
use sid_store::{
    AuthMethod, DbConnection, DefaultScope, GlobalStore, Host, PanelSide, QuickAction, Settings,
    Store, WorkspaceId, WorkspaceMeta,
};

fn open() -> (tempfile::TempDir, GlobalStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
    (dir, store)
}

fn host(alias: &str) -> Host {
    Host {
        alias: alias.into(),
        user: "u".into(),
        host: "h".into(),
        port: 22,
        secret_ref: None,
        auth: AuthMethod::default(),
        folder: None,
    }
}

#[test]
fn host_write_read_list_delete() {
    let (_d, s) = open();
    assert!(s.list_hosts().unwrap().is_empty());
    s.upsert_host(&host("prod")).unwrap();
    s.upsert_host(&host("staging")).unwrap();
    assert_eq!(s.list_hosts().unwrap().len(), 2);
    assert_eq!(s.get_host("prod").unwrap().unwrap().alias, "prod");
    assert!(s.remove_host("prod").unwrap());
    assert!(s.get_host("prod").unwrap().is_none());
    assert_eq!(s.list_hosts().unwrap().len(), 1);
}

#[test]
fn missing_key_is_none_and_remove_absent_is_false() {
    let (_d, s) = open();
    assert!(s.get_host("nope").unwrap().is_none());
    assert!(
        !s.remove_host("nope").unwrap(),
        "removing an absent key is Ok(false), not an error"
    );
}

#[test]
fn upsert_overwrites_same_identity() {
    let (_d, s) = open();
    s.upsert_host(&host("prod")).unwrap();
    let mut updated = host("prod");
    updated.user = "root".into();
    s.upsert_host(&updated).unwrap();
    assert_eq!(
        s.list_hosts().unwrap().len(),
        1,
        "same alias overwrites, not duplicates"
    );
    assert_eq!(s.get_host("prod").unwrap().unwrap().user, "root");
}

#[test]
fn persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    {
        let s = GlobalStore::open(&path).unwrap();
        s.upsert_host(&host("prod")).unwrap();
    }
    let reopened = GlobalStore::open(&path).unwrap();
    assert_eq!(reopened.get_host("prod").unwrap().unwrap().alias, "prod");
}

#[test]
fn connections_and_quick_actions_crud() {
    let (_d, s) = open();
    s.upsert_connection(&DbConnection {
        id: "pg".into(),
        dsn: "postgres://x".into(),
        secret_ref: Some("db.pg.pw".into()),
        kind: DbKind::Postgres,
        name: "PG".into(),
        folder: None,
    })
    .unwrap();
    let got = s.get_connection("pg").unwrap().unwrap();
    assert_eq!(got.dsn, "postgres://x");
    assert_eq!(got.secret_ref.as_deref(), Some("db.pg.pw"));
    assert_eq!(got.kind, DbKind::Postgres);
    assert_eq!(got.name, "PG");

    s.upsert_quick_action(&QuickAction {
        label: "build".into(),
        cmd: "cargo build".into(),
    })
    .unwrap();
    assert_eq!(s.list_quick_actions().unwrap().len(), 1);
}

#[test]
fn workspace_registry_roundtrip() {
    let (_d, s) = open();
    let w = WorkspaceMeta {
        id: WorkspaceId("/x/acme".into()),
        root: "/x/acme".into(),
        name: "acme".into(),
    };
    s.upsert_workspace(&w).unwrap();
    assert_eq!(s.get_workspace("/x/acme").unwrap().unwrap(), w);
    assert_eq!(s.list_workspaces().unwrap().len(), 1);
}

#[test]
fn settings_missing_key_is_default_ask() {
    let (_d, s) = open();
    assert_eq!(
        s.get_settings().unwrap(),
        Settings::default(),
        "an unset SETTINGS table yields the default"
    );
    assert_eq!(s.get_settings().unwrap().default_scope, DefaultScope::Ask);
}

#[test]
fn settings_roundtrip() {
    let (_d, s) = open();
    let want = Settings {
        default_scope: DefaultScope::Workspace,
        file_browser_side: PanelSide::Right,
        secret_keyring_enabled: true,
        secret_file_enabled: true,
        theme: "cosmos".into(),
    };
    s.set_settings(&want).unwrap();
    assert_eq!(s.get_settings().unwrap(), want);
}

#[test]
fn settings_persist_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    {
        let s = GlobalStore::open(&path).unwrap();
        s.set_settings(&Settings {
            default_scope: DefaultScope::Global,
            file_browser_side: PanelSide::Right,
            secret_keyring_enabled: true,
            secret_file_enabled: true,
            theme: "cosmos".into(),
        })
        .unwrap();
    }
    let reopened = GlobalStore::open(&path).unwrap();
    assert_eq!(
        reopened.get_settings().unwrap().default_scope,
        DefaultScope::Global
    );
    assert_eq!(
        reopened.get_settings().unwrap().file_browser_side,
        PanelSide::Right
    );
}

#[test]
fn facade_settings_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("sid.redb")).unwrap();
    assert_eq!(store.settings().unwrap(), Settings::default());
    store
        .set_settings(&Settings {
            default_scope: DefaultScope::Global,
            file_browser_side: PanelSide::Right,
            secret_keyring_enabled: true,
            secret_file_enabled: true,
            theme: "cosmos".into(),
        })
        .unwrap();
    assert_eq!(
        store.settings().unwrap().default_scope,
        DefaultScope::Global
    );
    assert_eq!(
        store.settings().unwrap().file_browser_side,
        PanelSide::Right
    );
}

#[test]
fn settings_secret_backend_toggles_default_true_and_round_trip_false() {
    let (_d, s) = open();
    assert!(s.get_settings().unwrap().secret_keyring_enabled);
    assert!(s.get_settings().unwrap().secret_file_enabled);

    let want = Settings {
        default_scope: DefaultScope::Ask,
        file_browser_side: PanelSide::Left,
        secret_keyring_enabled: false,
        secret_file_enabled: false,
        theme: "cosmos".into(),
    };
    s.set_settings(&want).unwrap();
    let got = s.get_settings().unwrap();
    assert!(!got.secret_keyring_enabled);
    assert!(!got.secret_file_enabled);
}
