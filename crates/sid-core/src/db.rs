//! Database engine discriminator and the `DbClient` adapter seam — the shared
//! contract between the store (`DbConnection.kind`), the adapter impls
//! (`sid-db`), and the frontend registry.
//!
//! The label round-trip is the single source of truth widgets/registries use instead
//! of matching variants directly. New engines append a variant (postcard encodes by
//! position, so appending is migration-safe).
//!
//! Ported near-verbatim from the `sid-poc` adapter
//! (`sid_core::adapters::db_client`), flattened into this module so it sits
//! beside [`DbKind`] rather than in a separate `adapters` tree. Where the POC
//! used a local `DbKind::ConfigReader` pseudo-engine variant, this module uses
//! [`DbKind::Redb`] — the canonical discriminator already defined here.
//! Concrete impls (`PostgresClient`, `SqliteClient`, the redb browse client)
//! live in `sid-db`; nothing in this crate names a concrete driver.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Which database engine a saved connection targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DbKind {
    /// PostgreSQL (adapter uses `tokio-postgres`).
    #[default]
    Postgres,
    /// SQLite (adapter uses bundled `rusqlite`).
    Sqlite,
    /// sid's own redb store, browsed read-only (the POC's `ConfigReader` pseudo-engine).
    Redb,
}

impl DbKind {
    /// Stable lowercase label used in committed config, registries, and UI selectors.
    ///
    /// # Examples
    /// ```
    /// use sid_core::db::DbKind;
    /// assert_eq!(DbKind::Postgres.label(), "postgres");
    /// ```
    pub fn label(self) -> &'static str {
        match self {
            DbKind::Postgres => "postgres",
            DbKind::Sqlite => "sqlite",
            DbKind::Redb => "redb",
        }
    }

    /// Parse a [`label`](Self::label); `None` if unrecognized.
    ///
    /// # Examples
    /// ```
    /// use sid_core::db::DbKind;
    /// assert_eq!(DbKind::from_label("sqlite"), Some(DbKind::Sqlite));
    /// assert_eq!(DbKind::from_label("mysql"), None);
    /// ```
    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "postgres" => Some(DbKind::Postgres),
            "sqlite" => Some(DbKind::Sqlite),
            "redb" => Some(DbKind::Redb),
            _ => None,
        }
    }
}

/// Domain-shaped error returned by every [`DbClient`] method.
///
/// # Examples
///
/// ```
/// use sid_core::db::DbError;
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
    /// Caller passed an invalid argument (e.g., wrong [`DbKind`], or a mutating
    /// call against a read-only engine such as [`DbKind::Redb`]).
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

/// How a SQLite connection should treat the target file: open an existing
/// database, or create a fresh one. This is an *open-time* choice (it is not
/// encoded in the DSN), so it lives on [`OpenParams`] rather than in the path.
///
/// Non-SQLite engines leave [`OpenParams::sqlite_mode`] as `None`.
///
/// # Examples
///
/// ```
/// use sid_core::db::SqliteMode;
/// assert_ne!(SqliteMode::OpenExisting, SqliteMode::CreateNew);
/// // It is `Copy`, so it can be passed around without moving.
/// let m = SqliteMode::OpenExisting;
/// let _copy = m;
/// assert_eq!(m, SqliteMode::OpenExisting);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SqliteMode {
    /// Open an existing database file; fail if it does not exist. Default for
    /// back-compatibility (a `None` mode is treated as this).
    OpenExisting,
    /// Create the database file if it is missing (the historical behaviour of
    /// a bare `rusqlite::Connection::open`).
    CreateNew,
}

/// Parameters used to open a connection.
///
/// # Examples
///
/// ```
/// use sid_core::db::{DbKind, OpenParams};
/// let p = OpenParams {
///     kind: DbKind::Sqlite,
///     dsn: ":memory:".into(),
///     password: None,
///     sqlite_mode: None,
/// };
/// assert_eq!(p.kind, DbKind::Sqlite);
/// assert!(p.sqlite_mode.is_none());
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
    /// SQLite open-vs-create mode. `None` for non-SQLite engines (or "don't
    /// care"); a `None` mode is treated as [`SqliteMode::OpenExisting`] by the
    /// SQLite client for back-compatibility.
    pub sqlite_mode: Option<SqliteMode>,
}

