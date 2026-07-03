//! Docker-Postgres integration test for BUG 2: `cancel()` used to deadlock
//! behind the SAME query mutex `query_paged`/`execute` hold for the whole
//! duration of a live query, so it could never run until the query it was
//! meant to interrupt had already finished. `#[ignore]`d so the default
//! `cargo test --workspace` stays fast/Docker-free.
//!
//! Prereqs: `docker/docker-compose.test.yml`'s `postgres` service running
//! (see `scripts/test-db-matrix.sh`).
//!
//! Run manually:
//!   docker compose -f docker/docker-compose.test.yml up -d postgres
//!   cargo test -p sid-db --test postgres_cancel -- --ignored
//!   docker compose -f docker/docker-compose.test.yml down -v

use std::time::Duration;

use sid_core::db::{DbClient, DbKind, OpenParams};
use sid_db::PostgresClient;

fn test_dsn() -> String {
    std::env::var("SID_TEST_PG_DSN").unwrap_or_else(|_| {
        "postgres://sid_test:sid_test_pw@localhost:55432/sid_test?sslmode=disable".to_string()
    })
}

async fn open_client() -> std::sync::Arc<dyn DbClient> {
    let factory = PostgresClient::factory();
    factory
        .open(OpenParams {
            kind: DbKind::Postgres,
            dsn: test_dsn(),
            password: None,
            sqlite_mode: None,
        })
        .await
        .expect("connect to dockerized postgres (is `docker compose -f docker/docker-compose.test.yml up -d postgres` running?)")
}

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-db-matrix.sh"]
async fn cancel_interrupts_an_inflight_query_without_waiting_for_it() {
    let client = open_client().await;
    let client2 = client.clone();

    let start = std::time::Instant::now();
    let query_task =
        tokio::spawn(async move { client2.query_paged("SELECT pg_sleep(3)", None, 10).await });

    tokio::time::sleep(Duration::from_millis(400)).await;
    let before_cancel = start.elapsed();
    let cancel_result = client.cancel().await;
    let cancel_elapsed = start.elapsed() - before_cancel;

    // THE LOAD-BEARING ASSERTION (BUG 2): cancel() used to block on the same
    // mutex the live query holds for its whole 3s duration — it couldn't even
    // START running until the query finished. It must now return almost
    // immediately, well before the 3s sleep completes.
    assert!(
        cancel_elapsed < Duration::from_secs(1),
        "cancel() took {cancel_elapsed:?} to return -- looks like it's still \
         blocked on the query mutex"
    );
    cancel_result.expect("cancel() itself should succeed");

    let query_result = query_task.await.expect("join query task");
    let total_elapsed = start.elapsed();
    assert!(
        total_elapsed < Duration::from_millis(2500),
        "query_paged(pg_sleep(3)) took {total_elapsed:?} -- ran to completion, \
         cancel did not interrupt it"
    );
    assert!(
        query_result.is_err(),
        "a cancelled query must error, not return Ok: {query_result:?}"
    );
}
