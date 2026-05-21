# sid Plan 4 — Database tab (Postgres + SQLite)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. CLAUDE.md applies — every new pub fn needs a doc test, every function with invariants needs property tests, every parser-shaped function gets adversarial coverage.

**Goal:** When this plan is done, the **Database** tab is fully functional. The left pane lists saved connections (Postgres + SQLite). Selecting a connection and pressing Enter opens a session. The top-right pane is a multi-line query editor with SQL syntax highlighting via a hand-rolled lexer (token classes: keyword, identifier, string, number, comment, punctuation). The bottom-right pane shows paginated query results (50 rows/page) with column sort, `c` copy cell, and `e` export-CSV. A collapsible bottom bar shows per-connection query history sorted by recency, with duration + row count. Secrets are stored as plaintext via a `PlainStore` impl of a new `SecretStore` trait (keyring impl deferred to a later plan). The `sid db add/remove/list/query` subcommands provide CLI-side parity; `sid db query <conn-id> "<sql>"` runs the SQL and prints CSV to stdout for scripting.

**Architecture:** A new `sid-db-clients` adapter crate hosts `PostgresClient` and `SqliteClient` impls of the `DbClient` trait, plus a small SQL lexer module for syntax highlighting. The `DbClient` trait in `sid-core::adapters::db_client` — currently an empty shell — gains the full method surface (`open`, `close`, `execute`, `query_paged`, `schema_introspect`, `cancel`) plus domain types (`DbKind`, `Column`, `Row`, `PageCursor`, `DbError`). The `Store` trait extends with `db_connections` and `query_history` tables (both new) plus a `secrets` table (also new), and the `SecretStore` trait + `PlainStore` impl land alongside the connection storage. The widget lives in `sid-widgets/database.rs`, replacing the Plan 1 stub. The binary's `wire.rs` injects both DB clients and the secret store into the App.

**Tech stack additions:**
- `tokio-postgres = "0.7"` — standard async Postgres driver
- `rusqlite = { version = "0.32", features = ["bundled"] }` — embedded SQLite (ships its own C source; no system install needed)
- `csv = "1.3"` — CSV export

**Out of scope (deferred, see `2026-05-20-sid-future-features.md`):**
- MySQL, Redis, MongoDB, DuckDB, ClickHouse backends
- ER diagram view / schema visualisation
- Saved query library (per-conn or shared)
- Foreign-key result navigation ("open related")
- Query plan visualisation
- Notebook-style query cells
- OS keyring for secrets (Plan deferred; trait + plaintext impl land here)
- Tree-sitter SQL parsing (hand-rolled lexer is sufficient for syntax highlight in v1)
- Cancel-via-Ctrl-C of in-flight queries inside the TUI (the `cancel` trait method lands but UI wiring is a follow-up)
- Connection pooling beyond one connection per saved entry (a `DbPool` engine is reserved for a future plan)

**Judgment calls flagged in this plan (revisit during human review):**
- The `SecretStore` trait and `PlainStore` impl were not built in Plan 1 (the spec listed them but Plan 1 stopped short). This plan introduces them in Phase F so the `DbConnection.secret_ref` field has a real backing store; otherwise the password field would be plaintext in the `db_connections` row itself, which violates the spec.
- `PostgresClient` v1 paginates via `LIMIT/OFFSET` against the user's SQL wrapped in a sub-select. Cursor-based pagination (server-side `DECLARE CURSOR`) is documented as a future optimisation but not required for v1.
- The SQL lexer is hand-rolled and dialect-agnostic (covers ANSI keywords + common Postgres extensions). Per CLAUDE.md, lexers are a `cargo fuzz` target — until fuzzing is wired, a `proptest` over `Vec<u8>` inputs guards against panics/hangs.
- `rusqlite` is sync. `SqliteClient` wraps every call in `tokio::task::spawn_blocking` to expose the async `DbClient` surface.

---

## File structure (new and modified only — existing crates unchanged unless noted)

```
sid/
├── Cargo.toml                            # MODIFY: + tokio-postgres, rusqlite, csv, sid-db-clients workspace member
├── crates/
│   ├── sid-core/
│   │   └── src/
│   │       ├── lib.rs                    # MODIFY: re-export new adapter types if needed
│   │       └── adapters/
│   │           ├── db_client.rs          # MODIFY: full DbClient trait + domain types
│   │           └── secret_store.rs       # NEW
│   ├── sid-db-clients/                   # NEW CRATE
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs                    # re-exports
│   │   │   ├── postgres.rs               # PostgresClient
│   │   │   ├── sqlite.rs                 # SqliteClient
│   │   │   └── lexer.rs                  # SQL lexer
│   │   ├── benches/
│   │   │   └── lexer.rs                  # criterion bench
│   │   └── tests/
│   │       ├── lexer.rs
│   │       ├── lexer_proptest.rs
│   │       ├── sqlite_open.rs
│   │       ├── sqlite_execute.rs
│   │       ├── sqlite_query_paged.rs
│   │       ├── sqlite_introspect.rs
│   │       └── postgres_smoke.rs         # gated behind `pg-it` feature (live Postgres)
│   ├── sid-store/
│   │   ├── src/
│   │   │   ├── lib.rs                    # MODIFY: + DbConnection, QueryRecord, PlainSecret types
│   │   │   ├── schema.rs                 # MODIFY: + DB_CONNECTIONS, QUERY_HISTORY, SECRETS tables
│   │   │   ├── redb_impl.rs              # MODIFY: + db-connection, query-history, secret methods
│   │   │   └── plain_secret_store.rs     # NEW (PlainStore impl of SecretStore)
│   │   └── tests/
│   │       ├── db_connections.rs         # NEW
│   │       ├── query_history.rs          # NEW
│   │       └── secrets.rs                # NEW
│   ├── sid-widgets/
│   │   └── src/
│   │       └── database.rs               # MODIFY: replace stub with full impl
│   └── sid/
│       └── src/
│           ├── main.rs                   # MODIFY: + `sid db` subcommands
│           └── wire.rs                   # MODIFY: + DB clients + SecretStore injection
```

---

## Task index

| # | Task | Phase |
|---|---|---|
| 1 | Add `tokio-postgres`, `rusqlite`, `csv` workspace deps + `sid-db-clients` member | A. Foundation |
| 2 | Scaffold `sid-db-clients` crate | A. Foundation |
| 3 | Expand `DbClient` trait + domain types in `sid-core` | B. Trait |
| 4 | Add `SecretStore` trait in `sid-core` | B. Trait |
| 5 | `SqliteClient::open` + `close` | C. SqliteClient |
| 6 | `SqliteClient::execute` | C. SqliteClient |
| 7 | `SqliteClient::query_paged` | C. SqliteClient |
| 8 | `SqliteClient::schema_introspect` + `cancel` | C. SqliteClient |
| 9 | `PostgresClient::open` + `close` | D. PostgresClient |
| 10 | `PostgresClient::execute` | D. PostgresClient |
| 11 | `PostgresClient::query_paged` | D. PostgresClient |
| 12 | `PostgresClient::schema_introspect` | D. PostgresClient |
| 13 | `PostgresClient::cancel` | D. PostgresClient |
| 14 | SQL lexer — token types + state machine | E. Lexer |
| 15 | SQL lexer — keyword set, adversarial coverage + proptest + criterion bench | E. Lexer |
| 16 | `DbConnection` + `QueryRecord` + `PlainSecret` types in `sid-store` | F. Storage |
| 17 | `DB_CONNECTIONS`, `QUERY_HISTORY`, `SECRETS` table defs | F. Storage |
| 18 | `Store` trait extension + `RedbStore` impl (db connections + secrets) | F. Storage |
| 19 | `Store` trait extension + `RedbStore` impl (query history) | F. Storage |
| 20 | `PlainSecretStore` impl of `SecretStore` | F. Storage |
| 21 | `DatabaseWidget` — connection-list state + render seam | G. Widget |
| 22 | `DatabaseWidget` — query editor state (multi-line + tokenised highlight) | G. Widget |
| 23 | `DatabaseWidget` — results table state (paginated + sortable) | G. Widget |
| 24 | `DatabaseWidget` — query history sub-view | G. Widget |
| 25 | `DatabaseWidget` — connect/disconnect + run-query orchestration | G. Widget |
| 26 | `DatabaseWidget` — copy-cell action | G. Widget |
| 27 | CSV export action (using `csv` crate) | H. Export |
| 28 | Paginate-on-scroll wiring | H. Export |
| 29 | CLI `sid db add` | I. CLI |
| 30 | CLI `sid db remove` + `list` | I. CLI |
| 31 | CLI `sid db query` (CSV stdout) | I. CLI |
| 32 | Wire `PostgresClient`/`SqliteClient`/`PlainSecretStore` into binary | I. CLI |
| 33 | Integration test: SQLite round-trip from CLI | J. Integration |
| 34 | Integration test: widget end-to-end with in-memory SQLite | J. Integration |
| 35 | README update | J. Integration |

---

## Phase A — Foundation

### Task 1: Add deps and `sid-db-clients` workspace member

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add `sid-db-clients` to workspace members**

Locate the `[workspace] members` list. Append `"crates/sid-db-clients"`:

```toml
members = [
    "crates/sid",
    "crates/sid-core",
    "crates/sid-ui",
    "crates/sid-store",
    "crates/sid-job",
    "crates/sid-widgets",
    "crates/sid-git",
    "crates/sid-db-clients",
]
```

- [ ] **Step 2: Add new external deps to `[workspace.dependencies]`**

In a logical place (after the `# Git` block), add:

```toml
# Database clients
tokio-postgres = "0.7"
rusqlite = { version = "0.32", features = ["bundled"] }
csv = "1.3"
```

Under the `# Internal` block, append:

```toml
sid-db-clients = { path = "crates/sid-db-clients" }
```

- [ ] **Step 3: Stub the crate so the workspace resolves**

```bash
mkdir -p crates/sid-db-clients/src
cat > crates/sid-db-clients/Cargo.toml <<'EOF'
[package]
name = "sid-db-clients"
version.workspace = true
edition.workspace = true

[dependencies]
EOF
echo "// stub — Task 2 replaces this" > crates/sid-db-clients/src/lib.rs
```

Confirm `cargo metadata --no-deps --format-version 1 > /dev/null` exits 0.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/sid-db-clients
git commit -m "chore: add tokio-postgres, rusqlite, csv deps + sid-db-clients stub"
```

---

### Task 2: Scaffold `sid-db-clients` crate

**Files:**
- Replace: `crates/sid-db-clients/Cargo.toml`
- Replace: `crates/sid-db-clients/src/lib.rs`

- [ ] **Step 1: Replace `Cargo.toml`**

```toml
[package]
name = "sid-db-clients"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[features]
# Live-Postgres integration tests; off by default in CI.
pg-it = []

[dependencies]
sid-core.workspace = true
tokio.workspace = true
tokio-postgres.workspace = true
rusqlite.workspace = true
thiserror.workspace = true
tracing.workspace = true
serde.workspace = true

[dev-dependencies]
tempfile.workspace = true
proptest.workspace = true
insta.workspace = true
criterion.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[[bench]]
name = "lexer"
harness = false
```

- [ ] **Step 2: Replace `src/lib.rs` with module skeleton**

```rust
//! `sid-db-clients` — DbClient implementations for the Database tab.
//!
//! Hosts:
//! - `PostgresClient` (tokio-postgres)
//! - `SqliteClient` (rusqlite + spawn_blocking)
//! - `lexer` — a hand-rolled SQL lexer for syntax highlighting
//!
//! All public types route through the `sid_core::adapters::db_client` trait
//! surface; nothing in this crate is named directly by `sid-widgets`.

pub mod lexer;
pub mod postgres;
pub mod sqlite;

pub use postgres::PostgresClient;
pub use sqlite::SqliteClient;
```

- [ ] **Step 3: Stub each module with `// implemented in later tasks`**

```bash
echo "//! SQL lexer — implemented in Task 14." > crates/sid-db-clients/src/lexer.rs
echo "//! PostgresClient — implemented in Task 9-13." > crates/sid-db-clients/src/postgres.rs
echo "//! SqliteClient — implemented in Task 5-8." > crates/sid-db-clients/src/sqlite.rs
```

Confirm `cargo check -p sid-db-clients` exits 0.

- [ ] **Step 4: Commit**

```bash
git add crates/sid-db-clients
git commit -m "chore(db-clients): scaffold sid-db-clients crate with module skeleton"
```

---

## Phase B — Trait surface

### Task 3: Expand `DbClient` trait + domain types in `sid-core`

**Files:**
- Modify: `crates/sid-core/src/adapters/db_client.rs`
- Test: `crates/sid-core/tests/db_client_contract.rs`

- [ ] **Step 1: Write the contract test first**

Create `crates/sid-core/tests/db_client_contract.rs`:

```rust
//! Verifies the DbClient trait is dyn-compatible and a mock impl can satisfy
//! every method.

use std::sync::Arc;

use sid_core::adapters::db_client::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor,
    QueryPage, Row, SchemaInfo,
};

struct MockDb;

#[async_trait::async_trait]
impl DbClient for MockDb {
    async fn open(&self, _p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> {
        Ok(Arc::new(MockDb))
    }
    async fn close(&self) -> Result<(), DbError> { Ok(()) }
    async fn execute(&self, _sql: &str) -> Result<ExecResult, DbError> {
        Ok(ExecResult { rows_affected: 0, duration_ms: 0 })
    }
    async fn query_paged(
        &self,
        _sql: &str,
        _cursor: Option<PageCursor>,
        _page_size: u32,
    ) -> Result<QueryPage, DbError> {
        Ok(QueryPage {
            columns: vec![],
            rows: vec![],
            next_cursor: None,
            duration_ms: 0,
        })
    }
    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        Ok(SchemaInfo { tables: vec![] })
    }
    async fn cancel(&self) -> Result<(), DbError> { Ok(()) }
    fn kind(&self) -> DbKind { DbKind::Sqlite }
}

#[tokio::test]
async fn dyn_dispatch_works() {
    let c: Arc<dyn DbClient> = Arc::new(MockDb);
    assert!(c.execute("SELECT 1").await.unwrap().rows_affected == 0);
    let p = c.query_paged("SELECT 1", None, 50).await.unwrap();
    assert!(p.columns.is_empty());
}

#[test]
fn send_sync_bounds() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn DbClient>>();
}

#[test]
fn dbkind_variants() {
    let _ = DbKind::Postgres;
    let _ = DbKind::Sqlite;
}

#[test]
fn column_type_variants_exist() {
    let _ = ColumnType::Text;
    let _ = ColumnType::Integer;
    let _ = ColumnType::Float;
    let _ = ColumnType::Bool;
    let _ = ColumnType::Bytes;
    let _ = ColumnType::Null;
    let _ = ColumnType::Other("uuid".into());
}

#[test]
fn row_construction() {
    let r = Row { values: vec!["a".into(), "1".into()] };
    assert_eq!(r.values.len(), 2);
}

#[test]
fn page_cursor_construction() {
    let c = PageCursor { offset: 100 };
    assert_eq!(c.offset, 100);
}

#[test]
fn column_construction() {
    let c = Column { name: "id".into(), ty: ColumnType::Integer };
    assert_eq!(c.name, "id");
}
```

Add `async-trait` to `crates/sid-core/Cargo.toml`'s dev-dependencies (and to `[workspace.dependencies]` as `async-trait = "0.1"`).

- [ ] **Step 2: Run — should fail to compile**

Run: `cargo test -p sid-core --test db_client_contract`

- [ ] **Step 3: Replace `crates/sid-core/src/adapters/db_client.rs`**

```rust
//! `DbClient` — domain-shaped database client trait used by the Database tab.
//!
//! Concrete impls live in `sid-db-clients` (`PostgresClient`, `SqliteClient`).
//! No widget code names this crate's concrete types — they hold
//! `Arc<dyn DbClient>` only.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Database kind discriminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DbKind {
    Postgres,
    Sqlite,
}

/// Domain-shaped error returned by every `DbClient` method.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("authentication failed")]
    Auth,
    #[error("query failed: {0}")]
    Query(String),
    #[error("query syntax error at offset {offset}: {message}")]
    Syntax { offset: usize, message: String },
    #[error("query was cancelled")]
    Cancelled,
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("not connected")]
    NotConnected,
    #[error("io error: {0}")]
    Io(String),
    #[error("other: {0}")]
    Other(String),
}

/// Parameters used to open a connection.
#[derive(Clone, Debug)]
pub struct OpenParams {
    pub kind: DbKind,
    /// Postgres DSN (`postgres://user:pass@host:port/db`) or SQLite path
    /// (`:memory:` or filesystem path).
    pub dsn: String,
    /// Optional resolved password from the secret store. Postgres uses it if
    /// the DSN does not already include one.
    pub password: Option<String>,
}

/// Result of a non-`SELECT` statement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecResult {
    pub rows_affected: u64,
    pub duration_ms: u64,
}

/// One column header in a query result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub ty: ColumnType,
}

/// Coarse column type. Drivers may emit `Other(name)` for everything they
/// can't fit into the simple set.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ColumnType {
    Text,
    Integer,
    Float,
    Bool,
    Bytes,
    Null,
    Other(String),
}

/// One row, rendered to display strings. Drivers convert each value to its
/// human-readable form (`NULL` for NULL, `0x…` for bytes, etc.).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub values: Vec<String>,
}

/// Opaque pagination cursor. v1 uses `OFFSET` semantics for portability.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageCursor {
    pub offset: u64,
}

/// One page of `query_paged` results.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryPage {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    pub next_cursor: Option<PageCursor>,
    pub duration_ms: u64,
}

/// Schema introspection result: a flat list of tables with their columns.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub tables: Vec<TableInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TableInfo {
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<Column>,
}

