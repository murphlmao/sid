//! These tests run only when `systemctl` is in PATH (i.e. a Linux dev box with
//! systemd). On macOS / CI without systemd they self-skip.

use sid_core::adapters::systemctl::{SystemctlClient, SystemctlError, UnitBus, UnitFilter};
use sid_system::SystemctlCmdClient;

fn have_systemctl() -> bool {
    which::which("systemctl").is_ok() && which::which("journalctl").is_ok()
}

#[test]
fn list_units_user_bus_returns_something() {
    if !have_systemctl() {
        eprintln!("skip: systemctl/journalctl missing");
        return;
    }
    let client = SystemctlCmdClient::new().unwrap();
    let units = client
        .list_units(UnitFilter {
            bus: UnitBus::User,
            ..Default::default()
        })
        .unwrap();
    // User-bus may legitimately have zero units on bare systems — assert no panic.
    let _ = units;
}

#[test]
fn list_units_with_name_filter() {
    if !have_systemctl() {
        return;
    }
    let client = SystemctlCmdClient::new().unwrap();
    let units = client
        .list_units(UnitFilter {
            name_substring: Some("ssh".into()),
            bus: UnitBus::System,
            ..Default::default()
        })
        .unwrap();
    assert!(units.iter().all(|u| u.name.contains("ssh")));
}

#[test]
fn list_units_with_both_buses() {
    if !have_systemctl() {
        return;
    }
    let client = SystemctlCmdClient::new().unwrap();
    let _ = client
        .list_units(UnitFilter {
            bus_both: true,
            ..Default::default()
        })
        .unwrap();
}

#[test]
fn status_of_known_user_unit_or_skips() {
    if !have_systemctl() {
        return;
    }
    let client = SystemctlCmdClient::new().unwrap();
    // Pick a user unit that almost always exists: `default.target` (target unit).
    let r = client.status(UnitBus::User, "default.target");
    match r {
        Ok(u) => assert_eq!(u.name, "default.target"),
        Err(_) => {
            // Acceptable: not every host has a user-default.target loaded.
        }
    }
}

#[test]
fn start_system_unit_without_sudo_returns_sudo_required_or_not_found() {
    if !have_systemctl() {
        return;
    }
    if std::env::var("USER").as_deref() == Ok("root") {
        return;
    }
    let client = SystemctlCmdClient::new().unwrap();
    let r = client.start(UnitBus::System, "sid-test-bogus-unit.service");
    match r {
        Err(SystemctlError::SudoRequired)
        | Err(SystemctlError::UnitNotFound(_))
        | Err(SystemctlError::NonZeroExit(_)) => {}
        other => panic!("expected SudoRequired/UnitNotFound/NonZeroExit, got {other:?}"),
    }
}

#[test]
fn journal_tail_returns_some_lines_or_error() {
    if !have_systemctl() {
        return;
    }
    let client = SystemctlCmdClient::new().unwrap();
    let r = client.journal_tail(UnitBus::System, "systemd-journald.service", 10);
    match r {
        Ok(entries) => assert!(entries.len() <= 10),
        Err(SystemctlError::UnitNotFound(_))
        | Err(SystemctlError::NonZeroExit(_))
        | Err(SystemctlError::Io(_)) => {}
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn journal_tail_invalid_unit_returns_error_or_empty() {
    if !have_systemctl() {
        return;
    }
    let client = SystemctlCmdClient::new().unwrap();
    let r = client.journal_tail(UnitBus::System, "this-cant-possibly-exist-xx.service", 10);
    assert!(r.is_err() || r.unwrap().is_empty());
}

#[test]
fn new_returns_error_when_path_disabled() {
    if !have_systemctl() {
        let err = SystemctlCmdClient::new().unwrap_err();
        assert!(matches!(
            err,
            SystemctlError::SystemctlMissing | SystemctlError::JournalctlMissing
        ));
    }
}
