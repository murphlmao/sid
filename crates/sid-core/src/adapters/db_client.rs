//! DbClient — filled out in Plan 4 (Database tab).

/// Trait for database connections. Concrete impl lives in `sid-db`.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::DbClient;
///
/// struct NoopDb;
/// impl DbClient for NoopDb {}
///
/// fn accepts_db(_d: &dyn DbClient) {}
/// accepts_db(&NoopDb);
/// ```
pub trait DbClient: Send + Sync {}