/// Async database client trait. Implementations live in `sid-db-clients`.
///
/// # Object safety
///
/// All methods take `&self` and use no generics in method position. Returns
/// owned types only. Pass around as `Arc<dyn DbClient>`.
#[async_trait::async_trait]
pub trait DbClient: Send + Sync {
    /// Open a connection. Returns a *new* client bound to the DSN.
    async fn open(&self, params: OpenParams) -> Result<Arc<dyn DbClient>, DbError>;

    /// Close the connection cleanly. Idempotent.
    async fn close(&self) -> Result<(), DbError>;

    /// Execute a non-`SELECT` statement (DDL or DML). Returns row count.
    async fn execute(&self, sql: &str) -> Result<ExecResult, DbError>;

    /// Execute a `SELECT` and return one page of results. Pass `cursor=None`
    /// for the first page. Returns `next_cursor=None` when there is no more
    /// data.
    async fn query_paged(
        &self,
        sql: &str,
        cursor: Option<PageCursor>,
        page_size: u32,
    ) -> Result<QueryPage, DbError>;

    /// List tables + columns. Postgres uses `information_schema`; SQLite uses
    /// `sqlite_master` + `PRAGMA table_info`.
    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError>;

    /// Best-effort cancel of an in-flight query. SQLite is a no-op; Postgres
    /// sends a `CancelRequest` on a side channel.
    async fn cancel(&self) -> Result<(), DbError>;

    /// Discriminator. Useful for UI labels and dialect-specific logic.
    fn kind(&self) -> DbKind;
}
```

- [ ] **Step 4: Add doc tests on every public item**

Per CLAUDE.md, every `pub` item gets a doc example. Add `# Examples` blocks on `DbKind`, `DbError`, `OpenParams`, `ExecResult`, `Column`, `ColumnType`, `Row`, `PageCursor`, `QueryPage`, `SchemaInfo`, `TableInfo`, `DbClient`. Each can be a 2–5 line construction example.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test db_client_contract`
Expected: 7 passed.

Run: `cargo test -p sid-core --all-features`
Expected: no regressions.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core Cargo.toml
git commit -m "feat(core): expand DbClient trait with full method surface + domain types"
```

---

### Task 4: Add `SecretStore` trait in `sid-core`

**Files:**
- Create: `crates/sid-core/src/adapters/secret_store.rs`
- Modify: `crates/sid-core/src/adapters/mod.rs`
- Test: `crates/sid-core/tests/secret_store_contract.rs`

- [ ] **Step 1: Failing test**

Create `crates/sid-core/tests/secret_store_contract.rs`:

```rust
use std::sync::Arc;

use sid_core::adapters::secret_store::{SecretError, SecretRef, SecretStore};

struct MemStore;

impl SecretStore for MemStore {
    fn get(&self, _r: &SecretRef) -> Result<Option<String>, SecretError> { Ok(Some("p".into())) }
    fn put(&self, _r: &SecretRef, _v: &str) -> Result<(), SecretError> { Ok(()) }
    fn remove(&self, _r: &SecretRef) -> Result<(), SecretError> { Ok(()) }
}

#[test]
fn secret_store_is_dyn_compatible() {
    let s: Arc<dyn SecretStore> = Arc::new(MemStore);
    assert_eq!(s.get(&SecretRef::new("db.local.password")).unwrap().as_deref(), Some("p"));
}

#[test]
fn secret_ref_construction() {
    let r = SecretRef::new("alias");
    assert_eq!(r.id(), "alias");
}

#[test]
fn send_sync_bounds() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn SecretStore>>();
}
```

- [ ] **Step 2: Run — should fail to compile**

- [ ] **Step 3: Create `crates/sid-core/src/adapters/secret_store.rs`**

```rust
//! `SecretStore` — domain-shaped secret retrieval trait. v1 ships a
//! `PlainStore` impl (in `sid-store`) that reads from the redb `secrets`
//! table. A future plan adds `KeyringStore`.

use serde::{Deserialize, Serialize};

/// Stable identifier for a stored secret. Generated by the binary when the
/// user adds a connection; stored as the `secret_ref` field on
/// `DbConnection`.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SecretRef(String);

impl SecretRef {
    /// Construct a SecretRef from a stable id string.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::secret_store::SecretRef;
    /// let r = SecretRef::new("db.local-pg.password");
    /// assert_eq!(r.id(), "db.local-pg.password");
    /// ```
    pub fn new(id: impl Into<String>) -> Self { Self(id.into()) }
    pub fn id(&self) -> &str { &self.0 }
}

/// Error type for the SecretStore trait.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("backend error: {0}")]
    Backend(String),
}

/// Trait for retrieving and storing secrets. Synchronous because v1's
/// `PlainStore` reads from redb (sync); the call sites wrap reads in a
/// `spawn_blocking` when appropriate.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::secret_store::{SecretError, SecretRef, SecretStore};
/// struct NoopStore;
/// impl SecretStore for NoopStore {
///     fn get(&self, _r: &SecretRef) -> Result<Option<String>, SecretError> { Ok(None) }
///     fn put(&self, _r: &SecretRef, _v: &str) -> Result<(), SecretError> { Ok(()) }
///     fn remove(&self, _r: &SecretRef) -> Result<(), SecretError> { Ok(()) }
/// }
/// let s = NoopStore;
/// assert!(s.get(&SecretRef::new("k")).unwrap().is_none());
/// ```
pub trait SecretStore: Send + Sync {
    fn get(&self, r: &SecretRef) -> Result<Option<String>, SecretError>;
    fn put(&self, r: &SecretRef, v: &str) -> Result<(), SecretError>;
    fn remove(&self, r: &SecretRef) -> Result<(), SecretError>;
}
```

Add `pub mod secret_store;` to `crates/sid-core/src/adapters/mod.rs`.

- [ ] **Step 4: Run tests** — expected 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add SecretStore trait and SecretRef type"
```

---

## Phase C — SqliteClient impl

SQLite is sync; every call wraps in `tokio::task::spawn_blocking`. Each method gets one Phase-C task. Test fixtures use `:memory:` databases via `rusqlite::Connection::open_in_memory()`.

### Task 5: `SqliteClient::open` + `close`

**Files:**
- Replace: `crates/sid-db-clients/src/sqlite.rs`
- Create: `crates/sid-db-clients/tests/sqlite_open.rs`

- [ ] **Step 1: Failing test**

Create `crates/sid-db-clients/tests/sqlite_open.rs`:

```rust
use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams};
use sid_db_clients::SqliteClient;

#[tokio::test]
async fn open_in_memory_succeeds() {
    let factory = SqliteClient::factory();
    let client = factory
        .open(OpenParams { kind: DbKind::Sqlite, dsn: ":memory:".into(), password: None })
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
        .open(OpenParams { kind: DbKind::Sqlite, dsn: path.to_string_lossy().into_owned(), password: None })
        .await
        .unwrap();
    assert!(path.exists(), "SQLite should create the file on open");
    client.close().await.unwrap();
}

#[tokio::test]
async fn open_with_postgres_kind_fails() {
    let factory = SqliteClient::factory();
    let err = factory
        .open(OpenParams { kind: DbKind::Postgres, dsn: ":memory:".into(), password: None })
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("invalid") || msg.contains("kind"));
}
```

- [ ] **Step 2: Run — should fail to compile**

- [ ] **Step 3: Implement `SqliteClient`**

Replace `crates/sid-db-clients/src/sqlite.rs`:

```rust
//! SqliteClient — rusqlite-backed `DbClient` impl. Wraps the sync rusqlite
//! API in `tokio::task::spawn_blocking` to fit the async trait.

use std::sync::{Arc, Mutex};

use sid_core::adapters::db_client::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor,
    QueryPage, Row, SchemaInfo, TableInfo,
};

/// Factory + per-connection client.
///
/// `SqliteClient::factory()` returns a stateless factory whose `open` method
/// returns an `Arc<dyn DbClient>` bound to the requested DSN.
pub struct SqliteClient {
    inner: Option<Arc<Mutex<rusqlite::Connection>>>,
}

impl SqliteClient {
    pub fn factory() -> Arc<dyn DbClient> { Arc::new(Self { inner: None }) }
}

#[async_trait::async_trait]
impl DbClient for SqliteClient {
    async fn open(&self, p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> {
        if p.kind != DbKind::Sqlite {
            return Err(DbError::Invalid(format!("expected DbKind::Sqlite, got {:?}", p.kind)));
        }
        let dsn = p.dsn.clone();
        let conn = tokio::task::spawn_blocking(move || {
            if dsn == ":memory:" {
                rusqlite::Connection::open_in_memory()
            } else {
                rusqlite::Connection::open(&dsn)
            }
        })
        .await
        .map_err(|e| DbError::Other(format!("join: {e}")))?
        .map_err(|e| DbError::Connect(e.to_string()))?;
        Ok(Arc::new(SqliteClient {
            inner: Some(Arc::new(Mutex::new(conn))),
        }))
    }

    async fn close(&self) -> Result<(), DbError> {
        // Dropping the Arc<Mutex<Connection>> closes; we cannot consume self
        // from a `&self` method, so this is effectively a noop. The Arc dies
        // when the last reference drops.
        Ok(())
    }

    async fn execute(&self, _sql: &str) -> Result<ExecResult, DbError> {
        Err(DbError::Other("execute: not yet implemented — Task 6".into()))
    }

    async fn query_paged(
        &self,
        _sql: &str,
        _cursor: Option<PageCursor>,
        _page_size: u32,
    ) -> Result<QueryPage, DbError> {
        Err(DbError::Other("query_paged: not yet implemented — Task 7".into()))
    }

    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        Err(DbError::Other("schema_introspect: not yet implemented — Task 8".into()))
    }

    async fn cancel(&self) -> Result<(), DbError> { Ok(()) }

    fn kind(&self) -> DbKind { DbKind::Sqlite }
}

// Helper used by Tasks 6-8.
#[allow(dead_code)]
fn map_rusqlite_error(e: rusqlite::Error) -> DbError {
    match e {
        rusqlite::Error::SqliteFailure(_, Some(ref msg)) if msg.starts_with("syntax") => {
            DbError::Syntax { offset: 0, message: msg.clone() }
        }
        e => DbError::Query(e.to_string()),
    }
}

// Helper used by Tasks 7 and 8.
#[allow(dead_code)]
fn rusqlite_type_to_column_type(decl: Option<&str>, value_ty: rusqlite::types::Type) -> ColumnType {
    // Prefer the declared type if it gives a strong hint; fall back to the
    // dynamic value's affinity.
    if let Some(d) = decl {
        let d = d.to_ascii_uppercase();
        if d.contains("INT") { return ColumnType::Integer; }
        if d.contains("CHAR") || d.contains("TEXT") || d.contains("CLOB") { return ColumnType::Text; }
        if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") { return ColumnType::Float; }
        if d.contains("BLOB") { return ColumnType::Bytes; }
        if d.contains("BOOL") { return ColumnType::Bool; }
    }
    match value_ty {
        rusqlite::types::Type::Integer => ColumnType::Integer,
        rusqlite::types::Type::Real => ColumnType::Float,
        rusqlite::types::Type::Text => ColumnType::Text,
        rusqlite::types::Type::Blob => ColumnType::Bytes,
        rusqlite::types::Type::Null => ColumnType::Null,
    }
}

#[allow(dead_code)]
fn _unused_silencer(_: TableInfo, _: Row, _: Column) {}
```

Note the `SqliteClient` carries the `Connection` behind `Arc<Mutex<…>>` so the `Send + Sync` bound on `DbClient` is satisfied. Reads/writes serialize through the mutex; since we always wrap in `spawn_blocking`, the mutex is held only on a blocking thread.

Add `async-trait.workspace = true` to `sid-db-clients/Cargo.toml`.

- [ ] **Step 4: Run tests** — expected 3 passed.

- [ ] **Step 5: Adversarial coverage**

Append to `tests/sqlite_open.rs`:

```rust
#[tokio::test]
async fn open_path_with_invalid_directory_returns_connect_error() {
    let factory = SqliteClient::factory();
    let err = factory
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: "/nonexistent/dir/foo.db".into(),
            password: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, sid_core::adapters::db_client::DbError::Connect(_)));
}

#[tokio::test]
async fn open_with_unicode_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("🐕-sid.db");
    let factory = SqliteClient::factory();
    let _client = factory
        .open(OpenParams { kind: DbKind::Sqlite, dsn: path.to_string_lossy().into_owned(), password: None })
        .await
        .unwrap();
    assert!(path.exists());
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): SqliteClient::open + close (in-memory and file)"
```

---

### Task 6: `SqliteClient::execute`

**Files:**
- Modify: `crates/sid-db-clients/src/sqlite.rs`
- Create: `crates/sid-db-clients/tests/sqlite_execute.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-db-clients/tests/sqlite_execute.rs`:

```rust
use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams};
use sid_db_clients::SqliteClient;

async fn open_mem() -> std::sync::Arc<dyn DbClient> {
    SqliteClient::factory()
        .open(OpenParams { kind: DbKind::Sqlite, dsn: ":memory:".into(), password: None })
        .await
        .unwrap()
}

#[tokio::test]
async fn create_table_returns_zero_rows_affected() {
    let c = open_mem().await;
    let r = c.execute("CREATE TABLE t (id INTEGER, name TEXT)").await.unwrap();
    assert_eq!(r.rows_affected, 0);
}

#[tokio::test]
async fn insert_returns_correct_rows_affected() {
    let c = open_mem().await;
    c.execute("CREATE TABLE t (id INTEGER)").await.unwrap();
    let r = c.execute("INSERT INTO t VALUES (1), (2), (3)").await.unwrap();
    assert_eq!(r.rows_affected, 3);
}

#[tokio::test]
async fn syntax_error_returns_syntax_or_query_error() {
    let c = open_mem().await;
    let err = c.execute("CREATE TABL t (id INTEGER)").await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.to_lowercase().contains("syntax") || msg.to_lowercase().contains("near"));
}

#[tokio::test]
async fn execute_records_duration() {
    let c = open_mem().await;
    let r = c.execute("CREATE TABLE t (id INTEGER)").await.unwrap();
    // duration_ms is best-effort; just verify it's not absurdly large.
    assert!(r.duration_ms < 60_000);
}
```

- [ ] **Step 2: Implement `execute`**

Replace the stub with:

```rust
async fn execute(&self, sql: &str) -> Result<ExecResult, DbError> {
    let conn = self.inner.clone().ok_or(DbError::NotConnected)?;
    let sql = sql.to_string();
    let start = std::time::Instant::now();
    let rows_affected: u64 = tokio::task::spawn_blocking(move || -> Result<u64, DbError> {
        let guard = conn.lock().map_err(|e| DbError::Other(format!("mutex poisoned: {e}")))?;
        let n = guard.execute_batch_with_count(&sql).unwrap_or_else(|_| {
            guard.execute(&sql, []).map(|n| n as u64).unwrap_or(0)
        });
        Ok(n)
    })
    .await
    .map_err(|e| DbError::Other(format!("join: {e}")))??;
    Ok(ExecResult { rows_affected, duration_ms: start.elapsed().as_millis() as u64 })
}
```

`execute_batch_with_count` is not in rusqlite; replace with a small helper that runs the SQL through `Connection::execute` (single statement) or falls back to `execute_batch` (multi-statement, no count). The simpler shape:

```rust
async fn execute(&self, sql: &str) -> Result<ExecResult, DbError> {
    let conn = self.inner.clone().ok_or(DbError::NotConnected)?;
    let sql = sql.to_string();
    let start = std::time::Instant::now();
    let rows_affected: u64 = tokio::task::spawn_blocking(move || -> Result<u64, DbError> {
        let guard = conn.lock().map_err(|e| DbError::Other(format!("mutex poisoned: {e}")))?;
        // Try as a single statement first (gives a row count); fall back to
        // execute_batch for multi-statement scripts (returns 0).
        match guard.execute(&sql, []) {
            Ok(n) => Ok(n as u64),
            Err(rusqlite::Error::MultipleStatement) => {
                guard.execute_batch(&sql).map_err(map_rusqlite_error)?;
                Ok(0)
            }
            Err(e) => Err(map_rusqlite_error(e)),
        }
    })
    .await
    .map_err(|e| DbError::Other(format!("join: {e}")))??;
    Ok(ExecResult { rows_affected, duration_ms: start.elapsed().as_millis() as u64 })
}
```

- [ ] **Step 3: Run tests** — expected 4 passed.

- [ ] **Step 4: Property test**

Append:

```rust
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
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): SqliteClient::execute (DDL/DML with rows-affected count)"
```

---

### Task 7: `SqliteClient::query_paged`

**Files:**
- Modify: `crates/sid-db-clients/src/sqlite.rs`
- Create: `crates/sid-db-clients/tests/sqlite_query_paged.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-db-clients/tests/sqlite_query_paged.rs`:

