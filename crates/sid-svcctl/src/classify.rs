//! `systemctl` stderr → [`SvcError`] classification.
//!
//! Cribbed from the archived `sid-poc`'s `sid-system::client::SystemctlCmdClient::
//! run_action` (`~/vcs/sid-poc/crates/sid-system/src/client.rs`), which mapped the
//! same polkit/dbus stderr substrings to `SystemctlError::SudoRequired` /
//! `UnitNotFound`. Adapted here: the POC's `SudoRequired` is this crate's
//! `SvcError::PermissionDenied` (same meaning — a system-scope action needs root and
//! sid never auto-escalates); the POC had no `--output=json` so its `list_units`
//! parsing lived in `parse_list_units` doing whitespace-column splitting — this
//! rebuild's [`crate::parse`] parses JSON instead (see that module's doc comment).

use sid_core::svc::SvcError;

/// Classify `systemctl`'s stderr for a non-zero exit. `unit` names the single target
/// unit for the `NotFound` message; pass `""` when there's no single target (e.g. a
/// `list-units` call), in which case the trimmed stderr itself becomes the message.
///
/// Substring-matching only — never panics on adversarial input.
pub(crate) fn classify_stderr(stderr: &str, unit: &str) -> SvcError {
    let trimmed = stderr.trim();
    if stderr.contains("Access denied")
        || stderr.contains("Permission denied")
        || stderr.contains("Interactive authentication required")
        || stderr.contains("Authentication is required")
        || stderr.contains("Operation not permitted")
        || stderr.contains("Failed to enable bus")
    {
        return SvcError::PermissionDenied(trimmed.to_string());
    }
    if stderr.contains("not found") || stderr.contains("could not be found") {
        let name = if unit.is_empty() {
            trimmed.to_string()
        } else {
            unit.to_string()
        };
        return SvcError::NotFound(name);
    }
    SvcError::Other(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_denied_is_permission_denied() {
        let e = classify_stderr("Access denied\n", "nginx.service");
        assert!(matches!(e, SvcError::PermissionDenied(_)));
    }

    #[test]
    fn interactive_authentication_required_is_permission_denied() {
        let e = classify_stderr("Interactive authentication required.\n", "nginx.service");
        assert!(matches!(e, SvcError::PermissionDenied(_)));
    }

    #[test]
    fn authentication_is_required_is_permission_denied() {
        let e = classify_stderr(
            "Authentication is required to restart 'nginx.service'.\n",
            "nginx.service",
        );
        assert!(matches!(e, SvcError::PermissionDenied(_)));
    }

    #[test]
    fn failed_to_enable_bus_is_permission_denied() {
        let e = classify_stderr("Failed to enable bus.\n", "x.service");
        assert!(matches!(e, SvcError::PermissionDenied(_)));
    }

    #[test]
    fn unit_not_found_carries_unit_name() {
        let e = classify_stderr("Unit nope.service could not be found.\n", "nope.service");
        match e {
            SvcError::NotFound(name) => assert_eq!(name, "nope.service"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn unrecognized_stderr_is_other() {
        let e = classify_stderr("some other systemd failure\n", "x.service");
        match e {
            SvcError::Other(msg) => assert_eq!(msg, "some other systemd failure"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn empty_stderr_with_no_unit_is_other_with_empty_message() {
        let e = classify_stderr("", "");
        assert!(matches!(e, SvcError::Other(m) if m.is_empty()));
    }
}
