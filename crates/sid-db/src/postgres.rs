//! PostgresClient — tokio-postgres-backed `DbClient` impl.

use std::collections::BTreeMap;
use std::sync::Arc;

use sid_core::db::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, ForeignKey, OpenParams, PageCursor,
    QueryPage, Row, SchemaGraph, SchemaInfo, TableInfo,
};
use tokio::sync::Mutex;

/// Factory + per-connection client. `factory()` returns a stateless factory.
///
/// # Examples
///
/// ```
/// use sid_db::PostgresClient;
/// let _factory = PostgresClient::factory();
/// ```
pub struct PostgresClient {
    inner: Option<Arc<Mutex<PostgresInner>>>,
    /// Cancel side-channel, deliberately OUTSIDE `inner`'s mutex (BUG 2 fix).
    /// `query_paged`/`execute` hold that mutex for the whole duration of a
    /// live query, so a `cancel()` that also needed the lock could never run
    /// until the query it's meant to interrupt already finished. A
    /// `CancelToken` dials its own separate connection to send the
    /// cancel-request frame — it needs no access to the primary client at
    /// all, so it doesn't need the mutex either.
    cancel: Option<PgCancel>,
}

struct PostgresInner {
    client: tokio_postgres::Client,
    /// Handle for the spawned connection task. Aborted on drop.
    conn_task: tokio::task::JoinHandle<()>,
}

/// Everything `cancel()` needs, cloned/captured at `open()` time.
struct PgCancel {
    /// Used to send the cancel-request frame on a side channel.
    token: tokio_postgres::CancelToken,
    /// Which transport the connection was opened with — `cancel` must reuse
    /// the same choice (a cancel request over `NoTls` to a TLS-only server,
    /// or vice versa, fails).
    tls: PgTls,
}

/// The transport a [`PostgresInner`] was actually opened with. Carries the
/// live rustls connector (rather than recomputing one) so `cancel` reuses the
/// exact trust store the connection was established with.
enum PgTls {
    Plain,
    Tls(tokio_postgres_rustls::MakeRustlsConnect),
}

/// Pure TLS/plaintext decision. No I/O, no globals — a straight function of
/// the parsed connection config, so it's unit-testable without a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TlsChoice {
    /// Cleartext connection. Only for a locally-trusted target.
    Plain,
    /// rustls-encrypted connection, platform trust store, full certificate +
    /// hostname verification (i.e. libpq's `verify-full`).
    Tls,
}

/// Decide `Plain` vs `Tls` for a parsed `tokio_postgres::Config`.
///
/// Policy (fails closed — remote/ambiguous → `Tls`):
/// - every host is `localhost` / `127.0.0.1` / `::1` / a Unix socket (a locally-
///   trusted transport): honor `sslmode` — `disable`/`prefer` → `Plain`,
///   `require`/`verify-*` → `Tls`.
/// - any remote TCP host (or an empty host list) → `Tls`, unconditionally.
///   `sslmode=disable` does NOT downgrade a remote connection (credential exposure).
pub(crate) fn tls_choice(config: &tokio_postgres::Config) -> TlsChoice {
    use tokio_postgres::config::SslMode;
    let hosts = config.get_hosts();
    // Empty host list → treat as non-local (fail safe: never assume plaintext).
    let all_local = !hosts.is_empty() && hosts.iter().all(is_local_host);
    if all_local {
        // Loopback / unix-socket: plaintext is safe (no network to eavesdrop).
        // Honor an explicit TLS request (`require`/`verify-*`); otherwise plaintext.
        match config.get_ssl_mode() {
            SslMode::Disable | SslMode::Prefer => TlsChoice::Plain,
            _ => TlsChoice::Tls,
        }
    } else {
        // Remote host (or unknown): ALWAYS TLS. `sslmode=disable` must NOT downgrade
        // an off-machine connection — that would send credentials in cleartext, and a
        // committed workspace `.sid/config.toml` (which travels with a cloned repo)
        // could weaponize it. Fails closed.
        TlsChoice::Tls
    }
}