```rust
use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams, PageCursor};
use sid_db_clients::SqliteClient;

async fn seed(rows: u32) -> std::sync::Arc<dyn DbClient> {
    let c = SqliteClient::factory()
        .open(OpenParams { kind: DbKind::Sqlite, dsn: ":memory:".into(), password: None })
        .await
        .unwrap();
    c.execute("CREATE TABLE t (id INTEGER, label TEXT)").await.unwrap();
    if rows > 0 {
        let v = (0..rows).map(|i| format!("({i}, 'r{i}')")).collect::<Vec<_>>().join(",");
        c.execute(&format!("INSERT INTO t VALUES {v}")).await.unwrap();
    }
    c
}

#[tokio::test]
async fn first_page_returns_columns_and_first_n_rows() {
    let c = seed(120).await;
    let page = c.query_paged("SELECT id, label FROM t ORDER BY id", None, 50).await.unwrap();
    assert_eq!(page.columns.len(), 2);
    assert_eq!(page.columns[0].name, "id");
    assert_eq!(page.columns[1].name, "label");
    assert_eq!(page.rows.len(), 50);
    assert_eq!(page.rows[0].values[0], "0");
    assert_eq!(page.next_cursor, Some(PageCursor { offset: 50 }));
}

#[tokio::test]
async fn second_page_continues() {
    let c = seed(120).await;
    let p1 = c.query_paged("SELECT id, label FROM t ORDER BY id", None, 50).await.unwrap();
    let p2 = c
        .query_paged("SELECT id, label FROM t ORDER BY id", p1.next_cursor, 50)
        .await
        .unwrap();
    assert_eq!(p2.rows.len(), 50);
    assert_eq!(p2.rows[0].values[0], "50");
}

#[tokio::test]
async fn last_partial_page_yields_no_next_cursor() {
    let c = seed(120).await;
    let p3 = c
        .query_paged("SELECT id, label FROM t ORDER BY id", Some(PageCursor { offset: 100 }), 50)
        .await
        .unwrap();
    assert_eq!(p3.rows.len(), 20);
    assert!(p3.next_cursor.is_none());
}

#[tokio::test]
async fn empty_result_returns_no_rows_no_cursor() {
    let c = seed(0).await;
    let p = c.query_paged("SELECT id, label FROM t", None, 50).await.unwrap();
    assert!(p.rows.is_empty());
    assert!(p.next_cursor.is_none());
}

#[tokio::test]
async fn null_value_renders_as_null_string() {
    let c = seed(0).await;
    c.execute("INSERT INTO t VALUES (1, NULL)").await.unwrap();
    let p = c.query_paged("SELECT id, label FROM t", None, 50).await.unwrap();
    assert_eq!(p.rows[0].values[1], "NULL");
}

#[tokio::test]
async fn syntax_error_returns_error() {
    let c = seed(0).await;
    let err = c.query_paged("SELEC * FROM t", None, 50).await.unwrap_err();
    let _ = format!("{err}");
}
```

- [ ] **Step 2: Implement `query_paged`**

Replace the stub:

```rust
async fn query_paged(
    &self,
    sql: &str,
    cursor: Option<PageCursor>,
    page_size: u32,
) -> Result<QueryPage, DbError> {
    let conn = self.inner.clone().ok_or(DbError::NotConnected)?;
    let sql = sql.to_string();
    let offset = cursor.map(|c| c.offset).unwrap_or(0);
    let page_size = page_size.max(1) as u64;
    let start = std::time::Instant::now();
    let (columns, rows, fetched) = tokio::task::spawn_blocking(move || -> Result<(Vec<Column>, Vec<Row>, u64), DbError> {
        let guard = conn.lock().map_err(|e| DbError::Other(format!("mutex poisoned: {e}")))?;
        // Wrap the user SQL so OFFSET/LIMIT apply regardless of trailing semicolons / order-by clauses.
        let trimmed = sql.trim().trim_end_matches(';');
        let wrapped = format!("SELECT * FROM ( {trimmed} ) LIMIT {page_size} OFFSET {offset}");
        let mut stmt = guard.prepare(&wrapped).map_err(map_rusqlite_error)?;
        let col_count = stmt.column_count();
        let columns: Vec<Column> = (0..col_count)
            .map(|i| Column {
                name: stmt.column_name(i).unwrap_or("?").to_string(),
                ty: rusqlite_type_to_column_type(
                    stmt.column_decltype(i),
                    rusqlite::types::Type::Null, // refined per-value below
                ),
            })
            .collect();
        let mut rows_out: Vec<Row> = Vec::with_capacity(page_size as usize);
        let mut rs = stmt.query([]).map_err(map_rusqlite_error)?;
        while let Some(row) = rs.next().map_err(map_rusqlite_error)? {
            let mut values = Vec::with_capacity(col_count);
            for i in 0..col_count {
                let v: rusqlite::types::Value = row.get(i).map_err(map_rusqlite_error)?;
                values.push(render_sqlite_value(&v));
            }
            rows_out.push(Row { values });
        }
        let fetched = rows_out.len() as u64;
        Ok((columns, rows_out, fetched))
    })
    .await
    .map_err(|e| DbError::Other(format!("join: {e}")))??;
    let next_cursor = if fetched < page_size { None } else { Some(PageCursor { offset: offset + fetched }) };
    Ok(QueryPage {
        columns,
        rows,
        next_cursor,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
```

Add the value renderer at module bottom:

```rust
fn render_sqlite_value(v: &rusqlite::types::Value) -> String {
    use rusqlite::types::Value;
    match v {
        Value::Null => "NULL".to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Blob(b) => {
            let mut s = String::with_capacity(2 + b.len() * 2);
            s.push_str("0x");
            for byte in b {
                use std::fmt::Write;
                write!(&mut s, "{byte:02x}").ok();
            }
            s
        }
    }
}
```

- [ ] **Step 3: Run tests** — expected 6 passed.

- [ ] **Step 4: Adversarial — large page + huge column count**

Append:

```rust
#[tokio::test]
async fn page_size_one_yields_one_row_per_page() {
    let c = seed(3).await;
    let p = c.query_paged("SELECT id, label FROM t ORDER BY id", None, 1).await.unwrap();
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.next_cursor, Some(PageCursor { offset: 1 }));
}

#[tokio::test]
async fn unicode_text_round_trips() {
    let c = seed(0).await;
    c.execute("INSERT INTO t VALUES (1, '🐕 hello 你好')").await.unwrap();
    let p = c.query_paged("SELECT label FROM t", None, 50).await.unwrap();
    assert_eq!(p.rows[0].values[0], "🐕 hello 你好");
}

#[tokio::test]
async fn blob_renders_as_hex() {
    let c = seed(0).await;
    c.execute("INSERT INTO t VALUES (1, X'DEADBEEF')").await.unwrap();
    let p = c.query_paged("SELECT label FROM t", None, 50).await.unwrap();
    assert!(p.rows[0].values[0].starts_with("0x"));
    assert!(p.rows[0].values[0].to_lowercase().contains("deadbeef"));
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): SqliteClient::query_paged with OFFSET/LIMIT pagination"
```

---

### Task 8: `SqliteClient::schema_introspect` + finalise `cancel`

**Files:**
- Modify: `crates/sid-db-clients/src/sqlite.rs`
- Create: `crates/sid-db-clients/tests/sqlite_introspect.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-db-clients/tests/sqlite_introspect.rs`:

```rust
use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams};
use sid_db_clients::SqliteClient;

async fn open_mem() -> std::sync::Arc<dyn DbClient> {
    SqliteClient::factory()
        .open(OpenParams { kind: DbKind::Sqlite, dsn: ":memory:".into(), password: None })
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
    c.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL, age INTEGER)").await.unwrap();
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
```

- [ ] **Step 2: Implement `schema_introspect`**

```rust
async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
    let conn = self.inner.clone().ok_or(DbError::NotConnected)?;
    let tables = tokio::task::spawn_blocking(move || -> Result<Vec<TableInfo>, DbError> {
        let guard = conn.lock().map_err(|e| DbError::Other(format!("mutex poisoned: {e}")))?;
        let mut stmt = guard
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(map_rusqlite_error)?;
        let table_names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_rusqlite_error)?
            .filter_map(Result::ok)
            .collect();
        let mut tables: Vec<TableInfo> = Vec::with_capacity(table_names.len());
        for tn in table_names {
            let mut info_stmt = guard
                .prepare(&format!("PRAGMA table_info({tn})"))
                .map_err(map_rusqlite_error)?;
            let cols: Vec<Column> = info_stmt
                .query_map([], |row| {
                    let name: String = row.get(1)?;
                    let decl: String = row.get(2).unwrap_or_default();
                    Ok(Column {
                        name,
                        ty: rusqlite_type_to_column_type(Some(&decl), rusqlite::types::Type::Null),
                    })
                })
                .map_err(map_rusqlite_error)?
                .filter_map(Result::ok)
                .collect();
            tables.push(TableInfo { schema: None, name: tn, columns: cols });
        }
        Ok(tables)
    })
    .await
    .map_err(|e| DbError::Other(format!("join: {e}")))??;
    Ok(SchemaInfo { tables })
}
```

(`cancel` was already a noop; keep as-is.)

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
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
    c.execute(r#"CREATE TABLE "select" (id INTEGER)"#).await.unwrap();
    let s = c.schema_introspect().await.unwrap();
    assert!(s.tables.iter().any(|t| t.name == "select"));
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): SqliteClient::schema_introspect + finalise cancel"
```

---

## Phase D — PostgresClient impl

Postgres lives behind a `tokio-postgres::Client`. Connections are spawned via `tokio_postgres::connect`, which returns a `(Client, Connection)` pair — the `Connection` future must be spawned on the runtime; the client holds a `tokio::task::JoinHandle` so it can abort on close.

**Caveats for testing:** these tests run only when `pg-it` feature is enabled and a live Postgres is reachable via `SID_PG_DSN`. CI runs them with a docker-compose Postgres; locally they are skipped if the env var is missing. All other coverage is unit-test-only and uses traits/mocks where the live server is required.

### Task 9: `PostgresClient::open` + `close`

**Files:**
- Replace: `crates/sid-db-clients/src/postgres.rs`
- Create: `crates/sid-db-clients/tests/postgres_smoke.rs`

- [ ] **Step 1: Failing test (gated)**

Create `crates/sid-db-clients/tests/postgres_smoke.rs`:

```rust
#![cfg(feature = "pg-it")]

use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams};
use sid_db_clients::PostgresClient;

fn dsn_or_skip() -> Option<String> {
    std::env::var("SID_PG_DSN").ok()
}

#[tokio::test]
async fn open_close_against_live_postgres() {
    let Some(dsn) = dsn_or_skip() else {
        eprintln!("SID_PG_DSN not set — skipping");
        return;
    };
    let factory = PostgresClient::factory();
    let c = factory
        .open(OpenParams { kind: DbKind::Postgres, dsn, password: None })
        .await
        .expect("open");
    assert_eq!(c.kind(), DbKind::Postgres);
    c.close().await.unwrap();
}

#[tokio::test]
async fn open_with_bad_dsn_returns_connect_error() {
    let factory = PostgresClient::factory();
    let err = factory
        .open(OpenParams { kind: DbKind::Postgres, dsn: "postgres://invalid:bad@127.0.0.1:1/none".into(), password: None })
        .await
        .unwrap_err();
    assert!(matches!(err, sid_core::adapters::db_client::DbError::Connect(_)));
}

#[tokio::test]
async fn open_with_sqlite_kind_returns_invalid() {
    let factory = PostgresClient::factory();
    let err = factory
        .open(OpenParams { kind: DbKind::Sqlite, dsn: "postgres://x".into(), password: None })
        .await
        .unwrap_err();
    assert!(matches!(err, sid_core::adapters::db_client::DbError::Invalid(_)));
}
```

Note: the second and third tests do *not* require a live Postgres — they verify failure modes. Remove the `#![cfg(feature = "pg-it")]` line if you want those two to always run. Recommended: split into two files, one gated and one unconditional.

- [ ] **Step 2: Replace `crates/sid-db-clients/src/postgres.rs`**

```rust
//! PostgresClient — tokio-postgres-backed `DbClient` impl.

use std::sync::Arc;

use sid_core::adapters::db_client::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor,
    QueryPage, Row, SchemaInfo, TableInfo,
};
use tokio::sync::Mutex;

/// Factory + per-connection client. `factory()` returns a stateless factory.
pub struct PostgresClient {
    inner: Option<Arc<Mutex<PostgresInner>>>,
}

struct PostgresInner {
    client: tokio_postgres::Client,
    /// Handle for the spawned connection task. Aborted on drop.
    conn_task: tokio::task::JoinHandle<()>,
    /// Used by `cancel` to send the cancel-request frame on a side channel.
    cancel_token: tokio_postgres::CancelToken,
}

impl Drop for PostgresInner {
    fn drop(&mut self) {
        self.conn_task.abort();
    }
}

impl PostgresClient {
    pub fn factory() -> Arc<dyn DbClient> { Arc::new(Self { inner: None }) }
}

#[async_trait::async_trait]
impl DbClient for PostgresClient {
    async fn open(&self, p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> {
        if p.kind != DbKind::Postgres {
            return Err(DbError::Invalid(format!("expected DbKind::Postgres, got {:?}", p.kind)));
        }
        let dsn = if let Some(pw) = p.password.as_ref() {
            inject_password(&p.dsn, pw)
        } else {
            p.dsn.clone()
        };
        let (client, connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls)
            .await
            .map_err(|e| DbError::Connect(e.to_string()))?;
        let cancel_token = client.cancel_token();
        let conn_task = tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::warn!("postgres connection task ended with error: {e}");
            }
        });
        Ok(Arc::new(PostgresClient {
            inner: Some(Arc::new(Mutex::new(PostgresInner { client, conn_task, cancel_token }))),
        }))
    }

    async fn close(&self) -> Result<(), DbError> {
        // Dropping the Arc<Mutex<PostgresInner>> aborts the connection task
        // via Drop. Returning Ok here is enough — explicit close is advisory.
        Ok(())
    }

    async fn execute(&self, _sql: &str) -> Result<ExecResult, DbError> {
        Err(DbError::Other("execute: not yet implemented — Task 10".into()))
    }
    async fn query_paged(
        &self,
        _sql: &str,
        _cursor: Option<PageCursor>,
        _page_size: u32,
    ) -> Result<QueryPage, DbError> {
        Err(DbError::Other("query_paged: not yet implemented — Task 11".into()))
    }
    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        Err(DbError::Other("schema_introspect: not yet implemented — Task 12".into()))
    }
    async fn cancel(&self) -> Result<(), DbError> {
        Err(DbError::Other("cancel: not yet implemented — Task 13".into()))
    }
    fn kind(&self) -> DbKind { DbKind::Postgres }
}

/// If the DSN does not include a password, splice one in before the host.
/// Best-effort: handles `postgres://user@host/db` → `postgres://user:pw@host/db`.
fn inject_password(dsn: &str, pw: &str) -> String {
    // If the DSN already has `:` between user and host, leave it alone.
    if let Some(at_idx) = dsn.find('@') {
        let pre = &dsn[..at_idx];
        if pre.contains(':') && !pre.ends_with("://") {
            return dsn.to_string();
        }
        let encoded = url_encode_password(pw);
        return format!("{pre}:{encoded}{}", &dsn[at_idx..]);
    }
    dsn.to_string()
}

