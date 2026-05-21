use sid_core::adapters::systemctl::{UnitBus, UnitState};
use sid_system::parse::parse_list_units;

const SAMPLE: &str = "\
nginx.service                          loaded active   running  A high performance web server
foo.service                            loaded failed   failed   Foo service that broke
sshd.service                           loaded active   running  OpenSSH server daemon
empty-desc.service                     loaded inactive dead
";

#[test]
fn parses_three_typical_rows_plus_no_description() {
    let units = parse_list_units(SAMPLE, UnitBus::System).unwrap();
    assert_eq!(units.len(), 4);
    assert_eq!(units[0].name, "nginx.service");
    assert_eq!(units[0].load_state, "loaded");
    assert_eq!(units[0].state, UnitState::Active);
    assert_eq!(units[0].sub_state, "running");
    assert_eq!(units[0].description, "A high performance web server");
    assert_eq!(units[0].bus, UnitBus::System);

    assert_eq!(units[1].state, UnitState::Failed);
    assert_eq!(units[3].description, "");
}

#[test]
fn parses_empty_input_as_empty_list() {
    let units = parse_list_units("", UnitBus::User).unwrap();
    assert!(units.is_empty());
}

#[test]
fn parses_lines_with_unicode_descriptions() {
    let s = "x.service                              loaded active   running  ✦ starlight ★\n";
    let units = parse_list_units(s, UnitBus::User).unwrap();
    assert_eq!(units[0].description, "✦ starlight ★");
}

#[test]
fn parses_crlf_line_endings() {
    let s = "a.service loaded active running desc\r\nb.service loaded inactive dead other\r\n";
    let units = parse_list_units(s, UnitBus::User).unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(units[1].sub_state, "dead");
}

#[test]
fn skips_header_lines() {
    let s = "UNIT LOAD ACTIVE SUB DESCRIPTION\nnginx.service loaded active running desc\n";
    let units = parse_list_units(s, UnitBus::User).unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].name, "nginx.service");
}
