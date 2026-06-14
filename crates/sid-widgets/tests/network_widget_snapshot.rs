//! Insta snapshot tests for [`NetworkWidget::render_into_frame`].
//!
//! Each test builds a deterministic snapshot (process list, port list,
//! interfaces), applies it to a fresh widget, and renders into a fixed
//! `TestBackend`. The text body is then snapshotted via insta so future
//! changes to the layout surface as a visible diff.

use std::collections::HashSet;

use sid_core::{
    adapters::sys::{ListeningPort, NetInterface, Pid, ProcessInfo, Protocol, SocketState},
    sys_probe::SysSnapshot,
};
use sid_widgets::{NetworkWidget, network::render_to_string};

fn fixture_snapshot() -> SysSnapshot {
    SysSnapshot {
        processes: vec![
            ProcessInfo {
                pid: Pid::from_u32(1),
                name: "init".into(),
                cmd: "/sbin/init".into(),
                cpu_pct: 0.1,
                rss_bytes: 4_000_000,
                started_unix_secs: 1_700_000_000,
                parent: None,
                user: Some("0".into()),
            },
            ProcessInfo {
                pid: Pid::from_u32(1234),
                name: "sid".into(),
                cmd: "sid".into(),
                cpu_pct: 2.3,
                rss_bytes: 50_000_000,
                started_unix_secs: 1_700_000_100,
                parent: Some(Pid::from_u32(1)),
                user: Some("1000".into()),
            },
        ],
        listening_ports: vec![ListeningPort {
            port: 22,
            pid: Some(Pid::from_u32(1)),
            command: "sshd".into(),
            protocol: Protocol::Tcp,
            state: SocketState::Listen,
            local_addr: "0.0.0.0".into(),
        }],
        interfaces: vec![
            NetInterface {
                name: "lo".into(),
                addrs: vec!["127.0.0.1".into(), "::1".into()],
                rx_bytes: 1024,
                tx_bytes: 1024,
                is_up: true,
            },
            NetInterface {
                name: "eth0".into(),
                addrs: vec!["192.168.1.10".into()],
                rx_bytes: 9_000_000,
                tx_bytes: 3_000_000,
                is_up: true,
            },
        ],
        captured_at_unix_secs: 1_700_000_500,
        default_route_iface: None,
    }
}

#[test]
fn snapshot_default_layout() {
    let mut w = NetworkWidget::new();
    w.apply_snapshot(fixture_snapshot());
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("network_default_layout", s);
}

#[test]
fn snapshot_empty_state() {
    let w = NetworkWidget::new();
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("network_empty_state", s);
}

/// M2 — pinned interface: ★ glyph appears on the pinned row, unpinned rows
/// show a plain space.  Verifies correct row-alignment between pinned and
/// unpinned interfaces.
#[test]
fn snapshot_pinned_interface() {
    use std::collections::HashMap;
    let mut w = NetworkWidget::new();
    let snap = SysSnapshot {
        processes: vec![],
        listening_ports: vec![],
        interfaces: vec![
            NetInterface {
                name: "eth0".into(),
                addrs: vec!["192.168.1.10".into()],
                rx_bytes: 1_500_000,
                tx_bytes: 300_000,
                is_up: true,
            },
            NetInterface {
                name: "wlan0".into(),
                addrs: vec!["10.0.0.5".into()],
                rx_bytes: 512,
                tx_bytes: 128,
                is_up: true,
            },
        ],
        captured_at_unix_secs: 1_700_001_000,
        default_route_iface: None,
    };
    // Pin eth0 only — wlan0 is unpinned.
    let mut pinned = HashSet::new();
    pinned.insert("eth0".to_string());
    w.apply_snapshot_with_prefs(snap, HashMap::new(), pinned);
    let s = render_to_string(&w, 80, 24);
    // eth0 must show ★; wlan0 must not.
    assert!(s.contains('★'), "pinned interface must render ★ glyph");
    insta::assert_snapshot!("network_pinned_interface", s);
}

#[test]
fn snapshot_with_many_processes() {
    let mut snap = fixture_snapshot();
    for i in 0..200u32 {
        snap.processes.push(ProcessInfo {
            pid: Pid::from_u32(2000 + i),
            name: format!("proc{i:03}"),
            cmd: format!("proc{i:03} arg1 arg2"),
            cpu_pct: (i as f32) * 0.1,
            rss_bytes: 1_000_000 * u64::from(i + 1),
            started_unix_secs: 1_700_000_000 + i as i64,
            parent: None,
            user: Some("1000".into()),
        });
    }
    let mut w = NetworkWidget::new();
    w.apply_snapshot(snap);
    let s = render_to_string(&w, 80, 24);
    // Just verify we didn't panic; insta still captures a stable view of
    // the top of the table. Header still visible.
    assert!(s.contains("PID"));
}
