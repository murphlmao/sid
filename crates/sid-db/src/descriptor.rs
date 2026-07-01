//! Connection-form descriptors for the Database tab's extensible engine list.
//!
//! Each engine (Postgres, SQLite) implements [`DbClientDescriptor`] from
//! `sid-core`, declaring the form fields it needs and converting collected
//! form values <-> [`OpenParams`]. The binary's registry (a later phase)
//! instantiates these descriptors; nothing here wires the registry.
//!
//! Adding a new engine to the Database tab requires only a new descriptor impl
//! plus the matching [`DbClient`](sid_core::db::DbClient) factory — no changes
//! to the form-rendering or DSN-prefill machinery. [`DbKind::Redb`] deliberately
//! has no descriptor here — it is a synthetic, always-present connection, never
//! a form choice.

use std::collections::BTreeMap;

use sid_core::db::{ConnField, ConnFieldKind, DbClientDescriptor, DbKind, OpenParams, SqliteMode};

/// Display label for [`SqliteMode::OpenExisting`] in the connection form.
const SQLITE_MODE_OPEN_EXISTING: &str = "Open existing";
/// Display label for [`SqliteMode::CreateNew`] in the connection form.
const SQLITE_MODE_CREATE_NEW: &str = "Create new";

/// Descriptor for the PostgreSQL engine.
///
/// Declares the `host`/`port`/`database`/`user`/`password` form fields and
/// assembles a password-free `postgres://user@host:port/database` DSN, routing
/// the password (if any) separately through [`OpenParams::password`].
///
/// ```
/// use std::collections::BTreeMap;
/// use sid_core::db::{DbClientDescriptor, DbKind};
/// use sid_db::PostgresDescriptor;
///
/// let d = PostgresDescriptor;
/// assert_eq!(d.kind(), DbKind::Postgres);
/// let vals = BTreeMap::from([
///     ("host".to_string(), "db.example".to_string()),
///     ("port".to_string(), "5432".to_string()),
///     ("database".to_string(), "app".to_string()),
///     ("user".to_string(), "alice".to_string()),
/// ]);
/// let params = d.assemble_params(&vals).unwrap();
/// assert_eq!(params.dsn, "postgres://alice@db.example:5432/app");
/// assert_eq!(params.password, None);
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct PostgresDescriptor;

impl DbClientDescriptor for PostgresDescriptor {
    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }

    fn connection_fields(&self) -> Vec<ConnField> {
        vec![
            ConnField::new("host", "Host", ConnFieldKind::Text).required(),
            ConnField::new("port", "Port", ConnFieldKind::Port).with_default("5432"),
            ConnField::new("database", "Database", ConnFieldKind::Text).required(),
            ConnField::new("user", "User", ConnFieldKind::Text).required(),
            ConnField::new("password", "Password", ConnFieldKind::Password),
        ]
    }

    fn assemble_params(&self, values: &BTreeMap<String, String>) -> Result<OpenParams, String> {
        let host = require(values, "host", "Host")?;
        let port = require(values, "port", "Port")?;
        let database = require(values, "database", "Database")?;
        let user = require(values, "user", "User")?;

        // Password travels in OpenParams.password, never in the DSN. The
        // PostgresClient::open path encodes it onto the URL itself.
        let password = values
            .get("password")
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string());

        let dsn = format!("postgres://{user}@{host}:{port}/{database}");

        Ok(OpenParams {
            kind: DbKind::Postgres,
            dsn,
            password,
            // Postgres ignores the SQLite open-vs-create mode entirely.
            sqlite_mode: None,
        })
    }

    fn dsn_to_field_values(&self, dsn: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();

        // Strip the scheme; tolerate its absence rather than erroring.
        let rest = dsn.strip_prefix("postgres://").unwrap_or(dsn);

        // Split `user@host:port/database`. The user portion is everything
        // before the first '@' (if present); the password is never in the DSN.
        let (user, authority_and_path) = match rest.split_once('@') {
            Some((u, tail)) => (Some(u), tail),
            None => (None, rest),
        };
        if let Some(user) = user {
            if !user.is_empty() {
                out.insert("user".to_string(), user.to_string());
            }
        }

        // `host:port/database` — split off the path at the first '/'.
        let (authority, database) = match authority_and_path.split_once('/') {
            Some((a, db)) => (a, Some(db)),
            None => (authority_and_path, None),
        };
        if let Some(database) = database {
            if !database.is_empty() {
                out.insert("database".to_string(), database.to_string());
            }
        }

        // `host:port` — port is optional; omit it when absent.
        match authority.rsplit_once(':') {
            Some((host, port)) => {
                if !host.is_empty() {
                    out.insert("host".to_string(), host.to_string());
                }
                if !port.is_empty() {
                    out.insert("port".to_string(), port.to_string());
                }
            }
            None => {
                if !authority.is_empty() {
                    out.insert("host".to_string(), authority.to_string());
                }
            }
        }

        out
    }
}

