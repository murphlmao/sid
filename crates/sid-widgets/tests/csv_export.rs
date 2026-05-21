use sid_core::adapters::db_client::{Column, ColumnType, QueryPage, Row};
use sid_widgets::csv_export::write_page_csv;

#[test]
fn writes_headers_and_rows() {
    let page = QueryPage {
        columns: vec![
            Column {
                name: "id".into(),
                ty: ColumnType::Integer,
            },
            Column {
                name: "name".into(),
                ty: ColumnType::Text,
            },
        ],
        rows: vec![
            Row {
                values: vec!["1".into(), "alpha".into()],
            },
            Row {
                values: vec!["2".into(), "be,ta".into()],
            },
        ],
        next_cursor: None,
        duration_ms: 0,
    };
    let mut buf = Vec::new();
    write_page_csv(&page, &mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.starts_with("id,name\n"));
    assert!(s.contains("\"be,ta\""));
}

#[test]
fn empty_page_writes_header_only() {
    let page = QueryPage {
        columns: vec![Column {
            name: "id".into(),
            ty: ColumnType::Integer,
        }],
        rows: vec![],
        next_cursor: None,
        duration_ms: 0,
    };
    let mut buf = Vec::new();
    write_page_csv(&page, &mut buf).unwrap();
    assert_eq!(String::from_utf8(buf).unwrap(), "id\n");
}

#[test]
fn csv_quotes_newline_containing_cell() {
    let p = QueryPage {
        columns: vec![Column {
            name: "c".into(),
            ty: ColumnType::Text,
        }],
        rows: vec![Row {
            values: vec!["line1\nline2".into()],
        }],
        next_cursor: None,
        duration_ms: 0,
    };
    let mut buf = Vec::new();
    write_page_csv(&p, &mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("\"line1\nline2\""));
}
