//! Unit + adversarial tests for the /proc/net/route parser.

use sid_sysinfo::default_route::parse_proc_net_route;

#[test]
fn parses_two_line_with_one_default_route() {
    let body = "Iface\tDestination\tGateway\nwlan0\t00000000\t0102A8C0\n";
    assert_eq!(parse_proc_net_route(body), Some("wlan0".to_string()));
}

#[test]
fn first_default_route_wins_when_multiple() {
    let body = "Iface\tDestination\tGateway\nwlan0\t00000000\t1\neth0\t00000000\t2\n";
    assert_eq!(parse_proc_net_route(body), Some("wlan0".to_string()));
}

#[test]
fn empty_body_returns_none() {
    assert_eq!(parse_proc_net_route(""), None);
}

#[test]
fn header_only_returns_none() {
    assert_eq!(parse_proc_net_route("Iface\tDestination\tGateway\n"), None);
}

#[test]
fn non_default_routes_are_ignored() {
    let body = "Iface\tDestination\tGateway\ndocker0\tABCDEF01\t0\n";
    assert_eq!(parse_proc_net_route(body), None);
}

#[test]
fn malformed_row_does_not_panic() {
    let body = "Iface\tDestination\nwlan0\n00000000\n";
    let _ = parse_proc_net_route(body);
}

#[test]
fn skips_non_default_then_finds_default() {
    let body = "Iface\tDestination\tGateway\n\
        docker0\tABCDEF01\t0\n\
        wlan0\t00000000\t0102A8C0\n";
    assert_eq!(parse_proc_net_route(body), Some("wlan0".to_string()));
}

// Live test: hits /proc/net/route on Linux. Marked ignored so CI/macOS
// don't fail. Run with `cargo test -p sid-sysinfo -- --ignored` to
// exercise on a real Linux host.
#[cfg(target_os = "linux")]
#[test]
#[ignore]
fn live_proc_net_route_does_not_panic() {
    let r = sid_sysinfo::default_route::read_default_route_iface();
    // Just confirm it returns SOME Result without panicking.
    let _ = r;
}
