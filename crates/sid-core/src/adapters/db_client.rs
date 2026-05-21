//! `DbClient` — domain-shaped database client trait used by the Database tab.
//!
//! Concrete impls live in `sid-db-clients` (`PostgresClient`, `SqliteClient`).
//! No widget code names this crate's concrete types — they hold
//! `Arc<dyn DbClient>` only.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Database kind discriminator.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::DbKind;
/// assert_ne!(DbKind::Postgres, DbKind::Sqlite);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DbKind {
    /// PostgreSQL — async via `tokio-postgres`.
    Postgres,
    /// SQLite — sync via `rusqlite`, wrapped in `spawn_blocking`.
    Sqlite,
}

/// Domain-shaped error returned by every [`DbClient`] method.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::DbError;
/// let err = DbError::Auth;
/// assert!(format!("{err}").contains("auth"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Connection could not be established (DNS, refused, timeout, TLS, etc.).
    #[error("connection failed: {0}")]
    Connect(String),
    /// Authentication failed.
    #[error("authentication failed")]
    Auth,
    /// Query failed for any reason other than a syntax error.
    #[error("query failed: {0}")]
    Query(String),
    /// The driver reported a syntax error and gave us a position.
    #[error("query syntax error at offset {offset}: {message}")]
    Syntax {
        /// 0-based byte offset of the error within the SQL text.
        offset: usize,
        /// Driver-provided human-readable error message.
        message: String,
    },
    /// Query was cancelled (server-side or via the cancel-token side channel).
    #[error("query was cancelled")]
    Cancelled,
    /// Caller passed an invalid argument (e.g., wrong [`DbKind`]).
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// The client has not yet been opened (factory state).
    #[error("not connected")]
    NotConnected,
    /// I/O error from the underlying transport.
    #[error("io error: {0}")]
    Io(String),
    /// Fallback for anything that doesn't fit the above.
    #[error("other: {0}")]
    Other(String),
}

/// Parameters used to open a connection.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::{DbKind, OpenParams};
/// let p = OpenParams { kind: DbKind::Sqlite, dsn: ":memory:".into(), password: None };
/// assert_eq!(p.kind, DbKind::Sqlite);
/// ```
#[derive(Clone, Debug)]
pub struct OpenParams {
    /// Discriminator. Each driver checks this and rejects mismatches.
    pub kind: DbKind,
    /// Postgres DSN (`postgres://user:pass@host:port/db`) or SQLite path
    /// (`:memory:` or filesystem path).
    pub dsn: String,
    /// Optional resolved password from the secret store. Postgres uses it if
    /// the DSN does not already include one. Never logged.
    pub password: Option<String>,
}

/// Result of a non-`SELECT` statement.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::ExecResult;
/// let r = ExecResult { rows_affected: 3, duration_ms: 1 };
/// assert_eq!(r.rows_affected, 3);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecResult {
    /// Number of rows affected by the statement.
    pub rows_affected: u64,
    /// Wall-clock duration of the execution in milliseconds.
    pub duration_ms: u64,
}

/// One column header in a query result.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::{Column, ColumnType};
/// let c = Column { name: "id".into(), ty: ColumnType::Integer };
/// assert_eq!(c.name, "id");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Column {
    /// Column name as reported by the driver.
    pub name: String,
    /// Coarse type classification.
    pub ty: ColumnType,
}

/// Coarse column type. Drivers may emit `Other(name)` for everything they
/// can't fit into the simple set.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::ColumnType;
/// let _ = ColumnType::Text;
/// let _ = ColumnType::Other("uuid".into());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ColumnType {
    /// Textual / character data.
    Text,
    /// Integer of any width.
    Integer,
    /// Floating-point or numeric.
    Float,
    /// Boolean.
    Bool,
    /// Binary blob.
    Bytes,
    /// Always-null column.
    Null,
    /// Driver-specific type name we didn't map to one of the above.
    Other(String),
}

/// One row, rendered to display strings. Drivers convert each value to its
/// human-readable form (`NULL` for NULL, `0x…` for bytes, etc.).
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::Row;
/// let r = Row { values: vec!["a".into(), "1".into()] };
/// assert_eq!(r.values.len(), 2);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// One display string per column.
    pub values: Vec<String>,
}

/// Opaque pagination cursor. v1 uses `OFFSET` semantics for portability.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::PageCursor;
/// let c = PageCursor { offset: 100 };
/// assert_eq!(c.offset, 100);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageCursor {
    /// Number of rows already returned by previous pages.
    pub offset: u64,
}

/// One page of [`DbClient::query_paged`] results.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::QueryPage;
/// let p = QueryPage { columns: vec![], rows: vec![], next_cursor: None, duration_ms: 0 };
/// assert!(p.columns.is_empty());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryPage {
    /// Column headers (same length as each row's `values`).
    pub columns: Vec<Column>,
    /// Rows of the current page.
    pub rows: Vec<Row>,
    /// Cursor for the next page, or `None` at end-of-stream.
    pub next_cursor: Option<PageCursor>,
    /// Wall-clock duration of the query in milliseconds.
    pub duration_ms: u64,
}

/// Schema introspection result: a flat list of tables with their columns.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::SchemaInfo;
/// let s = SchemaInfo { tables: vec![] };
/// assert!(s.tables.is_empty());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SchemaInfo {
    /// Tables in the database, ordered by `(schema, name)` for Postgres or
    /// `name` for SQLite.
    pub tables: Vec<TableInfo>,
}

/// One row of [`SchemaInfo::tables`].
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::TableInfo;
/// let t = TableInfo { schema: None, name: "users".into(), columns: vec![] };
/// assert_eq!(t.name, "users");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TableInfo {
    /// Schema/namespace (Postgres). `None` for SQLite.
    pub schema: Option<String>,
    /// Table name.
    pub name: String,
    /// Columns in their defined order.
    pub columns: Vec<Column>,
}

/// Async database client trait. Implementations live in `sid-db-clients`.
///
/// # Object safety
///
/// All methods take `&self` and use no generics in method position. Returns
/// owned types only. Pass around as `Arc<dyn DbClient>`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use sid_core::adapters::db_client::{
///     DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor, QueryPage, SchemaInfo,
/// };
///
/// struct NoopDb;
/// #[async_trait::async_trait]
/// impl DbClient for NoopDb {
///     async fn open(&self, _p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> { Ok(Arc::new(NoopDb)) }
///     async fn close(&self) -> Result<(), DbError> { Ok(()) }
///     async fn execute(&self, _: &str) -> Result<ExecResult, DbError> {
///         Ok(ExecResult { rows_affected: 0, duration_ms: 0 })
///     }
///     async fn query_paged(&self, _: &str, _: Option<PageCursor>, _: u32) -> Result<QueryPage, DbError> {
///         Ok(QueryPage { columns: vec![], rows: vec![], next_cursor: None, duration_ms: 0 })
///     }
///     async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> { Ok(SchemaInfo { tables: vec![] }) }
///     async fn cancel(&self) -> Result<(), DbError> { Ok(()) }
///     fn kind(&self) -> DbKind { DbKind::Sqlite }
/// }
/// let _: Arc<dyn DbClient> = Arc::new(NoopDb);
/// ```
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
