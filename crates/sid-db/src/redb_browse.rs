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
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, ExplainSupport, OpenParams,
    PageCursor, QueryPage, Row, SchemaGraph, SchemaInfo, TableInfo,
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

/// The store's table shape, without a store.
///
/// The same value [`DbClient::schema_introspect`] returns, available synchronously and
/// from nothing at all — this engine's shape is *fixed*, not discovered, so there is
/// nothing to await. The Database tab uses it to fill its schema tree the instant the
/// user selects the store, rather than spawning a task that resolves immediately.
///
/// # Examples
///
/// ```
/// let schema = sid_db::redb_browse::browse_schema();
/// assert_eq!(schema.tables.len(), 5);
/// assert_eq!(schema.tables[0].name, "hosts");
/// ```
pub fn browse_schema() -> SchemaInfo {
    let tables = TABLE_NAMES
        .iter()
        .map(|&name| TableInfo {
            schema: None,
            name: name.to_string(),
            columns: table_columns(name),
        })
        .collect();
    SchemaInfo { tables }
}

/// The store's relationship metadata: natural keys, and **no foreign keys** — the fixed
/// store tables do not reference each other, so there is no join surface to draw.
///
/// The synchronous companion to [`browse_schema`], for the same reason.
pub fn browse_graph() -> SchemaGraph {
    let mut primary_keys = std::collections::BTreeMap::new();
    for &name in TABLE_NAMES.iter() {
        let pk = table_primary_key(name);
        if !pk.is_empty() {
            primary_keys.insert(name.to_string(), pk);
        }
    }
    SchemaGraph {
        foreign_keys: Vec::new(),
        primary_keys,
    }
}

