use sid_core::adapters::db_client::{DbKind, OpenParams};
use sid_db_clients::SqliteClient;

#[tokio::test]
async fn open_in_memory_succeeds() {
    let factory = SqliteClient::factory();
    let client = factory
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: ":memory:".into(),
            password: None,
        })
        .await
        .expect("open in-memory");
    assert_eq!(client.kind(), DbKind::Sqlite);
    client.close().await.unwrap();
}

#[tokio::test]
async fn open_file_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let factory = SqliteClient::factory();
    let client = factory
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: path.to_string_lossy().into_owned(),
            password: None,
        })
        .await
        .unwrap();
    assert!(path.exists(), "SQLite should create the file on open");
    client.close().await.unwrap();
}

#[tokio::test]
async fn open_with_postgres_kind_fails() {
    let factory = SqliteClient::factory();
    let res = factory
        .open(OpenParams {
            kind: DbKind::Postgres,
            dsn: ":memory:".into(),
            password: None,
        })
        .await;
    let err = match res {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    let msg = format!("{err}");
    assert!(msg.contains("invalid") || msg.contains("kind"));
}

#[tokio::test]
async fn open_path_with_invalid_directory_returns_connect_error() {
    let factory = SqliteClient::factory();
    let res = factory
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: "/nonexistent/dir/foo.db".into(),
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
async fn open_with_unicode_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("\u{1f415}-sid.db");
    let factory = SqliteClient::factory();
    let _client = factory
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: path.to_string_lossy().into_owned(),
            password: None,
        })
        .await
        .unwrap();
    assert!(path.exists());
}
