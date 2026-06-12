use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::context::WidgetCtx;
use sid_core::event::{Event, KeyChord};
use sid_core::widget::Widget;
use sid_widgets::SshWidget;
use sid_widgets::ssh::SshFocus;

fn ctx() -> WidgetCtx {
    let (tx, _rx) = mpsc::channel();
    WidgetCtx::new(tx)
}

fn key(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyChord::new(code, mods))
}

#[test]
fn ssh_widget_has_expected_id_and_title() {
    let w = SshWidget::new();
    assert_eq!(w.id().as_str(), "ssh.root");
    assert_eq!(w.title(), "SSH");
}

#[test]
fn ssh_widget_default_matches_new() {
    let a = SshWidget::new();
    let b = SshWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn ssh_save_state_returns_empty() {
    let w = SshWidget::new();
    assert!(w.save_state().is_empty());
}

#[test]
fn ssh_load_state_is_noop() {
    let mut w = SshWidget::new();
    w.load_state(&[0xCA, 0xFE, 0xBA, 0xBE]);
    assert_eq!(w.id().as_str(), "ssh.root");
}

#[test]
fn ssh_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(SshWidget::new());
    assert_eq!(w.id().as_str(), "ssh.root");
    assert_eq!(w.title(), "SSH");
}

#[test]
fn ssh_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SshWidget>();
}

// ---------------------------------------------------------------------------
// Strict pane-focus model tests
// ---------------------------------------------------------------------------

#[test]
fn ssh_default_focus_is_hosts() {
    let w = SshWidget::new();
    assert_eq!(w.focused_pane(), SshFocus::Hosts);
    assert_eq!(w.focused_pane_label(), "Hosts");
}

#[test]
fn ssh_tab_cycles_focus_forward() {
    let mut w = SshWidget::new();
    let mut c = ctx();
    assert_eq!(w.focused_pane(), SshFocus::Hosts);
    w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
    assert_eq!(w.focused_pane(), SshFocus::Detail);
    w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
    assert_eq!(w.focused_pane(), SshFocus::Hosts);
}

#[test]
fn ssh_shift_tab_cycles_focus_backward() {
    let mut w = SshWidget::new();
    let mut c = ctx();
    // 2-way: Shift+Tab from Hosts → Detail (and vice versa).
    w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
    assert_eq!(w.focused_pane(), SshFocus::Detail);
    w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
    assert_eq!(w.focused_pane(), SshFocus::Hosts);
}

#[test]
fn ssh_j_only_acts_on_focused_pane() {
    use sid_store::{SshHost, SshHostSource};
    let hosts = vec![
        SshHost {
            alias: "alpha".into(),
            host: "a".into(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: vec![],
            last_sftp_path: None,
            auth_kind: sid_store::SshAuthKind::Agent,
        },
        SshHost {
            alias: "bravo".into(),
            host: "b".into(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: vec![],
            last_sftp_path: None,
            auth_kind: sid_store::SshAuthKind::Agent,
        },
    ];
    let state = sid_widgets::ssh::SshState::new(hosts, vec![], false);
    let mut w = SshWidget::with_state(state);
    let mut c = ctx();
    assert_eq!(w.state().selected_alias(), Some("alpha"));
    // Focus right pane: j must NOT advance host selection.
    w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
    assert_eq!(w.focused_pane(), SshFocus::Detail);
    w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
    assert_eq!(w.state().selected_alias(), Some("alpha"));
    // Refocus Hosts: now j advances.
    w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
    assert_eq!(w.focused_pane(), SshFocus::Hosts);
    w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
    assert_eq!(w.state().selected_alias(), Some("bravo"));
}

#[test]
fn ssh_border_follows_focus() {
    let mut w = SshWidget::new();
    let mut c = ctx();
    assert_eq!(w.focused_pane_label(), "Hosts");
    w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
    assert_eq!(w.focused_pane_label(), "Detail");
    w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
    assert_eq!(w.focused_pane_label(), "Hosts");
}

// ---------------------------------------------------------------------------
// focus_at — mouse-click pane routing
// ---------------------------------------------------------------------------

#[test]
fn focus_at_top_left_focuses_hosts() {
    use ratatui::layout::Rect;
    let mut w = SshWidget::new();
    // Pre-flip focus so we can prove `focus_at` mutates back to Hosts.
    w.focus_next();
    assert_eq!(w.focused_pane(), SshFocus::Detail);
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 24,
    };
    // Click well inside the left pane (col 5 of a 40-wide left pane).
    w.focus_at(area, 5, 3);
    assert_eq!(w.focused_pane(), SshFocus::Hosts);
}

#[test]
fn focus_at_top_right_focuses_detail() {
    use ratatui::layout::Rect;
    let mut w = SshWidget::new();
    assert_eq!(w.focused_pane(), SshFocus::Hosts);
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 24,
    };
    // Click well inside the right pane (col 80; right pane starts at col 40).
    w.focus_at(area, 80, 3);
    assert_eq!(w.focused_pane(), SshFocus::Detail);
}

