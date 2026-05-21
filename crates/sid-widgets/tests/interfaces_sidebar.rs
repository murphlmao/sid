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
    vec![iface("eth0", true), iface("lo", true), iface("wlan0", false)]
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
