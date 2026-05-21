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
        sql: &str,
        cursor: Option<PageCursor>,
        page_size: u32,
    ) -> Result<QueryPage, DbError> {
        let conn = self.inner.clone().ok_or(DbError::NotConnected)?;
        let sql = sql.to_string();
        let offset = cursor.map(|c| c.offset).unwrap_or(0);
        let page_size = page_size.max(1) as u64;
        let start = std::time::Instant::now();
        let (columns, rows, fetched) = tokio::task::spawn_blocking(
            move || -> Result<(Vec<Column>, Vec<Row>, u64), DbError> {
                let guard = conn
                    .lock()
                    .map_err(|e| DbError::Other(format!("mutex poisoned: {e}")))?;
                let trimmed = sql.trim().trim_end_matches(';');
                let wrapped = format!(
                    "SELECT * FROM ( {trimmed} ) LIMIT {page_size} OFFSET {offset}"
                );
                let mut stmt = guard.prepare(&wrapped).map_err(map_rusqlite_error)?;
                let columns: Vec<Column> = stmt
                    .columns()
                    .iter()
                    .map(|c| Column {
                        name: c.name().to_string(),
                        ty: rusqlite_type_to_column_type(
                            c.decl_type(),
                            rusqlite::types::Type::Null,
                        ),
                    })
                    .collect();
                let col_count = columns.len();
                let mut rows_out: Vec<Row> = Vec::with_capacity(page_size as usize);
                let mut rs = stmt.query([]).map_err(map_rusqlite_error)?;
                while let Some(row) = rs.next().map_err(map_rusqlite_error)? {
                    let mut values = Vec::with_capacity(col_count);
                    for i in 0..col_count {
                        let v: rusqlite::types::Value =
                            row.get(i).map_err(map_rusqlite_error)?;
                        values.push(render_sqlite_value(&v));
                    }
                    rows_out.push(Row { values });
                }
                let fetched = rows_out.len() as u64;
                Ok((columns, rows_out, fetched))
            },
        )
        .await
        .map_err(|e| DbError::Other(format!("join: {e}")))??;
        let next_cursor = if fetched < page_size {
            None
        } else {
            Some(PageCursor {
                offset: offset + fetched,
            })
        };
        Ok(QueryPage {
            columns,
            rows,
            next_cursor,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        let conn = self.inner.clone().ok_or(DbError::NotConnected)?;
        let tables = tokio::task::spawn_blocking(move || -> Result<Vec<TableInfo>, DbError> {
            let guard = conn
                .lock()
                .map_err(|e| DbError::Other(format!("mutex poisoned: {e}")))?;
            let mut stmt = guard
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .map_err(map_rusqlite_error)?;
            let table_names: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_rusqlite_error)?
                .filter_map(Result::ok)
                .collect();
            let mut tables: Vec<TableInfo> = Vec::with_capacity(table_names.len());
            for tn in table_names {
                // PRAGMA table_info() doesn't support param binding for the table
                // name, so we have to interpolate. Quote any embedded quote.
                let safe = tn.replace('"', "\"\"");
                let mut info_stmt = guard
                    .prepare(&format!(r#"PRAGMA table_info("{safe}")"#))
                    .map_err(map_rusqlite_error)?;
                let cols: Vec<Column> = info_stmt
                    .query_map([], |row| {
                        let name: String = row.get(1)?;
                        let decl: String = row.get(2).unwrap_or_default();
                        Ok(Column {
                            name,
                            ty: rusqlite_type_to_column_type(
                                Some(&decl),
                                rusqlite::types::Type::Null,
                            ),
                        })
                    })
                    .map_err(map_rusqlite_error)?
                    .filter_map(Result::ok)
                    .collect();
                tables.push(TableInfo {
                    schema: None,
                    name: tn,
                    columns: cols,
                });
            }
            Ok(tables)
        })
        .await
        .map_err(|e| DbError::Other(format!("join: {e}")))??;
        Ok(SchemaInfo { tables })
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
