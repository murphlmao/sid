//! P2.3 — WorkspaceStore (`.sid/config.toml`): round-trip, missing = empty, malformed = error.

use sid_core::db::DbKind;
use sid_store::entities::{AuthMethod, DbConnection, Host, QuickAction};
use sid_store::workspace::{WorkspaceConfig, WorkspaceStore};

fn host(alias: &str, secret: Option<&str>) -> Host {
    Host {
        alias: alias.into(),
        user: "deploy".into(),
        host: "prod.acme-api.internal".into(),
        port: 22,
        secret_ref: secret.map(Into::into),
        auth: AuthMethod::default(),
        folder: None,
    }
}

fn connection(id: &str) -> DbConnection {
    DbConnection {
        id: id.into(),
        dsn: "postgres://x".into(),
        secret_ref: None,
        kind: DbKind::Postgres,
        name: id.into(),
        folder: None,
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
        kind: DbKind::Postgres,
        name: "Acme PG".into(),
        folder: Some("acme".into()),
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
    assert!(
        text.contains("ssh.prod.key"),
        "the opaque ref value is written"
    );

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
    assert!(
        !ws.remove_host("prod").unwrap(),
        "removing an absent host is Ok(false)"
    );
    assert!(ws.load().unwrap().ssh.host.is_empty());
}

#[test]
fn remove_connection_reports_presence() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WorkspaceStore::new(dir.path());
    ws.upsert_connection(&connection("acme-pg")).unwrap();
    assert!(ws.remove_connection("acme-pg").unwrap());
    assert!(
        !ws.remove_connection("acme-pg").unwrap(),
        "removing an absent connection is Ok(false)"
    );
    assert!(ws.load().unwrap().db.connection.is_empty());
}

#[test]
fn auth_method_toml_roundtrips_all_variants() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WorkspaceStore::new(dir.path());
    let mut cfg = WorkspaceConfig::default();
    for (alias, auth) in [
        ("agent-host", AuthMethod::Agent),
        ("pw-host", AuthMethod::Password),
        (
            "key-host",
            AuthMethod::Key {
                path: "/home/u/.ssh/id_ed25519".into(),
            },
        ),
    ] {
        let mut h = host(alias, None);
        h.auth = auth;
        cfg.ssh.host.push(h);
    }
    ws.save(&cfg).unwrap();

    let got = ws.load().unwrap();
    assert_eq!(
        got.ssh.host, cfg.ssh.host,
        "all three auth variants survive"
    );
}

#[test]
fn toml_without_auth_key_defaults_to_agent() {
    // A pre-`auth` committed config (no `auth` on the host) must still parse.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".sid")).unwrap();
    std::fs::write(
        dir.path().join(".sid").join("config.toml"),
        "version = 1\n\n[[ssh.host]]\nalias = \"legacy\"\nuser = \"deploy\"\nhost = \"h\"\nport = 22\n",
    )
    .unwrap();
    let ws = WorkspaceStore::new(dir.path());
    let cfg = ws.load().unwrap();
    assert_eq!(cfg.ssh.host.len(), 1);
    assert_eq!(cfg.ssh.host[0].alias, "legacy");
    assert_eq!(
        cfg.ssh.host[0].auth,
        AuthMethod::Agent,
        "missing auth key defaults to Agent"
    );
}

#[test]
fn toml_without_kind_or_name_defaults_to_postgres_and_empty_name() {
    // A pre-`kind`/`name` committed config (no such keys on the connection) must still parse.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".sid")).unwrap();
    std::fs::write(
        dir.path().join(".sid").join("config.toml"),
        "version = 1\n\n[[db.connection]]\nid = \"legacy-pg\"\ndsn = \"postgres://x\"\n",
    )
    .unwrap();
    let ws = WorkspaceStore::new(dir.path());
    let cfg = ws.load().unwrap();
    assert_eq!(cfg.db.connection.len(), 1);
    assert_eq!(cfg.db.connection[0].id, "legacy-pg");
    assert_eq!(
        cfg.db.connection[0].kind,
        DbKind::Postgres,
        "missing kind key defaults to Postgres"
    );
    assert_eq!(
        cfg.db.connection[0].name, "",
        "missing name key defaults to empty string (not id)"
    );
}

