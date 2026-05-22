//! Tests for `InterfacesSidebarState` — wrap-around selection and
//! by-name selection-preservation across data refreshes.

use sid_core::adapters::sys::NetInterface;
use sid_widgets::network::interfaces_sidebar::InterfacesSidebarState;

fn iface(name: &str, up: bool) -> NetInterface {
    NetInterface {
        name: name.into(),
        addrs: vec![],
        rx_bytes: 0,
        tx_bytes: 0,
        is_up: up,
    }
}

fn sample() -> Vec<NetInterface> {
    vec![
        iface("eth0", true),
        iface("lo", true),
        iface("wlan0", false),
    ]
}

#[test]
fn select_next_wraps_at_end() {
    let mut s = InterfacesSidebarState::new();
    s.set_data(sample());
    s.select_next();
    s.select_next();
    s.select_next();
    assert_eq!(s.selected_index(), 0);
}

#[test]
fn select_prev_wraps_at_start() {
    let mut s = InterfacesSidebarState::new();
    s.set_data(sample());
    s.select_prev();
    assert_eq!(s.selected_index(), 2);
}

#[test]
fn empty_data_handles_navigation_without_panic() {
    let mut s = InterfacesSidebarState::new();
    s.set_data(vec![]);
    s.select_next();
    s.select_prev();
    assert!(s.selected_row().is_none());
}

#[test]
fn set_data_preserves_selection_by_name() {
    let mut s = InterfacesSidebarState::new();
    s.set_data(sample());
    s.select_next();
    s.select_next(); // wlan0
    assert_eq!(s.selected_row().unwrap().name, "wlan0");

    // Refresh with the same set, slightly reordered.
    s.set_data(vec![
        iface("wlan0", false),
        iface("eth0", true),
        iface("lo", true),
    ]);
    assert_eq!(s.selected_row().unwrap().name, "wlan0");
}

#[test]
fn set_data_resets_selection_if_name_gone() {
    let mut s = InterfacesSidebarState::new();
    s.set_data(sample());
    s.select_next();
    s.select_next(); // wlan0
    s.set_data(vec![iface("eth0", true), iface("lo", true)]);
    // wlan0 no longer present, selection resets to 0 (eth0).
    assert_eq!(s.selected_row().unwrap().name, "eth0");
}

#[test]
fn set_data_truncating_below_selection_resets_to_zero() {
    let mut s = InterfacesSidebarState::new();
    s.set_data(sample());
    s.select_next();
    s.select_next();
    // Replace with only one interface that doesn't include the old selection.
    s.set_data(vec![iface("br0", true)]);
    assert_eq!(s.selected_index(), 0);
    assert_eq!(s.selected_row().unwrap().name, "br0");
}

#[test]
fn set_data_empty_clears_selection() {
    let mut s = InterfacesSidebarState::new();
    s.set_data(sample());
    s.set_data(vec![]);
    assert_eq!(s.selected_index(), 0);
    assert!(s.selected_row().is_none());
}

// ---------------------------------------------------------------------------
// Branch #4 Task 4 — sort by score (WAN first, virtual last)
// ---------------------------------------------------------------------------

#[test]
fn wan_iface_sorts_first_when_default_route_set() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("lo", true),
        iface("docker0", true),
        iface("wlan0", true),
        iface("eth0", false),
    ];
    s.set_data_with_default_route(ifaces, Some("wlan0"));
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order[0], "wlan0", "wlan0 must sort first; got {order:?}");
}

#[test]
fn loopback_and_docker_sort_last() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("lo", true),
        iface("docker0", true),
        iface("wlan0", true),
    ];
    s.set_data_with_default_route(ifaces, Some("wlan0"));
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order[0], "wlan0");
    assert!(order.last() == Some(&"lo") || order.last() == Some(&"docker0"));
}

#[test]
fn down_interfaces_sort_below_up_when_no_default_route() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![iface("eth0", false), iface("wlan0", true)];
    s.set_data_with_default_route(ifaces, None);
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order, vec!["wlan0", "eth0"]);
}

#[test]
fn alphabetical_tiebreak_within_score_bucket() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![iface("wlan1", true), iface("wlan0", true)];
    s.set_data_with_default_route(ifaces, None);
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order, vec!["wlan0", "wlan1"]);
}

#[test]
fn no_default_route_falls_back_to_score_only() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![iface("lo", true), iface("eth0", true)];
    s.set_data_with_default_route(ifaces, None);
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order, vec!["eth0", "lo"]);
}

#[test]
fn err_from_default_route_collapses_to_alphabetical_via_none() {
    // sys_probe maps Err to Ok(None) at the snapshot layer; this test locks
    // in that the sidebar's score function handles None cleanly (no special-
    // case branch, just "no WAN to prioritize").
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![iface("eth0", true), iface("wlan0", true)];
    s.set_data_with_default_route(ifaces, None);
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order, vec!["eth0", "wlan0"]);
}

#[test]
fn sort_is_stable_across_repeated_set_data() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![iface("eth0", true), iface("eth1", true), iface("wlan0", true)];
    s.set_data_with_default_route(ifaces.clone(), Some("wlan0"));
    let first: Vec<String> = s.rows().iter().map(|i| i.name.clone()).collect();
    s.set_data_with_default_route(ifaces, Some("wlan0"));
    let second: Vec<String> = s.rows().iter().map(|i| i.name.clone()).collect();
    assert_eq!(first, second);
}
