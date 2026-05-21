//! Insta snapshot tests for [`DatabaseWidget::render_into_frame`].
//!
//! Each test builds a deterministic widget state, renders into a fixed
//! `TestBackend` via [`sid_widgets::database::render_to_string`], and
//! snapshots the text body so future layout changes surface as a diff.

use sid_core::adapters::db_client::{Column, ColumnType, DbKind, QueryPage, Row};
use sid_store::{DbConnection, QueryRecord};
use sid_widgets::DatabaseWidget;
use sid_widgets::database::{RightPane, render_to_string};

fn conn(id: &str, kind: DbKind) -> DbConnection {
    DbConnection {
        id: id.into(),
        kind,
        name: id.into(),
        dsn: ":memory:".into(),
        secret_ref: None,
        created_at: 0,
    }
}

fn fixture_three_conns() -> Vec<DbConnection> {
    vec![
        conn("prod-db", DbKind::Postgres),
        conn("analytics", DbKind::Sqlite),
        conn("scratch", DbKind::Sqlite),
    ]
}

fn sample_page() -> QueryPage {
    QueryPage {
        columns: vec![
            Column {
                name: "id".into(),
                ty: ColumnType::Integer,
            },
            Column {
                name: "name".into(),
                ty: ColumnType::Text,
            },
            Column {
                name: "created".into(),
                ty: ColumnType::Text,
            },
        ],
        rows: vec![
            Row {
                values: vec!["101".into(), "alice".into(), "2026-01-01".into()],
            },
            Row {
                values: vec!["102".into(), "bob".into(), "2026-01-02".into()],
            },
            Row {
                values: vec!["103".into(), "carol".into(), "2026-01-03".into()],
            },
        ],
        next_cursor: None,
        duration_ms: 4,
    }
}

#[test]
fn snapshot_empty_connection_list() {
    let w = DatabaseWidget::new(vec![]);
    let s = render_to_string(&w, 100, 24);
    insta::assert_snapshot!("database_empty_connections", s);
}

#[test]
fn snapshot_three_connections_editor_focused() {
    let mut w = DatabaseWidget::new(fixture_three_conns());
    // Default right pane is Editor; default selection is first row.
    assert_eq!(w.state().right_pane(), RightPane::Editor);
    // Drop a SQL fragment into the editor so the cursor block lands on text.
    for c in "SELECT * FROM users;".chars() {
        w.state_mut().editor.insert_char(c);
    }
    let s = render_to_string(&w, 100, 24);
    insta::assert_snapshot!("database_three_connections_editor", s);
}

#[test]
fn snapshot_active_connection_with_multiline_sql() {
    let mut w = DatabaseWidget::new(fixture_three_conns());
    w.state_mut().set_active_conn_id_for_tests("prod-db".into());
    // Multi-line query: SELECT … \n WHERE …;
    for c in "SELECT * FROM users".chars() {
        w.state_mut().editor.insert_char(c);
    }
    w.state_mut().editor.insert_newline();
    for c in "WHERE id > 100;".chars() {
        w.state_mut().editor.insert_char(c);
    }
    // Cursor lands at end of line 2.
    let s = render_to_string(&w, 100, 24);
    insta::assert_snapshot!("database_active_multiline_sql", s);
}

#[test]
fn snapshot_results_pane_focused_with_rows() {
    let mut w = DatabaseWidget::new(fixture_three_conns());
    w.state_mut().set_active_conn_id_for_tests("prod-db".into());
    w.set_results_for_tests(sample_page());
    w.state_mut().set_right_pane(RightPane::Results);
    w.state_mut().results.select_next_row(); // highlight row 2
    let s = render_to_string(&w, 100, 24);
    insta::assert_snapshot!("database_results_focused", s);
}

#[test]
fn snapshot_history_pane_focused() {
    let mut w = DatabaseWidget::new(fixture_three_conns());
    w.state_mut().set_active_conn_id_for_tests("prod-db".into());
    let records = (0u64..5)
        .map(|i| QueryRecord {
            conn_id: "prod-db".into(),
            sql: format!("SELECT {i} FROM t WHERE id = {i};"),
            duration_ms: i + 1,
            row_count: i,
            ts_ns: 1_700_000_000_000_000_000_u128 + u128::from(i),
        })
        .collect::<Vec<_>>();
    w.state_mut().apply_history(records);
    w.state_mut().set_right_pane(RightPane::History);
    w.state_mut().history.select_next(); // highlight record 2
    let s = render_to_string(&w, 100, 24);
    insta::assert_snapshot!("database_history_focused", s);
}
