//! Docker-Postgres integration test for BUG 1: `render_pg_value` used to
//! silently render present-but-undecodable values as the string "NULL"
//! (indistinguishable from a genuine SQL NULL). `#[ignore]`d so the default
//! `cargo test --workspace` stays fast/Docker-free.
//!
//! Prereqs: `docker/docker-compose.test.yml`'s `postgres` service running,
//! seeded with `docker/pg-init/02-rich-types.sql` (see
//! `scripts/test-db-matrix.sh`).
//!
//! Run manually:
//!   docker compose -f docker/docker-compose.test.yml up -d postgres
//!   cargo test -p sid-db --test postgres_rich_types -- --ignored
//!   docker compose -f docker/docker-compose.test.yml down -v

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
async fn rich_types_real_values_render_and_nulls_stay_distinguishable() {
    let client = open_client().await;
    let page = client
        .query_paged(
            "SELECT id, amount, created_at, payload, tags, active, nickname, duration \
             FROM public.rich_types ORDER BY nickname NULLS LAST",
            None,
            10,
        )
        .await
        .expect("query_paged rich_types");
    assert_eq!(page.rows.len(), 2);

    // Row 0 = 'row-one' — every column populated with a real, non-null value.
    let r0 = &page.rows[0];
    assert_eq!(r0.values[0], "11111111-1111-1111-1111-111111111111", "uuid");
    assert_eq!(r0.values[1], "123.45", "numeric");
    assert!(
        r0.values[2].contains("2026-01-15"),
        "timestamptz should render the real date, got {:?}",
        r0.values[2]
    );
    assert_eq!(r0.values[3], "{\"k\":\"v\"}", "jsonb");
    assert_eq!(r0.values[4], "{a,b,c}", "text[]");
    assert_eq!(r0.values[5], "true", "bool");
    assert_eq!(r0.values[6], "row-one", "text");
    // `duration` (INTERVAL) has no dedicated arm and isn't String-decodable —
    // THE LOAD-BEARING ASSERTION: a present value must render the distinct
    // undecodable marker, never "NULL".
    assert_eq!(
        r0.values[7], "⟨interval?⟩",
        "undecodable-but-present marker"
    );

    // Row 1 — the NULL-heavy row. id/amount/created_at/active/duration are
    // NOT NULL and hold real (if boundary-ish) values — none of these may
    // render "NULL" just because they look like zero/empty values.
    let r1 = &page.rows[1];
    assert_eq!(r1.values[0], "00000000-0000-0000-0000-000000000000");
    assert_ne!(
        r1.values[0], "NULL",
        "all-zeroes uuid is a REAL value, not NULL"
    );
    assert_eq!(r1.values[1], "0.00");
    assert_ne!(
        r1.values[1], "NULL",
        "zero numeric is a REAL value, not NULL"
    );
    assert!(
        r1.values[2].contains("1970-01-01"),
        "epoch timestamptz, got {:?}",
        r1.values[2]
    );
    assert_ne!(
        r1.values[2], "NULL",
        "epoch timestamptz is a REAL value, not NULL"
    );
    assert_eq!(r1.values[5], "false");
    assert_ne!(r1.values[5], "NULL", "false is a REAL value, not NULL");
    assert_eq!(
        r1.values[7], "⟨interval?⟩",
        "present-but-undecodable, not NULL"
    );

    // payload/tags/nickname are genuinely NULL in this row — "NULL" IS correct.
    assert_eq!(r1.values[3], "NULL", "payload is a genuine SQL NULL here");
    assert_eq!(r1.values[4], "NULL", "tags is a genuine SQL NULL here");
    assert_eq!(r1.values[6], "NULL", "nickname is a genuine SQL NULL here");
}