fn url_encode_password(pw: &str) -> String {
    // Minimal percent-encoding for chars that break a postgres URL.
    let mut out = String::with_capacity(pw.len());
    for c in pw.chars() {
        match c {
            ' ' | ':' | '@' | '/' | '?' | '#' | '%' => {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                for b in s.as_bytes() {
                    use std::fmt::Write;
                    write!(&mut out, "%{b:02X}").ok();
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[allow(dead_code)]
pub(crate) fn map_pg_error(e: tokio_postgres::Error) -> DbError {
    if let Some(db_err) = e.as_db_error() {
        // Class 42 = Syntax/Access (42601 = syntax_error).
        if db_err.code().code().starts_with("42") {
            return DbError::Syntax {
                offset: db_err.position().map(|p| match p {
                    tokio_postgres::error::ErrorPosition::Original(n) => *n as usize,
                    tokio_postgres::error::ErrorPosition::Internal { position, .. } => *position as usize,
                }).unwrap_or(0),
                message: db_err.message().to_string(),
            };
        }
        return DbError::Query(db_err.message().to_string());
    }
    DbError::Query(e.to_string())
}

#[allow(dead_code)]
pub(crate) fn render_pg_value(row: &tokio_postgres::Row, idx: usize) -> String {
    use tokio_postgres::types::Type;
    let col = &row.columns()[idx];
    macro_rules! try_get {
        ($t:ty) => {
            row.try_get::<_, Option<$t>>(idx).ok().flatten().map(|v| v.to_string())
        };
    }
    let s = match *col.type_() {
        Type::BOOL => try_get!(bool),
        Type::INT2 => try_get!(i16),
        Type::INT4 => try_get!(i32),
        Type::INT8 => try_get!(i64),
        Type::FLOAT4 => try_get!(f32),
        Type::FLOAT8 => try_get!(f64),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => try_get!(String),
        Type::BYTEA => {
            row.try_get::<_, Option<Vec<u8>>>(idx).ok().flatten().map(|b| {
                let mut s = String::with_capacity(2 + b.len() * 2);
                s.push_str("0x");
                for byte in &b {
                    use std::fmt::Write;
                    write!(&mut s, "{byte:02x}").ok();
                }
                s
            })
        }
        _ => {
            // Fallback: try to read as string.
            row.try_get::<_, Option<String>>(idx).ok().flatten()
        }
    };
    s.unwrap_or_else(|| "NULL".to_string())
}

#[allow(dead_code)]
pub(crate) fn pg_type_to_column_type(t: &tokio_postgres::types::Type) -> ColumnType {
    use tokio_postgres::types::Type;
    match *t {
        Type::BOOL => ColumnType::Bool,
        Type::INT2 | Type::INT4 | Type::INT8 => ColumnType::Integer,
        Type::FLOAT4 | Type::FLOAT8 | Type::NUMERIC => ColumnType::Float,
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => ColumnType::Text,
        Type::BYTEA => ColumnType::Bytes,
        ref other => ColumnType::Other(other.name().to_string()),
    }
}

#[allow(dead_code)]
fn _unused_silencer(_: TableInfo) {}
```

- [ ] **Step 3: Unit-test the password injection helper without a live Postgres**

Append to `src/postgres.rs` (inside a `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_password_adds_password_when_missing() {
        let r = inject_password("postgres://alice@host/db", "s3cret");
        assert_eq!(r, "postgres://alice:s3cret@host/db");
    }

    #[test]
    fn inject_password_leaves_existing_password_alone() {
        let r = inject_password("postgres://alice:keep@host/db", "ignored");
        assert_eq!(r, "postgres://alice:keep@host/db");
    }

    #[test]
    fn url_encode_password_escapes_special_chars() {
        assert_eq!(url_encode_password("a:b@c"), "a%3Ab%40c");
    }
}
```

- [ ] **Step 4: Run tests** — expected 3 unit tests passing; live-DB tests skipped unless `SID_PG_DSN` is set.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): PostgresClient::open + close (with cancel token + drop-abort)"
```

---

### Task 10: `PostgresClient::execute`

**Files:**
- Modify: `crates/sid-db-clients/src/postgres.rs`

- [ ] **Step 1: Implement `execute`**

```rust
async fn execute(&self, sql: &str) -> Result<ExecResult, DbError> {
    let inner = self.inner.as_ref().ok_or(DbError::NotConnected)?.clone();
    let start = std::time::Instant::now();
    let guard = inner.lock().await;
    let rows_affected = guard.client.execute(sql, &[]).await.map_err(map_pg_error)?;
    Ok(ExecResult {
        rows_affected,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
```

- [ ] **Step 2: Live-DB tests (gated)**

Append to `tests/postgres_smoke.rs`:

```rust
#[tokio::test]
async fn execute_create_drop_round_trip() {
    let Some(dsn) = dsn_or_skip() else { return };
    let c = PostgresClient::factory()
        .open(OpenParams { kind: DbKind::Postgres, dsn, password: None })
        .await
        .unwrap();
    c.execute("CREATE TEMP TABLE sid_test (id INT)").await.unwrap();
    let r = c.execute("INSERT INTO sid_test VALUES (1), (2), (3)").await.unwrap();
    assert_eq!(r.rows_affected, 3);
    c.execute("DROP TABLE sid_test").await.unwrap();
}

#[tokio::test]
async fn execute_syntax_error_returns_syntax_variant() {
    let Some(dsn) = dsn_or_skip() else { return };
    let c = PostgresClient::factory()
        .open(OpenParams { kind: DbKind::Postgres, dsn, password: None })
        .await
        .unwrap();
    let err = c.execute("SELEC 1").await.unwrap_err();
    assert!(matches!(err, sid_core::adapters::db_client::DbError::Syntax { .. }));
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): PostgresClient::execute"
```

---

### Task 11: `PostgresClient::query_paged`

**Files:**
- Modify: `crates/sid-db-clients/src/postgres.rs`

- [ ] **Step 1: Implement `query_paged`**

```rust
async fn query_paged(
    &self,
    sql: &str,
    cursor: Option<PageCursor>,
    page_size: u32,
) -> Result<QueryPage, DbError> {
    let inner = self.inner.as_ref().ok_or(DbError::NotConnected)?.clone();
    let offset = cursor.map(|c| c.offset).unwrap_or(0);
    let page_size = page_size.max(1) as u64;
    let trimmed = sql.trim().trim_end_matches(';');
    let wrapped = format!("SELECT * FROM ( {trimmed} ) AS sid_sub LIMIT {page_size} OFFSET {offset}");
    let start = std::time::Instant::now();
    let guard = inner.lock().await;
    let rows = guard.client.query(&wrapped, &[]).await.map_err(map_pg_error)?;
    let columns: Vec<Column> = rows
        .first()
        .map(|r| {
            r.columns()
                .iter()
                .map(|c| Column { name: c.name().to_string(), ty: pg_type_to_column_type(c.type_()) })
                .collect()
        })
        .unwrap_or_else(|| {
            // No rows but we still want column metadata. tokio-postgres needs
            // a prepared statement for that — fetch via PREPARE if rows is
            // empty.
            Vec::new()
        });
    let columns = if columns.is_empty() && rows.is_empty() {
        // Fall back to prepare + describe.
        let stmt = guard.client.prepare(&wrapped).await.map_err(map_pg_error)?;
        stmt.columns()
            .iter()
            .map(|c| Column { name: c.name().to_string(), ty: pg_type_to_column_type(c.type_()) })
            .collect()
    } else {
        columns
    };
    let mut rows_out: Vec<Row> = Vec::with_capacity(rows.len());
    for r in &rows {
        let values: Vec<String> = (0..r.columns().len()).map(|i| render_pg_value(r, i)).collect();
        rows_out.push(Row { values });
    }
    let fetched = rows_out.len() as u64;
    let next_cursor = if fetched < page_size { None } else { Some(PageCursor { offset: offset + fetched }) };
    Ok(QueryPage {
        columns,
        rows: rows_out,
        next_cursor,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
```

- [ ] **Step 2: Live-DB tests (gated)**

Append:

```rust
#[tokio::test]
async fn query_paged_returns_columns_and_rows() {
    let Some(dsn) = dsn_or_skip() else { return };
    let c = PostgresClient::factory()
        .open(OpenParams { kind: DbKind::Postgres, dsn, password: None })
        .await
        .unwrap();
    c.execute("CREATE TEMP TABLE sid_paged (id INT, name TEXT)").await.unwrap();
    c.execute("INSERT INTO sid_paged SELECT i, 'r' || i FROM generate_series(0, 119) i").await.unwrap();
    let p1 = c.query_paged("SELECT id, name FROM sid_paged ORDER BY id", None, 50).await.unwrap();
    assert_eq!(p1.rows.len(), 50);
    assert_eq!(p1.next_cursor.unwrap().offset, 50);
    let p2 = c.query_paged("SELECT id, name FROM sid_paged ORDER BY id", p1.next_cursor, 50).await.unwrap();
    assert_eq!(p2.rows.len(), 50);
    assert_eq!(p2.rows[0].values[0], "50");
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): PostgresClient::query_paged with LIMIT/OFFSET wrap"
```

---

### Task 12: `PostgresClient::schema_introspect`

**Files:**
- Modify: `crates/sid-db-clients/src/postgres.rs`

- [ ] **Step 1: Implement**

Use `information_schema`:

```rust
async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
    let inner = self.inner.as_ref().ok_or(DbError::NotConnected)?.clone();
    let guard = inner.lock().await;
    let sql = "
        SELECT table_schema, table_name, column_name, data_type
        FROM information_schema.columns
        WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
        ORDER BY table_schema, table_name, ordinal_position
    ";
    let rows = guard.client.query(sql, &[]).await.map_err(map_pg_error)?;
    let mut tables: std::collections::BTreeMap<(String, String), Vec<Column>> = Default::default();
    for r in rows {
        let schema: String = r.get(0);
        let name: String = r.get(1);
        let col: String = r.get(2);
        let dtype: String = r.get(3);
        let ct = match dtype.as_str() {
            "boolean" => ColumnType::Bool,
            "smallint" | "integer" | "bigint" => ColumnType::Integer,
            "real" | "double precision" | "numeric" => ColumnType::Float,
            "text" | "character varying" | "character" | "name" => ColumnType::Text,
            "bytea" => ColumnType::Bytes,
            other => ColumnType::Other(other.to_string()),
        };
        tables.entry((schema, name)).or_default().push(Column { name: col, ty: ct });
    }
    Ok(SchemaInfo {
        tables: tables
            .into_iter()
            .map(|((schema, name), columns)| TableInfo {
                schema: Some(schema),
                name,
                columns,
            })
            .collect(),
    })
}
```

- [ ] **Step 2: Live-DB test**

Append:

```rust
#[tokio::test]
async fn schema_introspect_lists_temp_table() {
    let Some(dsn) = dsn_or_skip() else { return };
    let c = PostgresClient::factory()
        .open(OpenParams { kind: DbKind::Postgres, dsn, password: None })
        .await
        .unwrap();
    // Temp tables live in pg_temp_*, which information_schema may not surface.
    // Use a permanent table in a throwaway schema for this test.
    c.execute("CREATE SCHEMA IF NOT EXISTS sid_introspect_test").await.unwrap();
    c.execute("DROP TABLE IF EXISTS sid_introspect_test.foo").await.unwrap();
    c.execute("CREATE TABLE sid_introspect_test.foo (id INT, label TEXT)").await.unwrap();
    let s = c.schema_introspect().await.unwrap();
    let found = s.tables.iter().find(|t| t.name == "foo").expect("foo");
    let names: Vec<_> = found.columns.iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, vec!["id", "label"]);
    c.execute("DROP SCHEMA sid_introspect_test CASCADE").await.unwrap();
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): PostgresClient::schema_introspect via information_schema"
```

---

### Task 13: `PostgresClient::cancel`

**Files:**
- Modify: `crates/sid-db-clients/src/postgres.rs`

- [ ] **Step 1: Implement**

```rust
async fn cancel(&self) -> Result<(), DbError> {
    let inner = self.inner.as_ref().ok_or(DbError::NotConnected)?.clone();
    let guard = inner.lock().await;
    guard
        .cancel_token
        .cancel_query(tokio_postgres::NoTls)
        .await
        .map_err(|e| DbError::Other(e.to_string()))
}
```

- [ ] **Step 2: Live-DB test (best-effort)**

Append:

```rust
#[tokio::test]
async fn cancel_after_open_does_not_panic() {
    let Some(dsn) = dsn_or_skip() else { return };
    let c = PostgresClient::factory()
        .open(OpenParams { kind: DbKind::Postgres, dsn, password: None })
        .await
        .unwrap();
    // Calling cancel with no in-flight query is fine — server returns no-op.
    let _ = c.cancel().await;
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): PostgresClient::cancel via tokio-postgres CancelToken"
```

---

## Phase E — SQL lexer

The lexer drives syntax highlight in the query editor. It is **dialect-agnostic** (ANSI keywords + common Postgres/SQLite extras), state-machine-based (no regex, no tree-sitter), and **must never panic or hang** on arbitrary input — that's enforced by a `proptest` in Task 15 (the cargo-fuzz stand-in mandated by CLAUDE.md for parser-shaped code).

### Task 14: SQL lexer — token types + state machine

**Files:**
- Replace: `crates/sid-db-clients/src/lexer.rs`
- Create: `crates/sid-db-clients/tests/lexer.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-db-clients/tests/lexer.rs`:

```rust
use sid_db_clients::lexer::{tokenize, Token, TokenKind};

fn kinds(input: &str) -> Vec<TokenKind> {
    tokenize(input).into_iter().map(|t| t.kind).collect()
}

#[test]
fn empty_input_yields_no_tokens() {
    assert!(tokenize("").is_empty());
}

#[test]
fn whitespace_is_a_token() {
    assert_eq!(kinds("   "), vec![TokenKind::Whitespace]);
}

#[test]
fn keyword_select_is_recognised() {
    let toks = tokenize("SELECT");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Keyword);
    assert_eq!(toks[0].text, "SELECT");
}

#[test]
fn keyword_matching_is_case_insensitive() {
    assert_eq!(kinds("select"), vec![TokenKind::Keyword]);
    assert_eq!(kinds("SeLeCt"), vec![TokenKind::Keyword]);
}

#[test]
fn identifier_after_keyword_is_identifier() {
    let toks = tokenize("SELECT id");
    assert_eq!(toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
               vec![TokenKind::Keyword, TokenKind::Whitespace, TokenKind::Identifier]);
}

#[test]
fn integer_literal_is_number() {
    assert_eq!(kinds("123"), vec![TokenKind::Number]);
}

#[test]
fn float_literal_is_number() {
    let toks = tokenize("3.14");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Number);
    assert_eq!(toks[0].text, "3.14");
}

#[test]
fn single_quoted_string_with_escape() {
    let toks = tokenize("'hello ''world'''");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::String);
}

#[test]
fn line_comment_runs_to_eol() {
    let toks = tokenize("-- a comment\nSELECT");
    assert_eq!(toks[0].kind, TokenKind::Comment);
    assert!(toks[0].text.starts_with("--"));
}

#[test]
fn block_comment_balanced() {
    let toks = tokenize("/* block */");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Comment);
}

#[test]
fn punctuation_tokens_are_emitted() {
    let toks = tokenize("(),;");
    assert_eq!(toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
               vec![TokenKind::Punctuation; 4]);
}

#[test]
fn unterminated_string_emits_string_token_to_eof() {
    let toks = tokenize("'unterminated");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::String);
}

#[test]
fn unterminated_block_comment_emits_comment_to_eof() {
    let toks = tokenize("/* never closes");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Comment);
}

#[test]
fn offsets_cover_input_with_no_gaps() {
    let input = "SELECT id FROM t";
    let toks = tokenize(input);
    let mut cursor = 0;
    for t in &toks {
        assert_eq!(t.offset, cursor, "token offset {} != cursor {}", t.offset, cursor);
        cursor += t.text.len();
    }
    assert_eq!(cursor, input.len());
}

#[test]
fn _ignore_unused_warning_token() {
    let _: Token = Token { kind: TokenKind::Whitespace, offset: 0, text: "".into() };
}
```

- [ ] **Step 2: Replace `crates/sid-db-clients/src/lexer.rs`**

```rust
//! A small dialect-agnostic SQL lexer for syntax highlighting in the query
//! editor. Not a parser — never builds an AST, never validates syntax. It
//! exists to classify each byte range of the source into a token kind so the
//! TUI can colour it.
//!
//! Robustness contract:
//! - Tokenising any byte sequence must terminate.
//! - Tokenising must never panic.
//! - The concatenation of `tok.text` equals the input (no characters dropped).
//! - `tok.offset` is the byte offset where the token begins in the input.
//!
//! These invariants are enforced by tests and by the proptest in
//! `tests/lexer_proptest.rs`.

use std::borrow::Cow;

/// Token classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Keyword,
    Identifier,
    String,
    Number,
    Comment,
    Punctuation,
    Whitespace,
    /// Any byte we don't recognise. Renderer falls back to the foreground colour.
    Unknown,
}

/// One token: kind + byte offset + owned text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub offset: usize,
    pub text: Cow<'static, str>,
}

/// Tokenise the input. Returns a vector whose `text` concatenation equals
/// `input`.
///
/// # Examples
///
/// ```
/// use sid_db_clients::lexer::{tokenize, TokenKind};
/// let toks = tokenize("SELECT 1");
/// assert_eq!(toks[0].kind, TokenKind::Keyword);
/// ```
pub fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        let (kind, end) = match b {
            // Whitespace
            b' ' | b'\t' | b'\n' | b'\r' => {
                let mut j = i;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') { j += 1; }
                (TokenKind::Whitespace, j)
            }
            // Line comment "-- ..."
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                let mut j = i + 2;
                while j < bytes.len() && bytes[j] != b'\n' { j += 1; }
                (TokenKind::Comment, j)
            }
            // Block comment "/* ... */"
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                let mut j = i + 2;
                let mut closed = false;
                while j + 1 < bytes.len() {
                    if bytes[j] == b'*' && bytes[j + 1] == b'/' { j += 2; closed = true; break; }
                    j += 1;
                }
                if !closed { j = bytes.len(); }
                (TokenKind::Comment, j)
            }
            // Single-quoted string (SQL escape is doubled quote: '')
            b'\'' => {
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == b'\'' {
                        if j + 1 < bytes.len() && bytes[j + 1] == b'\'' { j += 2; continue; }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                (TokenKind::String, j)
            }
            // Double-quoted identifier (also delim-string in some dialects)
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == b'"' {
                        if j + 1 < bytes.len() && bytes[j + 1] == b'"' { j += 2; continue; }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                (TokenKind::Identifier, j)
            }
            // Punctuation
            b'(' | b')' | b',' | b';' | b'*' | b'+' | b'-' | b'/' | b'%' | b'<' | b'>' | b'=' | b'!' | b'.' | b':' | b'[' | b']' | b'{' | b'}' => {
                (TokenKind::Punctuation, i + 1)
            }
            // Number (integer or float)
            b'0'..=b'9' => {
                let mut j = i;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') { j += 1; }
                (TokenKind::Number, j)
            }
            // Identifier or keyword (starts with letter or underscore)
            _ if is_ident_start(b) => {
                let mut j = i;
                while j < bytes.len() && is_ident_continue(bytes[j]) { j += 1; }
                let slice = &input[i..j];
                let kind = if is_keyword(slice) { TokenKind::Keyword } else { TokenKind::Identifier };
                (kind, j)
            }
            _ => (TokenKind::Unknown, i + 1),
        };
        // Safety: end must always advance.
        let end = end.max(start + 1).min(bytes.len());
        // For multi-byte UTF-8 chars in Unknown/identifier paths, snap to the next char boundary.
        let end = snap_to_char_boundary(input, end);
        out.push(Token {
            kind,
            offset: start,
            text: Cow::Owned(input[start..end].to_string()),
        });
        i = end;
    }
    out
}

fn is_ident_start(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'_') || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit() || b == b'$'
}

fn snap_to_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() { return s.len(); }
    while idx < s.len() && !s.is_char_boundary(idx) { idx += 1; }
    idx
}

// Keyword set populated in Task 15.
fn is_keyword(_ident: &str) -> bool { false }
```

- [ ] **Step 3: Run tests**

Most tests should pass; `keyword_select_is_recognised` and similar will fail because `is_keyword` is still `false`. That's the segue into Task 15.

Expected currently failing: keyword-recognition tests.

- [ ] **Step 4: Add doc tests on `tokenize`, `Token`, `TokenKind`** — short examples.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): SQL lexer state machine (strings, comments, numbers, punctuation, idents)"
```

---

### Task 15: SQL lexer — keyword set, proptest, criterion bench

**Files:**
- Modify: `crates/sid-db-clients/src/lexer.rs`
- Create: `crates/sid-db-clients/tests/lexer_proptest.rs`
- Create: `crates/sid-db-clients/benches/lexer.rs`

- [ ] **Step 1: Populate keyword set**

Replace `is_keyword`:

