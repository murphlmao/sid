use std::sync::Arc;

use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams};
use sid_db_clients::SqliteClient;

async fn open_mem() -> Arc<dyn DbClient> {
    SqliteClient::factory()
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: ":memory:".into(),
            password: None,
        })
        .await
        .expect("open in-memory")
}

#[tokio::test]
async fn create_table_returns_zero_rows_affected() {
    let c = open_mem().await;
    let r = c
        .execute("CREATE TABLE t (id INTEGER, name TEXT)")
        .await
        .unwrap();
    assert_eq!(r.rows_affected, 0);
}

#[tokio::test]
async fn insert_returns_correct_rows_affected() {
    let c = open_mem().await;
    c.execute("CREATE TABLE t (id INTEGER)").await.unwrap();
    let r = c
        .execute("INSERT INTO t VALUES (1), (2), (3)")
        .await
        .unwrap();
    assert_eq!(r.rows_affected, 3);
}

#[tokio::test]
async fn syntax_error_returns_syntax_or_query_error() {
    let c = open_mem().await;
    let res = c.execute("CREATE TABL t (id INTEGER)").await;
    let err = match res {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("syntax") || msg.to_lowercase().contains("near"),
        "got: {msg}"
    );
}

#[tokio::test]
async fn execute_records_duration() {
    let c = open_mem().await;
    let r = c.execute("CREATE TABLE t (id INTEGER)").await.unwrap();
    assert!(r.duration_ms < 60_000);
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_insert_count_matches_rows_affected(n in 1u32..50) {
        let r = tokio::runtime::Runtime::new().unwrap().block_on(async move {
            let c = open_mem().await;
            c.execute("CREATE TABLE t (id INTEGER)").await.unwrap();
            let values = (0..n).map(|i| format!("({i})")).collect::<Vec<_>>().join(",");
            c.execute(&format!("INSERT INTO t VALUES {values}")).await.unwrap()
        });
        prop_assert_eq!(r.rows_affected, n as u64);
    }
}
