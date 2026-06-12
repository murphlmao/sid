//! Database widget state tests — covers Tasks 21-26 and 28.

use sid_core::adapters::db_client::{Column, ColumnType, DbKind, PageCursor, QueryPage, Row};
use sid_store::{DbConnection, QueryRecord};
use sid_widgets::database::{DatabaseState, EditorState, HistoryState, ResultsState, RightPane};

fn conn(id: &str, k: DbKind) -> DbConnection {
    DbConnection {
        id: id.into(),
        kind: k,
        name: id.into(),
        dsn: ":memory:".into(),
        secret_ref: None,
        created_at: 0,
    }
}

// Task 21: connection-list state.
// UX-v2: DatabaseState::new() now defaults to add_new=true, so the cursor
// starts on the synthetic +add new row, not the first connection.
#[test]
fn new_state_starts_on_add_new_row() {
    let s = DatabaseState::new(vec![conn("a", DbKind::Sqlite), conn("b", DbKind::Postgres)]);
    // With add_new=true, cursor at AddNew — selected_connection() is None.
    assert!(s.selected_connection().is_none());
    assert!(s.is_add_new_selected());
}

// Using new_with_add_new(…, false) gives the old Item(0) start behavior.
#[test]
fn new_state_no_add_new_selects_first_connection() {
    let s = DatabaseState::new_with_add_new(
        vec![conn("a", DbKind::Sqlite), conn("b", DbKind::Postgres)],
        false,
    );
    assert_eq!(s.selected_connection().unwrap().id, "a");
}

#[test]
fn empty_state_has_no_selection() {
    let s = DatabaseState::new(vec![]);
    assert!(s.selected_connection().is_none());
}

#[test]
fn select_next_and_prev_cycle() {
    // Use add_new=false so navigation starts on Item(0) not AddNew.
    let mut s = DatabaseState::new_with_add_new(
        vec![conn("a", DbKind::Sqlite), conn("b", DbKind::Sqlite)],
        false,
    );
    assert_eq!(s.selected_connection().unwrap().id, "a");
    s.select_next();
    assert_eq!(s.selected_connection().unwrap().id, "b");
    // wrap from last back to first
    s.select_next();
    assert_eq!(s.selected_connection().unwrap().id, "a");
    s.select_prev();
    assert_eq!(s.selected_connection().unwrap().id, "b");
}

#[test]
fn right_pane_tab_cycles_editor_results_history() {
    let mut s = DatabaseState::new(vec![conn("a", DbKind::Sqlite)]);
    assert_eq!(s.right_pane(), RightPane::Editor);
    s.cycle_right_pane();
    assert_eq!(s.right_pane(), RightPane::Results);
    s.cycle_right_pane();
    assert_eq!(s.right_pane(), RightPane::History);
    s.cycle_right_pane();
    assert_eq!(s.right_pane(), RightPane::Editor);
}

// Task 22: editor state.
#[test]
fn empty_editor_has_one_blank_line() {
    let e = EditorState::default_blank();
    assert_eq!(e.lines, vec![String::new()]);
    assert_eq!(e.cursor_line, 0);
    assert_eq!(e.cursor_col, 0);
}

#[test]
fn insert_char_appends() {
    let mut e = EditorState::default_blank();
    e.insert_char('S');
    e.insert_char('E');
    assert_eq!(e.lines[0], "SE");
    assert_eq!(e.cursor_col, 2);
}

#[test]
fn newline_splits_line() {
    let mut e = EditorState::default_blank();
    e.insert_char('A');
    e.insert_newline();
    e.insert_char('B');
    assert_eq!(e.lines, vec!["A".to_string(), "B".to_string()]);
    assert_eq!(e.cursor_line, 1);
    assert_eq!(e.cursor_col, 1);
}

#[test]
fn backspace_at_line_start_joins_lines() {
    let mut e = EditorState::default_blank();
    e.insert_char('A');
    e.insert_newline();
    e.insert_char('B');
    e.move_cursor_to(1, 0);
    e.backspace();
    assert_eq!(e.lines, vec!["AB".to_string()]);
    assert_eq!(e.cursor_line, 0);
    assert_eq!(e.cursor_col, 1);
}

#[test]
fn full_source_returns_joined_text() {
    let mut e = EditorState::default_blank();
    e.insert_char('A');
    e.insert_newline();
    e.insert_char('B');
    assert_eq!(e.full_source(), "A\nB");
}