/// Result of a non-`SELECT` statement.
///
/// # Examples
///
/// ```
/// use sid_core::db::ExecResult;
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
/// use sid_core::db::{Column, ColumnType};
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
/// use sid_core::db::ColumnType;
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
/// use sid_core::db::Row;
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
/// use sid_core::db::PageCursor;
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
/// use sid_core::db::QueryPage;
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
/// use sid_core::db::SchemaInfo;
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
/// use sid_core::db::TableInfo;
/// let t = TableInfo { schema: None, name: "users".into(), columns: vec![] };
/// assert_eq!(t.name, "users");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TableInfo {
    /// Schema/namespace (Postgres). `None` for SQLite and the redb browse engine.
    pub schema: Option<String>,
    /// Table name.
    pub name: String,
    /// Columns in their defined order.
    pub columns: Vec<Column>,
}

/// One foreign-key edge in a [`SchemaGraph`]: `from_table.from_columns`
/// (the referencing, "many" side) points at `to_table.to_columns` (the
/// referenced, "one" side). Composite keys keep their column order.
///
/// Table names are **qualified the same way [`TableInfo`] displays them**:
/// `"schema.name"` when the engine has schemas (Postgres), bare `"name"`
/// otherwise — so the diagram view can join edges to table boxes by string
/// equality.
///
/// # Examples
///
/// ```
/// use sid_core::db::ForeignKey;
/// let fk = ForeignKey {
///     from_table: "orders".into(),
///     from_columns: vec!["customer_id".into()],
///     to_table: "customers".into(),
///     to_columns: vec!["id".into()],
/// };
/// assert_eq!(fk.to_table, "customers");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForeignKey {
    /// Referencing ("many"-side) table, qualified per the rule above.
    pub from_table: String,
    /// Referencing columns, in key order.
    pub from_columns: Vec<String>,
    /// Referenced ("one"-side) table, qualified per the rule above.
    pub to_table: String,
    /// Referenced columns, in key order (parallel to `from_columns`).
    pub to_columns: Vec<String>,
}

/// Relationship metadata for the diagram view, layered on top of
/// [`SchemaInfo`]: foreign-key edges plus each table's primary-key columns.
/// Engines without FK support return [`Default::default`] — the diagram
/// degrades to boxes with no lines.
///
/// # Examples
///
/// ```
/// use sid_core::db::SchemaGraph;
/// let g = SchemaGraph::default();
/// assert!(g.foreign_keys.is_empty() && g.primary_keys.is_empty());
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SchemaGraph {
    /// Every FK edge in the database, deterministically ordered
    /// (by `(from_table, to_table, from_columns)`).
    pub foreign_keys: Vec<ForeignKey>,
    /// Table (qualified, same rule as [`ForeignKey`]) → primary-key column
    /// names in key order. Tables without a PK are simply absent.
    pub primary_keys: std::collections::BTreeMap<String, Vec<String>>,
}

/// Async database client trait. Implementations live in `sid-db`.
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
/// use sid_core::db::{
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
    /// `sqlite_master` + `PRAGMA table_info`; the redb browse engine lists its
    /// fixed set of store tables.
    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError>;

    /// Relationship metadata (FK edges + primary keys) for the diagram view.
    /// Defaults to an empty graph so engines without foreign keys — or ones
    /// not yet wired — degrade to a diagram of boxes with no lines rather
    /// than an error.
    async fn schema_graph(&self) -> Result<SchemaGraph, DbError> {
        Ok(SchemaGraph::default())
    }

    /// Best-effort cancel of an in-flight query. SQLite and the redb browse
    /// engine are no-ops; Postgres sends a `CancelRequest` on a side channel.
    async fn cancel(&self) -> Result<(), DbError>;

    /// Discriminator. Useful for UI labels and dialect-specific logic.
    fn kind(&self) -> DbKind;
}