#[test]
fn malformed_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".sid")).unwrap();
    std::fs::write(
        dir.path().join(".sid").join("config.toml"),
        "this is [ not valid",
    )
    .unwrap();
    let ws = WorkspaceStore::new(dir.path());
    assert!(ws.load().is_err());
}

// ---------------------------------------------------------------------
// Round-D: duplicate-identity records (a normal git-merge artifact -- TOML parses two
// `[[ssh.host]]`/`[[db.connection]]` blocks with the same alias/id just fine) must not
// be silently dropped by the by-identity mutators.
// ---------------------------------------------------------------------

fn write_duplicate_alias_hosts(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join(".sid")).unwrap();
    std::fs::write(
        dir.join(".sid").join("config.toml"),
        "version = 1\n\n\
         [[ssh.host]]\n\
         alias = \"dup\"\n\
         user = \"alice\"\n\
         host = \"a.internal\"\n\
         port = 22\n\n\
         [[ssh.host]]\n\
         alias = \"dup\"\n\
         user = \"bob\"\n\
         host = \"b.internal\"\n\
         port = 2222\n",
    )
    .unwrap();
}

#[test]
fn load_does_not_silently_dedupe_a_merge_artifact_of_two_same_alias_hosts() {
    let dir = tempfile::tempdir().unwrap();
    write_duplicate_alias_hosts(dir.path());
    let ws = WorkspaceStore::new(dir.path());
    let cfg = ws.load().unwrap();
    assert_eq!(
        cfg.ssh.host.len(),
        2,
        "load() must parse (and keep) both duplicate-alias entries, not error or dedupe"
    );
}

#[test]
fn duplicates_reports_the_duplicate_alias_hosts() {
    let dir = tempfile::tempdir().unwrap();
    write_duplicate_alias_hosts(dir.path());
    let ws = WorkspaceStore::new(dir.path());
    let cfg = ws.load().unwrap();
    let dups = cfg.duplicates();
    assert_eq!(dups.len(), 1, "exactly one duplicated identity: {dups:?}");
    assert!(
        dups[0].contains("dup") && dups[0].contains('2'),
        "diagnostic should name the alias and the count: {dups:?}"
    );
}

#[test]
fn duplicates_is_empty_for_a_layer_with_no_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WorkspaceStore::new(dir.path());
    ws.upsert_host(&host("prod", None)).unwrap();
    ws.upsert_connection(&connection("acme-pg")).unwrap();
    assert!(ws.load().unwrap().duplicates().is_empty());
}

#[test]
fn upsert_collapses_duplicate_alias_hosts_to_one_with_the_new_value() {
    let dir = tempfile::tempdir().unwrap();
    write_duplicate_alias_hosts(dir.path());
    let ws = WorkspaceStore::new(dir.path());

    let mut resolved = host("dup", None);
    resolved.user = "resolved-user".into();
    ws.upsert_host(&resolved).unwrap();

    let cfg = ws.load().unwrap();
    assert_eq!(
        cfg.ssh.host.len(),
        1,
        "an explicit upsert collapses the duplicate down to a single entry"
    );
    assert_eq!(cfg.ssh.host[0].user, "resolved-user");
    assert!(cfg.duplicates().is_empty());
}

#[test]
fn remove_removes_both_duplicate_alias_hosts() {
    let dir = tempfile::tempdir().unwrap();
    write_duplicate_alias_hosts(dir.path());
    let ws = WorkspaceStore::new(dir.path());

    assert!(ws.remove_host("dup").unwrap());
    assert!(
        ws.load().unwrap().ssh.host.is_empty(),
        "remove_host must remove EVERY duplicate-alias entry, not just the first"
    );
}

fn write_duplicate_id_connections(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join(".sid")).unwrap();
    std::fs::write(
        dir.join(".sid").join("config.toml"),
        "version = 1\n\n\
         [[db.connection]]\n\
         id = \"dup\"\n\
         dsn = \"postgres://a\"\n\n\
         [[db.connection]]\n\
         id = \"dup\"\n\
         dsn = \"postgres://b\"\n",
    )
    .unwrap();
}

