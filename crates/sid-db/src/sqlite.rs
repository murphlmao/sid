//! SqliteClient — rusqlite-backed `DbClient` impl. Wraps the sync rusqlite
//! API in `tokio::task::spawn_blocking` to fit the async trait.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use sid_core::db::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, ForeignKey, OpenParams, PageCursor,
    QueryPage, Row, SchemaGraph, SchemaInfo, SqliteMode, TableInfo,
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
                        values.push(render_sqlite_value(v));
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

    async fn schema_graph(&self) -> Result<SchemaGraph, DbError> {
        let conn = self.inner.clone().ok_or(DbError::NotConnected)?;
        tokio::task::spawn_blocking(move || -> Result<SchemaGraph, DbError> {
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
            drop(stmt);

            // Primary keys per table — also used below to resolve `to`-NULL FK rows.
            let mut primary_keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for tn in &table_names {
                let pk = table_primary_key(&guard, tn)?;
                if !pk.is_empty() {
                    primary_keys.insert(tn.clone(), pk);
                }
            }

            let mut foreign_keys: Vec<ForeignKey> = Vec::new();
            for tn in &table_names {
                let safe = tn.replace('"', "\"\"");
                let mut fk_stmt = guard
                    .prepare(&format!(r#"PRAGMA foreign_key_list("{safe}")"#))
                    .map_err(map_rusqlite_error)?;
                // Columns: id, seq, table, from, to, on_update, on_delete, match.
                // `id` groups the columns of one FK constraint; `seq` orders a
                // composite FK's columns.
                let rows: Vec<(i64, i64, String, String, Option<String>)> = fk_stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    })
                    .map_err(map_rusqlite_error)?
                    .filter_map(Result::ok)
                    .collect();
                drop(fk_stmt);

                let mut groups: BTreeMap<i64, Vec<(i64, String, String, Option<String>)>> =
                    BTreeMap::new();
                for (id, seq, ref_table, from_col, to_col) in rows {
                    groups
                        .entry(id)
                        .or_default()
                        .push((seq, ref_table, from_col, to_col));
                }
                for (_id, mut cols) in groups {
                    cols.sort_by_key(|c| c.0);
                    let ref_table = cols[0].1.clone();
                    let from_columns: Vec<String> = cols.iter().map(|c| c.2.clone()).collect();
                    // DECISION: SQLite reports `to = NULL` when the FK declaration
                    // omitted the referenced columns (`REFERENCES parent` with no
                    // column list) — SQLite itself resolves that case to the
                    // referenced table's primary key. We mirror that resolution via
                    // the `primary_keys` map built above, but only when the PK's
                    // arity matches the FK's arity; otherwise we fall back to an
                    // empty string per unresolved column rather than guess.
                    let to_columns: Vec<String> = if cols.iter().any(|c| c.3.is_none()) {
                        match primary_keys.get(&ref_table) {
                            Some(pk) if pk.len() == from_columns.len() => pk.clone(),
                            _ => cols
                                .iter()
                                .map(|c| c.3.clone().unwrap_or_default())
                                .collect(),
                        }
                    } else {
                        cols.iter().map(|c| c.3.clone().unwrap()).collect()
                    };
                    foreign_keys.push(ForeignKey {
                        from_table: tn.clone(),
                        from_columns,
                        to_table: ref_table,
                        to_columns,
                    });
                }
            }
            foreign_keys.sort_by(|a, b| {
                (&a.from_table, &a.to_table, &a.from_columns).cmp(&(
                    &b.from_table,
                    &b.to_table,
                    &b.from_columns,
                ))
            });
            Ok(SchemaGraph {
                foreign_keys,
                primary_keys,
            })
        })
        .await
        .map_err(|e| DbError::Other(format!("join: {e}")))?
    }

    async fn cancel(&self) -> Result<(), DbError> {
        Ok(())
    }

    fn kind(&self) -> DbKind {
        DbKind::Sqlite
    }
}

