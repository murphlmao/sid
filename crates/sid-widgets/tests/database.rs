use sid_core::widget::Widget;
use sid_widgets::DatabaseWidget;

#[test]
fn database_widget_has_expected_id_and_title() {
    let w = DatabaseWidget::new(vec![]);
    assert_eq!(w.id().as_str(), "database.root");
    assert_eq!(w.title(), "Database");
}

#[test]
fn database_widget_default_matches_new() {
    let a = DatabaseWidget::new(vec![]);
    let b = DatabaseWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn database_save_state_returns_empty() {
    let w = DatabaseWidget::new(vec![]);
    assert!(w.save_state().is_empty());
}

#[test]
fn database_load_state_is_noop() {
    let mut w = DatabaseWidget::new(vec![]);
    w.load_state(&[0x01, 0x02, 0x03]);
    assert_eq!(w.id().as_str(), "database.root");
}

#[test]
fn database_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(DatabaseWidget::new(vec![]));
    assert_eq!(w.id().as_str(), "database.root");
    assert_eq!(w.title(), "Database");
}

#[test]
fn database_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DatabaseWidget>();
}

// ---------------------------------------------------------------------------
// Strict pane-focus model tests
// ---------------------------------------------------------------------------

mod focus {
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::adapters::db_client::{Column, ColumnType, DbKind, QueryPage, Row};
    use sid_core::context::WidgetCtx;
    use sid_core::event::{Event, KeyChord};
    use sid_core::widget::Widget;
    use sid_store::DbConnection;
    use sid_widgets::DatabaseWidget;
    use sid_widgets::database::{DbFocus, RightPane};
    use sid_widgets::list_cursor::CursorTarget;

    fn ctx() -> WidgetCtx {
        let (tx, _rx) = mpsc::channel();
        WidgetCtx::new(tx)
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyChord::new(code, mods))
    }

    fn conn(id: &str) -> DbConnection {
        DbConnection {
            id: id.into(),
            kind: DbKind::Sqlite,
            name: id.into(),
            dsn: ":memory:".into(),
            secret_ref: None,
            created_at: 0,
        }
    }

    fn results_page() -> QueryPage {
        QueryPage {
            columns: vec![Column {
                name: "x".into(),
                ty: ColumnType::Text,
            }],
            rows: vec![
                Row {
                    values: vec!["1".into()],
                },
                Row {
                    values: vec!["2".into()],
                },
            ],
            next_cursor: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn default_focus_is_connections() {
        let w = DatabaseWidget::new(vec![]);
        assert_eq!(w.focused_pane(), DbFocus::Connections);
        assert_eq!(w.focused_pane_label(), "Connections");
    }

    #[test]
    fn tab_cycles_focus_forward() {
        let mut w = DatabaseWidget::new(vec![]);
        let mut c = ctx();
        let order = [
            DbFocus::Editor,
            DbFocus::Results,
            DbFocus::History,
            DbFocus::Connections,
        ];
        for expected in order {
            w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
            assert_eq!(w.focused_pane(), expected);
        }
    }

    #[test]
    fn shift_tab_cycles_focus_backward() {
        let mut w = DatabaseWidget::new(vec![]);
        let mut c = ctx();
        let order = [
            DbFocus::History,
            DbFocus::Results,
            DbFocus::Editor,
            DbFocus::Connections,
        ];
        for expected in order {
            w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
            assert_eq!(w.focused_pane(), expected);
        }
    }

    #[test]
    fn tab_syncs_right_pane_with_focus() {
        let mut w = DatabaseWidget::new(vec![]);
        let mut c = ctx();
        // Connections → Editor: RightPane should switch to Editor.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().right_pane(), RightPane::Editor);
        // Editor → Results.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().right_pane(), RightPane::Results);
        // Results → History.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().right_pane(), RightPane::History);
    }

    #[test]
    fn j_only_acts_on_focused_pane() {
        // Use new_with_add_new(…, false) so the cursor starts on Item(0) not +AddNew.
        let mut w = DatabaseWidget::new_with_add_new(vec![conn("a"), conn("b")], false);
        let mut c = ctx();
        w.set_results_for_tests(results_page());
        // Focus is Connections. j should advance connection selection.
        assert_eq!(w.state().cursor.target(), CursorTarget::Item(0));
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().cursor.target(), CursorTarget::Item(1));
        // Tab to Editor; j must not advance connection selection.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), DbFocus::Editor);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().cursor.target(), CursorTarget::Item(1));
        // Tab to Results; j moves the result row, not the connection.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), DbFocus::Results);
        assert_eq!(w.state().results.selected_row, 0);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().results.selected_row, 1);
        assert_eq!(w.state().cursor.target(), CursorTarget::Item(1)); // unchanged
        // Tab to History; j must not move the connection list either.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), DbFocus::History);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().cursor.target(), CursorTarget::Item(1));
    }

    #[test]
    fn border_follows_focus() {
        let mut w = DatabaseWidget::new(vec![]);
        let mut c = ctx();
        assert_eq!(w.focused_pane_label(), "Connections");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "Editor");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "Results");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "History");
    }

    // -----------------------------------------------------------------------
    // focus_at — mouse-click pane routing
    // -----------------------------------------------------------------------

    #[test]
    fn focus_at_top_left_focuses_connections() {
        use ratatui::layout::Rect;
        let mut w = DatabaseWidget::new(vec![conn("c1")]);
        // Pre-flip focus so we can prove `focus_at` mutates back to Connections.
        w.focus_next();
        assert_eq!(w.focused_pane(), DbFocus::Editor);
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        // Click well inside the left 30% pane (col 5 of a 30-wide left pane).
        w.focus_at(area, 5, 5);
        assert_eq!(w.focused_pane(), DbFocus::Connections);
    }

    #[test]
    fn focus_at_top_right_focuses_editor() {
        use ratatui::layout::Rect;
        let mut w = DatabaseWidget::new(vec![]);
        assert_eq!(w.focused_pane(), DbFocus::Connections);
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        // Top-right region (col 70 of right pane; top 30% of height = rows 0..12).
        w.focus_at(area, 70, 2);
        assert_eq!(w.focused_pane(), DbFocus::Editor);
    }

    #[test]
    fn focus_at_middle_right_focuses_results_or_history() {
        use ratatui::layout::Rect;
        let mut w = DatabaseWidget::new(vec![]);
        // Default RightPane is Editor; focus_at on middle still picks Results
        // because Results is the default middle-pane focus.
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        // Row 20 is below the editor band (12 rows) and above the bottom status
        // row, so it lands on the middle pane.
        w.focus_at(area, 70, 20);
        assert_eq!(w.focused_pane(), DbFocus::Results);

        // Now flip the visible RightPane to History; focus_at should follow.
        w.state_mut().set_right_pane(RightPane::History);
        w.focus_at(area, 70, 20);
        assert_eq!(w.focused_pane(), DbFocus::History);
    }
}