/// Descriptor for the SQLite engine.
///
/// A `path` field plus a `mode` Choice (open an existing file vs create a new
/// one); the DSN is the path verbatim (`:memory:` or a filesystem path), and
/// SQLite has no password. The chosen mode is carried out-of-band on
/// [`OpenParams::sqlite_mode`] because it is an open-time decision, not part of
/// the DSN.
///
/// ```
/// use std::collections::BTreeMap;
/// use sid_core::db::{DbClientDescriptor, DbKind, SqliteMode};
/// use sid_db::SqliteDescriptor;
///
/// let d = SqliteDescriptor;
/// assert_eq!(d.kind(), DbKind::Sqlite);
/// let vals = BTreeMap::from([
///     ("path".to_string(), "/tmp/a.db".to_string()),
///     ("mode".to_string(), "Create new".to_string()),
/// ]);
/// let params = d.assemble_params(&vals).unwrap();
/// assert_eq!(params.dsn, "/tmp/a.db");
/// assert_eq!(params.password, None);
/// assert_eq!(params.sqlite_mode, Some(SqliteMode::CreateNew));
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct SqliteDescriptor;

impl DbClientDescriptor for SqliteDescriptor {
    fn kind(&self) -> DbKind {
        DbKind::Sqlite
    }

    fn connection_fields(&self) -> Vec<ConnField> {
        vec![
            ConnField::new("path", "File", ConnFieldKind::Path).required(),
            ConnField::new(
                "mode",
                "Mode",
                ConnFieldKind::Choice {
                    options: vec![
                        SQLITE_MODE_OPEN_EXISTING.to_string(),
                        SQLITE_MODE_CREATE_NEW.to_string(),
                    ],
                },
            )
            .required()
            .with_default(SQLITE_MODE_OPEN_EXISTING),
        ]
    }

    fn assemble_params(&self, values: &BTreeMap<String, String>) -> Result<OpenParams, String> {
        let path = require(values, "path", "File")?;
        let sqlite_mode = Some(parse_sqlite_mode(values));
        Ok(OpenParams {
            kind: DbKind::Sqlite,
            dsn: path,
            password: None,
            sqlite_mode,
        })
    }

    fn dsn_to_field_values(&self, dsn: &str) -> BTreeMap<String, String> {
        // The mode is not encoded in the DSN (it is an open-time choice), so
        // prefill always defaults it to "Open existing".
        BTreeMap::from([
            ("path".to_string(), dsn.to_string()),
            ("mode".to_string(), SQLITE_MODE_OPEN_EXISTING.to_string()),
        ])
    }
}

/// Map the `mode` form value to a [`SqliteMode`]. Unknown/missing values fall
/// back to [`SqliteMode::OpenExisting`] — the safe, non-destructive default
/// (it never silently creates a file).
fn parse_sqlite_mode(values: &BTreeMap<String, String>) -> SqliteMode {
    match values.get("mode").map(String::as_str) {
        Some(SQLITE_MODE_CREATE_NEW) => SqliteMode::CreateNew,
        _ => SqliteMode::OpenExisting,
    }
}