/// A table's primary-key columns in key order (`PRAGMA table_info`'s `pk`
/// column is the 1-based composite position; 0 means "not part of the PK").
/// Empty for a table with no PK (e.g. a `WITHOUT ROWID` table missing one, or
/// a plain rowid table that never declared one).
fn table_primary_key(guard: &rusqlite::Connection, table: &str) -> Result<Vec<String>, DbError> {
    let safe = table.replace('"', "\"\"");
    let mut stmt = guard
        .prepare(&format!(r#"PRAGMA table_info("{safe}")"#))
        .map_err(map_rusqlite_error)?;
    let mut rows: Vec<(i64, String)> = stmt
        .query_map([], |row| {
            let pk: i64 = row.get(5)?;
            let name: String = row.get(1)?;
            Ok((pk, name))
        })
        .map_err(map_rusqlite_error)?
        .filter_map(Result::ok)
        .filter(|(pk, _)| *pk > 0)
        .collect();
    rows.sort_by_key(|(pk, _)| *pk);
    Ok(rows.into_iter().map(|(_, name)| name).collect())
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

/// Takes `Value` by value (not `&Value`) so the `Text` arm can move the already-owned
/// `String` out instead of cloning it (perf audit finding #4) — the caller (`query_paged`
/// above) already holds an owned `Value` per cell (`row.get(i)`), so there was never a
/// shared borrow to preserve.
fn render_sqlite_value(v: rusqlite::types::Value) -> String {
    use rusqlite::types::Value;
    match v {
        Value::Null => "NULL".to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Text(s) => s,
        Value::Blob(b) => {
            let mut s = String::with_capacity(2 + b.len() * 2);
            s.push_str("0x");
            for byte in &b {
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
        assert_eq!(render_sqlite_value(Value::Null), "NULL");
        assert_eq!(render_sqlite_value(Value::Integer(7)), "7");
        assert_eq!(render_sqlite_value(Value::Blob(vec![0xAB, 0xCD])), "0xabcd");
    }

    #[tokio::test]
    async fn schema_graph_reports_single_and_composite_fks_plus_pks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.sqlite");
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
            .execute("PRAGMA foreign_keys = ON")
            .await
            .expect("enable fk pragma");
        client
            .execute("CREATE TABLE publishers (id INTEGER PRIMARY KEY, name TEXT)")
            .await
            .expect("create publishers");
        client
            .execute(
                "CREATE TABLE editions (year INTEGER, region TEXT, name TEXT, \
                 PRIMARY KEY (year, region))",
            )
            .await
            .expect("create editions");
        client
            .execute(
                "CREATE TABLE books (\
                    id INTEGER PRIMARY KEY, \
                    publisher_id INTEGER REFERENCES publishers(id), \
                    year INTEGER, \
                    region TEXT, \
                    FOREIGN KEY (year, region) REFERENCES editions(year, region)\
                 )",
            )
            .await
            .expect("create books");

        let graph = client.schema_graph().await.expect("schema_graph");

        // PKs: single-column and a composite PK, key order preserved.
        assert_eq!(
            graph.primary_keys.get("books"),
            Some(&vec!["id".to_string()])
        );
        assert_eq!(
            graph.primary_keys.get("publishers"),
            Some(&vec!["id".to_string()])
        );
        assert_eq!(
            graph.primary_keys.get("editions"),
            Some(&vec!["year".to_string(), "region".to_string()])
        );

        // FKs: deterministically ordered by (from_table, to_table, from_columns).
        // Both edges share from_table "books"; "editions" < "publishers" sorts first.
        assert_eq!(graph.foreign_keys.len(), 2);
        assert_eq!(graph.foreign_keys[0].from_table, "books");
        assert_eq!(graph.foreign_keys[0].to_table, "editions");
        assert_eq!(
            graph.foreign_keys[0].from_columns,
            vec!["year".to_string(), "region".to_string()]
        );
        assert_eq!(
            graph.foreign_keys[0].to_columns,
            vec!["year".to_string(), "region".to_string()]
        );
        assert_eq!(graph.foreign_keys[1].from_table, "books");
        assert_eq!(graph.foreign_keys[1].to_table, "publishers");
        assert_eq!(
            graph.foreign_keys[1].from_columns,
            vec!["publisher_id".to_string()]
        );
        assert_eq!(graph.foreign_keys[1].to_columns, vec!["id".to_string()]);
    }

    #[tokio::test]
    async fn schema_graph_resolves_to_null_fk_via_referenced_pk() {
        // `REFERENCES parent` with no column list makes SQLite report the FK's
        // `to` column as NULL in `PRAGMA foreign_key_list` — we resolve that to
        // the referenced table's primary key (see the DECISION comment in
        // `schema_graph`).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("implicit_ref.sqlite");
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
            .execute("CREATE TABLE parent (id INTEGER PRIMARY KEY)")
            .await
            .expect("create parent");
        client
            .execute(
                "CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent)",
            )
            .await
            .expect("create child");

        let graph = client.schema_graph().await.expect("schema_graph");
        assert_eq!(graph.foreign_keys.len(), 1);
        let fk = &graph.foreign_keys[0];
        assert_eq!(fk.from_table, "child");
        assert_eq!(fk.from_columns, vec!["parent_id".to_string()]);
        assert_eq!(fk.to_table, "parent");
        assert_eq!(fk.to_columns, vec!["id".to_string()]);
    }

    #[tokio::test]
    async fn schema_graph_is_empty_for_a_schema_with_no_tables() {
        let factory = SqliteClient::factory();
        let client = factory
            .open(OpenParams {
                kind: DbKind::Sqlite,
                dsn: ":memory:".into(),
                password: None,
                sqlite_mode: None,
            })
            .await
            .expect("open in-memory");
        let graph = client.schema_graph().await.expect("schema_graph");
        assert!(graph.foreign_keys.is_empty());
        assert!(graph.primary_keys.is_empty());
    }
}
