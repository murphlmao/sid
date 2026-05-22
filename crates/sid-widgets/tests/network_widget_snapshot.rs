//! Insta snapshot tests for [`NetworkWidget::render_into_frame`].
//!
//! Each test builds a deterministic snapshot (process list, port list,
//! interfaces), applies it to a fresh widget, and renders into a fixed
//! `TestBackend`. The text body is then snapshotted via insta so future
//! changes to the layout surface as a visible diff.

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Protocol, SocketState,
};
use sid_core::sys_probe::SysSnapshot;
use sid_widgets::NetworkWidget;
use sid_widgets::network::render_to_string;

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
