//! End-to-end DatabaseWidget integration test against in-memory SQLite.

use sid_core::adapters::db_client::{DbKind, OpenParams};
use sid_db_clients::SqliteClient;
use sid_store::DbConnection;
use sid_widgets::database::{DatabaseState, RightPane};

#[tokio::test]
async fn widget_end_to_end_against_in_memory_sqlite() {
    let conn = DbConnection {
        id: "mem".into(),
        kind: DbKind::Sqlite,
        name: "memory".into(),
        dsn: ":memory:".into(),
        secret_ref: None,
        created_at: 0,
    };
    let mut state = DatabaseState::new(vec![conn.clone()]);

    let client = SqliteClient::factory()
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: ":memory:".into(),
            password: None,
        })
        .await
        .unwrap();
    state.apply_connect_result(conn.id.clone(), client.clone());
    assert_eq!(state.active_conn_id(), Some("mem"));

    client
        .execute("CREATE TABLE t (id INT, v TEXT)")
        .await
        .unwrap();
    client
        .execute("INSERT INTO t VALUES (1, 'a'), (2, 'b')")
        .await
        .unwrap();

    let page = client
        .query_paged("SELECT id, v FROM t ORDER BY id", None, 50)
        .await
        .unwrap();
    state.apply_query_result(page, None);
    assert_eq!(state.results.page.as_ref().unwrap().rows.len(), 2);
    assert_eq!(state.right_pane(), RightPane::Results);
}
