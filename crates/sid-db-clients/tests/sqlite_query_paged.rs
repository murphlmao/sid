use std::sync::Arc;

use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams, PageCursor};
use sid_db_clients::SqliteClient;

async fn seed(rows: u32) -> Arc<dyn DbClient> {
    let c = SqliteClient::factory()
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: ":memory:".into(),
            password: None,
        })
        .await
        .unwrap();
    c.execute("CREATE TABLE t (id INTEGER, label TEXT)")
        .await
        .unwrap();
    if rows > 0 {
        let v = (0..rows)
            .map(|i| format!("({i}, 'r{i}')"))
            .collect::<Vec<_>>()
            .join(",");
        c.execute(&format!("INSERT INTO t VALUES {v}"))
            .await
            .unwrap();
    }
    c
}

#[tokio::test]
async fn first_page_returns_columns_and_first_n_rows() {
    let c = seed(120).await;
    let page = c
        .query_paged("SELECT id, label FROM t ORDER BY id", None, 50)
        .await
        .unwrap();
    assert_eq!(page.columns.len(), 2);
    assert_eq!(page.columns[0].name, "id");
    assert_eq!(page.columns[1].name, "label");
    assert_eq!(page.rows.len(), 50);
    assert_eq!(page.rows[0].values[0], "0");
    assert_eq!(page.next_cursor, Some(PageCursor { offset: 50 }));
}

#[tokio::test]
async fn second_page_continues() {
    let c = seed(120).await;
    let p1 = c
        .query_paged("SELECT id, label FROM t ORDER BY id", None, 50)
        .await
        .unwrap();
    let p2 = c
        .query_paged("SELECT id, label FROM t ORDER BY id", p1.next_cursor, 50)
        .await
        .unwrap();
    assert_eq!(p2.rows.len(), 50);
    assert_eq!(p2.rows[0].values[0], "50");
}

#[tokio::test]
async fn last_partial_page_yields_no_next_cursor() {
    let c = seed(120).await;
    let p3 = c
        .query_paged(
            "SELECT id, label FROM t ORDER BY id",
            Some(PageCursor { offset: 100 }),
            50,
        )
        .await
        .unwrap();
    assert_eq!(p3.rows.len(), 20);
    assert!(p3.next_cursor.is_none());
}

#[tokio::test]
async fn empty_result_returns_no_rows_no_cursor() {
    let c = seed(0).await;
    let p = c
        .query_paged("SELECT id, label FROM t", None, 50)
        .await
        .unwrap();
    assert!(p.rows.is_empty());
    assert!(p.next_cursor.is_none());
}

#[tokio::test]
async fn null_value_renders_as_null_string() {
    let c = seed(0).await;
    c.execute("INSERT INTO t VALUES (1, NULL)").await.unwrap();
    let p = c
        .query_paged("SELECT id, label FROM t", None, 50)
        .await
        .unwrap();
    assert_eq!(p.rows[0].values[1], "NULL");
}

#[tokio::test]
async fn syntax_error_returns_error() {
    let c = seed(0).await;
    let res = c.query_paged("SELEC * FROM t", None, 50).await;
    let err = match res {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    let _ = format!("{err}");
}

#[tokio::test]
async fn page_size_one_yields_one_row_per_page() {
    let c = seed(3).await;
    let p = c
        .query_paged("SELECT id, label FROM t ORDER BY id", None, 1)
        .await
        .unwrap();
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.next_cursor, Some(PageCursor { offset: 1 }));
}

#[tokio::test]
async fn unicode_text_round_trips() {
    let c = seed(0).await;
    c.execute("INSERT INTO t VALUES (1, '\u{1f415} hello \u{4f60}\u{597d}')")
        .await
        .unwrap();
    let p = c
        .query_paged("SELECT label FROM t", None, 50)
        .await
        .unwrap();
    assert_eq!(p.rows[0].values[0], "\u{1f415} hello \u{4f60}\u{597d}");
}

#[tokio::test]
async fn blob_renders_as_hex() {
    let c = seed(0).await;
    c.execute("INSERT INTO t VALUES (1, X'DEADBEEF')")
        .await
        .unwrap();
    let p = c
        .query_paged("SELECT label FROM t", None, 50)
        .await
        .unwrap();
    assert!(p.rows[0].values[0].starts_with("0x"));
    assert!(p.rows[0].values[0].to_lowercase().contains("deadbeef"));
}