#[test]
fn tokens_for_current_source_classifies_keywords() {
    use sid_db_clients::lexer::TokenKind;
    let mut e = EditorState::default_blank();
    for c in "SELECT 1".chars() {
        e.insert_char(c);
    }
    let toks = e.tokens();
    assert!(toks.iter().any(|t| t.kind == TokenKind::Keyword));
    assert!(toks.iter().any(|t| t.kind == TokenKind::Number));
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_insert_then_full_source_roundtrips(s in "[a-zA-Z0-9 ;\n]{0,200}") {
        let mut e = EditorState::default_blank();
        for c in s.chars() {
            if c == '\n' { e.insert_newline(); } else { e.insert_char(c); }
        }
        prop_assert_eq!(e.full_source(), s);
    }
}

// Task 23: results state.
fn page(rows: Vec<Vec<&str>>, next: Option<u64>) -> QueryPage {
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
        ],
        rows: rows
            .into_iter()
            .map(|r| Row {
                values: r.into_iter().map(String::from).collect(),
            })
            .collect(),
        next_cursor: next.map(|o| PageCursor { offset: o }),
        duration_ms: 1,
    }
}

#[test]
fn sets_page_and_initial_selection() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["1", "a"], vec!["2", "b"]], None));
    assert_eq!(s.selected_row, 0);
    assert_eq!(s.selected_col, 0);
}

#[test]
fn next_row_advances() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["1", "a"], vec!["2", "b"]], None));
    s.select_next_row();
    assert_eq!(s.selected_row, 1);
}

#[test]
fn sort_toggle_flips_order() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["2", "b"], vec!["1", "a"]], None));
    s.toggle_sort(0);
    let rows: Vec<_> = s
        .page
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.values[0].clone())
        .collect();
    assert_eq!(rows, vec!["1", "2"]);
    s.toggle_sort(0);
    let rows: Vec<_> = s
        .page
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.values[0].clone())
        .collect();
    assert_eq!(rows, vec!["2", "1"]);
}

#[test]
fn selected_cell_returns_value() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["1", "alpha"], vec!["2", "beta"]], None));
    s.select_next_row();
    s.select_next_col();
    assert_eq!(s.selected_cell(), Some("beta"));
}

#[test]
fn select_on_empty_page_is_noop() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![], None));
    s.select_next_row();
    s.select_next_col();
    assert!(s.selected_cell().is_none());
}

#[test]
fn append_page_extends_rows() {
    let mut s = ResultsState::default();
    s.set_page(page(vec![vec!["1", "a"]], Some(1)));
    s.append_page(page(vec![vec!["2", "b"]], None));
    assert_eq!(s.page.as_ref().unwrap().rows.len(), 2);
    assert!(s.page.as_ref().unwrap().next_cursor.is_none());
}

// Task 24: history state.
fn rec(sql: &str, ts: u128) -> QueryRecord {
    QueryRecord {
        conn_id: "c".into(),
        sql: sql.into(),
        duration_ms: 1,
        row_count: 0,
        ts_ns: ts,
    }
}

#[test]
fn set_records_resets_selection() {
    let mut s = HistoryState::default();
    s.set_records(vec![rec("Q1", 1), rec("Q2", 2)]);
    assert_eq!(s.selected, 0);
}

#[test]
fn navigation_wraps() {
    let mut s = HistoryState::default();
    s.set_records(vec![rec("Q1", 1), rec("Q2", 2)]);
    s.select_next();
    assert_eq!(s.selected, 1);
    s.select_next();
    assert_eq!(s.selected, 0);
    s.select_prev();
    assert_eq!(s.selected, 1);
}

#[test]
fn selected_record_returns_current() {
    let mut s = HistoryState::default();
    s.set_records(vec![rec("Q1", 1), rec("Q2", 2)]);
    assert_eq!(s.selected_record().unwrap().sql, "Q1");
}

// Task 25: apply_query_result swaps right pane.
#[test]
fn apply_query_result_swaps_right_pane_to_results() {
    let mut s = DatabaseState::new(vec![]);
    let p = QueryPage {
        columns: vec![],
        rows: vec![],
        next_cursor: None,
        duration_ms: 0,
    };
    s.apply_query_result(p, None);
    assert_eq!(s.right_pane(), RightPane::Results);
}
