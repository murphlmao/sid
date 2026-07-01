//! SqliteClient — rusqlite-backed `DbClient` impl. Wraps the sync rusqlite
//! API in `tokio::task::spawn_blocking` to fit the async trait.

use std::sync::{Arc, Mutex};

use sid_core::db::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor, QueryPage,
    Row, SchemaInfo, SqliteMode, TableInfo,
};

/// Factory + per-connection client.
///
/// `SqliteClient::factory()` returns a stateless factory whose `open` method
/// returns an `Arc<dyn DbClient>` bound to the requested DSN.
///
/// # Examples
///
/// ```
/// use sid_db::SqliteClient;
/// let _factory = SqliteClient::factory();
/// ```
pub struct SqliteClient {
    inner: Option<Arc<Mutex<rusqlite::Connection>>>,
}

impl SqliteClient {
    /// Construct a stateless factory. Call [`DbClient::open`] on the returned
    /// handle to bind a real connection.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_db::SqliteClient;
    /// let factory = SqliteClient::factory();
    /// assert_eq!(
    ///     sid_core::db::DbClient::kind(&*factory),
    ///     sid_core::db::DbKind::Sqlite,
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
        // `None` is treated as OpenExisting for back-compatibility (a stored
        // connection from before the mode field existed must not silently
        // create a fresh, empty database when re-opened).
        let mode = p.sqlite_mode.unwrap_or(SqliteMode::OpenExisting);
        let conn = tokio::task::spawn_blocking(move || -> Result<rusqlite::Connection, DbError> {
            use rusqlite::OpenFlags;

            // `:memory:` is always openable regardless of mode — there is no
            // file to exist or create.
            if dsn == ":memory:" {
                return rusqlite::Connection::open_in_memory()
                    .map_err(|e| DbError::Connect(e.to_string()));
            }

            match mode {
                // CreateNew = the historical behaviour: create the file if it
                // is missing, otherwise open it.
                SqliteMode::CreateNew => {
                    rusqlite::Connection::open(&dsn).map_err(|e| DbError::Connect(e.to_string()))
                }
                // OpenExisting = open without SQLITE_OPEN_CREATE so a missing
                // file is a clear error rather than a surprise empty database.
                SqliteMode::OpenExisting => {
                    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
                        | OpenFlags::SQLITE_OPEN_URI
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
                    rusqlite::Connection::open_with_flags(&dsn, flags).map_err(|e| {
                        DbError::Connect(format!(
                            "SQLite database '{dsn}' could not be opened \
                             (does the file exist? use \"Create new\" to create it): {e}"
                        ))
                    })
                }
            }
        })
        .await
        .map_err(|e| DbError::Other(format!("join: {e}")))??;
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
                // NOTE (single-SELECT limitation): wrapping in a subquery only
                // works for a single SELECT statement — multi-statement input
                // or non-SELECT input will fail or behave oddly. The query
                // editor is expected to send one SELECT at a time.
                let trimmed = sql.trim().trim_end_matches(';');
                let wrapped =
                    format!("SELECT * FROM ( {trimmed} ) LIMIT {page_size} OFFSET {offset}");
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
                        let v: rusqlite::types::Value = row.get(i).map_err(map_rusqlite_error)?;
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

fn rusqlite_type_to_column_type(decl: Option<&str>, value_ty: rusqlite::types::Type) -> ColumnType {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_existing_fails_when_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.sqlite");
        let factory = SqliteClient::factory();
        let result = factory
            .open(OpenParams {
                kind: DbKind::Sqlite,
                dsn: path.to_string_lossy().into_owned(),
                password: None,
                sqlite_mode: Some(SqliteMode::OpenExisting),
            })
            .await;
        match result {
            Err(DbError::Connect(_)) => {}
            Err(other) => panic!("expected DbError::Connect, got {other:?}"),
            Ok(_) => panic!("missing file should not silently create a db"),
        }
    }

