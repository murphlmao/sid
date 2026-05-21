use sid_store::{OpenStore, QueryRecord, RedbStore, Store};
use tempfile::tempdir;

fn rec(conn: &str, sql: &str, ts: u128) -> QueryRecord {
    QueryRecord {
        conn_id: conn.into(),
        sql: sql.into(),
        duration_ms: 1,
        row_count: 0,
        ts_ns: ts,
    }
}

#[test]
fn append_and_recent_returns_most_recent_first() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    s.append_query_record(&rec("c1", "SELECT 1", 1)).unwrap();
    s.append_query_record(&rec("c1", "SELECT 2", 2)).unwrap();
    let got = s.recent_queries("c1", 10).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].sql, "SELECT 2");
    assert_eq!(got[1].sql, "SELECT 1");
}

#[test]
fn recent_respects_limit() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    for i in 0..20u128 {
        s.append_query_record(&rec("c1", &format!("Q{i}"), i + 1))
            .unwrap();
    }
    let got = s.recent_queries("c1", 5).unwrap();
    assert_eq!(got.len(), 5);
    assert_eq!(got[0].sql, "Q19");
}

#[test]
fn recent_filters_by_connection_id() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    s.append_query_record(&rec("a", "A", 1)).unwrap();
    s.append_query_record(&rec("b", "B", 2)).unwrap();
    s.append_query_record(&rec("a", "A2", 3)).unwrap();
    let got = s.recent_queries("a", 10).unwrap();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|r| r.conn_id == "a"));
}

#[test]
fn recent_empty_when_no_records() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    assert!(s.recent_queries("nope", 10).unwrap().is_empty());
}
