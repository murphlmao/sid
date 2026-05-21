//! SqliteClient — rusqlite-backed `DbClient` impl. Wraps the sync rusqlite
//! API in `tokio::task::spawn_blocking` to fit the async trait.

use std::sync::{Arc, Mutex};

use sid_core::adapters::db_client::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor, QueryPage,
    Row, SchemaInfo, TableInfo,
};

/// Factory + per-connection client.
///
/// `SqliteClient::factory()` returns a stateless factory whose `open` method
/// returns an `Arc<dyn DbClient>` bound to the requested DSN.
///
/// # Examples
///
/// ```
/// use sid_db_clients::SqliteClient;
/// let _factory = SqliteClient::factory();
/// ```
pub struct SqliteClient {
    #[allow(dead_code)]
    inner: Option<Arc<Mutex<rusqlite::Connection>>>,
}

impl SqliteClient {
    /// Construct a stateless factory. Call [`DbClient::open`] on the returned
    /// handle to bind a real connection.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_db_clients::SqliteClient;
    /// let factory = SqliteClient::factory();
    /// assert_eq!(
    ///     sid_core::adapters::db_client::DbClient::kind(&*factory),
    ///     sid_core::adapters::db_client::DbKind::Sqlite,
    /// );
    /// ```
    pub fn factory() -> Arc<dyn DbClient> {
        Arc::new(Self { inner: None })
    }
}

#[async_trait::async_trait]
impl DbClient for SqliteClient {
    async fn open(&self, p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> {
        if p.kind != DbKind::Sqlite {
            return Err(DbError::Invalid(format!(
                "expected DbKind::Sqlite, got {:?}",
                p.kind
            )));
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

    async fn execute(&self, sql: &str) -> Result<ExecResult, DbError> {
        let conn = self.inner.clone().ok_or(DbError::NotConnected)?;
        let sql = sql.to_string();
        let start = std::time::Instant::now();
        let rows_affected: u64 = tokio::task::spawn_blocking(move || -> Result<u64, DbError> {
            let guard = conn
                .lock()
                .map_err(|e| DbError::Other(format!("mutex poisoned: {e}")))?;
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
        Ok(ExecResult {
            rows_affected,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn query_paged(
        &self,
        _sql: &str,
        _cursor: Option<PageCursor>,
        _page_size: u32,
    ) -> Result<QueryPage, DbError> {
        Err(DbError::Other(
            "query_paged: not yet implemented — Task 7".into(),
        ))
    }

    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        Err(DbError::Other(
            "schema_introspect: not yet implemented — Task 8".into(),
        ))
    }

    async fn cancel(&self) -> Result<(), DbError> {
        Ok(())
    }

    fn kind(&self) -> DbKind {
        DbKind::Sqlite
    }
}

// Helper used by Tasks 6-8.
#[allow(dead_code)]
fn map_rusqlite_error(e: rusqlite::Error) -> DbError {
    match e {
        rusqlite::Error::SqliteFailure(_, Some(ref msg)) if msg.starts_with("syntax") => {
            DbError::Syntax {
                offset: 0,
                message: msg.clone(),
            }
        }
        e => DbError::Query(e.to_string()),
    }
}

// Helper used by Tasks 7 and 8.
#[allow(dead_code)]
fn rusqlite_type_to_column_type(
    decl: Option<&str>,
    value_ty: rusqlite::types::Type,
) -> ColumnType {
    if let Some(d) = decl {
        let d = d.to_ascii_uppercase();
        if d.contains("INT") {
            return ColumnType::Integer;
        }
        if d.contains("CHAR") || d.contains("TEXT") || d.contains("CLOB") {
            return ColumnType::Text;
        }
        if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") {
            return ColumnType::Float;
        }
        if d.contains("BLOB") {
            return ColumnType::Bytes;
        }
        if d.contains("BOOL") {
            return ColumnType::Bool;
        }
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