/// The widget-facing kind of a single connection-form field. The binary maps
/// each variant to a concrete form field; the rendering widget never needs to
/// know which engine produced it.
///
/// # Examples
///
/// ```
/// use sid_core::db::ConnFieldKind;
/// let port = ConnFieldKind::Port;
/// assert!(matches!(port, ConnFieldKind::Port));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnFieldKind {
    /// Free text (host, database name, user, ...).
    Text,
    /// A TCP port — numeric, validated 1..=65535 by the binary.
    Port,
    /// A filesystem path (e.g. a SQLite file). `~` expansion is the binary's job.
    Path,
    /// A secret — masked in the UI, routed to the secret store, never persisted
    /// in the `DbConnection.dsn`.
    Password,
    /// A closed set of string options (rendered as a cycling/Choice control).
    Choice {
        /// The allowed values, in display order.
        options: Vec<String>,
    },
    /// A boolean toggle.
    Bool,
}

/// One field a database engine needs collected to open a connection. A
/// [`DbClientDescriptor`] returns an ordered `Vec<ConnField>`; the binary
/// renders the connection form from it, so no engine-specific form layout is
/// hardcoded.
///
/// # Examples
///
/// ```
/// use sid_core::db::{ConnField, ConnFieldKind};
/// let f = ConnField::new("host", "Host", ConnFieldKind::Text).required();
/// assert_eq!(f.key, "host");
/// assert!(f.required);
/// assert_eq!(f.default, None);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnField {
    /// Stable field key (the form value map is keyed by this).
    pub key: String,
    /// Human-facing label.
    pub label: String,
    /// What kind of control/validation this field needs.
    pub kind: ConnFieldKind,
    /// Whether the field must be non-empty on submit.
    pub required: bool,
    /// Optional default value pre-filled into a fresh form.
    pub default: Option<String>,
}

impl ConnField {
    /// A new optional field with no default.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::db::{ConnField, ConnFieldKind};
    /// let f = ConnField::new("path", "File", ConnFieldKind::Path);
    /// assert!(!f.required);
    /// ```
    pub fn new(key: impl Into<String>, label: impl Into<String>, kind: ConnFieldKind) -> Self {
        ConnField {
            key: key.into(),
            label: label.into(),
            kind,
            required: false,
            default: None,
        }
    }

    /// Mark the field required (builder style).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::db::{ConnField, ConnFieldKind};
    /// assert!(ConnField::new("host", "Host", ConnFieldKind::Text).required().required);
    /// ```
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Set the field's default value (builder style).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::db::{ConnField, ConnFieldKind};
    /// let f = ConnField::new("port", "Port", ConnFieldKind::Port).with_default("5432");
    /// assert_eq!(f.default.as_deref(), Some("5432"));
    /// ```
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }
}

/// Engine metadata that makes the Database tab extensible: it declares which
/// fields a connection form needs and knows how to turn collected values into
/// [`OpenParams`] (and back, for prefill). Adding a database engine is then a
/// new [`DbClient`] impl + a `DbKind` variant + a `DbClientDescriptor` impl +
/// one registry registration in the binary — with **no** changes to the
/// Database widget.
///
/// Kept as a sibling of [`DbClient`] (rather than a method on it) so the
/// existing client trait and its implementors are untouched. The redb browse
/// engine ([`DbKind::Redb`]) deliberately has **no** descriptor — it is a
/// synthetic, always-present connection, never a form choice.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use sid_core::db::{
///     ConnField, ConnFieldKind, DbClientDescriptor, DbKind, OpenParams,
/// };
///
/// struct SqliteDesc;
/// impl DbClientDescriptor for SqliteDesc {
///     fn kind(&self) -> DbKind { DbKind::Sqlite }
///     fn connection_fields(&self) -> Vec<ConnField> {
///         vec![ConnField::new("path", "File", ConnFieldKind::Path).required()]
///     }
///     fn assemble_params(&self, values: &BTreeMap<String, String>) -> Result<OpenParams, String> {
///         let path = values.get("path").filter(|p| !p.is_empty())
///             .ok_or("path is required")?;
///         Ok(OpenParams { kind: DbKind::Sqlite, dsn: path.clone(), password: None, sqlite_mode: None })
///     }
///     fn dsn_to_field_values(&self, dsn: &str) -> BTreeMap<String, String> {
///         BTreeMap::from([("path".to_string(), dsn.to_string())])
///     }
/// }
///
/// let d = SqliteDesc;
/// assert_eq!(d.connection_fields().len(), 1);
/// let vals = BTreeMap::from([("path".to_string(), "/tmp/a.db".to_string())]);
/// assert_eq!(d.assemble_params(&vals).unwrap().dsn, "/tmp/a.db");
/// ```
pub trait DbClientDescriptor: Send + Sync {
    /// The engine this descriptor describes.
    fn kind(&self) -> DbKind;