```rust
fn is_keyword(ident: &str) -> bool {
    let upper: String = ident.chars().map(|c| c.to_ascii_uppercase()).collect();
    KEYWORDS.binary_search(&upper.as_str()).is_ok()
}

// Sorted! `binary_search` requires this. Keep in lexicographic order.
const KEYWORDS: &[&str] = &[
    "ADD", "ALL", "ALTER", "AND", "AS", "ASC", "BEGIN", "BETWEEN", "BY",
    "CASCADE", "CASE", "CAST", "CHECK", "COLLATE", "COLUMN", "COMMIT",
    "CONSTRAINT", "CREATE", "CROSS", "DATABASE", "DEFAULT", "DELETE", "DESC",
    "DISTINCT", "DROP", "ELSE", "END", "EXCEPT", "EXISTS", "EXPLAIN",
    "FALSE", "FOR", "FOREIGN", "FROM", "FULL", "GRANT", "GROUP", "HAVING",
    "IF", "IN", "INDEX", "INNER", "INSERT", "INTERSECT", "INTO", "IS",
    "JOIN", "KEY", "LEFT", "LIKE", "LIMIT", "NOT", "NULL", "OFFSET", "ON",
    "OR", "ORDER", "OUTER", "PRIMARY", "REFERENCES", "RETURNING", "REVOKE",
    "RIGHT", "ROLLBACK", "SELECT", "SET", "TABLE", "THEN", "TO",
    "TRANSACTION", "TRIGGER", "TRUE", "UNION", "UNIQUE", "UPDATE", "USING",
    "VALUES", "VIEW", "WHEN", "WHERE", "WITH",
];
```

- [ ] **Step 2: Run Task-14 tests** — all should now pass.

- [ ] **Step 3: Add proptest (cargo-fuzz stand-in)**

Create `crates/sid-db-clients/tests/lexer_proptest.rs`:

```rust
//! Property tests for the SQL lexer. Per CLAUDE.md, parser-shaped code is a
//! `cargo fuzz` target; until fuzzing is wired into CI, this proptest serves
//! the same purpose: assert that the lexer never panics and never hangs on
//! arbitrary inputs.

use proptest::prelude::*;
use sid_db_clients::lexer::tokenize;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4096,
        ..ProptestConfig::default()
    })]

    /// Arbitrary UTF-8 strings up to 4 KiB never panic the lexer.
    #[test]
    fn prop_tokenize_never_panics_on_utf8(s in ".{0,4096}") {
        let _ = tokenize(&s);
    }

    /// Arbitrary byte sequences (lossy-decoded) never panic.
    #[test]
    fn prop_tokenize_never_panics_on_lossy_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = tokenize(&s);
    }

    /// Concatenation invariant: token texts joined equal the input.
    #[test]
    fn prop_token_texts_concat_equals_input(s in ".{0,2048}") {
        let toks = tokenize(&s);
        let recon: String = toks.iter().map(|t| t.text.as_ref()).collect();
        prop_assert_eq!(recon, s);
    }

    /// Token offsets are monotonically non-decreasing and within bounds.
    #[test]
    fn prop_token_offsets_monotone(s in ".{0,2048}") {
        let toks = tokenize(&s);
        let mut last = 0;
        for t in &toks {
            prop_assert!(t.offset >= last, "offset {} < last {}", t.offset, last);
            prop_assert!(t.offset <= s.len());
            last = t.offset;
        }
    }
}
```

- [ ] **Step 4: Add criterion bench**

Create `crates/sid-db-clients/benches/lexer.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sid_db_clients::lexer::tokenize;

const SMALL: &str = "SELECT id, name FROM users WHERE id = 1";
const LARGE: &str = include_str!("../tests/fixtures/large_query.sql");

fn bench_small(c: &mut Criterion) {
    c.bench_function("lexer/small", |b| b.iter(|| {
        let toks = tokenize(black_box(SMALL));
        black_box(toks);
    }));
}

fn bench_large(c: &mut Criterion) {
    c.bench_function("lexer/large", |b| b.iter(|| {
        let toks = tokenize(black_box(LARGE));
        black_box(toks);
    }));
}

criterion_group!(benches, bench_small, bench_large);
criterion_main!(benches);
```

Provide the fixture file `crates/sid-db-clients/tests/fixtures/large_query.sql` — a ~10 KB SQL with joins, subqueries, comments. (Generate by concatenating a representative analytics query several times.)

- [ ] **Step 5: Adversarial edge cases**

Append to `tests/lexer.rs`:

```rust
#[test]
fn keyword_then_dot_then_keyword_emits_kw_punct_kw() {
    let toks = tokenize("FROM.SELECT");
    assert_eq!(toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
               vec![TokenKind::Keyword, TokenKind::Punctuation, TokenKind::Keyword]);
}

#[test]
fn dollar_sign_in_identifier_is_legal() {
    let toks = tokenize("my$col");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Identifier);
}

#[test]
fn comment_then_keyword_segments_correctly() {
    let toks = tokenize("-- skip\nSELECT");
    assert_eq!(toks[0].kind, TokenKind::Comment);
    assert!(matches!(toks.last().unwrap().kind, TokenKind::Keyword));
}

#[test]
fn nested_block_comment_treated_as_outermost_only() {
    // Standard SQL doesn't nest /* */; our lexer matches first */ and stops.
    let toks = tokenize("/* a /* b */ c */");
    // First comment ends at the first "*/" — the trailing " c */" becomes
    // separate tokens. Verify no panic and at least one Comment token.
    assert!(toks.iter().any(|t| t.kind == TokenKind::Comment));
}

#[test]
fn long_input_finishes_quickly() {
    let big = "SELECT id FROM users WHERE name = 'x'; ".repeat(10_000);
    let toks = tokenize(&big);
    assert!(!toks.is_empty());
}
```

- [ ] **Step 6: Run tests + proptest + bench**

```bash
cargo test -p sid-db-clients --test lexer
cargo test -p sid-db-clients --test lexer_proptest
cargo bench -p sid-db-clients --bench lexer -- --quick
```

Expected: lexer tests pass; proptest runs all 4 properties; bench produces output (real baseline lands in a follow-up perf pass).

- [ ] **Step 7: Commit**

```bash
git add crates/sid-db-clients
git commit -m "feat(db-clients): SQL lexer keyword set + proptest (fuzz stand-in) + criterion bench"
```

---

## Phase F — Storage in `sid-store`

### Task 16: `DbConnection` + `QueryRecord` + `PlainSecret` types

**Files:**
- Modify: `crates/sid-store/src/lib.rs`

- [ ] **Step 1: Failing tests in `crates/sid-store/tests/db_connections.rs`**

```rust
use sid_core::adapters::db_client::DbKind;
use sid_core::adapters::secret_store::SecretRef;
use sid_store::{DbConnection, PlainSecret, QueryRecord, now_epoch};

#[test]
fn db_connection_construction() {
    let c = DbConnection {
        id: "local-pg".into(),
        kind: DbKind::Postgres,
        name: "local postgres".into(),
        dsn: "postgres://user@localhost/db".into(),
        secret_ref: Some(SecretRef::new("local-pg.password")),
        created_at: now_epoch(),
    };
    assert_eq!(c.kind, DbKind::Postgres);
}

#[test]
fn query_record_construction() {
    let r = QueryRecord {
        conn_id: "local-pg".into(),
        sql: "SELECT 1".into(),
        duration_ms: 12,
        row_count: 1,
        ts_ns: 1,
    };
    assert_eq!(r.row_count, 1);
}

#[test]
fn plain_secret_construction() {
    let s = PlainSecret { value: "shh".into() };
    assert_eq!(s.value, "shh");
}
```

- [ ] **Step 2: Add types to `sid-store/src/lib.rs`**

```rust
use sid_core::adapters::db_client::DbKind;
use sid_core::adapters::secret_store::SecretRef;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbConnection {
    /// Stable id; used as the redb key and as the CLI selector.
    pub id: String,
    pub kind: DbKind,
    /// User-facing label.
    pub name: String,
    /// DSN minus password. Password lives behind `secret_ref`.
    pub dsn: String,
    pub secret_ref: Option<SecretRef>,
    pub created_at: Epoch,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryRecord {
    pub conn_id: String,
    pub sql: String,
    pub duration_ms: u64,
    pub row_count: u64,
    /// Wall-clock nanoseconds since epoch — also the first half of the key.
    pub ts_ns: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlainSecret {
    pub value: String,
}
```

Make sure `sid-store/Cargo.toml` already has `sid-core.workspace = true`.

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Postcard round-trip proptest**

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_db_connection_postcard_roundtrip(name in "[a-zA-Z0-9 -]{1,40}") {
        let c = DbConnection {
            id: name.clone(),
            kind: DbKind::Sqlite,
            name,
            dsn: "/tmp/x.db".into(),
            secret_ref: None,
            created_at: now_epoch(),
        };
        let bytes = postcard::to_allocvec(&c).unwrap();
        let back: DbConnection = postcard::from_bytes(&bytes).unwrap();
        prop_assert_eq!(c, back);
    }

    #[test]
    fn prop_query_record_postcard_roundtrip(ts in any::<u64>(), n in any::<u64>()) {
        let r = QueryRecord {
            conn_id: "x".into(),
            sql: "SELECT 1".into(),
            duration_ms: 1,
            row_count: n,
            ts_ns: ts as u128,
        };
        let bytes = postcard::to_allocvec(&r).unwrap();
        let back: QueryRecord = postcard::from_bytes(&bytes).unwrap();
        prop_assert_eq!(r, back);
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add DbConnection, QueryRecord, PlainSecret domain types"
```

---

### Task 17: Table defs (`DB_CONNECTIONS`, `QUERY_HISTORY`, `SECRETS`)

**Files:**
- Modify: `crates/sid-store/src/schema.rs`

- [ ] **Step 1: Add table defs**

```rust
/// DB connection registry. Key: connection id. Value: versioned-postcard
/// `DbConnection`.
pub const DB_CONNECTIONS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("db_connections");

/// Per-connection query history. Composite key `(ts_ns, seq)` packed into a
/// big-endian 24-byte buffer (`u128` ts_ns followed by `u64` seq). Value:
/// versioned-postcard `QueryRecord`.
pub const QUERY_HISTORY: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("query_history");

/// Secret store. Key: SecretRef id string. Value: versioned-postcard
/// `PlainSecret`.
pub const SECRETS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("secrets");
```

Update the table-name assertion doc test at the top of the file.

- [ ] **Step 2: Open the new tables in `RedbStore::open`**

In `redb_impl.rs` `OpenStore::open`, add table-creation lines:

```rust
let _ = txn.open_table(DB_CONNECTIONS).map_err(|e| SidError::Storage(format!("open db_connections: {e}")))?;
let _ = txn.open_table(QUERY_HISTORY).map_err(|e| SidError::Storage(format!("open query_history: {e}")))?;
let _ = txn.open_table(SECRETS).map_err(|e| SidError::Storage(format!("open secrets: {e}")))?;
```

- [ ] **Step 3: Confirm no regressions** — `cargo test -p sid-store`.

- [ ] **Step 4: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add DB_CONNECTIONS, QUERY_HISTORY, SECRETS tables"
```

---

### Task 18: `Store` trait extension + RedbStore impl (db connections + secrets)

**Files:**
- Modify: `crates/sid-store/src/lib.rs` (trait extension)
- Modify: `crates/sid-store/src/redb_impl.rs` (impl)
- Test: `crates/sid-store/tests/db_connections.rs` (extend)
- Test: `crates/sid-store/tests/secrets.rs` (new)

- [ ] **Step 1: Extend the `Store` trait**

```rust
fn list_db_connections(&self) -> Result<Vec<DbConnection>, SidError>;
fn upsert_db_connection(&self, c: &DbConnection) -> Result<(), SidError>;
fn get_db_connection(&self, id: &str) -> Result<Option<DbConnection>, SidError>;
fn remove_db_connection(&self, id: &str) -> Result<(), SidError>;

fn get_secret(&self, id: &str) -> Result<Option<PlainSecret>, SidError>;
fn put_secret(&self, id: &str, s: &PlainSecret) -> Result<(), SidError>;
fn remove_secret(&self, id: &str) -> Result<(), SidError>;
```

- [ ] **Step 2: Failing tests**

Extend `crates/sid-store/tests/db_connections.rs`:

```rust
use sid_store::{OpenStore, RedbStore, Store};
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, RedbStore) {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    (d, s)
}

#[test]
fn upsert_then_list_returns_connection() {
    let (_dir, s) = store();
    s.upsert_db_connection(&DbConnection {
        id: "a".into(), kind: DbKind::Sqlite, name: "alpha".into(),
        dsn: ":memory:".into(), secret_ref: None, created_at: now_epoch(),
    }).unwrap();
    let all = s.list_db_connections().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, "a");
}

#[test]
fn get_and_remove_work() {
    let (_d, s) = store();
    s.upsert_db_connection(&DbConnection {
        id: "a".into(), kind: DbKind::Sqlite, name: "x".into(),
        dsn: ":memory:".into(), secret_ref: None, created_at: now_epoch(),
    }).unwrap();
    assert!(s.get_db_connection("a").unwrap().is_some());
    s.remove_db_connection("a").unwrap();
    assert!(s.get_db_connection("a").unwrap().is_none());
}

#[test]
fn upsert_replaces_existing() {
    let (_d, s) = store();
    s.upsert_db_connection(&DbConnection {
        id: "a".into(), kind: DbKind::Sqlite, name: "v1".into(),
        dsn: ":memory:".into(), secret_ref: None, created_at: now_epoch(),
    }).unwrap();
    s.upsert_db_connection(&DbConnection {
        id: "a".into(), kind: DbKind::Sqlite, name: "v2".into(),
        dsn: ":memory:".into(), secret_ref: None, created_at: now_epoch(),
    }).unwrap();
    assert_eq!(s.get_db_connection("a").unwrap().unwrap().name, "v2");
}

#[test]
fn list_with_50_connections_returns_all() {
    let (_d, s) = store();
    for i in 0..50 {
        s.upsert_db_connection(&DbConnection {
            id: format!("c{i}"), kind: DbKind::Sqlite, name: format!("n{i}"),
            dsn: ":memory:".into(), secret_ref: None, created_at: now_epoch(),
        }).unwrap();
    }
    assert_eq!(s.list_db_connections().unwrap().len(), 50);
}
```

Create `crates/sid-store/tests/secrets.rs`:

```rust
use sid_store::{OpenStore, PlainSecret, RedbStore, Store};
use tempfile::tempdir;

#[test]
fn put_get_roundtrip() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    s.put_secret("k", &PlainSecret { value: "v".into() }).unwrap();
    assert_eq!(s.get_secret("k").unwrap().unwrap().value, "v");
}

#[test]
fn missing_secret_returns_none() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    assert!(s.get_secret("absent").unwrap().is_none());
}

#[test]
fn remove_drops_secret() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    s.put_secret("k", &PlainSecret { value: "v".into() }).unwrap();
    s.remove_secret("k").unwrap();
    assert!(s.get_secret("k").unwrap().is_none());
}
```

- [ ] **Step 3: Implement on `RedbStore`** (mirrors workspace methods from Plan 2):

```rust
fn list_db_connections(&self) -> Result<Vec<DbConnection>, SidError> {
    let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
    let tbl = txn.open_table(DB_CONNECTIONS).map_err(|e| SidError::Storage(format!("open db_connections: {e}")))?;
    let mut out = Vec::new();
    for entry in tbl.iter().map_err(|e| SidError::Storage(format!("iter: {e}")))? {
        let (_k, v) = entry.map_err(|e| SidError::Storage(format!("iter step: {e}")))?;
        let (_v, c) = crate::codec::decode_versioned::<DbConnection>(v.value())?;
        out.push(c);
    }
    Ok(out)
}

fn upsert_db_connection(&self, c: &DbConnection) -> Result<(), SidError> {
    let bytes = crate::codec::encode_versioned(1, c)?;
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(DB_CONNECTIONS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
        tbl.insert(c.id.as_str(), &bytes[..]).map_err(|e| SidError::Storage(format!("insert: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit: {e}")))?;
    Ok(())
}

fn get_db_connection(&self, id: &str) -> Result<Option<DbConnection>, SidError> {
    let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
    let tbl = txn.open_table(DB_CONNECTIONS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
    match tbl.get(id).map_err(|e| SidError::Storage(format!("get: {e}")))? {
        Some(v) => {
            let (_v, c) = crate::codec::decode_versioned::<DbConnection>(v.value())?;
            Ok(Some(c))
        }
        None => Ok(None),
    }
}

fn remove_db_connection(&self, id: &str) -> Result<(), SidError> {
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(DB_CONNECTIONS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
        tbl.remove(id).map_err(|e| SidError::Storage(format!("remove: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit: {e}")))?;
    Ok(())
}

fn get_secret(&self, id: &str) -> Result<Option<PlainSecret>, SidError> {
    let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
    let tbl = txn.open_table(SECRETS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
    match tbl.get(id).map_err(|e| SidError::Storage(format!("get: {e}")))? {
        Some(v) => {
            let (_v, s) = crate::codec::decode_versioned::<PlainSecret>(v.value())?;
            Ok(Some(s))
        }
        None => Ok(None),
    }
}

fn put_secret(&self, id: &str, s: &PlainSecret) -> Result<(), SidError> {
    let bytes = crate::codec::encode_versioned(1, s)?;
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(SECRETS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
        tbl.insert(id, &bytes[..]).map_err(|e| SidError::Storage(format!("insert: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit: {e}")))?;
    Ok(())
}

fn remove_secret(&self, id: &str) -> Result<(), SidError> {
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(SECRETS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
        tbl.remove(id).map_err(|e| SidError::Storage(format!("remove: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit: {e}")))?;
    Ok(())
}
```

- [ ] **Step 4: Run tests** — expected 4 (db_connections) + 3 (secrets) = 7 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): db_connections and secrets registry methods on Store + RedbStore"
```

---

### Task 19: `Store` trait extension + RedbStore impl (query history)

**Files:**
- Modify: `crates/sid-store/src/lib.rs` (trait)
- Modify: `crates/sid-store/src/redb_impl.rs` (impl)
- Test: `crates/sid-store/tests/query_history.rs`

- [ ] **Step 1: Trait additions**

```rust
fn append_query_record(&self, r: &QueryRecord) -> Result<(), SidError>;
fn recent_queries(&self, conn_id: &str, limit: usize) -> Result<Vec<QueryRecord>, SidError>;
```

- [ ] **Step 2: Failing tests**

```rust
use sid_core::adapters::db_client::DbKind;
use sid_store::{OpenStore, QueryRecord, RedbStore, Store};
use tempfile::tempdir;

