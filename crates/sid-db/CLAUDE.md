# sid-db (database adapters)

`DbClient` / `DbClientDescriptor` implementations. The only crate that names `tokio_postgres`
and `rusqlite`.

## Surface
`PostgresClient` (+`PostgresDescriptor`), `SqliteClient` (+`SqliteDescriptor`, `spawn_blocking`),
`redb_browse::RedbBrowseClient` (read-only pseudo-engine over a `sid_store::GlobalStore`),
`demo` (seeded FK-rich SQLite), `lexer` (SQL highlighting). Each client exposes `factory()`;
the app registers them in `crates/sid/src/db_registry.rs`.

## Invariants
- All public types route through `sid_core::db`; frontends never name these concrete types.
- Postgres/Timescale value decoding is hardened — unknown types degrade to text, never panic.
- Redb browse is read-only.

## Test
`nice -n 10 cargo test -p sid-db -j 3` (default suite, includes `explain_matrix.rs`).
Docker-backed `#[ignore]`d suites: `scripts/test-integration.sh` (`just test-db`; Postgres,
`postgres_integration`), `scripts/test-db-matrix.sh` (Postgres + TimescaleDB: `postgres_auth`,
`postgres_cancel`, `postgres_rich_types`, `postgres_timescale`). They run `--test-threads=1`.

## Landmines
- Depends on `sid-store` (for redb browse) — don't add more reverse edges.
- Use a unique `COMPOSE_PROJECT_NAME` for docker runs and tear down.