/// Fetch a required field's value, erroring with `"{label} is required"` when
/// the key is missing or its value is empty. Returns the owned, non-empty
/// value on success.
fn require(values: &BTreeMap<String, String>, key: &str, label: &str) -> Result<String, String> {
    match values.get(key) {
        Some(v) if !v.is_empty() => Ok(v.to_string()),
        _ => Err(format!("{label} is required")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg_values() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("host".to_string(), "db.example".to_string()),
            ("port".to_string(), "5432".to_string()),
            ("database".to_string(), "app".to_string()),
            ("user".to_string(), "alice".to_string()),
        ])
    }

    #[test]
    fn postgres_kind_is_postgres() {
        assert_eq!(PostgresDescriptor.kind(), DbKind::Postgres);
    }

    #[test]
    fn postgres_connection_fields_exact_shape() {
        let fields = PostgresDescriptor.connection_fields();
        assert_eq!(fields.len(), 5);

        let expected: [(&str, &str, ConnFieldKind, bool, Option<&str>); 5] = [
            ("host", "Host", ConnFieldKind::Text, true, None),
            ("port", "Port", ConnFieldKind::Port, false, Some("5432")),
            ("database", "Database", ConnFieldKind::Text, true, None),
            ("user", "User", ConnFieldKind::Text, true, None),
            ("password", "Password", ConnFieldKind::Password, false, None),
        ];

        for (field, (key, label, kind, required, default)) in fields.iter().zip(expected) {
            assert_eq!(field.key, key, "key");
            assert_eq!(field.label, label, "label for {key}");
            assert_eq!(field.kind, kind, "kind for {key}");
            assert_eq!(field.required, required, "required for {key}");
            assert_eq!(field.default.as_deref(), default, "default for {key}");
        }
    }

    #[test]
    fn postgres_assemble_happy_path_no_password() {
        let params = PostgresDescriptor.assemble_params(&pg_values()).unwrap();
        assert_eq!(params.kind, DbKind::Postgres);
        assert_eq!(params.dsn, "postgres://alice@db.example:5432/app");
        assert_eq!(params.password, None);
    }

    #[test]
    fn postgres_assemble_routes_password_to_open_params() {
        let mut values = pg_values();
        values.insert("password".to_string(), "s3cr3t".to_string());
        let params = PostgresDescriptor.assemble_params(&values).unwrap();
        // Password must NOT appear in the DSN.
        assert_eq!(params.dsn, "postgres://alice@db.example:5432/app");
        assert!(!params.dsn.contains("s3cr3t"));
        assert_eq!(params.password.as_deref(), Some("s3cr3t"));
    }

    #[test]
    fn postgres_empty_password_routes_to_none() {
        let mut values = pg_values();
        values.insert("password".to_string(), String::new());
        let params = PostgresDescriptor.assemble_params(&values).unwrap();
        assert_eq!(params.password, None);
    }

    #[test]
    fn postgres_missing_required_field_errors() {
        for (key, label) in [("host", "Host"), ("database", "Database"), ("user", "User")] {
            let mut values = pg_values();
            values.remove(key);
            let err = PostgresDescriptor.assemble_params(&values).unwrap_err();
            assert_eq!(err, format!("{label} is required"), "missing {key}");
        }
    }

    #[test]
    fn postgres_empty_required_field_errors() {
        for (key, label) in [("host", "Host"), ("database", "Database"), ("user", "User")] {
            let mut values = pg_values();
            values.insert(key.to_string(), String::new());
            let err = PostgresDescriptor.assemble_params(&values).unwrap_err();
            assert_eq!(err, format!("{label} is required"), "empty {key}");
        }
    }

    #[test]
    fn postgres_missing_port_errors_even_though_it_has_a_default() {
        // The default is for form prefill, not assembly. An empty `port` value
        // on submit is still a validation failure.
        let mut values = pg_values();
        values.remove("port");
        let err = PostgresDescriptor.assemble_params(&values).unwrap_err();
        assert_eq!(err, "Port is required");
    }

    #[test]
    fn postgres_round_trip_recovers_fields() {
        let params = PostgresDescriptor.assemble_params(&pg_values()).unwrap();
        let recovered = PostgresDescriptor.dsn_to_field_values(&params.dsn);
        assert_eq!(
            recovered.get("host").map(String::as_str),
            Some("db.example")
        );
        assert_eq!(recovered.get("port").map(String::as_str), Some("5432"));
        assert_eq!(recovered.get("database").map(String::as_str), Some("app"));
        assert_eq!(recovered.get("user").map(String::as_str), Some("alice"));
        // Password is never in the DSN, so it is never recovered.
        assert_eq!(recovered.get("password"), None);
    }

    #[test]
    fn postgres_dsn_parse_tolerates_missing_port() {
        let recovered = PostgresDescriptor.dsn_to_field_values("postgres://bob@localhost/mydb");
        assert_eq!(recovered.get("host").map(String::as_str), Some("localhost"));
        assert_eq!(recovered.get("user").map(String::as_str), Some("bob"));
        assert_eq!(recovered.get("database").map(String::as_str), Some("mydb"));
        // Port omitted rather than fabricated.
        assert_eq!(recovered.get("port"), None);
    }

    #[test]
    fn postgres_dsn_parse_tolerates_missing_scheme() {
        let recovered = PostgresDescriptor.dsn_to_field_values("bob@localhost:6543/mydb");
        assert_eq!(recovered.get("host").map(String::as_str), Some("localhost"));
        assert_eq!(recovered.get("port").map(String::as_str), Some("6543"));
        assert_eq!(recovered.get("user").map(String::as_str), Some("bob"));
        assert_eq!(recovered.get("database").map(String::as_str), Some("mydb"));
    }

    #[test]
    fn postgres_dsn_parse_garbage_does_not_panic() {
        // Best-effort: weird shapes yield a partial/empty map, never a panic.
        for dsn in ["", "postgres://", "@", ":", "///", "postgres://@:/"] {
            let _ = PostgresDescriptor.dsn_to_field_values(dsn);
        }
    }

    #[test]
    fn sqlite_kind_is_sqlite() {
        assert_eq!(SqliteDescriptor.kind(), DbKind::Sqlite);
    }

    #[test]
    fn sqlite_connection_fields_path_then_mode() {
        let fields = SqliteDescriptor.connection_fields();
        assert_eq!(fields.len(), 2);

        // First field: path.
        assert_eq!(fields[0].key, "path");
        assert_eq!(fields[0].label, "File");
        assert_eq!(fields[0].kind, ConnFieldKind::Path);
        assert!(fields[0].required);
        assert_eq!(fields[0].default, None);

        // Second field: mode Choice with both options + default.
        assert_eq!(fields[1].key, "mode");
        assert_eq!(fields[1].label, "Mode");
        assert_eq!(
            fields[1].kind,
            ConnFieldKind::Choice {
                options: vec!["Open existing".to_string(), "Create new".to_string()],
            }
        );
        assert!(fields[1].required);
        assert_eq!(fields[1].default.as_deref(), Some("Open existing"));
    }

    #[test]
    fn sqlite_assemble_happy_path_defaults_to_open_existing() {
        // No mode value -> defaults to OpenExisting (safe, non-destructive).
        let values = BTreeMap::from([("path".to_string(), "/tmp/a.db".to_string())]);
        let params = SqliteDescriptor.assemble_params(&values).unwrap();
        assert_eq!(params.kind, DbKind::Sqlite);
        assert_eq!(params.dsn, "/tmp/a.db");
        assert_eq!(params.password, None);
        assert_eq!(params.sqlite_mode, Some(SqliteMode::OpenExisting));
    }

    #[test]
    fn sqlite_assemble_mode_label_round_trips_to_enum() {
        for (label, expected) in [
            ("Open existing", SqliteMode::OpenExisting),
            ("Create new", SqliteMode::CreateNew),
        ] {
            let values = BTreeMap::from([
                ("path".to_string(), "/tmp/a.db".to_string()),
                ("mode".to_string(), label.to_string()),
            ]);
            let params = SqliteDescriptor.assemble_params(&values).unwrap();
            assert_eq!(params.sqlite_mode, Some(expected), "for label {label}");
        }
    }

    #[test]
    fn sqlite_assemble_unknown_mode_label_falls_back_to_open_existing() {
        let values = BTreeMap::from([
            ("path".to_string(), "/tmp/a.db".to_string()),
            ("mode".to_string(), "garbage".to_string()),
        ]);
        let params = SqliteDescriptor.assemble_params(&values).unwrap();
        assert_eq!(params.sqlite_mode, Some(SqliteMode::OpenExisting));
    }

    #[test]
    fn sqlite_assemble_empty_path_errors() {
        // Missing key.
        let err = SqliteDescriptor
            .assemble_params(&BTreeMap::new())
            .unwrap_err();
        assert_eq!(err, "File is required");

        // Present-but-empty value.
        let values = BTreeMap::from([("path".to_string(), String::new())]);
        let err = SqliteDescriptor.assemble_params(&values).unwrap_err();
        assert_eq!(err, "File is required");
    }

    #[test]
    fn sqlite_round_trip_recovers_path_and_defaults_mode() {
        let values = BTreeMap::from([
            ("path".to_string(), "/var/db/app.sqlite".to_string()),
            ("mode".to_string(), "Create new".to_string()),
        ]);
        let params = SqliteDescriptor.assemble_params(&values).unwrap();
        assert_eq!(params.sqlite_mode, Some(SqliteMode::CreateNew));

        // Mode is not encoded in the DSN, so prefill recovers the path verbatim
        // and resets the mode field to the safe "Open existing" default.
        let recovered = SqliteDescriptor.dsn_to_field_values(&params.dsn);
        assert_eq!(
            recovered.get("path").map(String::as_str),
            Some("/var/db/app.sqlite")
        );
        assert_eq!(
            recovered.get("mode").map(String::as_str),
            Some("Open existing")
        );
    }

    /// A third, made-up engine. Its sole purpose is to prove that the
    /// `DbClientDescriptor` trait is object-safe and that adding a new engine
    /// to the Database tab needs nothing more than a descriptor impl.
    #[derive(Clone, Copy, Debug, Default)]
    struct DummyDescriptor;

    impl DbClientDescriptor for DummyDescriptor {
        fn kind(&self) -> DbKind {
            // Reuse an existing discriminant; this engine is fictional and the
            // value is irrelevant to the object-safety assertion.
            DbKind::Sqlite
        }

        fn connection_fields(&self) -> Vec<ConnField> {
            vec![ConnField::new("endpoint", "Endpoint", ConnFieldKind::Text).required()]
        }

        fn assemble_params(&self, values: &BTreeMap<String, String>) -> Result<OpenParams, String> {
            let endpoint = require(values, "endpoint", "Endpoint")?;
            Ok(OpenParams {
                kind: DbKind::Sqlite,
                dsn: endpoint,
                password: None,
                sqlite_mode: None,
            })
        }

        fn dsn_to_field_values(&self, dsn: &str) -> BTreeMap<String, String> {
            BTreeMap::from([("endpoint".to_string(), dsn.to_string())])
        }
    }

    #[test]
    fn trait_is_object_safe_for_a_new_engine() {
        // The whole extensibility story: a new engine plugs in behind the
        // trait object without the registry knowing its concrete type.
        let descriptors: Vec<Box<dyn DbClientDescriptor>> = vec![
            Box::new(PostgresDescriptor),
            Box::new(SqliteDescriptor),
            Box::new(DummyDescriptor),
        ];
        assert_eq!(descriptors.len(), 3);

        let dummy: Box<dyn DbClientDescriptor> = Box::new(DummyDescriptor);
        let values = BTreeMap::from([("endpoint".to_string(), "wss://x".to_string())]);
        let params = dummy.assemble_params(&values).unwrap();
        assert_eq!(params.dsn, "wss://x");
        assert!(dummy.assemble_params(&BTreeMap::new()).is_err());
    }
}
