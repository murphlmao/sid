//! `DbRegistry` — the binary's wiring layer mapping each [`DbKind`] to its
//! concrete `DbClient` factory + [`DbClientDescriptor`] (both implemented in
//! `sid-db`).
//!
//! This is the one file in `crates/sid` allowed to name `sid_db`'s concrete
//! types (`PostgresClient`/`SqliteClient`/`RedbBrowseClient`/
//! `PostgresDescriptor`/`SqliteDescriptor`) directly — everything downstream
//! (the connection form, the query editor) works through the
//! `sid_core::db::{DbClient, DbClientDescriptor}` trait objects this registry
//! hands back. Adding a new engine is: a new `DbClient` impl + a `DbKind`
//! variant + a `DbClientDescriptor` impl (both in `sid-db`) + one
//! `register(..)` call here.
//!
//! [`DbKind::Redb`] deliberately has no descriptor — see
//! [`DbClientDescriptor`]'s doc comment ("a synthetic, always-present
//! connection, never a form choice"). [`DbRegistry::descriptor`] returns
//! `None` for it rather than panicking; [`DbRegistry::client`] still returns a
//! working client for all three kinds.
//!
//! Not yet constructed anywhere (W3 wires it into app init) — this task lands
//! the module and its pure-logic tests only.

use std::sync::Arc;

use sid_core::db::{DbClient, DbClientDescriptor, DbKind};

/// One registered engine: its client factory and (optionally) the descriptor
/// that drives its connection form.
struct Entry {
    client: Arc<dyn DbClient>,
    descriptor: Option<Box<dyn DbClientDescriptor>>,
}

/// Maps each [`DbKind`] to its concrete adapter. Construct with
/// [`DbRegistry::new`] (or [`Default::default`]); both register
/// Postgres, SQLite, and Redb.
pub struct DbRegistry {
    entries: Vec<(DbKind, Entry)>,
}

impl DbRegistry {
    /// Build a registry with all three current engines registered: Postgres
    /// and SQLite (each with a descriptor), and Redb (no descriptor — see the
    /// module doc comment).
    pub fn new() -> Self {
        let mut reg = Self {
            entries: Vec::new(),
        };
        reg.register(
            sid_db::PostgresClient::factory(),
            Some(Box::new(sid_db::PostgresDescriptor)),
        );
        reg.register(
            sid_db::SqliteClient::factory(),
            Some(Box::new(sid_db::SqliteDescriptor)),
        );
        reg.register(sid_db::RedbBrowseClient::factory(), None);
        reg
    }

    /// Register one engine's client factory + optional descriptor, keyed off
    /// `client.kind()` (Redb has no descriptor to key off instead).
    fn register(
        &mut self,
        client: Arc<dyn DbClient>,
        descriptor: Option<Box<dyn DbClientDescriptor>>,
    ) {
        let kind = client.kind();
        debug_assert!(
            descriptor.as_deref().is_none_or(|d| d.kind() == kind),
            "descriptor kind mismatch for {kind:?}"
        );
        self.entries.push((kind, Entry { client, descriptor }));
    }

    /// The client factory registered for `kind`. Panics if `kind` was never
    /// registered — currently unreachable, since all three [`DbKind`]
    /// variants are registered by [`DbRegistry::new`].
    pub fn client(&self, kind: DbKind) -> Arc<dyn DbClient> {
        self.entries
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, e)| e.client.clone())
            .unwrap_or_else(|| panic!("no DbClient registered for {kind:?}"))
    }

    /// The connection-form descriptor for `kind`, or `None` for an engine with
    /// no form (currently only [`DbKind::Redb`]).
    pub fn descriptor(&self, kind: DbKind) -> Option<&dyn DbClientDescriptor> {
        self.entries
            .iter()
            .find(|(k, _)| *k == kind)
            .and_then(|(_, e)| e.descriptor.as_deref())
    }

    /// All registered kinds, in registration order.
    pub fn kinds(&self) -> Vec<DbKind> {
        self.entries.iter().map(|(k, _)| *k).collect()
    }
}

impl Default for DbRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_returns_factory_for_each_kind() {
        let reg = DbRegistry::new();
        for kind in [DbKind::Postgres, DbKind::Sqlite, DbKind::Redb] {
            assert_eq!(reg.client(kind).kind(), kind);
        }
    }

    #[test]
    fn descriptor_returns_matching_engine_for_each_kind() {
        let reg = DbRegistry::new();
        for kind in [DbKind::Postgres, DbKind::Sqlite] {
            assert_eq!(reg.descriptor(kind).unwrap().kind(), kind);
        }
    }

    #[test]
    fn descriptor_is_none_for_redb() {
        let reg = DbRegistry::new();
        assert!(reg.descriptor(DbKind::Redb).is_none());
    }

    #[test]
    fn descriptor_drives_connection_fields() {
        let reg = DbRegistry::new();

        let pg_keys: Vec<String> = reg
            .descriptor(DbKind::Postgres)
            .unwrap()
            .connection_fields()
            .into_iter()
            .map(|f| f.key)
            .collect();
        assert!(pg_keys.contains(&"host".to_string()));
        assert!(pg_keys.contains(&"password".to_string()));

        let sqlite_keys: Vec<String> = reg
            .descriptor(DbKind::Sqlite)
            .unwrap()
            .connection_fields()
            .into_iter()
            .map(|f| f.key)
            .collect();
        assert_eq!(sqlite_keys, vec!["path".to_string(), "mode".to_string()]);
    }

    #[test]
    fn kinds_lists_all_three_registered_engines() {
        let reg = DbRegistry::new();
        assert_eq!(
            reg.kinds(),
            vec![DbKind::Postgres, DbKind::Sqlite, DbKind::Redb]
        );
    }

    #[test]
    fn default_matches_new() {
        assert_eq!(DbRegistry::default().kinds(), DbRegistry::new().kinds());
    }
}
