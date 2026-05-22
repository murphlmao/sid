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
        },
    ];
    let state = sid_widgets::ssh::SshState::new(hosts, vec![]);
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
