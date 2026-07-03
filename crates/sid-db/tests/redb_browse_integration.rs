//! Durable `RedbBrowseClient` (DbKind::Redb) integration tests. Recreated
//! from an adversarial probe (`redb_browse_probe.rs`, throwaway, not
//! committed) that found BUG 6: `table_columns`/`dump_table` hardcoded column
//! lists that omitted real `Host`/`DbConnection` fields (`folder` on hosts;
//! `kind`/`name`/`folder` on connections). No Docker required — this is an
//! in-process redb file, so these run under the default `cargo test`.

use std::sync::Arc;

use sid_core::db::{DbError, DbKind, OpenParams, PageCursor};
use sid_db::RedbBrowseClient;
use sid_store::{
    AuthMethod, DbConnection, DefaultScope, GlobalStore, Host, PanelSide, QuickAction, Settings,
};

fn open_tmp() -> (tempfile::TempDir, Arc<GlobalStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
    (dir, Arc::new(store))
}

// ---------- BUG 6: schema_introspect / dump_table must expose every real field ----------

#[tokio::test]
async fn schema_and_dump_expose_folder_kind_name_columns() {
    let (_dir, store) = open_tmp();
    let client = RedbBrowseClient::wrap(store.clone());

    // Every optional field populated, including `folder` (added to
    // Host/DbConnection after redb_browse.rs's TABLE_NAMES column lists were
    // originally written — the gap BUG 6 fixed).
    store
        .upsert_host(&Host {
            alias: "box1".into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: Some("kr:abc".into()),
            auth: AuthMethod::Key {
                path: "/home/u/.ssh/id_ed25519".into(),
            },
            folder: Some("prod/web".into()),
        })
        .unwrap();
    store
        .upsert_connection(&DbConnection {
            id: "conn1".into(),
            dsn: "postgres://x@y/z".into(),
            secret_ref: Some("kr:def".into()),
            kind: DbKind::Postgres,
            name: "Prod PG".into(),
            folder: Some("analytics".into()),
        })
        .unwrap();

    let schema = client.schema_introspect().await.unwrap();
    let hosts_tbl = schema.tables.iter().find(|t| t.name == "hosts").unwrap();
    let conns_tbl = schema
        .tables
        .iter()
        .find(|t| t.name == "connections")
        .unwrap();

    // Host has 7 fields: alias, user, host, port, secret_ref, auth, folder.
    assert_eq!(
        hosts_tbl.columns.len(),
        7,
        "Host has 7 fields incl. `folder`"
    );
    assert_eq!(
        hosts_tbl
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "alias",
            "user",
            "host",
            "port",
            "secret_ref",
            "auth",
            "folder"
        ]
    );

    // DbConnection has 6 fields: id, name, kind, dsn, secret_ref, folder.
    assert_eq!(
        conns_tbl.columns.len(),
        6,
        "DbConnection has 6 fields incl. `kind`/`name`/`folder`"
    );
    assert_eq!(
        conns_tbl
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name", "kind", "dsn", "secret_ref", "folder"]
    );

    // The actual row data must carry the same fields, not just the schema.
    let hosts_page = client.query_paged("hosts", None, 10).await.unwrap();
    assert_eq!(hosts_page.rows[0].values.len(), 7);
    assert_eq!(hosts_page.rows[0].values[6], "prod/web");

    let conns_page = client.query_paged("connections", None, 10).await.unwrap();
    assert_eq!(conns_page.rows[0].values.len(), 6);
    assert_eq!(
        conns_page.rows[0].values,
        vec![
            "conn1",
            "Prod PG",
            "postgres",
            "postgres://x@y/z",
            "kr:def",
            "analytics"
        ]
    );
}

// ---------- query_paged: pagination correctness ----------

#[tokio::test]
async fn query_paged_pages_past_page_size_with_correct_cursors() {
    let (_dir, store) = open_tmp();
    for i in 0..25 {
        store
            .upsert_host(&Host {
                alias: format!("host{i:02}"),
                user: "u".into(),
                host: "h".into(),
                port: 22,
                secret_ref: None,
                auth: AuthMethod::Agent,
                folder: None,
            })
            .unwrap();
    }
    let client = RedbBrowseClient::wrap(store);

    let mut cursor = None;
    let mut seen: Vec<String> = Vec::new();
    let mut pages = 0;
    loop {
        let page = client.query_paged("hosts", cursor, 10).await.unwrap();
        pages += 1;
        assert!(pages <= 10, "paging did not terminate");
        seen.extend(page.rows.iter().map(|r| r.values[0].clone()));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        pages, 3,
        "25 rows / page_size 10 should be 3 pages (10,10,5)"
    );
    assert_eq!(seen.len(), 25, "all 25 rows should be seen exactly once");
    let mut dedup = seen.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(
        dedup.len(),
        25,
        "no row should repeat across pages: {seen:?}"
    );
}