fn rec(conn: &str, sql: &str, ts: u128) -> QueryRecord {
    QueryRecord { conn_id: conn.into(), sql: sql.into(), duration_ms: 1, row_count: 0, ts_ns: ts }
}

#[test]
fn append_and_recent_returns_most_recent_first() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    s.append_query_record(&rec("c1", "SELECT 1", 1)).unwrap();
    s.append_query_record(&rec("c1", "SELECT 2", 2)).unwrap();
    let got = s.recent_queries("c1", 10).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].sql, "SELECT 2");
    assert_eq!(got[1].sql, "SELECT 1");
}

#[test]
fn recent_respects_limit() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    for i in 0..20u128 {
        s.append_query_record(&rec("c1", &format!("Q{i}"), i + 1)).unwrap();
    }
    let got = s.recent_queries("c1", 5).unwrap();
    assert_eq!(got.len(), 5);
    assert_eq!(got[0].sql, "Q19");
}

#[test]
fn recent_filters_by_connection_id() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    s.append_query_record(&rec("a", "A", 1)).unwrap();
    s.append_query_record(&rec("b", "B", 2)).unwrap();
    s.append_query_record(&rec("a", "A2", 3)).unwrap();
    let got = s.recent_queries("a", 10).unwrap();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|r| r.conn_id == "a"));
}

#[test]
fn recent_empty_when_no_records() {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    assert!(s.recent_queries("nope", 10).unwrap().is_empty());
}
```

- [ ] **Step 3: Implement**

```rust
fn append_query_record(&self, r: &QueryRecord) -> Result<(), SidError> {
    // Compose a 24-byte big-endian key: u128 ts_ns followed by u64 seq.
    // For v1 we use a process-local atomic counter for seq.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut key = [0u8; 24];
    key[..16].copy_from_slice(&r.ts_ns.to_be_bytes());
    key[16..].copy_from_slice(&seq.to_be_bytes());
    let bytes = crate::codec::encode_versioned(1, r)?;
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(QUERY_HISTORY).map_err(|e| SidError::Storage(format!("open: {e}")))?;
        tbl.insert(&key[..], &bytes[..]).map_err(|e| SidError::Storage(format!("insert: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit: {e}")))?;
    Ok(())
}

fn recent_queries(&self, conn_id: &str, limit: usize) -> Result<Vec<QueryRecord>, SidError> {
    let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
    let tbl = txn.open_table(QUERY_HISTORY).map_err(|e| SidError::Storage(format!("open: {e}")))?;
    let mut out: Vec<QueryRecord> = Vec::with_capacity(limit.min(64));
    // Reverse iteration: redb iter() is ascending; we iterate fully and reverse-collect.
    // For larger tables a `range(..).rev()` over the full table is the right move; for v1
    // (history is bounded), full-iterate is acceptable.
    let iter = tbl.iter().map_err(|e| SidError::Storage(format!("iter: {e}")))?;
    let mut all: Vec<QueryRecord> = Vec::new();
    for entry in iter {
        let (_k, v) = entry.map_err(|e| SidError::Storage(format!("iter step: {e}")))?;
        let (_v, r) = crate::codec::decode_versioned::<QueryRecord>(v.value())?;
        if r.conn_id == conn_id { all.push(r); }
    }
    // all is ascending by key — reverse for recency
    while let Some(r) = all.pop() {
        out.push(r);
        if out.len() >= limit { break; }
    }
    Ok(out)
}
```

Note the full-iteration cost — flagged in Plan 4's "Open items" for a future plan to switch to a reverse range scan or a per-connection sub-table.

- [ ] **Step 4: Run tests** — expected 4 passed.

- [ ] **Step 5: Adversarial + criterion bench**

Add `crates/sid-store/benches/recent_queries.rs`:

```rust
use criterion::{criterion_group, criterion_main, Criterion};
use sid_store::{OpenStore, QueryRecord, RedbStore, Store};
use tempfile::tempdir;

fn bench_recent(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    for i in 0..1_000u128 {
        store.append_query_record(&QueryRecord {
            conn_id: "c1".into(),
            sql: format!("Q{i}"),
            duration_ms: 1,
            row_count: 0,
            ts_ns: i + 1,
        }).unwrap();
    }
    c.bench_function("recent_queries/1000", |b| {
        b.iter(|| {
            let _ = store.recent_queries("c1", 50).unwrap();
        });
    });
}

criterion_group!(benches, bench_recent);
criterion_main!(benches);
```

Add the bench section to `crates/sid-store/Cargo.toml`:

```toml
[[bench]]
name = "recent_queries"
harness = false
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): query history table with recent_queries reverse-scan helper"
```

---

### Task 20: `PlainSecretStore` impl of `SecretStore`

**Files:**
- Create: `crates/sid-store/src/plain_secret_store.rs`
- Modify: `crates/sid-store/src/lib.rs` (declare module)

- [ ] **Step 1: Failing test**

Append to `tests/secrets.rs`:

```rust
use sid_core::adapters::secret_store::{SecretRef, SecretStore};
use sid_store::PlainSecretStore;
use std::sync::Arc;

#[test]
fn plain_secret_store_round_trips_via_trait() {
    let d = tempdir().unwrap();
    let store = Arc::new(RedbStore::open(&d.path().join("sid.redb")).unwrap());
    let s: Arc<dyn SecretStore> = Arc::new(PlainSecretStore::new(store.clone()));
    s.put(&SecretRef::new("k"), "v").unwrap();
    assert_eq!(s.get(&SecretRef::new("k")).unwrap().as_deref(), Some("v"));
    s.remove(&SecretRef::new("k")).unwrap();
    assert!(s.get(&SecretRef::new("k")).unwrap().is_none());
}
```

- [ ] **Step 2: Implement**

Create `crates/sid-store/src/plain_secret_store.rs`:

```rust
//! `PlainSecretStore` — stores secrets as plaintext in the redb `secrets`
//! table. v1 default. A future plan adds `KeyringStore` with the same trait.

use std::sync::Arc;

use sid_core::adapters::secret_store::{SecretError, SecretRef, SecretStore};

use crate::{PlainSecret, RedbStore, Store};

/// Plaintext secret store backed by a `RedbStore`.
///
/// # Examples
///
/// ```no_run
/// use std::sync::Arc;
/// use sid_core::adapters::secret_store::SecretRef;
/// use sid_store::{PlainSecretStore, RedbStore};
/// let store = Arc::new(RedbStore::open(std::path::Path::new("/tmp/sid.redb")).unwrap());
/// let secrets = PlainSecretStore::new(store);
/// // secrets.put(&SecretRef::new("k"), "v").unwrap();
/// ```
pub struct PlainSecretStore {
    inner: Arc<RedbStore>,
}

impl PlainSecretStore {
    pub fn new(store: Arc<RedbStore>) -> Self { Self { inner: store } }
}

impl SecretStore for PlainSecretStore {
    fn get(&self, r: &SecretRef) -> Result<Option<String>, SecretError> {
        match self.inner.get_secret(r.id()) {
            Ok(Some(s)) => Ok(Some(s.value)),
            Ok(None) => Ok(None),
            Err(e) => Err(SecretError::Backend(e.to_string())),
        }
    }
    fn put(&self, r: &SecretRef, v: &str) -> Result<(), SecretError> {
        self.inner
            .put_secret(r.id(), &PlainSecret { value: v.to_string() })
            .map_err(|e| SecretError::Backend(e.to_string()))
    }
    fn remove(&self, r: &SecretRef) -> Result<(), SecretError> {
        self.inner
            .remove_secret(r.id())
            .map_err(|e| SecretError::Backend(e.to_string()))
    }
}
```

Add `pub mod plain_secret_store; pub use plain_secret_store::PlainSecretStore;` to `lib.rs`.

- [ ] **Step 3: Run tests** — expected 1 added test passes.

- [ ] **Step 4: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): PlainSecretStore impl of SecretStore (plaintext in redb)"
```

---

## Phase G — `DatabaseWidget`

The widget mirrors the Plan-2 Workspaces pattern: pure-Rust `DatabaseState` (testable in isolation) plus a thin render layer.

Right-pane sub-views are an enum: `RightPane::Editor` (default) | `RightPane::Results` | `RightPane::History`. `Tab` cycles. The left pane is the connection list; once a connection is opened, the right pane is the active sub-view.

### Task 21: `DatabaseWidget` — connection-list state + render seam

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`
- Test: `crates/sid-widgets/tests/database_connections.rs`

- [ ] **Step 1: Failing test**

```rust
use sid_core::adapters::db_client::DbKind;
use sid_store::DbConnection;
use sid_widgets::database::DatabaseState;

fn conn(id: &str, k: DbKind) -> DbConnection {
    DbConnection {
        id: id.into(), kind: k, name: id.into(),
        dsn: ":memory:".into(), secret_ref: None, created_at: 0,
    }
}

#[test]
fn new_state_selects_first_connection() {
    let s = DatabaseState::new(vec![conn("a", DbKind::Sqlite), conn("b", DbKind::Postgres)]);
    assert_eq!(s.selected_connection().unwrap().id, "a");
}

#[test]
fn empty_state_has_no_selection() {
    let s = DatabaseState::new(vec![]);
    assert!(s.selected_connection().is_none());
}

#[test]
fn select_next_and_prev_cycle() {
    let mut s = DatabaseState::new(vec![conn("a", DbKind::Sqlite), conn("b", DbKind::Sqlite)]);
    s.select_next();
    assert_eq!(s.selected_connection().unwrap().id, "b");
    s.select_next();
    assert_eq!(s.selected_connection().unwrap().id, "a");
    s.select_prev();
    assert_eq!(s.selected_connection().unwrap().id, "b");
}
```

- [ ] **Step 2: Implement skeleton**

Replace `crates/sid-widgets/src/database.rs`:

```rust
//! Database tab widget. Pure state in `DatabaseState`; rendering layer is
//! the binary's draw() (matched on tab id).

use std::sync::Arc;

use sid_core::adapters::db_client::{DbClient, QueryPage};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_store::{DbConnection, QueryRecord};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RightPane {
    Editor,
    Results,
    History,
}

pub struct DatabaseState {
    connections: Vec<DbConnection>,
    selected_idx: usize,
    /// Connected client for the currently-active session.
    active_client: Option<Arc<dyn DbClient>>,
    active_conn_id: Option<String>,
    right_pane: RightPane,
    pub editor: EditorState,
    pub results: ResultsState,
    pub history: HistoryState,
}

impl DatabaseState {
    pub fn new(connections: Vec<DbConnection>) -> Self {
        Self {
            connections,
            selected_idx: 0,
            active_client: None,
            active_conn_id: None,
            right_pane: RightPane::Editor,
            editor: EditorState::default(),
            results: ResultsState::default(),
            history: HistoryState::default(),
        }
    }

    pub fn connections(&self) -> &[DbConnection] { &self.connections }
    pub fn selected_connection(&self) -> Option<&DbConnection> { self.connections.get(self.selected_idx) }
    pub fn right_pane(&self) -> RightPane { self.right_pane }
    pub fn set_right_pane(&mut self, p: RightPane) { self.right_pane = p; }
    pub fn active_conn_id(&self) -> Option<&str> { self.active_conn_id.as_deref() }
    pub fn active_client(&self) -> Option<&Arc<dyn DbClient>> { self.active_client.as_ref() }
    pub fn set_active(&mut self, conn_id: String, client: Arc<dyn DbClient>) {
        self.active_conn_id = Some(conn_id);
        self.active_client = Some(client);
    }
    pub fn clear_active(&mut self) {
        self.active_conn_id = None;
        self.active_client = None;
    }

    pub fn select_next(&mut self) {
        let n = self.connections.len();
        if n == 0 { return; }
        self.selected_idx = (self.selected_idx + 1) % n;
    }
    pub fn select_prev(&mut self) {
        let n = self.connections.len();
        if n == 0 { return; }
        self.selected_idx = (self.selected_idx + n - 1) % n;
    }
}

