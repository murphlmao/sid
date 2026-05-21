use sid_core::adapters::systemctl::{SystemctlError, UnitBus, UnitState};
use sid_system::parse::parse_status;

const ACTIVE: &str = "\
● nginx.service - A high performance web server
     Loaded: loaded (/lib/systemd/system/nginx.service; enabled)
     Active: active (running) since Tue 2026-05-21 08:30:11 UTC; 2h 14min ago
   Main PID: 12345 (nginx)
";

const FAILED: &str = "\
● foo.service - Foo
     Loaded: loaded (/etc/systemd/system/foo.service; disabled)
     Active: failed (Result: exit-code) since Tue 2026-05-21 09:00:00 UTC; 1h ago
";

const NOT_FOUND: &str = "\
Unit not-here.service could not be found.
";

#[test]
fn parses_active_unit() {
    let u = parse_status(ACTIVE, "nginx.service", UnitBus::System).unwrap();
    assert_eq!(u.name, "nginx.service");
    assert_eq!(u.description, "A high performance web server");
    assert_eq!(u.state, UnitState::Active);
    assert_eq!(u.sub_state, "running");
    assert_eq!(u.load_state, "loaded");
}

#[test]
fn parses_failed_unit() {
    let u = parse_status(FAILED, "foo.service", UnitBus::User).unwrap();
    assert_eq!(u.state, UnitState::Failed);
    assert!(u.sub_state.starts_with("Result"));
}

#[test]
fn unit_not_found_returns_error() {
    let err = parse_status(NOT_FOUND, "not-here.service", UnitBus::System).unwrap_err();
    assert!(matches!(err, SystemctlError::UnitNotFound(_)));
}

#[test]
fn empty_output_errors_with_parse() {
    let err = parse_status("", "x.service", UnitBus::System).unwrap_err();
    assert!(matches!(err, SystemctlError::Parse(_)));
}

#[test]
fn description_handling_with_dash_in_name() {
    let s = "● my-thing.service - Something - with dashes\n     Loaded: loaded (x)\n     Active: active (running) since x\n";
    let u = parse_status(s, "my-thing.service", UnitBus::System).unwrap();
    assert_eq!(u.description, "Something - with dashes");
}