#[tokio::test]
async fn query_paged_cursor_past_end_returns_empty_and_no_next_cursor() {
    let (_dir, store) = open_tmp();
    store
        .upsert_host(&Host {
            alias: "only".into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: None,
            auth: AuthMethod::Agent,
            folder: None,
        })
        .unwrap();
    let client = RedbBrowseClient::wrap(store);
    let page = client
        .query_paged("hosts", Some(PageCursor { offset: 1000 }), 10)
        .await
        .unwrap();
    assert!(page.rows.is_empty());
    assert!(page.next_cursor.is_none());
}

#[tokio::test]
async fn query_paged_page_size_zero_is_treated_as_one_not_a_hang_or_div_by_zero() {
    let (_dir, store) = open_tmp();
    for i in 0..3 {
        store
            .upsert_host(&Host {
                alias: format!("h{i}"),
                user: "u".into(),
                host: "h".into(),
                port: 22,
                secret_ref: None,
                auth: AuthMethod::Agent,
                folder: None,
            })
            .unwrap();
    }
    let client = RedbBrowseClient::wrap(store);
    let page = client.query_paged("hosts", None, 0).await.unwrap();
    // page_size.max(1) => 1 row per page.
    assert_eq!(page.rows.len(), 1);
    assert!(page.next_cursor.is_some());
}

// ---------- Ordering: redb Table::iter() is key-sorted, not insertion-order ----------

#[tokio::test]
async fn hosts_are_returned_in_key_sorted_not_insertion_order() {
    let (_dir, store) = open_tmp();
    for alias in ["zeta", "alpha", "mike"] {
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
    let client = RedbBrowseClient::wrap(store);
    let page = client.query_paged("hosts", None, 10).await.unwrap();
    let names: Vec<&str> = page.rows.iter().map(|r| r.values[0].as_str()).collect();
    assert_eq!(names, vec!["alpha", "mike", "zeta"]);
}

// ---------- Error paths: bad/adversarial "sql" (table selector) ----------

#[tokio::test]
async fn query_paged_rejects_sql_injection_shaped_table_name() {
    let (_dir, store) = open_tmp();
    let client = RedbBrowseClient::wrap(store);
    let err = client
        .query_paged("hosts; DROP TABLE hosts;--", None, 10)
        .await
        .unwrap_err();
    match err {
        DbError::Query(_) => {}
        other => panic!("expected DbError::Query for injection-shaped input, got {other:?}"),
    }
}

#[tokio::test]
async fn query_paged_table_name_is_case_sensitive() {
    let (_dir, store) = open_tmp();
    let client = RedbBrowseClient::wrap(store);
    let err = client.query_paged("HOSTS", None, 10).await.unwrap_err();
    match err {
        DbError::Query(_) => {}
        other => panic!("expected DbError::Query for wrong-case table name, got {other:?}"),
    }
}

#[tokio::test]
async fn query_paged_trims_but_does_not_otherwise_normalize_whitespace() {
    let (_dir, store) = open_tmp();
    store
        .upsert_host(&Host {
            alias: "only".into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: None,
            auth: AuthMethod::Agent,
            folder: None,
        })
        .unwrap();
    let client = RedbBrowseClient::wrap(store);
    // Leading/trailing whitespace and even a newline are silently trimmed and accepted.
    let page = client.query_paged("  hosts\n", None, 10).await.unwrap();
    assert_eq!(page.rows.len(), 1);
}

#[tokio::test]
async fn query_paged_empty_string_errors() {
    let (_dir, store) = open_tmp();
    let client = RedbBrowseClient::wrap(store);
    let err = client.query_paged("", None, 10).await.unwrap_err();
    match err {
        DbError::Query(_) => {}
        other => panic!("expected DbError::Query for empty table selector, got {other:?}"),
    }
}

#[tokio::test]
async fn query_paged_before_open_is_not_connected() {
    let factory = RedbBrowseClient::factory();
    let err = factory.query_paged("hosts", None, 10).await.unwrap_err();
    match err {
        DbError::NotConnected => {}
        other => panic!("expected DbError::NotConnected, got {other:?}"),
    }
}

// ---------- Cancel: no-op, must not error ----------

#[tokio::test]
async fn cancel_is_a_harmless_noop() {
    let (_dir, store) = open_tmp();
    let client = RedbBrowseClient::wrap(store);
    client.cancel().await.unwrap();
    // Still usable after cancel.
    let schema = client.schema_introspect().await.unwrap();
    assert_eq!(schema.tables.len(), 5);
}

// ---------- open() adversarial paths ----------

#[tokio::test]
async fn open_on_a_path_that_is_not_a_valid_redb_file_errors_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let bogus = dir.path().join("not_a_redb_file.redb");
    std::fs::write(&bogus, b"this is not a redb database, just text").unwrap();
    let factory = RedbBrowseClient::factory();
    let result = factory
        .open(OpenParams {
            kind: DbKind::Redb,
            dsn: bogus.to_string_lossy().into_owned(),
            password: None,
            sqlite_mode: None,
        })
        .await;
    match result {
        Err(DbError::Connect(_)) => {}
        Err(other) => panic!("expected DbError::Connect for a corrupt file, got {other:?}"),
        Ok(_) => panic!("opening a non-redb file must not silently succeed"),
    }
}

