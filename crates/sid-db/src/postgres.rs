//! PostgresClient — tokio-postgres-backed `DbClient` impl.

use std::sync::Arc;

use sid_core::db::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor, QueryPage,
    Row, SchemaInfo, TableInfo,
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
}

#[allow(dead_code)]
struct PostgresInner {
    client: tokio_postgres::Client,
    /// Handle for the spawned connection task. Aborted on drop.
    conn_task: tokio::task::JoinHandle<()>,
    /// Used by `cancel` to send the cancel-request frame on a side channel.
    cancel_token: tokio_postgres::CancelToken,
    /// Which transport this connection was opened with — `cancel` must reuse
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
/// Policy (fails closed — ambiguous or remote defaults to `Tls`):
/// - `sslmode=disable` → `Plain` (explicit opt-out).
/// - every host is `localhost` / `127.0.0.1` / `::1` / a Unix socket → `Plain`
///   (local, trusted transport; no network to eavesdrop).
/// - otherwise (any remote TCP host, including the driver's own `prefer`
///   default and an explicit `require`) → `Tls`.
pub(crate) fn tls_choice(config: &tokio_postgres::Config) -> TlsChoice {
    if config.get_ssl_mode() == tokio_postgres::config::SslMode::Disable {
        return TlsChoice::Plain;
    }
    if config.get_hosts().iter().all(is_local_host) {
        TlsChoice::Plain
    } else {
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
        Arc::new(Self { inner: None })
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
        let dsn = if let Some(pw) = p.password.as_ref() {
            inject_password(&p.dsn, pw)
        } else {
            p.dsn.clone()
        };
        // TLS (DU-TLS, docs/superpowers/plans/2026-07-01-db-slice.md): the transport is
        // chosen by `tls_choice` from the parsed DSN's `sslmode` + host — `NoTls` only for
        // an explicit `sslmode=disable` or a localhost/unix-socket target; every remote host
        // gets a rustls connection with platform trust-store verify-full. See
        // `build_rustls_connector`'s ponytail note for the one deferred case (self-signed).
        let config: tokio_postgres::Config = dsn
            .parse()
            .map_err(|e: tokio_postgres::Error| DbError::Connect(e.to_string()))?;
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
            inner: Some(Arc::new(Mutex::new(PostgresInner {
                client,
                conn_task,
                cancel_token,
                tls,
            }))),
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

    async fn cancel(&self) -> Result<(), DbError> {
        let inner = self.inner.as_ref().ok_or(DbError::NotConnected)?.clone();
        let guard = inner.lock().await;
        match &guard.tls {
            PgTls::Plain => guard.cancel_token.cancel_query(tokio_postgres::NoTls).await,
            PgTls::Tls(connector) => guard.cancel_token.cancel_query(connector.clone()).await,
        }
        .map_err(|e| DbError::Other(e.to_string()))
    }

    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }
}

/// If the DSN does not include a password, splice one in before the host.
/// Best-effort: handles `postgres://user@host/db` → `postgres://user:pw@host/db`.
///
/// Passwords never live in the persisted `DbConnection.dsn` — this splice
/// happens only in-memory, at open time, from the secret resolved out of the
/// keyring.
fn inject_password(dsn: &str, pw: &str) -> String {
    if let Some(at_idx) = dsn.find('@') {
        let pre = &dsn[..at_idx];
        // pre is like "postgres://user" or "postgres://user:pw".
        // If there's a colon after the scheme `://`, treat it as user:pw already.
        if let Some(scheme_end) = pre.find("://") {
            let userinfo = &pre[scheme_end + 3..];
            if userinfo.contains(':') {
                return dsn.to_string();
            }
        }
        let encoded = url_encode_password(pw);
        return format!("{pre}:{encoded}{}", &dsn[at_idx..]);
    }
    dsn.to_string()
}

fn url_encode_password(pw: &str) -> String {
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

pub(crate) fn render_pg_value(row: &tokio_postgres::Row, idx: usize) -> String {
    use tokio_postgres::types::Type;
    let col = &row.columns()[idx];
    macro_rules! try_get {
        ($t:ty) => {
            row.try_get::<_, Option<$t>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
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
        Type::BYTEA => row
            .try_get::<_, Option<Vec<u8>>>(idx)
            .ok()
            .flatten()
            .map(|b| {
                let mut s = String::with_capacity(2 + b.len() * 2);
                s.push_str("0x");
                for byte in &b {
                    use std::fmt::Write;
                    write!(&mut s, "{byte:02x}").ok();
                }
                s
            }),
        _ => row.try_get::<_, Option<String>>(idx).ok().flatten(),
    };
    s.unwrap_or_else(|| "NULL".to_string())
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

    #[test]
    fn tls_choice_disable_is_plain() {
        let config: tokio_postgres::Config =
            "host=example.com sslmode=disable".parse().expect("parse");
        assert_eq!(tls_choice(&config), TlsChoice::Plain);
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
}