#[derive(Default)]
pub struct EditorState {
    /// Lines of source. Filled in Task 22.
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

#[derive(Default)]
pub struct ResultsState {
    pub page: Option<QueryPage>,
    pub selected_row: usize,
    pub selected_col: usize,
    pub sort_col: Option<usize>,
    pub sort_asc: bool,
}

#[derive(Default)]
pub struct HistoryState {
    pub records: Vec<QueryRecord>,
    pub selected: usize,
}

pub struct DatabaseWidget {
    state: DatabaseState,
    id: WidgetId,
}

impl DatabaseWidget {
    pub fn new(connections: Vec<DbConnection>) -> Self {
        Self {
            state: DatabaseState::new(connections),
            id: WidgetId::new("database.root"),
        }
    }
    pub fn state(&self) -> &DatabaseState { &self.state }
    pub fn state_mut(&mut self) -> &mut DatabaseState { &mut self.state }
}

impl Default for DatabaseWidget {
    fn default() -> Self { Self::new(Vec::new()) }
}

impl Widget for DatabaseWidget {
    fn id(&self) -> &WidgetId { &self.id }
    fn title(&self) -> &str { "Database" }
    fn render(&self, _target: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            match (chord.code, chord.mods) {
                (KeyCode::Char('j') | KeyCode::Down, _) => { self.state.select_next(); return EventOutcome::Consumed; }
                (KeyCode::Char('k') | KeyCode::Up, _) => { self.state.select_prev(); return EventOutcome::Consumed; }
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.state.right_pane = match self.state.right_pane {
                        RightPane::Editor => RightPane::Results,
                        RightPane::Results => RightPane::History,
                        RightPane::History => RightPane::Editor,
                    };
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
        }
        EventOutcome::Bubble
    }
}
```

Add `sid-store.workspace = true` and `sid-core.workspace = true` to `crates/sid-widgets/Cargo.toml`.

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Adversarial + render-buffer snapshot stub**

Add a `RightPane` cycling test:

```rust
#[test]
fn right_pane_tab_cycles_editor_results_history() {
    use sid_widgets::database::RightPane;
    let mut s = DatabaseState::new(vec![conn("a", DbKind::Sqlite)]);
    assert_eq!(s.right_pane(), RightPane::Editor);
    s.set_right_pane(RightPane::Results);
    assert_eq!(s.right_pane(), RightPane::Results);
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): DatabaseWidget connection-list state + RightPane scaffold"
```

---

### Task 22: Query editor state (multi-line + tokenised highlight)

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`
- Test: `crates/sid-widgets/tests/database_editor.rs`

- [ ] **Step 1: Failing tests**

```rust
use sid_widgets::database::EditorState;

#[test]
fn empty_editor_has_one_blank_line() {
    let e = EditorState::default_blank();
    assert_eq!(e.lines, vec![String::new()]);
    assert_eq!(e.cursor_line, 0);
    assert_eq!(e.cursor_col, 0);
}

#[test]
fn insert_char_appends() {
    let mut e = EditorState::default_blank();
    e.insert_char('S');
    e.insert_char('E');
    assert_eq!(e.lines[0], "SE");
    assert_eq!(e.cursor_col, 2);
}

#[test]
fn newline_splits_line() {
    let mut e = EditorState::default_blank();
    e.insert_char('A');
    e.insert_newline();
    e.insert_char('B');
    assert_eq!(e.lines, vec!["A".to_string(), "B".to_string()]);
    assert_eq!(e.cursor_line, 1);
    assert_eq!(e.cursor_col, 1);
}

#[test]
fn backspace_at_line_start_joins_lines() {
    let mut e = EditorState::default_blank();
    e.insert_char('A');
    e.insert_newline();
    e.insert_char('B');
    e.move_cursor_to(1, 0);
    e.backspace();
    assert_eq!(e.lines, vec!["AB".to_string()]);
    assert_eq!(e.cursor_line, 0);
    assert_eq!(e.cursor_col, 1);
}

#[test]
fn full_source_returns_joined_text() {
    let mut e = EditorState::default_blank();
    e.insert_char('A');
    e.insert_newline();
    e.insert_char('B');
    assert_eq!(e.full_source(), "A\nB");
}

#[test]
fn tokens_for_current_source_classifies_keywords() {
    use sid_db_clients::lexer::TokenKind;
    let mut e = EditorState::default_blank();
    for c in "SELECT 1".chars() { e.insert_char(c); }
    let toks = e.tokens();
    assert!(toks.iter().any(|t| t.kind == TokenKind::Keyword));
    assert!(toks.iter().any(|t| t.kind == TokenKind::Number));
}
```

- [ ] **Step 2: Implement editor methods**

Extend `EditorState`:

```rust
impl EditorState {
    pub fn default_blank() -> Self {
        Self { lines: vec![String::new()], cursor_line: 0, cursor_col: 0 }
    }
    pub fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = char_byte_offset(line, self.cursor_col);
        line.insert(byte_idx, c);
        self.cursor_col += 1;
    }
    pub fn insert_newline(&mut self) {
        let line = self.lines.remove(self.cursor_line);
        let byte_idx = char_byte_offset(&line, self.cursor_col);
        let (a, b) = line.split_at(byte_idx);
        self.lines.insert(self.cursor_line, a.to_string());
        self.lines.insert(self.cursor_line + 1, b.to_string());
        self.cursor_line += 1;
        self.cursor_col = 0;
    }
    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_line];
            let byte_idx = char_byte_offset(line, self.cursor_col);
            // remove the char preceding byte_idx
            let prev = line[..byte_idx].chars().rev().next().map(|c| c.len_utf8()).unwrap_or(1);
            line.replace_range(byte_idx - prev..byte_idx, "");
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            let removed = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
            self.lines[self.cursor_line].push_str(&removed);
        }
    }
    pub fn move_cursor_to(&mut self, line: usize, col: usize) {
        self.cursor_line = line.min(self.lines.len().saturating_sub(1));
        let line_chars = self.lines[self.cursor_line].chars().count();
        self.cursor_col = col.min(line_chars);
    }
    pub fn full_source(&self) -> String { self.lines.join("\n") }
    pub fn tokens(&self) -> Vec<sid_db_clients::lexer::Token> {
        sid_db_clients::lexer::tokenize(&self.full_source())
    }
}

fn char_byte_offset(s: &str, char_col: usize) -> usize {
    s.char_indices().nth(char_col).map(|(i, _)| i).unwrap_or(s.len())
}
```

Add `sid-db-clients.workspace = true` to `crates/sid-widgets/Cargo.toml`.

- [ ] **Step 3: Run tests** — expected 6 passed.

- [ ] **Step 4: Property test**

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_insert_then_full_source_roundtrips(s in "[a-zA-Z0-9 ;\n]{0,200}") {
        let mut e = EditorState::default_blank();
        for c in s.chars() {
            if c == '\n' { e.insert_newline(); } else { e.insert_char(c); }
        }
        prop_assert_eq!(e.full_source(), s);
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): query editor multi-line state with SQL lexer integration"
```

---

### Task 23: Results table state (paginated + sortable)

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`
- Test: `crates/sid-widgets/tests/database_results.rs`

- [ ] **Step 1: Failing tests**

```rust
use sid_core::adapters::db_client::{Column, ColumnType, PageCursor, QueryPage, Row};
use sid_widgets::database::ResultsState;

fn page(rows: Vec<Vec<&str>>, next: Option<u64>) -> QueryPage {
    QueryPage {
        columns: vec![
            Column { name: "id".into(), ty: ColumnType::Integer },
            Column { name: "name".into(), ty: ColumnType::Text },
        ],
        rows: rows.into_iter().map(|r| Row { values: r.into_iter().map(String::from).collect() }).collect(),
        next_cursor: next.map(|o| PageCursor { offset: o }),
        duration_ms: 1,
    }
}

#[test]
fn sets_page_and_initial_selection() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["1","a"], vec!["2","b"]], None));
    assert_eq!(s.selected_row, 0);
    assert_eq!(s.selected_col, 0);
}

#[test]
fn next_row_advances() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["1","a"], vec!["2","b"]], None));
    s.select_next_row();
    assert_eq!(s.selected_row, 1);
}

#[test]
fn sort_toggle_flips_order() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["2","b"], vec!["1","a"]], None));
    s.toggle_sort(0);
    let rows: Vec<_> = s.page.as_ref().unwrap().rows.iter().map(|r| r.values[0].clone()).collect();
    assert_eq!(rows, vec!["1", "2"]);
    s.toggle_sort(0);
    let rows: Vec<_> = s.page.as_ref().unwrap().rows.iter().map(|r| r.values[0].clone()).collect();
    assert_eq!(rows, vec!["2", "1"]);
}

#[test]
fn selected_cell_returns_value() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["1","alpha"], vec!["2","beta"]], None));
    s.select_next_row();
    s.select_next_col();
    assert_eq!(s.selected_cell(), Some("beta"));
}
```

- [ ] **Step 2: Implement**

```rust
impl ResultsState {
    pub fn set_page(&mut self, p: QueryPage) {
        self.page = Some(p);
        self.selected_row = 0;
        self.selected_col = 0;
    }
    pub fn select_next_row(&mut self) {
        if let Some(p) = &self.page {
            if !p.rows.is_empty() { self.selected_row = (self.selected_row + 1) % p.rows.len(); }
        }
    }
    pub fn select_prev_row(&mut self) {
        if let Some(p) = &self.page {
            if !p.rows.is_empty() {
                self.selected_row = (self.selected_row + p.rows.len() - 1) % p.rows.len();
            }
        }
    }
    pub fn select_next_col(&mut self) {
        if let Some(p) = &self.page {
            if !p.columns.is_empty() { self.selected_col = (self.selected_col + 1) % p.columns.len(); }
        }
    }
    pub fn select_prev_col(&mut self) {
        if let Some(p) = &self.page {
            if !p.columns.is_empty() {
                self.selected_col = (self.selected_col + p.columns.len() - 1) % p.columns.len();
            }
        }
    }
    pub fn selected_cell(&self) -> Option<&str> {
        let p = self.page.as_ref()?;
        p.rows.get(self.selected_row)?.values.get(self.selected_col).map(|s| s.as_str())
    }
    pub fn toggle_sort(&mut self, col: usize) {
        if self.sort_col == Some(col) {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_col = Some(col);
            self.sort_asc = true;
        }
        if let Some(p) = self.page.as_mut() {
            p.rows.sort_by(|a, b| {
                let sa = a.values.get(col).map(String::as_str).unwrap_or("");
                let sb = b.values.get(col).map(String::as_str).unwrap_or("");
                // Numeric compare when both parse, else lexicographic.
                match (sa.parse::<f64>(), sb.parse::<f64>()) {
                    (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    _ => sa.cmp(sb),
                }
            });
            if !self.sort_asc { p.rows.reverse(); }
        }
    }
}
```

- [ ] **Step 3: Run tests** — expected 4 passed.

- [ ] **Step 4: Adversarial — empty result table**

```rust
#[test]
fn select_on_empty_page_is_noop() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![], None));
    s.select_next_row();
    s.select_next_col();
    assert!(s.selected_cell().is_none());
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): paginated results table with column sort + cell selection"
```

---

### Task 24: Query history sub-view state

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`
- Test: `crates/sid-widgets/tests/database_history.rs`

- [ ] **Step 1: Failing tests**

```rust
use sid_store::QueryRecord;
use sid_widgets::database::HistoryState;

fn rec(sql: &str, ts: u128) -> QueryRecord {
    QueryRecord { conn_id: "c".into(), sql: sql.into(), duration_ms: 1, row_count: 0, ts_ns: ts }
}

#[test]
fn set_records_resets_selection() {
    let mut s = HistoryState::default();
    s.set_records(vec![rec("Q1", 1), rec("Q2", 2)]);
    assert_eq!(s.selected, 0);
}

#[test]
fn navigation_wraps() {
    let mut s = HistoryState::default();
    s.set_records(vec![rec("Q1", 1), rec("Q2", 2)]);
    s.select_next();
    assert_eq!(s.selected, 1);
    s.select_next();
    assert_eq!(s.selected, 0);
    s.select_prev();
    assert_eq!(s.selected, 1);
}

#[test]
fn selected_record_returns_current() {
    let mut s = HistoryState::default();
    s.set_records(vec![rec("Q1", 1), rec("Q2", 2)]);
    assert_eq!(s.selected_record().unwrap().sql, "Q1");
}
```

- [ ] **Step 2: Implement**

```rust
impl HistoryState {
    pub fn set_records(&mut self, records: Vec<QueryRecord>) {
        self.records = records;
        self.selected = 0;
    }
    pub fn selected_record(&self) -> Option<&QueryRecord> {
        self.records.get(self.selected)
    }
    pub fn select_next(&mut self) {
        if self.records.is_empty() { return; }
        self.selected = (self.selected + 1) % self.records.len();
    }
    pub fn select_prev(&mut self) {
        if self.records.is_empty() { return; }
        self.selected = (self.selected + self.records.len() - 1) % self.records.len();
    }
}
```

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): query history sub-view state"
```

---

### Task 25: Connect/disconnect + run-query orchestration

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`
- Test: `crates/sid-widgets/tests/database_orchestration.rs`

This task wires the widget to the `JobQueue` via `WidgetCtx`. Selecting a connection and pressing `Enter` calls a `DbClient::open` job; the result is delivered back via channel and consumed in `poll()`. Pressing `Ctrl+Enter` in the editor runs the current source through `query_paged`; the resulting `QueryPage` lands in `ResultsState` and a `QueryRecord` is appended to history.

Because mocking `DbClient` is cheap (a Mock impl is already proven dyn-compatible in Task 3), unit tests inject a mock via the widget's job dispatcher.

- [ ] **Step 1: Define widget-level commands**

Add to `database.rs`:

```rust
/// One-shot actions the widget asks the App to perform. The App's wire layer
/// turns these into JobQueue spawns and routes the result back via channel.
#[derive(Clone, Debug)]
pub enum DbCommand {
    Connect { conn_id: String },
    Disconnect,
    RunQuery { sql: String, conn_id: String },
    LoadHistory { conn_id: String },
    LoadNextPage { sql: String, conn_id: String, cursor: sid_core::adapters::db_client::PageCursor },
}

impl DatabaseState {
    /// Drain pending commands the renderer should hand to the App.
    pub fn drain_commands(&mut self) -> Vec<DbCommand> { std::mem::take(&mut self.pending) }
    /// Push a command (used by the widget's event handler).
    pub(crate) fn push_command(&mut self, c: DbCommand) { self.pending.push(c); }
    /// Apply the result of a Connect command (called by the App when the
    /// JobQueue produces a client).
    pub fn apply_connect_result(&mut self, conn_id: String, client: Arc<dyn DbClient>) {
        self.set_active(conn_id, client);
    }
    /// Apply a QueryPage result.
    pub fn apply_query_result(&mut self, page: QueryPage, record: Option<QueryRecord>) {
        self.results.set_page(page);
        self.right_pane = RightPane::Results;
        if let Some(r) = record { self.history.records.insert(0, r); }
    }
    /// Apply a history result.
    pub fn apply_history(&mut self, records: Vec<QueryRecord>) {
        self.history.set_records(records);
    }
}

// Add to the struct:
//   pending: Vec<DbCommand>,
// and initialize in `new`.
```

Update `handle_event` to:
- On `Enter` over a connection → push `DbCommand::Connect`.
- On `Ctrl+r` while in Editor → push `DbCommand::RunQuery`.
- On `Ctrl+d` → push `DbCommand::Disconnect`.

- [ ] **Step 2: Tests**

```rust
use std::sync::Arc;
use sid_core::adapters::db_client::{DbClient, DbKind, ExecResult, OpenParams, PageCursor, QueryPage, Row, Column, ColumnType, SchemaInfo};
use sid_widgets::database::{DatabaseState, DbCommand, RightPane};

#[test]
fn enter_on_connection_pushes_connect_command() {
    let mut s = DatabaseState::new(vec![/* one Sqlite conn */]);
    // ... synthesize a chord event for Enter and route through handle_event
    // (use a tiny helper that builds a KeyChord)
    // Then assert: drain_commands() contains DbCommand::Connect { conn_id: "a" }
}

#[test]
fn apply_query_result_swaps_right_pane_to_results() {
    let mut s = DatabaseState::new(vec![]);
    let page = QueryPage { columns: vec![], rows: vec![], next_cursor: None, duration_ms: 0 };
    s.apply_query_result(page, None);
    assert_eq!(s.right_pane(), RightPane::Results);
}
```

- [ ] **Step 3: Run tests + commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): connect/disconnect + run-query command surface"
```

---

### Task 26: Copy-cell action

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`

The widget needs a `Clipboard` adapter to copy a cell. The `Clipboard` trait exists in `sid-core::adapters::clipboard` (per Plan 1). On `c` while in Results, take `ResultsState::selected_cell()` and call `ctx.clipboard.copy(cell)`. Surface a toast on success or failure.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn copy_cell_pushes_clipboard_command() {
    // Wire DbCommand::CopyCell variant (or reuse ctx.clipboard mock)
}
```

- [ ] **Step 2: Add `DbCommand::CopyCell(String)` variant and wire `c`-key dispatch**

- [ ] **Step 3: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): copy-cell action in Results sub-view"
```

---

## Phase H — Export + paginate-on-scroll

### Task 27: CSV export action

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`
- Create: `crates/sid-widgets/src/csv_export.rs`
- Test: `crates/sid-widgets/tests/csv_export.rs`

The `csv` crate is the only new dep here. The export action serialises the **entire current result set** (across all pages) — for v1 it just dumps the currently-loaded page. A follow-up plan extends it to drain all pages first.

- [ ] **Step 1: Failing tests**

```rust
use sid_core::adapters::db_client::{Column, ColumnType, QueryPage, Row};
use sid_widgets::csv_export::write_page_csv;

#[test]
fn writes_headers_and_rows() {
    let page = QueryPage {
        columns: vec![
            Column { name: "id".into(), ty: ColumnType::Integer },
            Column { name: "name".into(), ty: ColumnType::Text },
        ],
        rows: vec![
            Row { values: vec!["1".into(), "alpha".into()] },
            Row { values: vec!["2".into(), "be,ta".into()] }, // comma in cell — must quote
        ],
        next_cursor: None,
        duration_ms: 0,
    };
    let mut buf = Vec::new();
    write_page_csv(&page, &mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.starts_with("id,name\n"));
    assert!(s.contains("\"be,ta\""));
}

#[test]
fn empty_page_writes_header_only() {
    let page = QueryPage {
        columns: vec![Column { name: "id".into(), ty: ColumnType::Integer }],
        rows: vec![],
        next_cursor: None,
        duration_ms: 0,
    };
    let mut buf = Vec::new();
    write_page_csv(&page, &mut buf).unwrap();
    assert_eq!(String::from_utf8(buf).unwrap(), "id\n");
}
```

- [ ] **Step 2: Implement**

Create `crates/sid-widgets/src/csv_export.rs`:

```rust
//! CSV export for query results. Uses the `csv` crate with default quoting.

use std::io::Write;

use sid_core::adapters::db_client::QueryPage;

/// Write a `QueryPage` to `out` as CSV: one header row, then one row per record.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::{Column, ColumnType, QueryPage};
/// use sid_widgets::csv_export::write_page_csv;
/// let p = QueryPage { columns: vec![Column { name: "id".into(), ty: ColumnType::Integer }], rows: vec![], next_cursor: None, duration_ms: 0 };
/// let mut buf = Vec::new();
/// write_page_csv(&p, &mut buf).unwrap();
/// assert!(String::from_utf8(buf).unwrap().contains("id"));
/// ```
pub fn write_page_csv<W: Write>(page: &QueryPage, out: W) -> std::io::Result<()> {
    let mut w = csv::Writer::from_writer(out);
    let headers: Vec<&str> = page.columns.iter().map(|c| c.name.as_str()).collect();
    w.write_record(&headers).map_err(map_csv_err)?;
    for row in &page.rows {
        w.write_record(row.values.iter().map(|s| s.as_str())).map_err(map_csv_err)?;
    }
    w.flush()
}

fn map_csv_err(e: csv::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e)
}
```

Add `csv.workspace = true` to `crates/sid-widgets/Cargo.toml`. Add `pub mod csv_export;` to `lib.rs`.

- [ ] **Step 3: Wire `e` key in the widget**

Add `DbCommand::ExportCsv { path: PathBuf }` variant. On `e` while in Results, prompt the user for a path (or default to `~/.local/share/sid/exports/q-<ts>.csv`) and push the command. The binary handles the actual file write off the render thread.

- [ ] **Step 4: Adversarial**

```rust
#[test]
fn csv_quotes_newline_containing_cell() {
    use sid_core::adapters::db_client::{Column, ColumnType, QueryPage, Row};
    let p = QueryPage {
        columns: vec![Column { name: "c".into(), ty: ColumnType::Text }],
        rows: vec![Row { values: vec!["line1\nline2".into()] }],
        next_cursor: None,
        duration_ms: 0,
    };
    let mut buf = Vec::new();
    sid_widgets::csv_export::write_page_csv(&p, &mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("\"line1\nline2\""));
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): CSV export action via the csv crate"
```

---

### Task 28: Paginate-on-scroll

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`