#[test]
fn focus_at_outside_area_is_noop() {
    use ratatui::layout::Rect;
    let mut w = SshWidget::new();
    let area = Rect {
        x: 10,
        y: 10,
        width: 50,
        height: 20,
    };
    let original = w.focused_pane();
    w.focus_at(area, 5, 5); // left of area.x
    assert_eq!(w.focused_pane(), original);
    w.focus_at(area, 200, 5); // right of area.right()
    assert_eq!(w.focused_pane(), original);
}

// ---------------------------------------------------------------------------
// pending_connect — Enter on a host in the Hosts pane queues a connect intent
// ---------------------------------------------------------------------------

fn host_for(alias: &str) -> sid_store::SshHost {
    sid_store::SshHost {
        alias: alias.into(),
        host: format!("{alias}.example"),
        port: 22,
        user: "u".into(),
        identity_file: None,
        source: sid_store::SshHostSource::Manual,
        last_connected: 0,
        command_history: vec![],
        last_sftp_path: None,
        auth_kind: sid_store::SshAuthKind::Agent,
    }
}

#[test]
fn enter_on_host_sets_pending_connect_and_marks_connecting() {
    use sid_widgets::ssh::ConnectionPhase;
    let state =
        sid_widgets::ssh::SshState::new(vec![host_for("alpha"), host_for("bravo")], vec![], false);
    let mut w = SshWidget::with_state(state);
    let mut c = ctx();
    assert_eq!(w.focused_pane(), SshFocus::Hosts);
    assert_eq!(w.connection().phase(), ConnectionPhase::Idle);
    assert!(w.peek_pending_connect().is_none());

    w.handle_event(&key(KeyCode::Enter, KeyModifiers::NONE), &mut c);

    assert_eq!(w.connection().phase(), ConnectionPhase::Connecting);
    assert_eq!(w.connection().alias(), Some("alpha"));
    assert_eq!(w.peek_pending_connect(), Some("alpha"));

    let taken = w.take_pending_connect();
    assert_eq!(taken.as_deref(), Some("alpha"));
    // Take is destructive — the second drain sees None.
    assert!(w.peek_pending_connect().is_none());
    assert!(w.take_pending_connect().is_none());
    // But the connection state stays Connecting until the wire layer flips
    // it after the connect future resolves.
    assert_eq!(w.connection().phase(), ConnectionPhase::Connecting);
}

#[test]
fn enter_on_empty_host_list_is_a_noop() {
    let mut w = SshWidget::new();
    let mut c = ctx();
    w.handle_event(&key(KeyCode::Enter, KeyModifiers::NONE), &mut c);
    assert!(w.peek_pending_connect().is_none());
}

#[test]
fn enter_when_detail_focused_does_not_queue_a_connect() {
    let state = sid_widgets::ssh::SshState::new(vec![host_for("alpha")], vec![], false);
    let mut w = SshWidget::with_state(state);
    let mut c = ctx();
    w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
    assert_eq!(w.focused_pane(), SshFocus::Detail);
    w.handle_event(&key(KeyCode::Enter, KeyModifiers::NONE), &mut c);
    assert!(w.peek_pending_connect().is_none());
}

#[test]
fn set_pending_connect_seed_then_drain() {
    let mut w = SshWidget::new();
    assert!(w.take_pending_connect().is_none());
    w.set_pending_connect(Some("forced".into()));
    assert_eq!(w.peek_pending_connect(), Some("forced"));
    assert_eq!(w.take_pending_connect().as_deref(), Some("forced"));
    assert!(w.peek_pending_connect().is_none());
    // Clearing back to None is also fine.
    w.set_pending_connect(Some("again".into()));
    w.set_pending_connect(None);
    assert!(w.take_pending_connect().is_none());
}
