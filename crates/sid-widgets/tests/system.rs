use sid_core::widget::Widget;
use sid_widgets::SystemWidget;

#[test]
fn system_widget_has_expected_id_and_title() {
    let w = SystemWidget::new();
    assert_eq!(w.id().as_str(), "system.root");
    assert_eq!(w.title(), "System");
}

#[test]
fn system_widget_default_matches_new() {
    let a = SystemWidget::new();
    let b = SystemWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn system_save_state_returns_empty() {
    let w = SystemWidget::new();
    assert!(w.save_state().is_empty());
}

#[test]
fn system_load_state_is_noop() {
    let mut w = SystemWidget::new();
    w.load_state(&[0xFF, 0xFE, 0xFD]);
    assert_eq!(w.id().as_str(), "system.root");
}

#[test]
fn system_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(SystemWidget::new());
    assert_eq!(w.id().as_str(), "system.root");
    assert_eq!(w.title(), "System");
}

#[test]
fn system_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SystemWidget>();
}

// ---------------------------------------------------------------------------
// Strict pane-focus model tests
// ---------------------------------------------------------------------------

mod focus {
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::context::WidgetCtx;
    use sid_core::event::{Event, KeyChord};
    use sid_core::widget::Widget;
    use sid_widgets::SystemWidget;
    use sid_widgets::system::SystemPane;

    fn ctx() -> WidgetCtx {
        let (tx, _rx) = mpsc::channel();
        WidgetCtx::new(tx)
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyChord::new(code, mods))
    }

    #[test]
    fn default_focus_is_pinned_configs() {
        let w = SystemWidget::new();
        assert_eq!(w.state().focused_pane(), SystemPane::PinnedConfigs);
        assert_eq!(w.state().focused_pane_label(), "PinnedConfigs");
    }

    #[test]
    fn tab_cycles_focus_forward() {
        let mut w = SystemWidget::new();
        let mut c = ctx();
        let order = [
            SystemPane::Services,
            SystemPane::QuickActions,
            SystemPane::PinnedConfigs,
        ];
        for expected in order {
            w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
            assert_eq!(w.state().focused_pane(), expected);
        }
    }

    #[test]
    fn shift_tab_cycles_focus_backward() {
        let mut w = SystemWidget::new();
        let mut c = ctx();
        let order = [
            SystemPane::QuickActions,
            SystemPane::Services,
            SystemPane::PinnedConfigs,
        ];
        for expected in order {
            w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
            assert_eq!(w.state().focused_pane(), expected);
        }
    }

    #[test]
    fn j_only_acts_on_focused_pane() {
        // System routes per-pane navigation through the binary's wire
        // layer; the widget itself bubbles j/k. What we verify here is
        // that the focused-pane marker is consistent across Tab events,
        // so any subsequent dispatch keys on the right pane.
        let mut w = SystemWidget::new();
        let mut c = ctx();
        let before = w.state().focused_pane();
        let outcome = w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        // j is widget-level bubbled (binary handles it); focused pane is
        // unchanged regardless.
        assert_eq!(outcome, sid_core::widget::EventOutcome::Bubble);
        assert_eq!(w.state().focused_pane(), before);
    }

    #[test]
    fn border_follows_focus() {
        let mut w = SystemWidget::new();
        let mut c = ctx();
        assert_eq!(w.state().focused_pane_label(), "PinnedConfigs");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().focused_pane_label(), "Services");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.state().focused_pane_label(), "QuickActions");
    }

    #[test]
    fn alt_key_does_not_change_focus() {
        let mut w = SystemWidget::new();
        let mut c = ctx();
        let before = w.state().focused_pane();
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::ALT), &mut c);
        assert_eq!(w.state().focused_pane(), before);
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::ALT), &mut c);
        // Tab with Alt is also reserved and shouldn't cycle focus.
        // Currently the existing Tab match has KeyCode::Tab, KeyModifiers::NONE;
        // Alt+Tab falls through. So focus stays.
        assert_eq!(w.state().focused_pane(), before);
    }

    // -----------------------------------------------------------------------
    // focus_at — mouse-click pane routing (no-op for SystemWidget since only
    // one pane is rendered at a time).
    // -----------------------------------------------------------------------

    #[test]
    fn focus_at_top_left_does_not_change_focus() {
        use ratatui::layout::Rect;
        let mut w = SystemWidget::new();
        let before = w.state().focused_pane();
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        w.focus_at(area, 5, 5);
        assert_eq!(w.state().focused_pane(), before);
    }

    #[test]
    fn focus_at_top_right_does_not_change_focus() {
        use ratatui::layout::Rect;
        let mut w = SystemWidget::new();
        let before = w.state().focused_pane();
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        w.focus_at(area, 80, 5);
        assert_eq!(w.state().focused_pane(), before);
    }
}