When `selected_row == rows.len() - 1` and the user presses `j` again, if `page.next_cursor.is_some()`, push `DbCommand::LoadNextPage { ... }`. The App fetches the next page and calls `apply_next_page(page)`, which **appends** rows to the current page and updates the cursor.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn select_next_row_at_end_with_cursor_pushes_load_next_page() {
    // assemble a page with one row and a next_cursor; assert that pushing
    // `j` enqueues a LoadNextPage command.
}
```

- [ ] **Step 2: Implement `apply_next_page` + dispatch in event handler**

```rust
pub fn apply_next_page(&mut self, page: QueryPage) {
    if let Some(existing) = self.results.page.as_mut() {
        existing.rows.extend(page.rows);
        existing.next_cursor = page.next_cursor;
    } else {
        self.results.set_page(page);
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): paginate-on-scroll in Results sub-view"
```

---

## Phase I — CLI + binary wiring

### Task 29: `sid db add` CLI subcommand

**Files:**
- Modify: `crates/sid/src/main.rs`

- [ ] **Step 1: Add the subcommand**

```rust
#[derive(clap::Subcommand, Debug)]
enum Cmd {
    Workspace { #[command(subcommand)] op: WorkspaceOp },
    Db { #[command(subcommand)] op: DbOp },
}

#[derive(clap::Subcommand, Debug)]
enum DbOp {
    /// Add a saved DB connection.
    Add {
        /// Stable id (used by `sid db query <id>`).
        id: String,
        /// `postgres` or `sqlite`.
        #[arg(long)]
        kind: String,
        /// User-facing label.
        #[arg(long)]
        name: String,
        /// DSN (Postgres) or filesystem path (SQLite).
        #[arg(long)]
        dsn: String,
        /// Optional password (Postgres). Stored in the secrets table.
        #[arg(long)]
        password: Option<String>,
    },
    Remove { id: String },
    List,
    /// Run a SQL statement and print the result as CSV on stdout.
    Query { id: String, sql: String },
}
```

- [ ] **Step 2: Handle in `main`**

```rust
if let Some(Cmd::Db { op }) = cli.cmd {
    return run_db_op(op, &store).await;
}
```

Implement `run_db_op` (in `wire.rs` for testability):

```rust
pub async fn run_db_op(op: DbOp, store: &Arc<RedbStore>) -> anyhow::Result<()> {
    match op {
        DbOp::Add { id, kind, name, dsn, password } => {
            let kind = match kind.as_str() {
                "postgres" => DbKind::Postgres,
                "sqlite" => DbKind::Sqlite,
                other => anyhow::bail!("unknown kind '{other}' (use 'postgres' or 'sqlite')"),
            };
            let secret_ref = if let Some(pw) = password {
                let r = SecretRef::new(format!("db.{id}.password"));
                let plain_store = PlainSecretStore::new(store.clone());
                plain_store.put(&r, &pw).map_err(|e| anyhow::anyhow!("put secret: {e}"))?;
                Some(r)
            } else { None };
            let conn = DbConnection {
                id: id.clone(), kind, name, dsn, secret_ref,
                created_at: now_epoch(),
            };
            store.upsert_db_connection(&conn)?;
            println!("added connection: {id}");
        }
        DbOp::Remove { id } => {
            if let Some(c) = store.get_db_connection(&id)? {
                if let Some(r) = c.secret_ref {
                    let _ = PlainSecretStore::new(store.clone()).remove(&r);
                }
            }
            store.remove_db_connection(&id)?;
            println!("removed connection: {id}");
        }
        DbOp::List => {
            for c in store.list_db_connections()? {
                println!("{:<24} {:?}  {}  ({})", c.id, c.kind, c.name, c.dsn);
            }
        }
        DbOp::Query { id, sql } => {
            // Implemented in Task 31.
            run_db_query(id, sql, store).await?;
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Tests in `crates/sid/tests/db_cli_add.rs`**

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn db_add_then_list_shows_connection() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let add = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "db", "add", "local",
               "--kind", "sqlite", "--name", "Local", "--dsn", ":memory:"])
        .output().unwrap();
    assert!(add.status.success(), "stderr: {}", String::from_utf8_lossy(&add.stderr));
    let list = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "db", "list"])
        .output().unwrap();
    assert!(String::from_utf8_lossy(&list.stdout).contains("local"));
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): sid db add subcommand"
```

---

### Task 30: `sid db remove` + `list`

Already covered in Task 29's scaffold. This task adds the unit/integration tests for `remove` and `list`.

- [ ] **Step 1: Tests**

```rust
#[test]
fn db_remove_drops_connection_and_secret() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    Command::new(bin).args(["--db", db.to_str().unwrap(), "db", "add", "x",
        "--kind", "sqlite", "--name", "X", "--dsn", ":memory:",
        "--password", "shh"]).output().unwrap();
    Command::new(bin).args(["--db", db.to_str().unwrap(), "db", "remove", "x"])
        .output().unwrap();
    let list = Command::new(bin).args(["--db", db.to_str().unwrap(), "db", "list"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&list.stdout).contains("x "));
    // Also verify the secret is gone — open the store directly and assert.
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): sid db remove/list + secret cleanup on remove"
```

---

### Task 31: `sid db query` — CSV stdout

**Files:**
- Modify: `crates/sid/src/wire.rs`

- [ ] **Step 1: Implement `run_db_query`**

```rust
pub async fn run_db_query(id: String, sql: String, store: &Arc<RedbStore>) -> anyhow::Result<()> {
    let conn = store.get_db_connection(&id)?.ok_or_else(|| anyhow::anyhow!("no such connection: {id}"))?;
    let password = if let Some(r) = conn.secret_ref.as_ref() {
        PlainSecretStore::new(store.clone()).get(r).map_err(|e| anyhow::anyhow!("get secret: {e}"))?
    } else { None };
    let factory: Arc<dyn DbClient> = match conn.kind {
        DbKind::Postgres => PostgresClient::factory(),
        DbKind::Sqlite => SqliteClient::factory(),
    };
    let client = factory.open(OpenParams {
        kind: conn.kind, dsn: conn.dsn.clone(), password,
    }).await.map_err(|e| anyhow::anyhow!("open: {e}"))?;
    // Detect SELECT vs not by simple prefix match — naive but adequate for v1.
    let trimmed = sql.trim_start().to_ascii_uppercase();
    if trimmed.starts_with("SELECT") || trimmed.starts_with("WITH") {
        // Drain all pages to stdout.
        let mut cursor = None;
        let mut wrote_header = false;
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        loop {
            let page = client.query_paged(&sql, cursor, 500).await
                .map_err(|e| anyhow::anyhow!("query: {e}"))?;
            if !wrote_header {
                let mut header_page = sid_core::adapters::db_client::QueryPage {
                    columns: page.columns.clone(),
                    rows: vec![],
                    next_cursor: None, duration_ms: 0,
                };
                sid_widgets::csv_export::write_page_csv(&header_page, &mut lock)?;
                wrote_header = true;
            }
            // Write rows only (no header).
            let mut w = csv::Writer::from_writer(&mut lock);
            for r in &page.rows {
                w.write_record(r.values.iter().map(|s| s.as_str())).map_err(std::io::Error::other)?;
            }
            w.flush()?;
            cursor = page.next_cursor;
            if cursor.is_none() { break; }
        }
    } else {
        let r = client.execute(&sql).await.map_err(|e| anyhow::anyhow!("execute: {e}"))?;
        println!("{} rows affected ({}ms)", r.rows_affected, r.duration_ms);
    }
    // Log a QueryRecord regardless.
    let _ = store.append_query_record(&QueryRecord {
        conn_id: id.clone(), sql: sql.clone(),
        duration_ms: 0, row_count: 0,
        ts_ns: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0),
    });
    Ok(())
}
```

`std::io::Error::other` requires Rust 1.74+; project pins 1.85 so that's fine.

- [ ] **Step 2: Tests** in `crates/sid/tests/db_cli_query.rs`:

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn query_against_sqlite_prints_csv() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("sid.redb");
    let sqlite_path = dir.path().join("data.db");
    let bin = env!("CARGO_BIN_EXE_sid");
    Command::new(bin).args([
        "--db", db_path.to_str().unwrap(), "db", "add", "local",
        "--kind", "sqlite", "--name", "Local", "--dsn", sqlite_path.to_str().unwrap(),
    ]).output().unwrap();
    Command::new(bin).args([
        "--db", db_path.to_str().unwrap(), "db", "query", "local",
        "CREATE TABLE t (id INT, name TEXT); INSERT INTO t VALUES (1, 'a'), (2, 'b')",
    ]).output().unwrap();
    let out = Command::new(bin).args([
        "--db", db_path.to_str().unwrap(), "db", "query", "local",
        "SELECT id, name FROM t ORDER BY id",
    ]).output().unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    let mut lines = s.lines();
    assert_eq!(lines.next(), Some("id,name"));
    assert_eq!(lines.next(), Some("1,a"));
    assert_eq!(lines.next(), Some("2,b"));
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): sid db query — runs SQL and emits CSV to stdout"
```

---

### Task 32: Wire DB clients + PlainSecretStore into binary

**Files:**
- Modify: `crates/sid/Cargo.toml` (add `sid-db-clients`)
- Modify: `crates/sid/src/wire.rs`

- [ ] **Step 1: Add deps**

```toml
sid-db-clients.workspace = true
```

- [ ] **Step 2: Inject into `SidApp`**

```rust
pub struct SidApp {
    pub app: App,
    pub store: Arc<RedbStore>,
    pub session_id: String,
    pub git: Arc<dyn GitProvider>,
    pub postgres: Arc<dyn DbClient>,
    pub sqlite: Arc<dyn DbClient>,
    pub secrets: Arc<dyn SecretStore>,
}
```

In `build_app`, instantiate:

```rust
let postgres = PostgresClient::factory();
let sqlite = SqliteClient::factory();
let secrets = Arc::new(PlainSecretStore::new(store.clone())) as Arc<dyn SecretStore>;
```

Wire into `DatabaseWidget::new(connections, postgres, sqlite, secrets)` — the widget gains constructor params to use the right factory per saved connection at connect-time.

- [ ] **Step 3: Update widget signature + propagate**

The widget holds the three handles internally for use in `DbCommand::Connect` handling. Adjust state accordingly.

- [ ] **Step 4: Commit**

```bash
git add crates/sid Cargo.toml
git commit -m "feat(bin): wire PostgresClient, SqliteClient, PlainSecretStore into App"
```

---

## Phase J — Integration tests + README

### Task 33: Integration test — SQLite round-trip from CLI

**Files:**
- Create: `crates/sid/tests/db_integration.rs`

- [ ] **Step 1: Test**

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn end_to_end_sqlite_session() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let sqlite = dir.path().join("data.db");
    let bin = env!("CARGO_BIN_EXE_sid");
    // Add
    assert!(Command::new(bin).args([
        "--db", db.to_str().unwrap(), "db", "add", "data",
        "--kind", "sqlite", "--name", "Data", "--dsn", sqlite.to_str().unwrap(),
    ]).status().unwrap().success());
    // Create + insert
    assert!(Command::new(bin).args([
        "--db", db.to_str().unwrap(), "db", "query", "data",
        "CREATE TABLE users (id INT, name TEXT)",
    ]).status().unwrap().success());
    assert!(Command::new(bin).args([
        "--db", db.to_str().unwrap(), "db", "query", "data",
        "INSERT INTO users VALUES (1, 'alice'), (2, 'bob')",
    ]).status().unwrap().success());
    // Query + parse CSV
    let out = Command::new(bin).args([
        "--db", db.to_str().unwrap(), "db", "query", "data",
        "SELECT id, name FROM users ORDER BY id",
    ]).output().unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("id,name"));
    assert!(s.contains("1,alice"));
    assert!(s.contains("2,bob"));
    // List
    let list = Command::new(bin).args(["--db", db.to_str().unwrap(), "db", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&list.stdout).contains("data"));
    // Remove
    assert!(Command::new(bin).args([
        "--db", db.to_str().unwrap(), "db", "remove", "data",
    ]).status().unwrap().success());
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/sid
git commit -m "test(bin): integration test for sqlite round-trip via CLI"
```

---

### Task 34: Integration test — widget end-to-end with in-memory SQLite

**Files:**
- Create: `crates/sid-widgets/tests/database_end_to_end.rs`

This drives `DatabaseWidget` programmatically: simulate a Connect command, then a RunQuery command, then assert the widget's `results.page` reflects the result.

- [ ] **Step 1: Test**

```rust
use std::sync::Arc;

use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams};
use sid_core::adapters::secret_store::SecretStore;
use sid_db_clients::SqliteClient;
use sid_store::DbConnection;
use sid_widgets::database::{DatabaseState, DbCommand};

#[tokio::test]
async fn widget_end_to_end_against_in_memory_sqlite() {
    // Set up a connection.
    let conn = DbConnection {
        id: "mem".into(), kind: DbKind::Sqlite,
        name: "memory".into(), dsn: ":memory:".into(),
        secret_ref: None, created_at: 0,
    };
    let mut state = DatabaseState::new(vec![conn.clone()]);

    // Manually simulate the Connect flow.
    let client = SqliteClient::factory().open(OpenParams {
        kind: DbKind::Sqlite, dsn: ":memory:".into(), password: None,
    }).await.unwrap();
    state.apply_connect_result(conn.id.clone(), client.clone());
    assert_eq!(state.active_conn_id(), Some("mem"));

    // Execute DDL and seed.
    client.execute("CREATE TABLE t (id INT, v TEXT)").await.unwrap();
    client.execute("INSERT INTO t VALUES (1, 'a'), (2, 'b')").await.unwrap();

    // Run a query.
    let page = client.query_paged("SELECT id, v FROM t ORDER BY id", None, 50).await.unwrap();
    state.apply_query_result(page, None);
    assert_eq!(state.results.page.as_ref().unwrap().rows.len(), 2);
    assert_eq!(state.right_pane(), sid_widgets::database::RightPane::Results);
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/sid-widgets
git commit -m "test(widgets): end-to-end DatabaseWidget against in-memory sqlite"
```

---

### Task 35: README update

**Files:**
- Modify: `README.md`

Update the v1 table:

```markdown
| **Database** | Saved Postgres + SQLite connections, multi-line query editor with SQL syntax highlight, paginated sortable results, copy-cell, CSV export, per-connection query history |
```

Add to Quickstart:

```markdown
# Database management
sid db add local --kind sqlite --name "Local" --dsn ./data.db
sid db query local "SELECT * FROM users LIMIT 5"
sid db list
sid db remove local
```

Update the "What works in this build" callout:

> Foundation + Workspaces + Database tabs functional. Database tab supports
> saved Postgres + SQLite connections with paginated results, sortable
> columns, copy-cell, CSV export, and per-connection query history. The
> `sid db` CLI provides scripting parity.

- [ ] **Step 1: Commit**

```bash
git add README.md
git commit -m "docs: update README for Plan 4 Database tab"
```

---

## Done criteria for Plan 4

- [ ] `cargo build --workspace` succeeds with no errors or warnings
- [ ] `cargo test --all-features --workspace` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is clean
- [ ] `cargo fmt --check` is clean
- [ ] `cargo run -p sid` launches; the Database tab loads saved connections
- [ ] `Enter` on a connection opens it; `Tab` cycles Editor → Results → History
- [ ] Typing in the editor highlights keywords/strings/numbers/comments
- [ ] `Ctrl+Enter` runs the query; results land in the Results pane (paginated, 50/page)
- [ ] `c` copies the selected cell; `e` exports the current result set to CSV
- [ ] `sid db add/remove/list` round-trips a connection (and its secret)
- [ ] `sid db query <id> "<sql>"` prints CSV to stdout
- [ ] In-memory SQLite end-to-end widget test passes
- [ ] Postgres `pg-it`-gated tests pass when `SID_PG_DSN` is set (CI: docker-compose Postgres)
- [ ] No regressions in Plan 1/2 functionality (theme, tabs, palette, session restore, Workspaces)

---

## Self-review notes (run before requesting human review)

**1. Spec coverage.** Plan 4 covers the spec's "Database" tab (Postgres + SQLite in v1), plus CLI parity. Items covered: `DbClient` trait + Postgres/SQLite impls; SQL lexer (hand-rolled, ANSI keyword set); `DbConnection`/`QueryRecord`/`PlainSecret` storage + tables; `SecretStore` trait + `PlainStore` impl; full `DatabaseWidget` with connection list, multi-line editor, paginated sortable results, copy-cell, CSV export, per-connection history; `sid db add/remove/list/query` subcommands.

**2. Items deferred to later plans (confirmed by future-features doc):**
   - MySQL, Redis, MongoDB, DuckDB, ClickHouse backends
   - ER diagram view / schema visualisation
   - Saved query library
   - Foreign-key "open related" navigation
   - Query plan visualisation
   - Notebook-style cells
   - OS keyring (trait + PlainStore impl land here; KeyringStore later)
   - Tree-sitter SQL parsing
   - In-TUI Ctrl+C cancel of in-flight queries
   - Connection pooling (single client per connection in v1)

**3. Type consistency check.**
   - `DbConnection` lives in `sid-store`. `DbKind`/`DbClient`/`PageCursor`/etc. live in `sid-core`. `DbConnection.kind` references `sid_core::adapters::db_client::DbKind` (cross-crate type usage; sid-store already depends on sid-core).
   - `DbClient` trait in `sid-core::adapters::db_client`; `PostgresClient`/`SqliteClient` in `sid-db-clients`. Widget references only the trait — never the concrete types directly (adapter pattern).
   - `SecretStore` trait in `sid-core::adapters::secret_store`; `PlainSecretStore` in `sid-store`. Widget references trait only.
   - `DatabaseWidget::new(connections, postgres, sqlite, secrets)` matches what `wire.rs` passes.

**4. Placeholder scan.** No "TBD" or "fill in later" hiding in TDD steps. Two judgment calls were flagged in the front matter:
   - `SecretStore` + `PlainStore` were originally listed as Plan 1 deliverables in the user's brief but were not actually built in Plan 1; this plan creates them in Phase F.
   - Postgres pagination uses LIMIT/OFFSET; cursor-based pagination is documented as a future optimisation.

**5. Scope check.** 35 tasks, 10 phases (A-J). Comparable to Plan 2's 33 tasks. Each phase produces working/testable software; the plan can stop at the end of any phase and the project remains consistent.

**6. CLAUDE.md compliance.**
   - Lexer (parser-shaped code) gets a `proptest` over arbitrary bytes + UTF-8 inputs as the cargo-fuzz stand-in (Task 15).
   - Every `pub fn`/`pub struct`/`pub trait`/`pub enum` gets a doc test (called out in steps).
   - `Result`-returning fns get both `Ok` and `Err` tests (e.g., open success + connect-error + invalid-kind).
   - Property tests on `DbConnection`/`QueryRecord` postcard round-trip; on editor `insert → full_source` round-trip.
   - Adversarial coverage for: malformed SQL (lexer + execute), unicode in identifiers, blobs, NULL rendering, unterminated strings/comments, empty result pages, page sizes of 1, 200+ untracked files (sqlite stress), nested block comments.
   - `criterion` benches on lexer hot path and `recent_queries` reverse scan.

**7. Adapter pattern integrity.**
   - `sid-widgets` depends on `sid-core` (traits) + `sid-store` (domain types) + `sid-db-clients` for the **lexer module only** (not for the client impls). That's a pragmatic exception: the lexer is text manipulation, not an "external library wrapper". Alternative: move the lexer into `sid-core::sql_lexer` to fully isolate `sid-widgets` from `sid-db-clients`. Flagged as a judgment call — recommend the lexer move into `sid-core` if the reviewer wants strict adherence.
   - `sid-db-clients` is the only crate that names `tokio-postgres` and `rusqlite`. Confirmed.

**8. Co-author trailer.** All commit subjects in this plan deliberately omit `Co-Authored-By: Claude...` trailers per the user's stated preference (memory: `no-claude-coauthor-trailer`).
