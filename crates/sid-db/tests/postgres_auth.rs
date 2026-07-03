//! Docker-Postgres integration test for BUG 4: a wrong-password connect used
//! to map to the generic `DbError::Connect` instead of the more specific
//! `DbError::Auth` (SQLSTATE `28P01`/`28000`). `#[ignore]`d so the default
//! `cargo test --workspace` stays fast/Docker-free.
//!
//! Prereqs: `docker/docker-compose.test.yml`'s `postgres` service running
//! (see `scripts/test-db-matrix.sh`).
//!
//! Run manually:
//!   docker compose -f docker/docker-compose.test.yml up -d postgres
//!   cargo test -p sid-db --test postgres_auth -- --ignored
//!   docker compose -f docker/docker-compose.test.yml down -v

use sid_core::db::{DbError, DbKind, OpenParams};
use sid_db::PostgresClient;

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-db-matrix.sh"]
async fn wrong_password_maps_to_db_error_auth() {
    let dsn = std::env::var("SID_TEST_PG_WRONG_PW_DSN").unwrap_or_else(|_| {
        "postgres://sid_test:totally_wrong_password@localhost:55432/sid_test?sslmode=disable"
            .to_string()
    });
    let factory = PostgresClient::factory();
    let result = factory
        .open(OpenParams {
            kind: DbKind::Postgres,
            dsn,
            password: None,
            sqlite_mode: None,
        })
        .await;
    match result {
        Ok(_) => panic!("wrong password must not succeed"),
        Err(DbError::Auth) => {}
        Err(other) => panic!("expected DbError::Auth, got {other:?}"),
    }
}