    /// The ordered, engine-specific fields the connection form must collect
    /// (the shared `name` + `kind` fields are added by the binary, not here).
    fn connection_fields(&self) -> Vec<ConnField>;

    /// Turn collected form values (keyed by [`ConnField::key`]) into
    /// [`OpenParams`]. Returns `Err(message)` on a validation failure (e.g. a
    /// missing required field). The password, if any, belongs in
    /// `OpenParams.password` (never spliced into `dsn`).
    fn assemble_params(
        &self,
        values: &std::collections::BTreeMap<String, String>,
    ) -> Result<OpenParams, String>;

    /// Inverse of [`assemble_params`](Self::assemble_params) for prefill: parse
    /// a stored `dsn` back into form field values. Best-effort — unknown shapes
    /// yield an empty/partial map rather than erroring.
    fn dsn_to_field_values(&self, dsn: &str) -> std::collections::BTreeMap<String, String>;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn label_round_trips_and_rejects_unknown() {
        for k in [DbKind::Postgres, DbKind::Sqlite, DbKind::Redb] {
            assert_eq!(DbKind::from_label(k.label()), Some(k));
        }
        assert_eq!(DbKind::from_label("mysql"), None);
    }

    #[test]
    fn sqlite_mode_variants_are_distinct() {
        assert_ne!(SqliteMode::OpenExisting, SqliteMode::CreateNew);
        assert_eq!(SqliteMode::OpenExisting, SqliteMode::OpenExisting);
        assert_eq!(SqliteMode::CreateNew, SqliteMode::CreateNew);
    }

    #[test]
    fn sqlite_mode_is_copy() {
        // Compiles only if SqliteMode: Copy — used by value in several places.
        let m = SqliteMode::CreateNew;
        let a = m;
        let b = m;
        assert_eq!(a, b);
    }

    #[test]
    fn sqlite_mode_serde_round_trips() {
        for mode in [SqliteMode::OpenExisting, SqliteMode::CreateNew] {
            let json = serde_json::to_string(&mode).expect("serialize");
            let back: SqliteMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(mode, back);
        }
    }

    #[test]
    fn open_params_carries_sqlite_mode() {
        let p = OpenParams {
            kind: DbKind::Sqlite,
            dsn: ":memory:".into(),
            password: None,
            sqlite_mode: Some(SqliteMode::CreateNew),
        };
        assert_eq!(p.sqlite_mode, Some(SqliteMode::CreateNew));

        let none = OpenParams {
            kind: DbKind::Postgres,
            dsn: "postgres://x@y/z".into(),
            password: None,
            sqlite_mode: None,
        };
        assert_eq!(none.sqlite_mode, None);
    }

    #[test]
    fn conn_field_builders_compose() {
        let f = ConnField::new("port", "Port", ConnFieldKind::Port)
            .required()
            .with_default("5432");
        assert_eq!(f.key, "port");
        assert_eq!(f.label, "Port");
        assert_eq!(f.kind, ConnFieldKind::Port);
        assert!(f.required);
        assert_eq!(f.default.as_deref(), Some("5432"));
    }

    #[test]
    fn conn_field_defaults_are_optional_and_no_default() {
        let f = ConnField::new("path", "File", ConnFieldKind::Path);
        assert!(!f.required);
        assert_eq!(f.default, None);
    }

