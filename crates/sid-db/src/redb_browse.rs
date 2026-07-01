//! `RedbBrowseClient` — a read-only `DbClient` over an opened `sid-store`
//! [`GlobalStore`], implementing the [`DbKind::Redb`] pseudo-engine (the POC's
//! `ConfigReader` idea, wired directly to the `DbClient` trait rather than a
//! separate reader trait).
//!
//! // ponytail: no SQL parsing here by design. `query_paged`'s `sql` argument
//! // is just a table-name selector (one of [`TABLE_NAMES`]) — this engine's
//! // whole job is "list tables / dump a table", not run a query language.
//! // Real SQL support would be solving a problem the redb store doesn't have.

use std::path::Path;
use std::sync::Arc;

use sid_core::db::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor, QueryPage,
    Row, SchemaInfo, TableInfo,
};
use sid_store::GlobalStore;

/// The fixed set of tables this read-only browse engine exposes, in the order
/// [`schema_introspect`](DbClient::schema_introspect) lists them.
const TABLE_NAMES: [&str; 5] = [
    "hosts",
    "connections",
    "quick_actions",
    "workspaces",
    "settings",
];

/// Read-only `DbClient` over a [`GlobalStore`]. Every mutating call
/// ([`execute`](DbClient::execute)) is rejected with [`DbError::Invalid`].
pub struct RedbBrowseClient {
    store: Option<Arc<GlobalStore>>,
}

impl RedbBrowseClient {
    /// Construct a stateless factory. Call [`DbClient::open`] to bind a store
    /// at a path, or use [`RedbBrowseClient::wrap`] to wrap an already-open
    /// store directly.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_db::RedbBrowseClient;
    /// let _factory = RedbBrowseClient::factory();
    /// ```
    pub fn factory() -> Arc<dyn DbClient> {
        Arc::new(Self { store: None })
    }

    /// Wrap an already-open store directly. This is how the Database tab
    /// wires the always-present redb pseudo-engine: the store is already open
    /// for the rest of the app, so there is no separate path-based DSN to
    /// open through [`DbClient::open`].
    pub fn wrap(store: Arc<GlobalStore>) -> Arc<dyn DbClient> {
        Arc::new(Self { store: Some(store) })
    }
}

#[async_trait::async_trait]
impl DbClient for RedbBrowseClient {
    async fn open(&self, p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> {
        if p.kind != DbKind::Redb {
            return Err(DbError::Invalid(format!(
                "expected DbKind::Redb, got {:?}",
                p.kind
            )));
        }
        let store =
            GlobalStore::open(Path::new(&p.dsn)).map_err(|e| DbError::Connect(e.to_string()))?;
        Ok(Self::wrap(Arc::new(store)))
    }

    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }

    async fn execute(&self, _sql: &str) -> Result<ExecResult, DbError> {
        Err(DbError::Invalid(
            "the redb browse engine is read-only".to_string(),
        ))
    }

