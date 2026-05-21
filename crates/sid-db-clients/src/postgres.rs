//! PostgresClient — tokio-postgres-backed `DbClient` impl.

use std::sync::Arc;

use sid_core::adapters::db_client::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor, QueryPage,
    Row, SchemaInfo, TableInfo,
};
use tokio::sync::Mutex;

/// Factory + per-connection client. `factory()` returns a stateless factory.
///
/// # Examples
///
/// ```
/// use sid_db_clients::PostgresClient;
/// let _factory = PostgresClient::factory();
/// ```
pub struct PostgresClient {
    #[allow(dead_code)]
    inner: Option<Arc<Mutex<PostgresInner>>>,
}

#[allow(dead_code)]
struct PostgresInner {
    client: tokio_postgres::Client,
    /// Handle for the spawned connection task. Aborted on drop.
    conn_task: tokio::task::JoinHandle<()>,
    /// Used by `cancel` to send the cancel-request frame on a side channel.
    cancel_token: tokio_postgres::CancelToken,
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
    /// use sid_db_clients::PostgresClient;
    /// let factory = PostgresClient::factory();
    /// assert_eq!(
    ///     sid_core::adapters::db_client::DbClient::kind(&*factory),
    ///     sid_core::adapters::db_client::DbKind::Postgres,
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
        let (client, connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls)
            .await
            .map_err(|e| DbError::Connect(e.to_string()))?;
        let cancel_token = client.cancel_token();
        let conn_task = tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::warn!("postgres connection task ended with error: {e}");
            }
        });
        Ok(Arc::new(PostgresClient {
            inner: Some(Arc::new(Mutex::new(PostgresInner {
                client,
                conn_task,
                cancel_token,
            }))),
        }))
    }

    async fn close(&self) -> Result<(), DbError> {
        // Dropping the Arc<Mutex<PostgresInner>> aborts the connection task
        // via Drop. Returning Ok here is enough — explicit close is advisory.
        Ok(())
    }

    async fn execute(&self, _sql: &str) -> Result<ExecResult, DbError> {
        Err(DbError::Other(
            "execute: not yet implemented — Task 10".into(),
        ))
    }

    async fn query_paged(
        &self,
        _sql: &str,
        _cursor: Option<PageCursor>,
        _page_size: u32,
    ) -> Result<QueryPage, DbError> {
        Err(DbError::Other(
            "query_paged: not yet implemented — Task 11".into(),
        ))
    }

    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        Err(DbError::Other(
            "schema_introspect: not yet implemented — Task 12".into(),
        ))
    }

    async fn cancel(&self) -> Result<(), DbError> {
        Err(DbError::Other("cancel: not yet implemented — Task 13".into()))
    }

    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }
}

/// If the DSN does not include a password, splice one in before the host.
/// Best-effort: handles `postgres://user@host/db` → `postgres://user:pw@host/db`.
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

#[allow(dead_code)]
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

#[allow(dead_code)]
pub(crate) fn render_pg_value(row: &tokio_postgres::Row, idx: usize) -> String {
    use tokio_postgres::types::Type;
    let col = &row.columns()[idx];
    macro_rules! try_get {
        ($t:ty) => {
            row.try_get::<_, Option<$t>>(idx).ok().flatten().map(|v| v.to_string())
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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn _unused_silencer(_: TableInfo, _: Row, _: Column) {}

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
}