    #[test]
    fn conn_field_kind_choice_carries_options() {
        let k = ConnFieldKind::Choice {
            options: vec!["open_existing".into(), "create_new".into()],
        };
        match k {
            ConnFieldKind::Choice { options } => assert_eq!(options.len(), 2),
            other => panic!("expected Choice, got {other:?}"),
        }
    }

    // A dummy descriptor proving the trait alone is enough to add an engine —
    // the extensibility contract, exercised without any widget/binary change.
    struct DummyDesc;
    impl DbClientDescriptor for DummyDesc {
        fn kind(&self) -> DbKind {
            DbKind::Sqlite
        }
        fn connection_fields(&self) -> Vec<ConnField> {
            vec![
                ConnField::new("endpoint", "Endpoint", ConnFieldKind::Text).required(),
                ConnField::new("tls", "Use TLS", ConnFieldKind::Bool).with_default("true"),
            ]
        }
        fn assemble_params(&self, values: &BTreeMap<String, String>) -> Result<OpenParams, String> {
            let endpoint = values
                .get("endpoint")
                .filter(|e| !e.is_empty())
                .ok_or("endpoint is required")?;
            Ok(OpenParams {
                kind: DbKind::Sqlite,
                dsn: endpoint.clone(),
                password: None,
                sqlite_mode: None,
            })
        }
        fn dsn_to_field_values(&self, dsn: &str) -> BTreeMap<String, String> {
            BTreeMap::from([("endpoint".to_string(), dsn.to_string())])
        }
    }

    #[test]
    fn descriptor_is_object_safe_and_drives_fields() {
        let d: &dyn DbClientDescriptor = &DummyDesc;
        let fields = d.connection_fields();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].key, "endpoint");
        assert!(fields[0].required);
    }

    #[test]
    fn descriptor_assemble_params_validates_required() {
        let d = DummyDesc;
        // Missing required field -> Err.
        let empty = BTreeMap::new();
        assert!(d.assemble_params(&empty).is_err());
        // Present -> Ok with the dsn carried through.
        let vals = BTreeMap::from([("endpoint".to_string(), "host:9000".to_string())]);
        let params = d.assemble_params(&vals).expect("valid");
        assert_eq!(params.dsn, "host:9000");
        assert!(params.password.is_none());
    }

    #[test]
    fn descriptor_dsn_round_trips_through_field_values() {
        let d = DummyDesc;
        let vals = d.dsn_to_field_values("host:9000");
        let params = d.assemble_params(&vals).expect("valid");
        assert_eq!(params.dsn, "host:9000");
    }

    #[test]
    fn dbclient_trait_objects_are_object_safe() {
        // Compiles only if `DbClient` is object-safe — the contract every
        // concrete impl (in `sid-db`) is built against.
        struct NoopDb;
        #[async_trait::async_trait]
        impl DbClient for NoopDb {
            async fn open(&self, _p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> {
                Ok(Arc::new(NoopDb))
            }
            async fn close(&self) -> Result<(), DbError> {
                Ok(())
            }
            async fn execute(&self, _: &str) -> Result<ExecResult, DbError> {
                Ok(ExecResult {
                    rows_affected: 0,
                    duration_ms: 0,
                })
            }
            async fn query_paged(
                &self,
                _: &str,
                _: Option<PageCursor>,
                _: u32,
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
            async fn cancel(&self) -> Result<(), DbError> {
                Ok(())
            }
            fn kind(&self) -> DbKind {
                DbKind::Sqlite
            }
        }
        let clients: Vec<Arc<dyn DbClient>> = vec![Arc::new(NoopDb)];
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].kind(), DbKind::Sqlite);
    }

    #[test]
    fn descriptor_trait_objects_compose_across_engines() {
        // The whole extensibility story: a new engine plugs in behind the
        // trait object without the registry knowing its concrete type.
        let descriptors: Vec<Box<dyn DbClientDescriptor>> =
            vec![Box::new(DummyDesc), Box::new(DummyDesc)];
        assert_eq!(descriptors.len(), 2);
    }
}
