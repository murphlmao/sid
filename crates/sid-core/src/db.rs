//! Database engine discriminator — the shared contract between the store
//! (`DbConnection.kind`), the adapter impls (`sid-db`), and the frontend registry.
//!
//! The label round-trip is the single source of truth widgets/registries use instead
//! of matching variants directly. New engines append a variant (postcard encodes by
//! position, so appending is migration-safe).

use serde::{Deserialize, Serialize};

/// Which database engine a saved connection targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DbKind {
    /// PostgreSQL (adapter uses `tokio-postgres`).
    #[default]
    Postgres,
    /// SQLite (adapter uses bundled `rusqlite`).
    Sqlite,
    /// sid's own redb store, browsed read-only (the POC's `ConfigReader` pseudo-engine).
    Redb,
}

impl DbKind {
    /// Stable lowercase label used in committed config, registries, and UI selectors.
    ///
    /// # Examples
    /// ```
    /// use sid_core::db::DbKind;
    /// assert_eq!(DbKind::Postgres.label(), "postgres");
    /// ```
    pub fn label(self) -> &'static str {
        match self {
            DbKind::Postgres => "postgres",
            DbKind::Sqlite => "sqlite",
            DbKind::Redb => "redb",
        }
    }

    /// Parse a [`label`](Self::label); `None` if unrecognized.
    ///
    /// # Examples
    /// ```
    /// use sid_core::db::DbKind;
    /// assert_eq!(DbKind::from_label("sqlite"), Some(DbKind::Sqlite));
    /// assert_eq!(DbKind::from_label("mysql"), None);
    /// ```
    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "postgres" => Some(DbKind::Postgres),
            "sqlite" => Some(DbKind::Sqlite),
            "redb" => Some(DbKind::Redb),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_round_trips_and_rejects_unknown() {
        for k in [DbKind::Postgres, DbKind::Sqlite, DbKind::Redb] {
            assert_eq!(DbKind::from_label(k.label()), Some(k));
        }
        assert_eq!(DbKind::from_label("mysql"), None);
    }
}
