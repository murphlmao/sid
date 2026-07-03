//! Docker-TimescaleDB integration tests for BUG 3: TimescaleDB's internal
//! `_timescaledb_internal`/`_timescaledb_catalog`/`_timescaledb_config`/
//! `timescaledb_information`/`timescaledb_experimental` schemas (and the
//! per-chunk FK/PK constraints Timescale creates under
//! `_timescaledb_internal`) used to leak into `schema_introspect` and
//! `schema_graph` as though they were user tables/edges. `#[ignore]`d so the
//! default `cargo test --workspace` stays fast/Docker-free.
//!
//! Prereqs: `docker/docker-compose.test.yml`'s `timescale` service running,
//! seeded with `docker/timescale-init/01-schema.sql` (see
//! `scripts/test-db-matrix.sh`).
//!
//! Run manually:
//!   docker compose -f docker/docker-compose.test.yml up -d timescale
//!   cargo test -p sid-db --test postgres_timescale -- --ignored --test-threads=1
//!   docker compose -f docker/docker-compose.test.yml down -v

use sid_core::db::{DbClient, DbKind, OpenParams};
use sid_db::PostgresClient;

fn test_dsn() -> String {
    std::env::var("SID_TEST_TIMESCALE_DSN").unwrap_or_else(|_| {
        "postgres://sid_test:sid_test_pw@localhost:55433/sid_test?sslmode=disable".to_string()
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
        .expect("connect to dockerized timescaledb (is `docker compose -f docker/docker-compose.test.yml up -d timescale` running?)")
}

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's timescale service; run via scripts/test-db-matrix.sh"]
async fn schema_introspect_excludes_timescale_internals() {
    let client = open_client().await;
    let schema = client.schema_introspect().await.expect("schema_introspect");

    let leaked: Vec<String> = schema
        .tables
        .iter()
        .filter(|t| {
            t.schema.as_deref().is_some_and(|s| {
                s.starts_with("_timescaledb")
                    || s == "timescaledb_information"
                    || s == "timescaledb_experimental"
            })
        })
        .map(|t| format!("{}.{}", t.schema.as_deref().unwrap_or(""), t.name))
        .collect();
    assert!(
        leaked.is_empty(),
        "TimescaleDB internal objects leaked into schema_introspect: {leaked:?}"
    );

    let devices = schema
        .tables
        .iter()
        .find(|t| t.schema.as_deref() == Some("public") && t.name == "devices")
        .expect("public.devices (the regular table) must be present");
    assert_eq!(
        devices
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name"]
    );

    let metrics = schema
        .tables
        .iter()
        .find(|t| t.schema.as_deref() == Some("public") && t.name == "metrics")
        .expect("public.metrics (the hypertable) must be present, with its real columns");
    assert_eq!(
        metrics
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["time", "device_id", "value"]
    );
}

#[tokio::test]
#[ignore = "requires docker/docker-compose.test.yml's timescale service; run via scripts/test-db-matrix.sh"]
async fn schema_graph_has_hypertable_pk_fk_with_no_duplicate_edges() {
    let client = open_client().await;
    let graph = client.schema_graph().await.expect("schema_graph");

    let leaked_fks: Vec<_> = graph
        .foreign_keys
        .iter()
        .filter(|fk| {
            fk.from_table.starts_with("_timescaledb") || fk.to_table.starts_with("_timescaledb")
        })
        .collect();
    assert!(
        leaked_fks.is_empty(),
        "TimescaleDB internal FK edges leaked: {leaked_fks:?}"
    );

    let leaked_pks: Vec<_> = graph
        .primary_keys
        .keys()
        .filter(|k| k.starts_with("_timescaledb"))
        .collect();
    assert!(
        leaked_pks.is_empty(),
        "TimescaleDB internal PK entries leaked: {leaked_pks:?}"
    );

    // The hypertable's real, composite PK (partition column + device_id) —
    // resolved to the DECLARED table, not one of its internal chunks.
    assert_eq!(
        graph.primary_keys.get("public.metrics"),
        Some(&vec!["time".to_string(), "device_id".to_string()])
    );

    // Exactly ONE metrics -> devices FK edge — Timescale's per-chunk inherited
    // constraints (in `_timescaledb_internal`) would otherwise show up as
    // additional near-identical edges; excluding that schema removes them.
    let metrics_to_devices: Vec<_> = graph
        .foreign_keys
        .iter()
        .filter(|fk| fk.from_table == "public.metrics" && fk.to_table == "public.devices")
        .collect();
    assert_eq!(
        metrics_to_devices.len(),
        1,
        "exactly one metrics->devices FK edge, no per-chunk duplicates: {metrics_to_devices:?}"
    );
    assert_eq!(
        metrics_to_devices[0].from_columns,
        vec!["device_id".to_string()]
    );
    assert_eq!(metrics_to_devices[0].to_columns, vec!["id".to_string()]);
}
