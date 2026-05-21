use sid_core::adapters::db_client::DbKind;
use sid_store::{DbConnection, OpenStore, RedbStore, Store, now_epoch};
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, RedbStore) {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    (d, s)
}

fn conn(id: &str, name: &str) -> DbConnection {
    DbConnection {
        id: id.into(),
        kind: DbKind::Sqlite,
        name: name.into(),
        dsn: ":memory:".into(),
        secret_ref: None,
        created_at: now_epoch(),
    }
}

#[test]
fn upsert_then_list_returns_connection() {
    let (_dir, s) = store();
    s.upsert_db_connection(&conn("a", "alpha")).unwrap();
    let all = s.list_db_connections().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, "a");
}

#[test]
fn get_and_remove_work() {
    let (_d, s) = store();
    s.upsert_db_connection(&conn("a", "x")).unwrap();
    assert!(s.get_db_connection("a").unwrap().is_some());
    s.remove_db_connection("a").unwrap();
    assert!(s.get_db_connection("a").unwrap().is_none());
}

#[test]
fn upsert_replaces_existing() {
    let (_d, s) = store();
    s.upsert_db_connection(&conn("a", "v1")).unwrap();
    s.upsert_db_connection(&conn("a", "v2")).unwrap();
    assert_eq!(s.get_db_connection("a").unwrap().unwrap().name, "v2");
}

#[test]
fn list_with_50_connections_returns_all() {
    let (_d, s) = store();
    for i in 0..50 {
        s.upsert_db_connection(&conn(&format!("c{i}"), &format!("n{i}")))
            .unwrap();
    }
    assert_eq!(s.list_db_connections().unwrap().len(), 50);
}

#[test]
fn get_missing_returns_none() {
    let (_d, s) = store();
    assert!(s.get_db_connection("absent").unwrap().is_none());
}

#[test]
fn remove_absent_is_noop() {
    let (_d, s) = store();
    s.remove_db_connection("never").unwrap();
}
