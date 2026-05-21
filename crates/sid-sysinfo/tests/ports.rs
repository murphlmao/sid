use std::net::TcpListener;

use sid_core::adapters::sys::{Protocol, SocketState, SysProvider};
use sid_sysinfo::SysinfoProvider;

#[test]
fn binding_a_local_tcp_port_makes_it_appear() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let bound = listener.local_addr().unwrap().port();
    let mut p = SysinfoProvider::new();
    let ports = p.list_listening_ports().unwrap();
    assert!(
        ports
            .iter()
            .any(|x| x.port == bound && x.protocol == Protocol::Tcp),
        "expected port {bound} in listening ports"
    );
    drop(listener);
}

#[test]
fn all_returned_entries_are_listen_state() {
    let mut p = SysinfoProvider::new();
    let ports = p.list_listening_ports().unwrap();
    for entry in &ports {
        assert_eq!(
            entry.state,
            SocketState::Listen,
            "non-LISTEN entry returned: {entry:?}"
        );
    }
}

#[test]
fn list_listening_ports_returns_ok() {
    let mut p = SysinfoProvider::new();
    let _ = p
        .list_listening_ports()
        .expect("must not error on a healthy host");
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_returned_entries_have_valid_protocol(_seed in 0u32..1) {
        let _ = _seed;
        let mut p = SysinfoProvider::new();
        let ports = p.list_listening_ports().unwrap();
        for entry in &ports {
            // port is u16 by type; verify protocol is one of the two variants.
            prop_assert!(matches!(entry.protocol, Protocol::Tcp | Protocol::Udp));
        }
    }
}

#[test]
fn binding_many_ports_does_not_lose_any() {
    let mut listeners = Vec::new();
    let mut bound_ports = Vec::new();
    for _ in 0..8 {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        bound_ports.push(l.local_addr().unwrap().port());
        listeners.push(l);
    }
    let mut p = SysinfoProvider::new();
    let ports = p.list_listening_ports().unwrap();
    for bp in &bound_ports {
        assert!(
            ports
                .iter()
                .any(|x| x.port == *bp && x.protocol == Protocol::Tcp),
            "expected port {bp} in listing"
        );
    }
    drop(listeners);
}

#[test]
fn output_sorted_by_port_ascending() {
    let mut p = SysinfoProvider::new();
    let ports = p.list_listening_ports().unwrap();
    for w in ports.windows(2) {
        assert!(w[0].port <= w[1].port, "ports not sorted: {ports:?}");
    }
}
