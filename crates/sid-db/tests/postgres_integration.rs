//! Docker-Postgres integration tests for `PostgresClient` (Plan: sid
//! integration/automation harness, 2026-07-02). `#[ignore]`d so the default
//! `cargo test --workspace` stays fast/green without Docker.
//!
//! These go through the public `sid_core::db::DbClient` trait only (never
//! `PostgresClient`'s internals) — the same seam the Database tab uses.
//!
//! Prereqs: `docker/docker-compose.test.yml`'s `postgres` service running
//! (`scripts/test-integration.sh` brings it up, seeds the FK-rich fixture
//! schema from `docker/pg-init/01-schema.sql`, runs these, tears down).
//!
//! Run manually:
//!   docker compose -f docker/docker-compose.test.yml up -d postgres
//!   cargo test -p sid-db --test postgres_integration -- --ignored --test-threads=1
//!   docker compose -f docker/docker-compose.test.yml down -v
//!
//! `--test-threads=1`: the fixture schema is shared (no per-test isolation);
//! tests are read-only against it except `execute_ddl_dml_round_trip`, which
//! creates/drops its own scratch table so it never collides with the others.

use std::collections::BTreeMap;

use sid_core::db::{DbClient, DbKind, ForeignKey, OpenParams};
use sid_db::PostgresClient;

/// DSN for the `postgres` service in `docker/docker-compose.test.yml`.
/// Host `localhost` + explicit `sslmode=disable` — `PostgresClient`'s
/// `tls_choice` (crates/sid-db/src/postgres.rs) treats any loopback host with
/// `sslmode=disable`/`prefer` as `TlsChoice::Plain`, so this exercises the
/// plaintext local-connect path deliberately, not by accident.
///
/// NOTE on TLS coverage: a full `sslmode=require`/`verify-full` round-trip
/// against this container is NOT covered here — it would need a certificate
/// chain trusted for the `localhost` hostname (rustls only supports
/// verify-full; see `build_rustls_connector`'s doc comment), which is more
/// PKI machinery than a throwaway test container warrants. The `tls_choice`
/// decision function itself (remote-always-TLS, local-honors-sslmode) is
/// fully covered by the pure unit tests already in `postgres.rs`; this file
/// only proves the *plaintext local* leg actually round-trips against a real
/// server. See docs/design/2026-07-02-testing-strategy.md.
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
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-integration.sh"]
async fn sslmode_disable_local_connect_and_query_paged_returns_rows() {
    let client = open_client().await;
    let page = client
        .query_paged(
            "SELECT id, name FROM public.customers ORDER BY id",
            None,
            10,
        )
        .await
        .expect("query_paged");
    assert_eq!(page.columns.len(), 2);
    assert_eq!(page.columns[0].name, "id");
    assert_eq!(page.columns[1].name, "name");
    assert_eq!(page.rows.len(), 2, "fixture seeds exactly 2 customers");
    assert_eq!(page.rows[0].values[1], "Ada Lovelace");
    assert_eq!(page.rows[1].values[1], "Grace Hopper");
    assert!(page.next_cursor.is_none(), "2 rows < page_size(10)");
}

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-integration.sh"]
async fn query_paged_pagination_walks_all_rows_via_cursor() {
    let client = open_client().await;
    // page_size=1 over the 3-row `orders` fixture forces multiple pages. The
    // offset-window contract (postgres.rs's `query_paged`) can't know a page
    // is the last one until it sees a *short* page — so a full page (fetched
    // == page_size) always carries a next_cursor, and the terminal signal is
    // an empty page with next_cursor=None. That's the documented contract
    // (`QueryPage::next_cursor`: "None at end-of-stream"), not a bug.
    let mut cursor = None;
    let mut seen = Vec::new();
    for _ in 0..10 {
        let page = client
            .query_paged("SELECT id FROM public.orders ORDER BY id", cursor, 1)
            .await
            .expect("query_paged page");
        if page.rows.is_empty() {
            assert!(page.next_cursor.is_none(), "an empty page must be terminal");
            break;
        }
        seen.push(page.rows[0].values[0].clone());
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, vec!["1", "2", "3"], "fixture seeds exactly 3 orders");
}

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-integration.sh"]
async fn execute_ddl_dml_round_trip() {
    let client = open_client().await;
    client
        .execute("CREATE TABLE public.sid_it_scratch (id INTEGER)")
        .await
        .expect("create scratch table");
    let insert = client
        .execute("INSERT INTO public.sid_it_scratch VALUES (1), (2), (3)")
        .await
        .expect("insert");
    assert_eq!(insert.rows_affected, 3);
    client
        .execute("DROP TABLE public.sid_it_scratch")
        .await
        .expect("drop scratch table");
}

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-integration.sh"]
async fn schema_introspect_lists_fixture_tables_and_columns() {
    let client = open_client().await;
    let schema = client.schema_introspect().await.expect("schema_introspect");

    let by_key: BTreeMap<(Option<String>, String), Vec<String>> = schema
        .tables
        .into_iter()
        .map(|t| {
            (
                (t.schema, t.name),
                t.columns.into_iter().map(|c| c.name).collect(),
            )
        })
        .collect();

    assert_eq!(
        by_key.get(&(Some("public".into()), "customers".into())),
        Some(&vec!["id".to_string(), "name".to_string()])
    );
    assert_eq!(
        by_key.get(&(Some("public".into()), "bins".into())),
        Some(&vec![
            "warehouse_id".to_string(),
            "bin_id".to_string(),
            "label".to_string()
        ])
    );
    assert_eq!(
        by_key.get(&(Some("public".into()), "orders".into())),
        Some(&vec![
            "id".to_string(),
            "customer_id".to_string(),
            "warehouse_id".to_string(),
            "bin_id".to_string(),
        ])
    );
    // The schema-qualified table (docker/pg-init/01-schema.sql's `billing`
    // namespace) must show up too — proves `information_schema.columns`'
    // `table_schema` filter isn't accidentally collapsing to `public` only.
    assert_eq!(
        by_key.get(&(Some("billing".into()), "invoices".into())),
        Some(&vec![
            "id".to_string(),
            "order_id".to_string(),
            "amount".to_string()
        ])
    );
}

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-integration.sh"]
async fn schema_graph_matches_fixture_exactly() {
    // This is the gap the demo SQLite fixture can't cover: a live
    // `pg_catalog` walk producing composite FKs and a cross-schema edge.
    let client = open_client().await;
    let graph = client.schema_graph().await.expect("schema_graph");

    let expected_fks = vec![
        // Cross-schema edge: billing.invoices -> public.orders.
        ForeignKey {
            from_table: "billing.invoices".into(),
            from_columns: vec!["order_id".into()],
            to_table: "public.orders".into(),
            to_columns: vec!["id".into()],
        },
        // Single-column FK: public.bins.warehouse_id -> public.warehouses.id.
        ForeignKey {
            from_table: "public.bins".into(),
            from_columns: vec!["warehouse_id".into()],
            to_table: "public.warehouses".into(),
            to_columns: vec!["id".into()],
        },
        // Composite FK: public.orders.(warehouse_id, bin_id) -> public.bins.
        ForeignKey {
            from_table: "public.orders".into(),
            from_columns: vec!["warehouse_id".into(), "bin_id".into()],
            to_table: "public.bins".into(),
            to_columns: vec!["warehouse_id".into(), "bin_id".into()],
        },
        // Single-column FK: public.orders.customer_id -> public.customers.id.
        ForeignKey {
            from_table: "public.orders".into(),
            from_columns: vec!["customer_id".into()],
            to_table: "public.customers".into(),
            to_columns: vec!["id".into()],
        },
    ];
    assert_eq!(
        graph.foreign_keys, expected_fks,
        "schema_graph's live pg_catalog walk must match the fixture's declared FKs exactly \
         (deterministic order: by (from_table, to_table, from_columns))"
    );

    assert_eq!(
        graph.primary_keys.get("public.customers"),
        Some(&vec!["id".to_string()])
    );
    assert_eq!(
        graph.primary_keys.get("public.warehouses"),
        Some(&vec!["id".to_string()])
    );
    assert_eq!(
        graph.primary_keys.get("public.bins"),
        Some(&vec!["warehouse_id".to_string(), "bin_id".to_string()]),
        "composite PK must preserve declared column order"
    );
    assert_eq!(
        graph.primary_keys.get("public.orders"),
        Some(&vec!["id".to_string()])
    );
    assert_eq!(
        graph.primary_keys.get("billing.invoices"),
        Some(&vec!["id".to_string()])
    );
}

