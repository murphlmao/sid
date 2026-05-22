use sid_core::widget::Widget;
use sid_widgets::WorkspacesWidget;

#[test]
fn workspaces_widget_has_expected_id_and_title() {
    let w = WorkspacesWidget::new(vec![], None);
    assert_eq!(w.id().as_str(), "workspaces.root");
    assert_eq!(w.title(), "Workspaces");
}

#[test]
fn workspaces_widget_default_matches_new() {
    let a = WorkspacesWidget::new(vec![], None);
    let b = WorkspacesWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn workspaces_save_state_returns_empty() {
    let w = WorkspacesWidget::new(vec![], None);
    assert!(w.save_state().is_empty());
}

#[test]
fn workspaces_load_state_is_noop() {
    let mut w = WorkspacesWidget::new(vec![], None);
    w.load_state(&[0xDE, 0xAD, 0xBE, 0xEF]); // arbitrary bytes — must not panic
    assert_eq!(w.id().as_str(), "workspaces.root");
}

#[test]
fn workspaces_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(WorkspacesWidget::new(vec![], None));
    assert_eq!(w.id().as_str(), "workspaces.root");
    assert_eq!(w.title(), "Workspaces");
}

/// Compile-time assertion: WorkspacesWidget implements Send + Sync.
#[test]
fn workspaces_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WorkspacesWidget>();
}

// ---------------------------------------------------------------------------
// Strict pane-focus model tests
// ---------------------------------------------------------------------------

mod focus {
    use std::path::PathBuf;
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::context::WidgetCtx;
    use sid_core::event::{Event, KeyChord};
    use sid_core::widget::Widget;
    use sid_core::workspace_metadata::WorkspaceKind;
    use sid_store::Workspace;
    use sid_widgets::WorkspacesWidget;
    use sid_widgets::workspaces::WsFocus;

    fn ctx() -> WidgetCtx {
        let (tx, _rx) = mpsc::channel();
        WidgetCtx::new(tx)
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyChord::new(code, mods))
    }

    fn ws(path: &str) -> Workspace {
        Workspace {
            path: PathBuf::from(path),
            name: path.trim_start_matches('/').to_string(),
            kind: WorkspaceKind::Repo,
            manifest_hash: 0,
            last_seen: 0,
            parent: None,
        }
    }

    #[test]
    fn default_focus_is_tree() {
        let w = WorkspacesWidget::new(vec![], None);
        assert_eq!(w.focused_pane(), WsFocus::Tree);
        assert_eq!(w.focused_pane_label(), "Tree");
    }

    #[test]
    fn tab_cycles_focus_forward() {
        let mut w = WorkspacesWidget::new(vec![], None);
        let mut c = ctx();
        assert_eq!(w.focused_pane(), WsFocus::Tree);
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), WsFocus::SubView);
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), WsFocus::Tree);
    }

    #[test]
    fn shift_tab_cycles_focus_backward() {
        let mut w = WorkspacesWidget::new(vec![], None);
        let mut c = ctx();
        w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
        assert_eq!(w.focused_pane(), WsFocus::SubView);
        w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
        assert_eq!(w.focused_pane(), WsFocus::Tree);
    }

    #[test]
    fn j_only_acts_on_focused_pane() {
        let mut w = WorkspacesWidget::new(vec![ws("/a"), ws("/b")], None);
        let mut c = ctx();
        // Tree focused: j advances the tree selection.
        assert_eq!(w.state().selected_path().unwrap(), PathBuf::from("/a"));
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().selected_path().unwrap(), PathBuf::from("/b"));
        // Tab to SubView — j must NOT advance the tree selection.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), WsFocus::SubView);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().selected_path().unwrap(), PathBuf::from("/b"));
    }

    #[test]
    fn border_follows_focus() {
        let mut w = WorkspacesWidget::new(vec![], None);
        let mut c = ctx();
        assert_eq!(w.focused_pane_label(), "Tree");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "SubView");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "Tree");
    }

    #[test]
    fn r_opens_actions_menu_regardless_of_focus() {
        use sid_widgets::workspaces::RightPane;
        let mut w = WorkspacesWidget::new(vec![ws("/a")], None);
        let mut c = ctx();
        // Switch focus to SubView and press 'r'. The Actions menu must open
        // (widget-global keybind).
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), WsFocus::SubView);
        w.handle_event(&key(KeyCode::Char('r'), KeyModifiers::NONE), &mut c);
        assert!(matches!(w.state().right_pane(), RightPane::Actions(_)));
    }
}
