//! Tests for `FilterInputState` and its row-match predicates.

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Protocol, SocketState,
};
use sid_widgets::network::filter_input::{
    FilterInputState, FilterMode, match_interface, match_listening_port, match_process,
};

#[test]
fn starts_inactive_with_empty_query() {
    let f = FilterInputState::new();
    assert_eq!(f.mode(), &FilterMode::Inactive);
    assert_eq!(f.query(), "");
    assert!(!f.is_filtering());
}

#[test]
fn enter_filter_clears_old_query_and_enters_editing() {
    let mut f = FilterInputState::new();
    f.enter_filter();
    f.push_char('x');
    f.cancel();
    f.enter_filter();
    assert_eq!(f.query(), "");
    assert_eq!(f.mode(), &FilterMode::Editing);
}

#[test]
fn push_char_accumulates_only_while_editing() {
    let mut f = FilterInputState::new();
    f.push_char('a'); // ignored — not in editing mode
    assert_eq!(f.query(), "");
    f.enter_filter();
    f.push_char('h');
    f.push_char('i');
    assert_eq!(f.query(), "hi");
}

#[test]
fn pop_char_on_empty_is_noop() {
    let mut f = FilterInputState::new();
    f.enter_filter();
    f.pop_char();
    assert_eq!(f.query(), "");
}

#[test]
fn pop_char_removes_last_only_while_editing() {
    let mut f = FilterInputState::new();
    f.enter_filter();
    f.push_char('a');
    f.push_char('b');
    f.pop_char();
    assert_eq!(f.query(), "a");
    f.submit();
    // After submit we are in Active; pop is a no-op.
    f.pop_char();
    assert_eq!(f.query(), "a");
}

#[test]
fn cancel_returns_to_inactive_and_clears() {
    let mut f = FilterInputState::new();
    f.enter_filter();
    f.push_char('q');
    f.cancel();
    assert_eq!(f.mode(), &FilterMode::Inactive);
    assert_eq!(f.query(), "");
}

#[test]
fn submit_only_moves_from_editing_to_active() {
    let mut f = FilterInputState::new();
    // Submit from Inactive: no-op.
    f.submit();
    assert_eq!(f.mode(), &FilterMode::Inactive);
    f.enter_filter();
    f.push_char('q');
    f.submit();
    assert_eq!(f.mode(), &FilterMode::Active);
    assert_eq!(f.query(), "q");
}

#[test]
fn is_filtering_only_when_query_nonempty_and_not_inactive() {
    let mut f = FilterInputState::new();
    assert!(!f.is_filtering());
    f.enter_filter();
    assert!(!f.is_filtering(), "empty query while editing");
    f.push_char('a');
    assert!(f.is_filtering());
    f.submit();
    assert!(f.is_filtering());
    f.cancel();
    assert!(!f.is_filtering());
}

#[test]
fn unicode_in_query_does_not_panic() {
    let mut f = FilterInputState::new();
    f.enter_filter();
    for c in "🐕日本語".chars() {
        f.push_char(c);
    }
    assert_eq!(f.query(), "🐕日本語");
    f.pop_char();
    assert!(f.query().ends_with("語") || f.query().ends_with("本"));
}

#[test]
fn very_long_query_does_not_panic() {
    let mut f = FilterInputState::new();
    f.enter_filter();
    for c in std::iter::repeat_n('x', 100_000) {
        f.push_char(c);
    }
    assert_eq!(f.query().len(), 100_000);
}

// ---- match predicates ----

fn port_row() -> ListeningPort {
    ListeningPort {
        port: 8080,
        pid: Some(Pid::from_u32(42)),
        command: "myserver".into(),
        protocol: Protocol::Tcp,
        state: SocketState::Listen,
        local_addr: "127.0.0.1".into(),
    }
}

#[test]
fn match_listening_port_empty_query_matches_everything() {
    assert!(match_listening_port("", &port_row()));
}

#[test]
fn match_listening_port_case_insensitive() {
    assert!(match_listening_port("MYSERVER", &port_row()));
    assert!(match_listening_port("myserver", &port_row()));
}

#[test]
fn match_listening_port_matches_port_number_as_string() {
    assert!(match_listening_port("8080", &port_row()));
    assert!(match_listening_port("80", &port_row()));
}

#[test]
fn match_listening_port_matches_local_addr() {
    assert!(match_listening_port("127", &port_row()));
}

#[test]
fn match_listening_port_no_match_returns_false() {
    assert!(!match_listening_port("zzz-no-match", &port_row()));
}

fn proc_row() -> ProcessInfo {
    ProcessInfo {
        pid: Pid::from_u32(1234),
        name: "sid".into(),
        cmd: "sid --start-tab=network".into(),
        cpu_pct: 0.0,
        rss_bytes: 0,
        started_unix_secs: 0,
        parent: None,
        user: Some("1000".into()),
    }
}

#[test]
fn match_process_empty_query_matches() {
    assert!(match_process("", &proc_row()));
}

#[test]
fn match_process_matches_pid_name_cmd_user() {
    assert!(match_process("1234", &proc_row()));
    assert!(match_process("sid", &proc_row()));
    assert!(match_process("network", &proc_row()));
    assert!(match_process("1000", &proc_row()));
}

#[test]
fn match_process_returns_false_on_nonmatch() {
    assert!(!match_process("nope-not-in-row", &proc_row()));
}

#[test]
fn match_process_user_is_optional() {
    let mut row = proc_row();
    row.user = None;
    assert!(match_process("sid", &row));
    assert!(!match_process("1000", &row));
}

fn iface_row() -> NetInterface {
    NetInterface {
        name: "eth0".into(),
        addrs: vec!["192.168.1.10".into(), "fe80::1".into()],
        rx_bytes: 0,
        tx_bytes: 0,
        is_up: true,
    }
}

#[test]
fn match_interface_empty_matches() {
    assert!(match_interface("", &iface_row()));
}

#[test]
fn match_interface_name_or_address() {
    assert!(match_interface("eth", &iface_row()));
    assert!(match_interface("192.168", &iface_row()));
    assert!(match_interface("fe80", &iface_row()));
    assert!(!match_interface("wlan", &iface_row()));
}