// ---------------------------------------------------------------------
// Round-D bug hunt: trailing SQL comments used to swallow `query_paged`'s own
// `sid_sub` wrapper tail (adopted from the round-D probe branch's
// `probe_roundd.rs` — see crates/sid-db/src/lexer.rs's `strip_trailing_trivia`
// and `PostgresClient::query_paged`'s round-D fix comment).
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-integration.sh"]
async fn trailing_line_comment_no_longer_swallows_the_wrapper_tail() {
    // Previously: `query_paged` appended its `) AS sid_sub LIMIT .. OFFSET ..` tail on
    // the SAME line as the end of the caller's (trimmed) SQL. A `--` line comment as the
    // last content of otherwise-valid SQL ate that tail and produced a misleading
    // "syntax error at end of input" — a very ordinary pattern (SQL pasted from docs/an
    // ORM, or a commented-out clause left at the end of a query). Fixed by putting the
    // wrapper tail on its own line.
    let client = open_client().await;
    let sql = "SELECT 1 AS one\n-- trailing comment";
    let page = client
        .query_paged(sql, None, 10)
        .await
        .expect("a trailing line comment must not break the sid_sub wrapper");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].values[0], "1");
}

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's postgres service; run via scripts/test-integration.sh"]
async fn trailing_semicolon_then_line_comment_no_longer_swallows_the_wrapper_tail() {
    // Same root cause via the common paste artifact of a trailing `;` followed by a
    // comment: the old `trim_end_matches(';')` doesn't understand SQL at all, so it left
    // the embedded `;` in place (itself a syntax error inside the wrapper's subquery
    // parens) AND the trailing comment still ate the wrapper tail. Fixed by
    // `strip_trailing_trivia` (lexer-backed: strips the `;` and the comment after it)
    // plus the tail-on-its-own-line belt-and-braces change.
    let client = open_client().await;
    let sql = "SELECT 1 AS one; -- trailing comment after semicolon";
    let page = client
        .query_paged(sql, None, 10)
        .await
        .expect("a trailing `; -- comment` must not break the sid_sub wrapper");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].values[0], "1");
}
