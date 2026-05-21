//! Tests for `PortsTableState` — sorting, scrolling, and empty-table
//! navigation.

use proptest::prelude::*;
use sid_core::adapters::sys::{ListeningPort, Pid, Protocol, SocketState};
use sid_widgets::network::ports_table::{PortsSortBy, PortsTableState, SortDir};

fn sample() -> Vec<ListeningPort> {
    vec![
        ListeningPort {
            port: 8080,
            pid: Some(Pid::from_u32(100)),
            command: "app".into(),
            protocol: Protocol::Tcp,
            state: SocketState::Listen,
            local_addr: "0.0.0.0".into(),
        },
        ListeningPort {
            port: 22,
            pid: Some(Pid::from_u32(50)),
            command: "sshd".into(),
            protocol: Protocol::Tcp,
            state: SocketState::Listen,
            local_addr: "0.0.0.0".into(),
        },
        ListeningPort {
            port: 53,
            pid: Some(Pid::from_u32(80)),
            command: "dnsmasq".into(),
            protocol: Protocol::Udp,
            state: SocketState::Listen,
            local_addr: "127.0.0.1".into(),
        },
    ]
}

#[test]
fn sort_by_port_ascending() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.set_sort(PortsSortBy::Port, SortDir::Asc);
    assert_eq!(
        s.rows().iter().map(|r| r.port).collect::<Vec<_>>(),
        vec![22, 53, 8080]
    );
}

#[test]
fn sort_by_pid_descending() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.set_sort(PortsSortBy::Pid, SortDir::Desc);
    assert_eq!(
        s.rows()
            .iter()
            .map(|r| r.pid.unwrap().as_u32())
            .collect::<Vec<_>>(),
        vec![100, 80, 50]
    );
}

#[test]
fn sort_by_command_ascending() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.set_sort(PortsSortBy::Command, SortDir::Asc);
    let names: Vec<&str> = s.rows().iter().map(|r| r.command.as_str()).collect();
    assert_eq!(names, vec!["app", "dnsmasq", "sshd"]);
}

#[test]
fn sort_by_protocol_lists_tcp_before_udp_asc() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.set_sort(PortsSortBy::Protocol, SortDir::Asc);
    assert_eq!(s.rows()[0].protocol, Protocol::Tcp);
    assert_eq!(s.rows().last().unwrap().protocol, Protocol::Udp);
}

#[test]
fn set_data_resorts_when_sort_is_configured() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.set_sort(PortsSortBy::Port, SortDir::Asc);
    // Re-feed data in a different order; should still come out sorted.
    let mut reverse = sample();
    reverse.reverse();
    s.set_data(reverse);
    assert_eq!(
        s.rows().iter().map(|r| r.port).collect::<Vec<_>>(),
        vec![22, 53, 8080]
    );
}

#[test]
fn select_next_wraps_at_end() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    assert_eq!(s.selected_index(), 0);
    s.select_next();
    s.select_next();
    s.select_next();
    assert_eq!(s.selected_index(), 0, "should wrap to start");
}

#[test]
fn select_prev_wraps_at_start() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.select_prev();
    assert_eq!(s.selected_index(), 2, "should wrap to end");
}

#[test]
fn empty_data_handles_navigation_without_panic() {
    let mut s = PortsTableState::new();
    s.set_data(vec![]);
    s.select_next();
    s.select_prev();
    assert!(s.selected_row().is_none());
}

#[test]
fn set_data_truncating_below_selection_resets_to_zero() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.select_next();
    s.select_next();
    assert_eq!(s.selected_index(), 2);
    // Replace with just one row.
    s.set_data(vec![sample()[0].clone()]);
    assert_eq!(s.selected_index(), 0);
}

proptest! {
    /// Property: sorting by port ascending then descending is the reverse of asc.
    #[test]
    fn prop_asc_then_desc_is_reverse(ports in proptest::collection::vec(1u16..=65535u16, 0..20)) {
        let rows: Vec<_> = ports.iter().map(|p| ListeningPort {
            port: *p,
            pid: None,
            command: String::new(),
            protocol: Protocol::Tcp,
            state: SocketState::Listen,
            local_addr: "0.0.0.0".into(),
        }).collect();
        let mut a = PortsTableState::new();
        a.set_data(rows.clone());
        a.set_sort(PortsSortBy::Port, SortDir::Asc);
        let mut d = PortsTableState::new();
        d.set_data(rows);
        d.set_sort(PortsSortBy::Port, SortDir::Desc);
        let av: Vec<_> = a.rows().iter().map(|r| r.port).collect();
        let mut dv: Vec<_> = d.rows().iter().map(|r| r.port).collect();
        dv.reverse();
        prop_assert_eq!(av, dv);
    }

    /// Property: selection index always stays within bounds across an
    /// arbitrary sequence of next/prev actions.
    #[test]
    fn prop_selection_in_bounds(actions in proptest::collection::vec(0u8..2, 0..50)) {
        let template = ListeningPort {
            port: 1,
            pid: None,
            command: String::new(),
            protocol: Protocol::Tcp,
            state: SocketState::Listen,
            local_addr: "0".into(),
        };
        let mut s = PortsTableState::new();
        s.set_data(vec![template; 5]);
        for a in actions {
            if a == 0 { s.select_next(); } else { s.select_prev(); }
            prop_assert!(s.selected_index() < 5);
        }
    }
}