/// One store table's page, read from an **already-open** [`GlobalStore`].
///
/// # Why this exists beside the `DbClient` impl
///
/// redb takes an **exclusive file lock**: `GlobalStore::open` goes to
/// `redb::Database::create`, so a second handle onto the store the app is already
/// running on fails to open. That rules out [`DbClient::open`] as the Database tab's
/// route to browsing sid's *own* store — the tab has to read through the handle the app
/// already holds (`sid_store::Store::global`), which is a `&GlobalStore` it does not own
/// and cannot put in an `Arc`.
///
/// So the read takes a borrow. [`DbClient::query_paged`] calls straight through to here,
/// which means there is exactly one implementation of what a store table looks like —
/// pinned by `browse_page_and_query_paged_return_the_same_page`.
///
/// `table` must be one of the fixed table names; **anything else is an error**. There is
/// no SQL parser behind this engine and nothing to fall back to, which is also what
/// makes it structurally impossible to smuggle a mutation through: `"hosts; DROP TABLE
/// hosts"` is not a statement, it is an unknown table name.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let store = sid_store::GlobalStore::open(std::path::Path::new("/tmp/x.redb"))?;
/// let page = sid_db::redb_browse::browse_page(&store, "hosts", None, 100)?;
/// assert!(sid_db::redb_browse::browse_page(&store, "not_a_table", None, 100).is_err());
/// # Ok(())
/// # }
/// ```
pub fn browse_page(
    store: &GlobalStore,
    table: &str,
    cursor: Option<PageCursor>,
    page_size: u32,
) -> Result<QueryPage, DbError> {
    let start = std::time::Instant::now();
    let (columns, all_rows) = dump_table(store, table.trim())?;
    let offset = cursor.map(|c| c.offset).unwrap_or(0) as usize;
    let page_size = page_size.max(1) as usize;
    let total = all_rows.len();
    let rows: Vec<Row> = all_rows.into_iter().skip(offset).take(page_size).collect();
    // `offset + rows.len() < total`, not "the page came back full": a table whose row
    // count is an exact multiple of the page size would otherwise offer a next page
    // that comes back empty. The total is known here because the dump is already
    // materialised — this engine has no streaming cursor to be coy about.
    let next_cursor = (offset + rows.len() < total).then_some(PageCursor {
        offset: (offset + rows.len()) as u64,
    });
    Ok(QueryPage {
        columns,
        rows,
        next_cursor,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

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
        // `sql` is a table-name selector, not a query — see the module doc. Delegated
        // so the borrowed-store entry point and this one cannot drift.
        let store = self.store.as_ref().ok_or(DbError::NotConnected)?;
        browse_page(store, sql, cursor, page_size)
    }

    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        // No store access needed — the table/column shape is fixed, not
        // discovered, so this works even before `open` (mirrors "list known
        // tables" rather than "ask the backend").
        Ok(browse_schema())
    }

    async fn schema_graph(&self) -> Result<SchemaGraph, DbError> {
        Ok(browse_graph())
    }

    fn explain_support(&self) -> ExplainSupport {
        // Not "unsupported" in the sense of "not built yet" — there is nothing
        // here to plan. `query_paged`'s argument is a table name, not a query
        // (see the module doc), so the whole execution strategy is "read that
        // table". The reason is worded for the user reading a disabled control.
        ExplainSupport::Unsupported {
            reason: "the sid store browser reads a table by name — there is no query to plan",
        }
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

/// The natural identity column for one of the fixed [`TABLE_NAMES`], per each
/// entity's [`sid_store::Identity::identity`] (or, for `workspaces`, the `id`
/// its store API keys lookups by). Empty for `settings` — a single global row
/// with no per-record key to expose.
fn table_primary_key(name: &str) -> Vec<String> {
    let col: Option<&str> = match name {
        "hosts" => Some("alias"),
        "connections" => Some("id"),
        "quick_actions" => Some("label"),
        "workspaces" => Some("id"),
        _ => None,
    };
    col.into_iter().map(str::to_string).collect()
}

/// Column names for one of the fixed [`TABLE_NAMES`]. Empty for anything else.
///
/// BUG 6: this list used to omit real [`sid_store::Host`]/[`sid_store::DbConnection`]
/// fields — `hosts` dropped `folder`, `connections` dropped `kind`/`name`/`folder`
/// entirely — so the redb browse engine silently hid columns that exist on the
/// entity. [`dump_table`] must emit values in this exact order.
///
/// Round-D fix (same shape as BUG 6): `settings` only listed `default_scope`, hiding
/// the other 3 [`sid_store::Settings`] fields (`file_browser_side`,
/// `secret_keyring_enabled`, `secret_file_enabled`) from the browse engine entirely.
/// `secret_file_enabled` stays listed even though it's currently dormant elsewhere —
/// it's still a real, persisted field and this engine's job is to show what's stored,
/// not to editorialize about which fields are "live".
fn table_columns(name: &str) -> Vec<Column> {
    let cols: &[&str] = match name {
        "hosts" => &[
            "alias",
            "user",
            "host",
            "port",
            "secret_ref",
            "auth",
            "folder",
        ],
        "connections" => &["id", "name", "kind", "dsn", "secret_ref", "folder"],
        "quick_actions" => &["label", "cmd"],
        "workspaces" => &["id", "root", "name"],
        "settings" => &[
            "default_scope",
            "file_browser_side",
            "secret_keyring_enabled",
            "secret_file_enabled",
            "theme",
        ],
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
                    h.folder.unwrap_or_default(),
                ],
            })
            .collect(),
        "connections" => store
            .list_connections()
            .map_err(|e| DbError::Other(e.to_string()))?
            .into_iter()
            .map(|c| Row {
                values: vec![
                    c.id,
                    c.name,
                    c.kind.label().to_string(),
                    c.dsn,
                    c.secret_ref.unwrap_or_default(),
                    c.folder.unwrap_or_default(),
                ],
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
                values: vec![
                    format!("{:?}", s.default_scope),
                    format!("{:?}", s.file_browser_side),
                    s.secret_keyring_enabled.to_string(),
                    s.secret_file_enabled.to_string(),
                    s.theme.clone(),
                ],
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

    /// A temp-file-backed store seeded with one host and one connection, both
    /// with `folder` set (and the connection's `name`/`kind` distinct from
    /// `id`) so the columns BUG 6 fixed have a non-default value to assert on.
    /// The returned `TempDir` must be kept alive for as long as the store is
    /// used (dropping it removes the backing file).
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
                folder: Some("prod".into()),
            })
            .unwrap();
        store
            .upsert_connection(&DbConnection {
                id: "conn1".into(),
                dsn: "postgres://x@y/z".into(),
                secret_ref: None,
                kind: sid_core::db::DbKind::Postgres,
                name: "Conn One".into(),
                folder: Some("analytics".into()),
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
        // 7, not 6 (BUG 6): alias, user, host, port, secret_ref, auth, folder.
        assert_eq!(hosts_table.columns.len(), 7);
    }

    #[tokio::test]
    async fn schema_introspect_settings_table_lists_all_five_fields() {
        // Round-D fix: `settings` used to expose only `default_scope`, silently
        // hiding the other `sid_store::Settings` fields from the browse engine.
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let schema = client.schema_introspect().await.unwrap();
        let settings_table = schema
            .tables
            .iter()
            .find(|t| t.name == "settings")
            .expect("settings table listed");
        let names: Vec<&str> = settings_table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "default_scope",
                "file_browser_side",
                "secret_keyring_enabled",
                "secret_file_enabled",
                "theme",
            ]
        );
    }

    #[tokio::test]
    async fn query_paged_dumps_seeded_hosts_table() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let page = client.query_paged("hosts", None, 10).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].values[0], "box1");
        // BUG 6: `folder` is column index 6 (alias,user,host,port,secret_ref,auth,folder).
        assert_eq!(page.rows[0].values.len(), 7);
        assert_eq!(page.rows[0].values[6], "prod");
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn query_paged_dumps_seeded_connections_table() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let page = client.query_paged("connections", None, 10).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        // BUG 6: column order is id, name, kind, dsn, secret_ref, folder.
        assert_eq!(
            page.rows[0].values,
            vec![
                "conn1",
                "Conn One",
                "postgres",
                "postgres://x@y/z",
                "",
                "analytics"
            ]
        );
    }

    #[tokio::test]
    async fn query_paged_dumps_settings_table_with_all_five_fields() {
        // Round-D fix: previously only `default_scope` was visible; the other
        // `sid_store::Settings` fields (including the dormant-but-still-persisted
        // `secret_file_enabled`) must show up too.
        let (_dir, store) = seeded_store();
        store
            .set_settings(&sid_store::Settings {
                default_scope: sid_store::DefaultScope::Global,
                file_browser_side: sid_store::PanelSide::Right,
                secret_keyring_enabled: false,
                secret_file_enabled: true,
                theme: "cosmos".into(),
            })
            .unwrap();
        let client = RedbBrowseClient::wrap(store);
        let page = client.query_paged("settings", None, 10).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(
            page.rows[0].values,
            vec!["Global", "Right", "false", "true", "cosmos"]
        );
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
    async fn schema_graph_has_no_fks_and_maps_natural_key_columns() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        let graph = client.schema_graph().await.unwrap();
        assert!(graph.foreign_keys.is_empty());
        assert_eq!(
            graph.primary_keys.get("hosts"),
            Some(&vec!["alias".to_string()])
        );
        assert_eq!(
            graph.primary_keys.get("connections"),
            Some(&vec!["id".to_string()])
        );
        assert_eq!(
            graph.primary_keys.get("quick_actions"),
            Some(&vec!["label".to_string()])
        );
        assert_eq!(
            graph.primary_keys.get("workspaces"),
            Some(&vec!["id".to_string()])
        );
        // `settings` is a singleton row — no per-record key to expose.
        assert!(!graph.primary_keys.contains_key("settings"));
    }

    // ---- the borrowed-store browse API (the Database tab's entry point) -----------

    #[test]
    fn browse_schema_matches_what_the_client_introspects() {
        // One description of the store's shape, reachable two ways. If these ever
        // disagreed, the tab's table list and the engine's own would be describing
        // different databases.
        let schema = browse_schema();
        let names: Vec<&str> = schema.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, TABLE_NAMES.to_vec());
        for table in &schema.tables {
            assert_eq!(
                table.columns,
                table_columns(&table.name),
                "{}: columns drifted",
                table.name
            );
        }
    }

    #[tokio::test]
    async fn browse_schema_and_the_async_client_agree() {
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store);
        assert_eq!(client.schema_introspect().await.unwrap(), browse_schema());
        assert_eq!(client.schema_graph().await.unwrap(), browse_graph());
    }

    #[test]
    fn browse_page_reads_an_already_open_store() {
        // The whole reason this function exists: redb takes an exclusive file lock,
        // so the running app's store cannot be opened a second time through
        // `DbClient::open`. The tab has to browse the handle it already holds.
        let (_dir, store) = seeded_store();
        let page = browse_page(&store, "connections", None, 10).unwrap();
        assert_eq!(page.columns.len(), 6);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].values[0], "conn1");
    }

    #[test]
    fn browse_page_reports_the_end_of_a_table_rather_than_offering_an_empty_page() {
        // `settings` is a singleton, so a page size of 1 fills the page exactly. The
        // cursor must still say "no more" — an offered next page that came back empty
        // is the paging bug this pins.
        let (_dir, store) = seeded_store();
        let page = browse_page(&store, "settings", None, 1).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert!(
            page.next_cursor.is_none(),
            "a full-but-final page must not offer another"
        );
    }

    #[test]
    fn browse_page_paginates_from_a_cursor() {
        let (_dir, store) = seeded_store();
        for alias in ["box2", "box3"] {
            store
                .upsert_host(&Host {
                    alias: alias.into(),
                    user: "u".into(),
                    host: "h".into(),
                    port: 22,
                    secret_ref: None,
                    auth: AuthMethod::Agent,
                    folder: None,
                })
                .unwrap();
        }
        let first = browse_page(&store, "hosts", None, 2).unwrap();
        assert_eq!(first.rows.len(), 2);
        let cursor = first.next_cursor.expect("more hosts remain");
        let second = browse_page(&store, "hosts", Some(cursor), 2).unwrap();
        assert_eq!(second.rows.len(), 1);
        assert!(second.next_cursor.is_none());
        // No row appears twice across the two pages.
        let mut seen: Vec<&str> = first
            .rows
            .iter()
            .chain(second.rows.iter())
            .map(|r| r.values[0].as_str())
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, vec!["box1", "box2", "box3"]);
    }

    #[test]
    fn browse_page_rejects_anything_that_is_not_one_of_the_fixed_tables() {
        // There is no SQL parser behind this engine — the selector is a table name and
        // nothing else, which is also what makes it impossible to smuggle a mutation
        // through it. A SQL-shaped string is just an unknown table name.
        let (_dir, store) = seeded_store();
        assert!(browse_page(&store, "sqlite_master", None, 10).is_err());
        assert!(browse_page(&store, "hosts; DROP TABLE hosts", None, 10).is_err());
        assert!(browse_page(&store, "SELECT * FROM hosts", None, 10).is_err());
        assert!(browse_page(&store, "", None, 10).is_err());
    }

    #[test]
    fn every_browse_table_is_readable_from_a_fresh_store() {
        // The tab lists all five and lets the user click any of them; none may be a
        // dead link, including the ones the seed leaves empty.
        let (_dir, store) = seeded_store();
        for name in TABLE_NAMES {
            let page =
                browse_page(&store, name, None, 100).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                page.columns,
                table_columns(name),
                "{name}: page columns disagree with the schema"
            );
        }
    }

    #[tokio::test]
    async fn browse_page_and_query_paged_return_the_same_page() {
        // `query_paged` delegates to `browse_page`, so there is exactly one
        // implementation of what a store table looks like. This is what pins that.
        let (_dir, store) = seeded_store();
        let client = RedbBrowseClient::wrap(store.clone());
        for name in TABLE_NAMES {
            let direct = browse_page(&store, name, None, 100).unwrap();
            let via_trait = client.query_paged(name, None, 100).await.unwrap();
            assert_eq!(direct.columns, via_trait.columns, "{name}");
            assert_eq!(direct.rows, via_trait.rows, "{name}");
            assert_eq!(direct.next_cursor, via_trait.next_cursor, "{name}");
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