#[test]
fn load_does_not_silently_dedupe_a_merge_artifact_of_two_same_id_connections() {
    let dir = tempfile::tempdir().unwrap();
    write_duplicate_id_connections(dir.path());
    let ws = WorkspaceStore::new(dir.path());
    let cfg = ws.load().unwrap();
    assert_eq!(cfg.db.connection.len(), 2);
}

#[test]
fn duplicates_reports_the_duplicate_id_connections() {
    let dir = tempfile::tempdir().unwrap();
    write_duplicate_id_connections(dir.path());
    let ws = WorkspaceStore::new(dir.path());
    let dups = ws.load().unwrap().duplicates();
    assert_eq!(dups.len(), 1);
    assert!(dups[0].contains("dup") && dups[0].contains('2'));
}

#[test]
fn upsert_collapses_duplicate_id_connections_to_one_with_the_new_value() {
    let dir = tempfile::tempdir().unwrap();
    write_duplicate_id_connections(dir.path());
    let ws = WorkspaceStore::new(dir.path());

    let mut resolved = connection("dup");
    resolved.dsn = "postgres://resolved".into();
    ws.upsert_connection(&resolved).unwrap();

    let cfg = ws.load().unwrap();
    assert_eq!(cfg.db.connection.len(), 1);
    assert_eq!(cfg.db.connection[0].dsn, "postgres://resolved");
    assert!(cfg.duplicates().is_empty());
}

#[test]
fn remove_removes_both_duplicate_id_connections() {
    let dir = tempfile::tempdir().unwrap();
    write_duplicate_id_connections(dir.path());
    let ws = WorkspaceStore::new(dir.path());

    assert!(ws.remove_connection("dup").unwrap());
    assert!(ws.load().unwrap().db.connection.is_empty());
}

// ---- Workspaces v1 foundation: register_workspace_at / unregister_workspace ----

#[test]
fn register_workspace_at_canonicalizes_and_derives_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = sid_store::Store::open(&dir.path().join("s.redb")).unwrap();
    let root = dir.path().join("my-project");
    std::fs::create_dir_all(&root).unwrap();

    let meta = store.register_workspace_at(&root).unwrap();
    assert_eq!(meta.name, "my-project");
    assert!(meta.root.is_absolute());
    // Registered and listed.
    let listed = store.list_workspaces().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, meta.id);

    // Re-registering the same path (even via a relative-ish spelling) is an
    // idempotent upsert, not a duplicate: canonicalization stabilizes the id.
    let again = store
        .register_workspace_at(&root.join("..").join("my-project"))
        .unwrap();
    assert_eq!(again.id, meta.id);
    assert_eq!(store.list_workspaces().unwrap().len(), 1);
}

#[test]
fn register_workspace_at_rejects_missing_and_non_dir_paths() {
    let dir = tempfile::tempdir().unwrap();
    let store = sid_store::Store::open(&dir.path().join("s.redb")).unwrap();
    assert!(
        store
            .register_workspace_at(&dir.path().join("nope"))
            .is_err()
    );
    let file = dir.path().join("a-file");
    std::fs::write(&file, "x").unwrap();
    assert!(store.register_workspace_at(&file).is_err());
}

#[test]
fn unregister_workspace_forgets_the_pointer_but_never_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let store = sid_store::Store::open(&dir.path().join("s.redb")).unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join(".sid")).unwrap();
    let config = root.join(".sid").join("config.toml");
    std::fs::write(&config, "# committed workspace config\n").unwrap();

    let meta = store.register_workspace_at(&root).unwrap();
    assert!(store.unregister_workspace(&meta.id).unwrap());
    assert!(store.list_workspaces().unwrap().is_empty());
    // Second unregister: already gone.
    assert!(!store.unregister_workspace(&meta.id).unwrap());
    // The committed file is untouched — attributive invariant.
    assert!(config.exists());
}