fn is_local_host(host: &tokio_postgres::config::Host) -> bool {
    match host {
        tokio_postgres::config::Host::Tcp(h) => {
            matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1")
        }
        #[cfg(unix)]
        tokio_postgres::config::Host::Unix(_) => true,
    }
}

/// Install rustls's `ring` crypto provider as the process default, if none is
/// installed yet. Idempotent: `install_default` errors if a provider is
/// already present (e.g. another `sid-db` client beat us to it, or a future
/// non-Postgres TLS consumer installed one first) — that's fine, we only
/// need *a* provider in place, not necessarily ours.
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Build a rustls connector trusting the platform's native certificate store
/// (verify-full: full chain + hostname validation, rustls's only mode).
///
/// // ponytail: verify-full only; self-signed/cert-pinning + explicit
/// // sslmode=require (encrypt-no-verify) deferred — a remote self-signed
/// // cert currently fails closed, which is the safe default. Users with a
/// // trusted self-signed setup use sslmode=disable on a trusted network for
/// // now.
fn build_rustls_connector() -> tokio_postgres_rustls::MakeRustlsConnect {
    ensure_crypto_provider();
    let mut roots = rustls::RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    for cert in loaded.certs {
        // Best-effort: a handful of unparsable platform certs shouldn't
        // block the rest of the trust store from loading.
        let _ = roots.add(cert);
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_postgres_rustls::MakeRustlsConnect::new(config)
}

impl Drop for PostgresInner {
    fn drop(&mut self) {
        self.conn_task.abort();
    }
}

impl PostgresClient {
    /// Construct a stateless factory. Call [`DbClient::open`] to bind a real
    /// connection.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_db::PostgresClient;
    /// let factory = PostgresClient::factory();
    /// assert_eq!(
    ///     sid_core::db::DbClient::kind(&*factory),
    ///     sid_core::db::DbKind::Postgres,
    /// );
    /// ```
    pub fn factory() -> Arc<dyn DbClient> {
        Arc::new(Self {
            inner: None,
            cancel: None,
        })
    }
}

#[async_trait::async_trait]
impl DbClient for PostgresClient {
    async fn open(&self, p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> {
        if p.kind != DbKind::Postgres {
            return Err(DbError::Invalid(format!(
                "expected DbKind::Postgres, got {:?}",
                p.kind
            )));
        }
        // Parse the DSN with tokio-postgres's OWN parser (the single source of truth), then
        // set the password STRUCTURALLY via `Config::password`. This avoids a parser
        // differential: a hand-rolled DSN splicer only understood the URL form and could
        // misplace the plaintext password (into a query param / wrong `@`) on key-value DSNs.
        let mut config: tokio_postgres::Config = p
            .dsn
            .parse()
            .map_err(|e: tokio_postgres::Error| DbError::Connect(e.to_string()))?;
        if let Some(pw) = p.password.as_ref() {
            config.password(pw);
        }
        // TLS (DU-TLS, docs/superpowers/plans/2026-07-01-db-slice.md): the transport is
        // chosen by `tls_choice` from the parsed config's host + `sslmode`. A REMOTE host is
        // ALWAYS TLS (verify-full via the platform trust store) — `sslmode=disable` cannot
        // downgrade an off-machine connection (credential exposure; committed workspace
        // configs travel with repos). `NoTls` only for loopback/unix-socket targets. See
        // `build_rustls_connector`'s ponytail note for the one deferred case (self-signed).
        let (client, conn_task, cancel_token, tls) = match tls_choice(&config) {
            TlsChoice::Plain => {
                let (client, connection) = config
                    .connect(tokio_postgres::NoTls)
                    .await
                    .map_err(|e| DbError::Connect(e.to_string()))?;
                let cancel_token = client.cancel_token();
                let conn_task = tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        // Best-effort diagnostic only; the connection task ending is
                        // not itself actionable by the caller (the client Arc has
                        // already been handed back).
                        eprintln!("postgres connection task ended with error: {e}");
                    }
                });
                (client, conn_task, cancel_token, PgTls::Plain)
            }
            TlsChoice::Tls => {
                let connector = build_rustls_connector();
                let (client, connection) = config
                    .connect(connector.clone())
                    .await
                    .map_err(|e| DbError::Connect(e.to_string()))?;
                let cancel_token = client.cancel_token();
                let conn_task = tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("postgres connection task ended with error: {e}");
                    }
                });
                (client, conn_task, cancel_token, PgTls::Tls(connector))
            }
        };
        Ok(Arc::new(PostgresClient {
            inner: Some(Arc::new(Mutex::new(PostgresInner { client, conn_task }))),
            cancel: Some(PgCancel {
                token: cancel_token,
                tls,
            }),
        }))
    }

    async fn close(&self) -> Result<(), DbError> {
        // Dropping the Arc<Mutex<PostgresInner>> aborts the connection task
        // via Drop. Returning Ok here is enough — explicit close is advisory.
        Ok(())
    }

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

    async fn query_paged(
        &self,
        sql: &str,
        cursor: Option<PageCursor>,
        page_size: u32,
    ) -> Result<QueryPage, DbError> {
        let inner = self.inner.as_ref().ok_or(DbError::NotConnected)?.clone();
        let offset = cursor.map(|c| c.offset).unwrap_or(0);
        let page_size = page_size.max(1) as u64;
        // NOTE (single-SELECT limitation): wrapping in a subquery only works
        // for a single SELECT statement. Multi-statement input (e.g. a
        // trailing second statement after `;`) or non-SELECT input will fail
        // or behave oddly — the query editor is expected to send one SELECT
        // at a time; this is not a general-purpose SQL splitter.
        let trimmed = sql.trim().trim_end_matches(';');
        let wrapped =
            format!("SELECT * FROM ( {trimmed} ) AS sid_sub LIMIT {page_size} OFFSET {offset}");
        let start = std::time::Instant::now();
        let guard = inner.lock().await;
        let rows = guard
            .client
            .query(&wrapped, &[])
            .await
            .map_err(map_pg_error)?;
        let columns: Vec<Column> = if let Some(r) = rows.first() {
            r.columns()
                .iter()
                .map(|c| Column {
                    name: c.name().to_string(),
                    ty: pg_type_to_column_type(c.type_()),
                })
                .collect()
        } else {
            let stmt = guard.client.prepare(&wrapped).await.map_err(map_pg_error)?;
            stmt.columns()
                .iter()
                .map(|c| Column {
                    name: c.name().to_string(),
                    ty: pg_type_to_column_type(c.type_()),
                })
                .collect()
        };
        let mut rows_out: Vec<Row> = Vec::with_capacity(rows.len());
        for r in &rows {
            let values: Vec<String> = (0..r.columns().len())
                .map(|i| render_pg_value(r, i))
                .collect();
            rows_out.push(Row { values });
        }
        let fetched = rows_out.len() as u64;
        let next_cursor = if fetched < page_size {
            None
        } else {
            Some(PageCursor {
                offset: offset + fetched,
            })
        };
        Ok(QueryPage {
            columns,
            rows: rows_out,
            next_cursor,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

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
        let mut tables: std::collections::BTreeMap<(String, String), Vec<Column>> =
            Default::default();
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
            tables
                .entry((schema, name))
                .or_default()
                .push(Column { name: col, ty: ct });
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

    async fn schema_graph(&self) -> Result<SchemaGraph, DbError> {
        let inner = self.inner.as_ref().ok_or(DbError::NotConnected)?.clone();
        let guard = inner.lock().await;

        // One row per (constraint, column-position): `unnest(conkey, confkey)
        // WITH ORDINALITY` walks the referencing/referenced key arrays in
        // lockstep so a composite FK's columns keep their declared order
        // (the `ord` column is that position). Namespaces are filtered the
        // same way `schema_introspect` filters them.
        let fk_sql = "
            SELECT
                ns.nspname, cl.relname,
                refns.nspname, refcl.relname,
                con.conname, u.ord,
                att.attname, refatt.attname
            FROM pg_constraint con
            JOIN pg_class cl ON cl.oid = con.conrelid
            JOIN pg_namespace ns ON ns.oid = cl.relnamespace
            JOIN pg_class refcl ON refcl.oid = con.confrelid
            JOIN pg_namespace refns ON refns.oid = refcl.relnamespace
            JOIN LATERAL unnest(con.conkey, con.confkey)
                WITH ORDINALITY AS u(conattnum, confattnum, ord) ON true
            JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = u.conattnum
            JOIN pg_attribute refatt
                ON refatt.attrelid = con.confrelid AND refatt.attnum = u.confattnum
            WHERE con.contype = 'f'
              AND ns.nspname NOT IN ('pg_catalog', 'information_schema')
            ORDER BY ns.nspname, cl.relname, con.conname, u.ord
        ";
        let fk_rows = guard
            .client
            .query(fk_sql, &[])
            .await
            .map_err(map_pg_error)?;
        let fk_input: Vec<PgFkRow> = fk_rows
            .iter()
            .map(|r| PgFkRow {
                schema: r.get(0),
                table: r.get(1),
                ref_schema: r.get(2),
                ref_table: r.get(3),
                conname: r.get(4),
                ordinal: r.get(5),
                from_col: r.get(6),
                to_col: r.get(7),
            })
            .collect();

        // Primary keys: same ordinality trick over `conkey` alone.
        let pk_sql = "
            SELECT ns.nspname, cl.relname, u.ord, att.attname
            FROM pg_constraint con
            JOIN pg_class cl ON cl.oid = con.conrelid
            JOIN pg_namespace ns ON ns.oid = cl.relnamespace
            JOIN LATERAL unnest(con.conkey) WITH ORDINALITY AS u(conattnum, ord) ON true
            JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = u.conattnum
            WHERE con.contype = 'p'
              AND ns.nspname NOT IN ('pg_catalog', 'information_schema')
            ORDER BY ns.nspname, cl.relname, u.ord
        ";
        let pk_rows = guard
            .client
            .query(pk_sql, &[])
            .await
            .map_err(map_pg_error)?;
        let pk_input: Vec<PgPkRow> = pk_rows
            .iter()
            .map(|r| PgPkRow {
                schema: r.get(0),
                table: r.get(1),
                ordinal: r.get(2),
                column: r.get(3),
            })
            .collect();

        Ok(SchemaGraph {
            foreign_keys: assemble_foreign_keys(fk_input),
            primary_keys: assemble_primary_keys(pk_input),
        })
    }

    async fn cancel(&self) -> Result<(), DbError> {
        // Deliberately does NOT touch `self.inner`'s mutex — see `PostgresClient::cancel`'s
        // doc comment (BUG 2). `CancelToken::cancel_query` dials its own side-channel
        // connection, so it can run concurrently with a live query still holding that lock.
        let cancel = self.cancel.as_ref().ok_or(DbError::NotConnected)?;
        match &cancel.tls {
            PgTls::Plain => cancel.token.cancel_query(tokio_postgres::NoTls).await,
            PgTls::Tls(connector) => cancel.token.cancel_query(connector.clone()).await,
        }
        .map_err(|e| DbError::Other(e.to_string()))
    }

    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }
}

/// One (table, referenced-table, column-position) row from the live FK query
/// (`u.ord` = the `WITH ORDINALITY` position). Factored out to a plain struct
/// so [`assemble_foreign_keys`] — the row→[`SchemaGraph`] assembly — is
/// unit-testable without a live Postgres connection.
#[derive(Debug, Clone)]
struct PgFkRow {
    schema: String,
    table: String,
    ref_schema: String,
    ref_table: String,
    conname: String,
    ordinal: i64,
    from_col: String,
    to_col: String,
}

/// One (table, column-position) row from the live PK query.
#[derive(Debug, Clone)]
struct PgPkRow {
    schema: String,
    table: String,
    ordinal: i64,
    column: String,
}

/// Qualify a table name the same way [`TableInfo`] / `schema_introspect`
/// displays Postgres tables: `"schema.name"`.
fn pg_qualify(schema: &str, table: &str) -> String {
    format!("{schema}.{table}")
}

/// `(from_schema, from_table, to_schema, to_table, constraint_name)` — the
/// grouping key for one FK constraint's columns.
type PgFkKey = (String, String, String, String, String);
/// `(ordinal, from_column, to_column)` — one FK constraint's per-column data.
type PgFkCols = Vec<(i64, String, String)>;

/// Pure assembly: group flat per-column FK rows by constraint, order each
/// group's columns by `ordinal`, then sort per the [`SchemaGraph`] contract —
/// `(from_table, to_table, from_columns)`.
fn assemble_foreign_keys(rows: Vec<PgFkRow>) -> Vec<ForeignKey> {
    let mut groups: BTreeMap<PgFkKey, PgFkCols> = BTreeMap::new();
    for r in rows {
        groups
            .entry((r.schema, r.table, r.ref_schema, r.ref_table, r.conname))
            .or_default()
            .push((r.ordinal, r.from_col, r.to_col));
    }
    let mut fks: Vec<ForeignKey> = groups
        .into_iter()
        .map(
            |((schema, table, ref_schema, ref_table, _conname), mut cols)| {
                cols.sort_by_key(|c| c.0);
                ForeignKey {
                    from_table: pg_qualify(&schema, &table),
                    from_columns: cols.iter().map(|c| c.1.clone()).collect(),
                    to_table: pg_qualify(&ref_schema, &ref_table),
                    to_columns: cols.into_iter().map(|c| c.2).collect(),
                }
            },
        )
        .collect();
    fks.sort_by(|a, b| {
        (&a.from_table, &a.to_table, &a.from_columns).cmp(&(
            &b.from_table,
            &b.to_table,
            &b.from_columns,
        ))
    });
    fks
}

/// Pure assembly for primary keys, same ordinality-preserving shape as
/// [`assemble_foreign_keys`].
fn assemble_primary_keys(rows: Vec<PgPkRow>) -> BTreeMap<String, Vec<String>> {
    let mut groups: BTreeMap<(String, String), Vec<(i64, String)>> = BTreeMap::new();
    for r in rows {
        groups
            .entry((r.schema, r.table))
            .or_default()
            .push((r.ordinal, r.column));
    }
    groups
        .into_iter()
        .map(|((schema, table), mut cols)| {
            cols.sort_by_key(|c| c.0);
            (
                pg_qualify(&schema, &table),
                cols.into_iter().map(|c| c.1).collect(),
            )
        })
        .collect()
}

pub(crate) fn map_pg_error(e: tokio_postgres::Error) -> DbError {
    if let Some(db_err) = e.as_db_error() {
        if db_err.code().code().starts_with("42") {
            return DbError::Syntax {
                offset: db_err
                    .position()
                    .map(|p| match p {
                        tokio_postgres::error::ErrorPosition::Original(n) => *n as usize,
                        tokio_postgres::error::ErrorPosition::Internal { position, .. } => {
                            *position as usize
                        }
                    })
                    .unwrap_or(0),
                message: db_err.message().to_string(),
            };
        }
        return DbError::Query(db_err.message().to_string());
    }
    DbError::Query(e.to_string())
}

/// Decode column `idx` as `Option<T>`, but keep NULL and decode-failure
/// distinct instead of collapsing both to `None` (the bug: `.ok().flatten()`
/// used to make a present-but-undecodable value indistinguishable from a real
/// SQL NULL). `Ok(Some(v))` = present + decoded; `Ok(None)` = a genuine SQL
/// NULL; `Err(())` = present but this Rust type couldn't decode it.
fn try_decode<'r, T>(row: &'r tokio_postgres::Row, idx: usize) -> Result<Option<T>, ()>
where
    T: tokio_postgres::types::FromSql<'r>,
{
    row.try_get::<_, Option<T>>(idx).map_err(|_| ())
}