    async fn query_paged(
        &self,
        sql: &str,
        cursor: Option<PageCursor>,
        page_size: u32,
    ) -> Result<QueryPage, DbError> {
        let store = self.store.as_ref().ok_or(DbError::NotConnected)?;
        let start = std::time::Instant::now();
        let table = sql.trim();
        let (columns, all_rows) = dump_table(store, table)?;
        let offset = cursor.map(|c| c.offset).unwrap_or(0) as usize;
        let page_size = page_size.max(1) as usize;
        let page: Vec<Row> = all_rows.into_iter().skip(offset).take(page_size).collect();
        let fetched = page.len() as u64;
        let next_cursor = if fetched < page_size as u64 {
            None
        } else {
            Some(PageCursor {
                offset: offset as u64 + fetched,
            })
        };
        Ok(QueryPage {
            columns,
            rows: page,
            next_cursor,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        // No store access needed — the table/column shape is fixed, not
        // discovered, so this works even before `open` (mirrors "list known
        // tables" rather than "ask the backend").
        let tables = TABLE_NAMES
            .iter()
            .map(|&name| TableInfo {
                schema: None,
                name: name.to_string(),
                columns: table_columns(name),
            })
            .collect();
        Ok(SchemaInfo { tables })
    }

    async fn cancel(&self) -> Result<(), DbError> {
        // Every query here is a synchronous, already-fast in-memory dump —
        // nothing to cancel.
        Ok(())
    }

    fn kind(&self) -> DbKind {
        DbKind::Redb
    }
}

/// Column names for one of the fixed [`TABLE_NAMES`]. Empty for anything else.
fn table_columns(name: &str) -> Vec<Column> {
    let cols: &[&str] = match name {
        "hosts" => &["alias", "user", "host", "port", "secret_ref", "auth"],
        "connections" => &["id", "dsn", "secret_ref"],
        "quick_actions" => &["label", "cmd"],
        "workspaces" => &["id", "root", "name"],
        "settings" => &["default_scope"],
        _ => &[],
    };
    cols.iter()
        .map(|&c| Column {
            name: c.to_string(),
            ty: ColumnType::Text,
        })
        .collect()
}

/// Render one store table's rows as display strings. `table` must be one of
/// [`TABLE_NAMES`]; anything else is a query error (there is no SQL parser to
/// fall back to).
fn dump_table(store: &GlobalStore, table: &str) -> Result<(Vec<Column>, Vec<Row>), DbError> {
    if !TABLE_NAMES.contains(&table) {
        return Err(DbError::Query(format!("unknown table: {table}")));
    }
    let columns = table_columns(table);
    let rows = match table {
        "hosts" => store
            .list_hosts()
            .map_err(|e| DbError::Other(e.to_string()))?
            .into_iter()
            .map(|h| Row {
                values: vec![
                    h.alias,
                    h.user,
                    h.host,
                    h.port.to_string(),
                    h.secret_ref.unwrap_or_default(),
                    format!("{:?}", h.auth),
                ],
            })
            .collect(),
        "connections" => store
            .list_connections()
            .map_err(|e| DbError::Other(e.to_string()))?
            .into_iter()
            .map(|c| Row {
                values: vec![c.id, c.dsn, c.secret_ref.unwrap_or_default()],
            })
            .collect(),
        "quick_actions" => store
            .list_quick_actions()
            .map_err(|e| DbError::Other(e.to_string()))?
            .into_iter()
            .map(|q| Row {
                values: vec![q.label, q.cmd],
            })
            .collect(),
        "workspaces" => store
            .list_workspaces()
            .map_err(|e| DbError::Other(e.to_string()))?
            .into_iter()
            .map(|w| Row {
                values: vec![
                    w.id.as_str().to_string(),
                    w.root.to_string_lossy().into_owned(),
                    w.name,
                ],
            })
            .collect(),
        "settings" => {
            let s = store
                .get_settings()
                .map_err(|e| DbError::Other(e.to_string()))?;
            vec![Row {
                values: vec![format!("{:?}", s.default_scope)],
            }]
        }
        _ => unreachable!("checked against TABLE_NAMES above"),
    };
    Ok((columns, rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_store::{AuthMethod, DbConnection, Host};

    /// A temp-file-backed store seeded with one host and one connection. The
    /// returned `TempDir` must be kept alive for as long as the store is used
    /// (dropping it removes the backing file).
    fn seeded_store() -> (tempfile::TempDir, Arc<GlobalStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        store
            .upsert_host(&Host {
                alias: "box1".into(),
                user: "u".into(),
                host: "h".into(),
                port: 22,
                secret_ref: None,
                auth: AuthMethod::Agent,
            })
            .unwrap();
        store
            .upsert_connection(&DbConnection {
                id: "conn1".into(),
                dsn: "postgres://x@y/z".into(),
                secret_ref: None,
            })
            .unwrap();
        (dir, Arc::new(store))
    }

    #[tokio::test]
    async fn schema_introspect_lists_all_store_tables() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let schema = client.schema_introspect().await.unwrap();
        let names: Vec<&str> = schema.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, TABLE_NAMES.to_vec());
        let hosts_table = &schema.tables[0];
        assert_eq!(hosts_table.columns.len(), 6);
    }

    #[tokio::test]
    async fn query_paged_dumps_seeded_hosts_table() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let page = client.query_paged("hosts", None, 10).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].values[0], "box1");
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn query_paged_dumps_seeded_connections_table() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let page = client.query_paged("connections", None, 10).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].values[0], "conn1");
        assert_eq!(page.rows[0].values[1], "postgres://x@y/z");
    }

    #[tokio::test]
    async fn query_paged_unknown_table_errors() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let err = client.query_paged("nope", None, 10).await.unwrap_err();
        match err {
            DbError::Query(_) => {}
            other => panic!("expected DbError::Query, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_is_rejected_as_read_only() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let err = client.execute("anything").await.unwrap_err();
        match err {
            DbError::Invalid(_) => {}
            other => panic!("expected DbError::Invalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn open_rejects_wrong_dbkind() {
        let factory = RedbBrowseClient::factory();
        let result = factory
            .open(OpenParams {
                kind: DbKind::Postgres,
                dsn: "irrelevant".into(),
                password: None,
                sqlite_mode: None,
            })
            .await;
        match result {
            Err(DbError::Invalid(_)) => {}
            Err(other) => panic!("expected DbError::Invalid, got {other:?}"),
            Ok(_) => panic!("wrong DbKind must not be accepted"),
        }
    }

    #[tokio::test]
    async fn open_by_path_creates_and_opens_a_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("via_open.redb");
        let factory = RedbBrowseClient::factory();
        let client = factory
            .open(OpenParams {
                kind: DbKind::Redb,
                dsn: path.to_string_lossy().into_owned(),
                password: None,
                sqlite_mode: None,
            })
            .await
            .unwrap();
        assert_eq!(client.kind(), DbKind::Redb);
        let schema = client.schema_introspect().await.unwrap();
        assert_eq!(schema.tables.len(), TABLE_NAMES.len());
    }
}
