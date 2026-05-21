use sid_core::adapters::db_client::DbKind;
use sid_core::adapters::secrets::SecretId;
use sid_store::{DbConnection, PlainSecret, QueryRecord, now_epoch};

#[test]
fn db_connection_construction() {
    let c = DbConnection {
        id: "local-pg".into(),
        kind: DbKind::Postgres,
        name: "local postgres".into(),
        dsn: "postgres://user@localhost/db".into(),
        secret_ref: Some(SecretId::new("local-pg.password")),
        created_at: now_epoch(),
    };
    assert_eq!(c.kind, DbKind::Postgres);
}

#[test]
fn query_record_construction() {
    let r = QueryRecord {
        conn_id: "local-pg".into(),
        sql: "SELECT 1".into(),
        duration_ms: 12,
        row_count: 1,
        ts_ns: 1,
    };
    assert_eq!(r.row_count, 1);
}

#[test]
fn plain_secret_construction() {
    let s = PlainSecret {
        value: "shh".into(),
    };
    assert_eq!(s.value, "shh");
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_db_connection_postcard_roundtrip(name in "[a-zA-Z0-9 -]{1,40}") {
        let c = DbConnection {
            id: name.clone(),
            kind: DbKind::Sqlite,
            name,
            dsn: "/tmp/x.db".into(),
            secret_ref: None,
            created_at: now_epoch(),
        };
        let bytes = postcard::to_allocvec(&c).unwrap();
        let back: DbConnection = postcard::from_bytes(&bytes).unwrap();
        prop_assert_eq!(c, back);
    }

    #[test]
    fn prop_query_record_postcard_roundtrip(ts in any::<u64>(), n in any::<u64>()) {
        let r = QueryRecord {
            conn_id: "x".into(),
            sql: "SELECT 1".into(),
            duration_ms: 1,
            row_count: n,
            ts_ns: ts as u128,
        };
        let bytes = postcard::to_allocvec(&r).unwrap();
        let back: QueryRecord = postcard::from_bytes(&bytes).unwrap();
        prop_assert_eq!(r, back);
    }

    #[test]
    fn prop_plain_secret_postcard_roundtrip(v in ".{0,200}") {
        let s = PlainSecret { value: v };
        let bytes = postcard::to_allocvec(&s).unwrap();
        let back: PlainSecret = postcard::from_bytes(&bytes).unwrap();
        prop_assert_eq!(s, back);
    }
}
