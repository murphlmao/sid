//! A global store written by an earlier build still opens, reads and writes.
//!
//! `fixtures/global-redb-4.1.redb` was written through `GlobalStore` by sid at d5947d1
//! (redb 4.1.0, the entity versions of that day): two hosts, one DB connection, one
//! quick action. The user's live `store.redb` is a file exactly like it, so a redb bump
//! that changed the on-disk format, or an entity change that broke an old postcard
//! version, fails here before it can strand anyone's data.

use sid_core::db::DbKind;
use sid_store::{AuthMethod, GlobalStore, Host};

fn open_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    // A copy: opening a redb file may rewrite its header, and the fixture is read-only
    // test data.
    std::fs::write(&path, include_bytes!("fixtures/global-redb-4.1.redb")).unwrap();
    (dir, path)
}

#[test]
fn store_written_by_redb_4_1_reads_back_unchanged() {
    let (_dir, path) = open_fixture();
    let s = GlobalStore::open(&path).unwrap();

    let mut hosts = s.list_hosts().unwrap();
    hosts.sort_by(|a, b| a.alias.cmp(&b.alias));
    assert_eq!(hosts.len(), 2);
    let home = &hosts[0];
    assert_eq!(home.alias, "home-server");
    assert_eq!(home.user, "you");
    assert_eq!(home.host, "192.168.1.10");
    assert_eq!(home.port, 22);
    assert_eq!(home.secret_ref.as_deref(), Some("ssh.home-server.pw"));
    assert_eq!(home.auth, AuthMethod::default());
    assert_eq!(home.folder.as_deref(), Some("lab"));
    let vps = &hosts[1];
    assert_eq!((vps.alias.as_str(), vps.port), ("vps-1", 2222));
    assert!(vps.secret_ref.is_none() && vps.folder.is_none());

    let conn = s
        .get_connection("acme-pg")
        .unwrap()
        .expect("connection survives");
    assert_eq!(conn.dsn, "postgres://acme@db.acme.internal/acme");
    assert_eq!(conn.secret_ref.as_deref(), Some("db.acme-pg.pw"));
    assert_eq!(conn.kind, DbKind::Postgres);
    assert_eq!(conn.name, "Acme PG");

    let actions = s.list_quick_actions().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].cmd, "journalctl -u acme-api -f");
}

#[test]
fn store_written_by_redb_4_1_accepts_new_writes() {
    let (_dir, path) = open_fixture();
    {
        let s = GlobalStore::open(&path).unwrap();
        s.upsert_host(&Host {
            alias: "new-box".into(),
            user: "me".into(),
            host: "10.0.0.9".into(),
            port: 22,
            secret_ref: None,
            auth: AuthMethod::default(),
            folder: None,
        })
        .unwrap();
        assert!(s.remove_host("vps-1").unwrap());
    }
    // Reopen: the old records and the new write both persisted.
    let s = GlobalStore::open(&path).unwrap();
    let mut aliases: Vec<String> = s
        .list_hosts()
        .unwrap()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    aliases.sort();
    assert_eq!(aliases, ["home-server", "new-box"]);
    assert!(s.get_connection("acme-pg").unwrap().is_some());
}
