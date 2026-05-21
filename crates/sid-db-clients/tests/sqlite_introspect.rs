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
        .unwrap()
}

#[tokio::test]
async fn empty_db_has_no_tables() {
    let c = open_mem().await;
    let s = c.schema_introspect().await.unwrap();
    assert!(s.tables.is_empty());
}

#[tokio::test]
async fn schema_lists_user_table_with_columns() {
    let c = open_mem().await;
    c.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL, age INTEGER)")
        .await
        .unwrap();
    let s = c.schema_introspect().await.unwrap();
    assert_eq!(s.tables.len(), 1);
    let t = &s.tables[0];
    assert_eq!(t.name, "users");
    let names: Vec<_> = t.columns.iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, vec!["id", "email", "age"]);
}

#[tokio::test]
async fn cancel_is_noop_and_succeeds() {
    let c = open_mem().await;
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn schema_ignores_sqlite_internal_tables() {
    let c = open_mem().await;
    c.execute("CREATE TABLE foo (id INTEGER)").await.unwrap();
    let s = c.schema_introspect().await.unwrap();
    assert!(s.tables.iter().all(|t| !t.name.starts_with("sqlite_")));
}

#[tokio::test]
async fn schema_table_with_reserved_name_round_trips() {
    let c = open_mem().await;
    c.execute(r#"CREATE TABLE "select" (id INTEGER)"#)
        .await
        .unwrap();
    let s = c.schema_introspect().await.unwrap();
    assert!(s.tables.iter().any(|t| t.name == "select"));
}
