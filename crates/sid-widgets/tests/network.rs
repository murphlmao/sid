use sid_core::widget::Widget;
use sid_widgets::NetworkWidget;

#[test]
fn network_widget_has_expected_id_and_title() {
    let w = NetworkWidget::new();
    assert_eq!(w.id().as_str(), "network.root");
    assert_eq!(w.title(), "Network");
}

#[test]
fn network_widget_default_matches_new() {
    let a = NetworkWidget::new();
    let b = NetworkWidget::default();
    assert_eq!(a.id().as_str(), b.id().as_str());
    assert_eq!(a.title(), b.title());
}

#[test]
fn network_save_state_returns_versioned_blob() {
    let w = NetworkWidget::new();
    // Plan 5: save_state now returns a versioned postcard blob of the
    // persisted prefs (focus + sort), no longer empty.
    let bytes = w.save_state();
    assert!(!bytes.is_empty());
    assert_eq!(bytes[0], 1, "version prefix should be 1");
}

#[test]
fn network_load_state_unknown_version_is_noop() {
    let mut w = NetworkWidget::new();
    // Unknown version byte: load must be a silent no-op (forward compat).
    w.load_state(&[0xDE, 0xAD]);
    assert_eq!(w.id().as_str(), "network.root");
}

#[test]
fn network_can_be_boxed_as_dyn_widget() {
    let w: Box<dyn Widget> = Box::new(NetworkWidget::new());
    assert_eq!(w.id().as_str(), "network.root");
    assert_eq!(w.title(), "Network");
}

#[test]
fn network_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NetworkWidget>();
}

// ---------------------------------------------------------------------------
// Strict pane-focus model tests
// ---------------------------------------------------------------------------

mod focus {
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::adapters::sys::{
        ListeningPort, NetInterface, Pid, ProcessInfo, Protocol, SocketState,
    };
    use sid_core::context::WidgetCtx;
    use sid_core::event::{Event, KeyChord};
    use sid_core::sys_probe::SysSnapshot;
    use sid_core::widget::Widget;
    use sid_widgets::NetworkWidget;
    use sid_widgets::network::NetFocus;

    fn ctx() -> WidgetCtx {
        let (tx, _rx) = mpsc::channel();
        WidgetCtx::new(tx)
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyChord::new(code, mods))
    }

    fn snap_two_ports() -> SysSnapshot {
        SysSnapshot {
            listening_ports: vec![
                ListeningPort {
                    port: 22,
                    pid: Some(Pid::from_u32(1234)),
                    command: "sshd".into(),
                    protocol: Protocol::Tcp,
                    state: SocketState::Listen,
                    local_addr: "0.0.0.0".into(),
                },
                ListeningPort {
                    port: 80,
                    pid: Some(Pid::from_u32(5678)),
                    command: "nginx".into(),
                    protocol: Protocol::Tcp,
                    state: SocketState::Listen,
                    local_addr: "0.0.0.0".into(),
                },
            ],
            processes: vec![ProcessInfo {
                pid: Pid::from_u32(1234),
                name: "sshd".into(),
                cmd: "sshd".into(),
                cpu_pct: 0.0,
                rss_bytes: 0,
                started_unix_secs: 0,
                parent: None,
                user: None,
            }],
            interfaces: vec![
                NetInterface {
                    name: "lo".into(),
                    addrs: vec!["127.0.0.1".into()],
                    rx_bytes: 0,
                    tx_bytes: 0,
                    is_up: true,
                },
                NetInterface {
                    name: "eth0".into(),
                    addrs: vec!["10.0.0.1".into()],
                    rx_bytes: 0,
                    tx_bytes: 0,
                    is_up: true,
                },
            ],
            captured_at_unix_secs: 1_700_000_000,
        }
    }

    #[test]
    fn default_focus_is_ports() {
        let w = NetworkWidget::new();
        assert_eq!(w.focused_pane(), NetFocus::Ports);
        assert_eq!(w.focused_pane_label(), "Ports");
    }

    #[test]
    fn tab_cycles_focus_forward() {
        let mut w = NetworkWidget::new();
        let mut c = ctx();
        let order = [NetFocus::Processes, NetFocus::Interfaces, NetFocus::Ports];
        for expected in order {
            w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
            assert_eq!(w.focused_pane(), expected);
        }
    }

    #[test]
    fn shift_tab_cycles_focus_backward() {
        let mut w = NetworkWidget::new();
        let mut c = ctx();
        let order = [NetFocus::Interfaces, NetFocus::Processes, NetFocus::Ports];
        for expected in order {
            w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
            assert_eq!(w.focused_pane(), expected);
        }
    }

    #[test]
    fn j_only_acts_on_focused_pane() {
        let mut w = NetworkWidget::new();
        w.apply_snapshot(snap_two_ports());
        let mut c = ctx();
        // Focus is Ports. j advances the ports selection.
        assert_eq!(w.ports().selected_index(), 0);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.ports().selected_index(), 1);
        // Tab to Processes; j advances processes selection (not ports).
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), NetFocus::Processes);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.ports().selected_index(), 1);
        // Tab to Interfaces; j advances ifs selection.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), NetFocus::Interfaces);
        assert_eq!(w.interfaces().selected_index(), 0);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.interfaces().selected_index(), 1);
        assert_eq!(w.ports().selected_index(), 1);
    }

    #[test]
    fn border_follows_focus() {
        let mut w = NetworkWidget::new();
        let mut c = ctx();
        assert_eq!(w.focused_pane_label(), "Ports");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "Processes");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "Interfaces");
    }

    #[test]
    fn capital_k_only_fires_on_ports_or_processes() {
        let mut w = NetworkWidget::new();
        w.apply_snapshot(snap_two_ports());
        let mut c = ctx();
        // On Interfaces: capital K should NOT open the kill modal.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), NetFocus::Interfaces);
        w.handle_event(&key(KeyCode::Char('K'), KeyModifiers::SHIFT), &mut c);
        assert!(w.kill_modal().is_closed());
        // Cycle back to Ports — K opens the modal.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), NetFocus::Ports);
        w.handle_event(&key(KeyCode::Char('K'), KeyModifiers::SHIFT), &mut c);
        assert!(w.kill_modal().is_confirm_sigterm());
    }
}
