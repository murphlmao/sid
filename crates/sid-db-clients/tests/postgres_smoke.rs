//! Postgres smoke tests. The "no live DB" tests run unconditionally; the live
//! tests require SID_PG_DSN to be set, otherwise they early-return.

use sid_core::adapters::db_client::{DbKind, OpenParams};
use sid_db_clients::PostgresClient;

fn dsn_or_skip() -> Option<String> {
    std::env::var("SID_PG_DSN").ok()
}

#[tokio::test]
async fn open_with_bad_dsn_returns_connect_error() {
    let factory = PostgresClient::factory();
    let res = factory
        .open(OpenParams {
            kind: DbKind::Postgres,
            dsn: "postgres://invalid:bad@127.0.0.1:1/none".into(),
            password: None,
        })
        .await;
    let err = match res {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(matches!(
        err,
        sid_core::adapters::db_client::DbError::Connect(_)
    ));
}

#[tokio::test]
async fn open_with_sqlite_kind_returns_invalid() {
    let factory = PostgresClient::factory();
    let res = factory
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: "postgres://x".into(),
            password: None,
        })
        .await;
    let err = match res {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(matches!(
        err,
        sid_core::adapters::db_client::DbError::Invalid(_)
    ));
}

#[cfg(feature = "pg-it")]
#[tokio::test]
async fn open_close_against_live_postgres() {
    let Some(dsn) = dsn_or_skip() else {
        eprintln!("SID_PG_DSN not set — skipping");
        return;
    };
    let factory = PostgresClient::factory();
    let c = factory
        .open(OpenParams {
            kind: DbKind::Postgres,
            dsn,
            password: None,
        })
        .await
        .expect("open");
    assert_eq!(c.kind(), DbKind::Postgres);
    c.close().await.unwrap();
}

#[allow(dead_code)]
fn _force_use_dsn_or_skip() {
    let _ = dsn_or_skip();
}