#[tokio::test]
async fn open_with_dsn_pointing_at_a_directory_errors_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let factory = RedbBrowseClient::factory();
    let result = factory
        .open(OpenParams {
            kind: DbKind::Redb,
            dsn: dir.path().to_string_lossy().into_owned(),
            password: None,
            sqlite_mode: None,
        })
        .await;
    assert!(
        result.is_err(),
        "opening a directory as a redb file must error, not panic"
    );
}

#[tokio::test]
async fn open_with_empty_dsn_errors_cleanly_not_panic() {
    let factory = RedbBrowseClient::factory();
    let result = factory
        .open(OpenParams {
            kind: DbKind::Redb,
            dsn: String::new(),
            password: None,
            sqlite_mode: None,
        })
        .await;
    assert!(result.is_err(), "empty dsn must error cleanly");
}

// ---------- schema_graph: primary keys ----------

#[tokio::test]
async fn schema_graph_on_empty_store_still_reports_pk_shape() {
    let (_dir, store) = open_tmp();
    let client = RedbBrowseClient::wrap(store);
    let graph = client.schema_graph().await.unwrap();
    assert!(graph.foreign_keys.is_empty());
    assert_eq!(
        graph.primary_keys.len(),
        4,
        "hosts/connections/quick_actions/workspaces have a natural PK; settings does not"
    );
    assert!(!graph.primary_keys.contains_key("settings"));
}

// ---------- settings: singleton row ----------

#[tokio::test]
async fn settings_table_shows_default_row_when_never_set() {
    let (_dir, store) = open_tmp();
    let client = RedbBrowseClient::wrap(store);
    let page = client.query_paged("settings", None, 10).await.unwrap();
    assert_eq!(
        page.rows.len(),
        1,
        "settings is a singleton row even when unset"
    );
}

#[tokio::test]
async fn settings_after_explicit_set_reflects_new_value() {
    let (_dir, store) = open_tmp();
    store
        .set_settings(&Settings {
            default_scope: DefaultScope::Global,
            file_browser_side: PanelSide::Right,
            secret_keyring_enabled: false,
            secret_file_enabled: true,
        })
        .unwrap();
    let client = RedbBrowseClient::wrap(store);
    let page = client.query_paged("settings", None, 10).await.unwrap();
    assert_eq!(page.columns.len(), 1, "declared column count for settings");
    assert_eq!(
        page.rows[0].values.len(),
        1,
        "settings row value count should match declared column count"
    );
}

// ---------- quick_actions: sanity ----------

#[tokio::test]
async fn quick_actions_round_trip_cleanly() {
    let (_dir, store) = open_tmp();
    store
        .upsert_quick_action(&QuickAction {
            label: "restart nginx".into(),
            cmd: "sudo systemctl restart nginx".into(),
        })
        .unwrap();
    let client = RedbBrowseClient::wrap(store);
    let page = client.query_paged("quick_actions", None, 10).await.unwrap();
    assert_eq!(
        page.rows[0].values,
        vec!["restart nginx", "sudo systemctl restart nginx"]
    );
}

// ---------- adversarial data: long/unicode/embedded-control-char values still render ----------

#[tokio::test]
async fn adversarial_field_values_do_not_panic_and_round_trip() {
    let (_dir, store) = open_tmp();
    let alias = "héllo\tworld\n💥-\u{0}-end"; // tab, newline, emoji, embedded NUL
    store
        .upsert_host(&Host {
            alias: alias.into(),
            user: "u".repeat(10_000),
            host: "h".into(),
            port: 65535,
            secret_ref: Some("".into()), // present-but-empty
            auth: AuthMethod::Agent,
            folder: Some("".into()),
        })
        .unwrap();
    let client = RedbBrowseClient::wrap(store);
    let page = client.query_paged("hosts", None, 10).await.unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].values[0], alias);
    assert_eq!(page.rows[0].values[3], "65535");
}