/// Render one column via [`try_decode`], applying `fmt` to a present value.
/// The `Err` arm is the load-bearing fix for BUG 1: a present-but-undecodable
/// value renders as a distinct `⟨type?⟩` marker — **never** "NULL" — so it's
/// never confused with a genuine SQL NULL (which still renders "NULL"; see
/// `Row::values`'s doc comment on the lack of an `Option<String>` sentinel).
fn render<'r, T>(row: &'r tokio_postgres::Row, idx: usize, fmt: impl FnOnce(T) -> String) -> String
where
    T: tokio_postgres::types::FromSql<'r>,
{
    match try_decode::<T>(row, idx) {
        Ok(Some(v)) => fmt(v),
        Ok(None) => "NULL".to_string(),
        Err(()) => format!("⟨{}?⟩", row.columns()[idx].type_().name()),
    }
}

/// Render a Postgres array column as `{a,b,NULL,c}` — `Vec<Option<T>>` so an
/// individual NULL *element* (a normal, expected thing inside an array) still
/// prints "NULL" inline without that being confused with the top-level
/// NULL-vs-undecodable distinction `render` enforces for the column as a whole.
fn render_array<'r, T>(row: &'r tokio_postgres::Row, idx: usize) -> String
where
    T: tokio_postgres::types::FromSql<'r> + ToString,
{
    render::<Vec<Option<T>>>(row, idx, |items| {
        let inner = items
            .into_iter()
            .map(|item| {
                item.map(|v| v.to_string())
                    .unwrap_or_else(|| "NULL".to_string())
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{{{inner}}}")
    })
}

pub(crate) fn render_pg_value(row: &tokio_postgres::Row, idx: usize) -> String {
    use tokio_postgres::types::Type;
    let col = &row.columns()[idx];
    match *col.type_() {
        Type::BOOL => render::<bool>(row, idx, |v| v.to_string()),
        Type::INT2 => render::<i16>(row, idx, |v| v.to_string()),
        Type::INT4 => render::<i32>(row, idx, |v| v.to_string()),
        Type::INT8 => render::<i64>(row, idx, |v| v.to_string()),
        Type::FLOAT4 => render::<f32>(row, idx, |v| v.to_string()),
        Type::FLOAT8 => render::<f64>(row, idx, |v| v.to_string()),
        // Not `.map(|v| v.to_string())` — that would be a no-op allocation/copy
        // on a value that's already an owned `String` (perf audit finding #3).
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => render::<String>(row, idx, |v| v),
        Type::BYTEA => render::<Vec<u8>>(row, idx, |b| {
            let mut s = String::with_capacity(2 + b.len() * 2);
            s.push_str("0x");
            for byte in &b {
                use std::fmt::Write;
                write!(&mut s, "{byte:02x}").ok();
            }
            s
        }),
        Type::UUID => render::<uuid::Uuid>(row, idx, |v| v.to_string()),
        Type::TIMESTAMPTZ => render::<chrono::DateTime<chrono::Utc>>(row, idx, |v| v.to_string()),
        Type::TIMESTAMP => render::<chrono::NaiveDateTime>(row, idx, |v| v.to_string()),
        Type::DATE => render::<chrono::NaiveDate>(row, idx, |v| v.to_string()),
        Type::TIME => render::<chrono::NaiveTime>(row, idx, |v| v.to_string()),
        Type::JSON | Type::JSONB => render::<serde_json::Value>(row, idx, |v| v.to_string()),
        Type::NUMERIC => render::<rust_decimal::Decimal>(row, idx, |v| v.to_string()),
        Type::TEXT_ARRAY | Type::VARCHAR_ARRAY | Type::BPCHAR_ARRAY | Type::NAME_ARRAY => {
            render_array::<String>(row, idx)
        }
        Type::INT2_ARRAY => render_array::<i16>(row, idx),
        Type::INT4_ARRAY => render_array::<i32>(row, idx),
        Type::INT8_ARRAY => render_array::<i64>(row, idx),
        Type::BOOL_ARRAY => render_array::<bool>(row, idx),
        Type::UUID_ARRAY => render_array::<uuid::Uuid>(row, idx),
        // Fallback for every other type (custom enums/domains, extension types
        // like `citext`, geometric types, etc.): still tri-state via `render`,
        // so an undecodable-as-String value gets the `⟨type?⟩` marker instead
        // of silently becoming "NULL" — this was BUG 1's exact failure mode.
        _ => render::<String>(row, idx, |v| v),
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_choice_remote_disable_is_still_tls() {
        // SECURITY: `sslmode=disable` must NOT downgrade a REMOTE connection to
        // plaintext — that would expose credentials. Remote always → Tls.
        let config: tokio_postgres::Config =
            "host=example.com sslmode=disable".parse().expect("parse");
        assert_eq!(tls_choice(&config), TlsChoice::Tls);
    }

    #[test]
    fn tls_choice_local_disable_is_plain() {
        // Loopback with explicit disable → plaintext is fine (no network).
        let config: tokio_postgres::Config =
            "host=localhost sslmode=disable".parse().expect("parse");
        assert_eq!(tls_choice(&config), TlsChoice::Plain);
    }

    #[test]
    fn tls_choice_local_require_is_tls() {
        // Loopback but explicit TLS request → honor it.
        let config: tokio_postgres::Config =
            "host=127.0.0.1 sslmode=require".parse().expect("parse");
        assert_eq!(tls_choice(&config), TlsChoice::Tls);
    }

    #[test]
    fn tls_choice_local_targets_are_plain() {
        for dsn in [
            "host=localhost",
            "host=127.0.0.1",
            "host=::1",
            "host=/var/run/postgresql",
        ] {
            let config: tokio_postgres::Config = dsn.parse().expect("parse");
            assert_eq!(tls_choice(&config), TlsChoice::Plain, "dsn: {dsn}");
        }
    }

    #[test]
    fn tls_choice_remote_default_is_tls() {
        // No `sslmode` given — tokio-postgres defaults to `Prefer`, which
        // this policy still upgrades to `Tls` for a non-local host.
        let config: tokio_postgres::Config = "host=example.com".parse().expect("parse");
        assert_eq!(tls_choice(&config), TlsChoice::Tls);
    }

    #[test]
    fn tls_choice_remote_require_is_tls() {
        let config: tokio_postgres::Config =
            "host=example.com sslmode=require".parse().expect("parse");
        assert_eq!(tls_choice(&config), TlsChoice::Tls);
    }

    /// Live-network path; `#[ignore]`d because it requires a reachable
    /// Postgres server. Run explicitly with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn open_and_query_against_real_postgres() {
        let factory = PostgresClient::factory();
        let client = factory
            .open(OpenParams {
                kind: DbKind::Postgres,
                dsn: "postgres://postgres@localhost:5432/postgres".into(),
                password: None,
                sqlite_mode: None,
            })
            .await
            .expect("connect");
        let page = client
            .query_paged("SELECT 1 AS one", None, 10)
            .await
            .expect("query");
        assert_eq!(page.rows.len(), 1);
    }

    /// Live-network path; `#[ignore]`d for the same reason as
    /// `open_and_query_against_real_postgres`.
    #[tokio::test]
    #[ignore]
    async fn schema_graph_against_real_postgres() {
        let factory = PostgresClient::factory();
        let client = factory
            .open(OpenParams {
                kind: DbKind::Postgres,
                dsn: "postgres://postgres@localhost:5432/postgres".into(),
                password: None,
                sqlite_mode: None,
            })
            .await
            .expect("connect");
        // Nothing asserted about content — this only proves the live query
        // parses/executes against a real server; row→SchemaGraph assembly is
        // covered by the fabricated-row unit tests below.
        client.schema_graph().await.expect("schema_graph");
    }

    fn fk_row(
        table: &str,
        ref_table: &str,
        conname: &str,
        ordinal: i64,
        from_col: &str,
        to_col: &str,
    ) -> PgFkRow {
        PgFkRow {
            schema: "public".into(),
            table: table.into(),
            ref_schema: "public".into(),
            ref_table: ref_table.into(),
            conname: conname.into(),
            ordinal,
            from_col: from_col.into(),
            to_col: to_col.into(),
        }
    }

    #[test]
    fn assemble_foreign_keys_orders_composite_columns_by_ordinal_and_sorts_edges() {
        let rows = vec![
            // Single-column FK: orders.customer_id -> customers.id.
            fk_row(
                "orders",
                "customers",
                "orders_customer_fk",
                1,
                "customer_id",
                "id",
            ),
            // Composite FK fed out of order to prove sort-by-ordinal, not row order.
            fk_row("orders", "bins", "orders_bin_fk", 2, "bin_id", "bin_id"),
            fk_row(
                "orders",
                "bins",
                "orders_bin_fk",
                1,
                "warehouse_id",
                "warehouse_id",
            ),
        ];
        let fks = assemble_foreign_keys(rows);
        assert_eq!(fks.len(), 2);
        // Deterministic order is (from_table, to_table, from_columns); both share
        // from_table "public.orders", so "public.bins" < "public.customers" sorts first.
        assert_eq!(fks[0].from_table, "public.orders");
        assert_eq!(fks[0].to_table, "public.bins");
        assert_eq!(fks[0].from_columns, vec!["warehouse_id", "bin_id"]);
        assert_eq!(fks[0].to_columns, vec!["warehouse_id", "bin_id"]);
        assert_eq!(fks[1].from_table, "public.orders");
        assert_eq!(fks[1].to_table, "public.customers");
        assert_eq!(fks[1].from_columns, vec!["customer_id"]);
        assert_eq!(fks[1].to_columns, vec!["id"]);
    }

    #[test]
    fn assemble_primary_keys_orders_composite_columns_by_ordinal() {
        let rows = vec![
            PgPkRow {
                schema: "public".into(),
                table: "bins".into(),
                ordinal: 2,
                column: "bin_id".into(),
            },
            PgPkRow {
                schema: "public".into(),
                table: "bins".into(),
                ordinal: 1,
                column: "warehouse_id".into(),
            },
            PgPkRow {
                schema: "public".into(),
                table: "customers".into(),
                ordinal: 1,
                column: "id".into(),
            },
        ];
        let pks = assemble_primary_keys(rows);
        assert_eq!(
            pks.get("public.bins"),
            Some(&vec!["warehouse_id".to_string(), "bin_id".to_string()])
        );
        assert_eq!(pks.get("public.customers"), Some(&vec!["id".to_string()]));
    }

    #[test]
    fn assemble_foreign_keys_of_empty_input_is_empty() {
        assert!(assemble_foreign_keys(Vec::new()).is_empty());
    }

    #[test]
    fn assemble_primary_keys_of_empty_input_is_empty() {
        assert!(assemble_primary_keys(Vec::new()).is_empty());
    }

    #[test]
    fn assemble_foreign_keys_two_edges_between_the_same_pair_of_tables_both_survive() {
        // Two independent FK constraints from the same table to the same
        // referenced table, over different columns — the grouping key includes
        // `conname`, so these must NOT collapse into one edge.
        let rows = vec![
            fk_row(
                "shipments",
                "warehouses",
                "shipments_origin_fk",
                1,
                "origin_id",
                "id",
            ),
            fk_row(
                "shipments",
                "warehouses",
                "shipments_dest_fk",
                1,
                "destination_id",
                "id",
            ),
        ];
        let fks = assemble_foreign_keys(rows);
        assert_eq!(fks.len(), 2, "distinct constraints must not be merged");
        let from_cols: std::collections::BTreeSet<_> =
            fks.iter().map(|fk| fk.from_columns[0].clone()).collect();
        assert_eq!(
            from_cols,
            std::collections::BTreeSet::from([
                "origin_id".to_string(),
                "destination_id".to_string()
            ])
        );
    }
}