    #[tokio::test]
    async fn create_new_creates_a_fresh_file_and_open_existing_then_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.sqlite");
        let factory = SqliteClient::factory();
        let created = factory
            .open(OpenParams {
                kind: DbKind::Sqlite,
                dsn: path.to_string_lossy().into_owned(),
                password: None,
                sqlite_mode: Some(SqliteMode::CreateNew),
            })
            .await
            .expect("create new");
        created
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)")
            .await
            .expect("create table");
        assert!(path.exists());

        // Reopening with OpenExisting must now succeed (file exists).
        let reopened = factory
            .open(OpenParams {
                kind: DbKind::Sqlite,
                dsn: path.to_string_lossy().into_owned(),
                password: None,
                sqlite_mode: Some(SqliteMode::OpenExisting),
            })
            .await
            .expect("open existing after create");
        assert_eq!(reopened.kind(), DbKind::Sqlite);
    }

    #[tokio::test]
    async fn query_paged_round_trips_rows_on_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.sqlite");
        let factory = SqliteClient::factory();
        let client = factory
            .open(OpenParams {
                kind: DbKind::Sqlite,
                dsn: path.to_string_lossy().into_owned(),
                password: None,
                sqlite_mode: Some(SqliteMode::CreateNew),
            })
            .await
            .expect("create new");
        client
            .execute("CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .await
            .expect("create table");
        client
            .execute("INSERT INTO people (name) VALUES ('alice'), ('bob')")
            .await
            .expect("insert");

        let page = client
            .query_paged("SELECT id, name FROM people ORDER BY id", None, 10)
            .await
            .expect("query");
        assert_eq!(page.columns.len(), 2);
        assert_eq!(page.columns[0].name, "id");
        assert_eq!(page.columns[1].name, "name");
        assert_eq!(page.rows.len(), 2);
        assert_eq!(
            page.rows[0].values,
            vec!["1".to_string(), "alice".to_string()]
        );
        assert_eq!(
            page.rows[1].values,
            vec!["2".to_string(), "bob".to_string()]
        );
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn schema_introspect_lists_created_table_and_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema.sqlite");
        let factory = SqliteClient::factory();
        let client = factory
            .open(OpenParams {
                kind: DbKind::Sqlite,
                dsn: path.to_string_lossy().into_owned(),
                password: None,
                sqlite_mode: Some(SqliteMode::CreateNew),
            })
            .await
            .expect("create new");
        client
            .execute("CREATE TABLE widgets (id INTEGER PRIMARY KEY, label TEXT)")
            .await
            .expect("create table");

        let schema = client.schema_introspect().await.expect("introspect");
        assert_eq!(schema.tables.len(), 1);
        assert_eq!(schema.tables[0].name, "widgets");
        assert_eq!(schema.tables[0].columns.len(), 2);
        assert_eq!(schema.tables[0].columns[0].name, "id");
        assert_eq!(schema.tables[0].columns[1].name, "label");
    }

    #[test]
    fn map_rusqlite_error_maps_query_error() {
        let e = rusqlite::Error::InvalidQuery;
        match map_rusqlite_error(e) {
            DbError::Query(_) => {}
            other => panic!("expected DbError::Query, got {other:?}"),
        }
    }

    #[test]
    fn rusqlite_type_to_column_type_maps_declared_types() {
        assert_eq!(
            rusqlite_type_to_column_type(Some("INTEGER"), rusqlite::types::Type::Null),
            ColumnType::Integer
        );
        assert_eq!(
            rusqlite_type_to_column_type(Some("TEXT"), rusqlite::types::Type::Null),
            ColumnType::Text
        );
        assert_eq!(
            rusqlite_type_to_column_type(None, rusqlite::types::Type::Real),
            ColumnType::Float
        );
    }

    #[test]
    fn render_sqlite_value_formats_each_variant() {
        use rusqlite::types::Value;
        assert_eq!(render_sqlite_value(&Value::Null), "NULL");
        assert_eq!(render_sqlite_value(&Value::Integer(7)), "7");
        assert_eq!(
            render_sqlite_value(&Value::Blob(vec![0xAB, 0xCD])),
            "0xabcd"
        );
    }
}
