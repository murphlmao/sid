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

    #[test]
    fn plain_permission_denied_text_is_permission_denied() {
        // "Permission denied" (no "Access") is a distinct substring branch from
        // "Access denied" — exercise it directly rather than relying on the
        // "Access denied" case to stand in for the whole permission family.
        let e = classify_stderr("Permission denied\n", "x.service");
        assert!(matches!(e, SvcError::PermissionDenied(_)));
    }

    #[test]
    fn operation_not_permitted_is_permission_denied() {
        let e = classify_stderr("Operation not permitted\n", "x.service");
        assert!(matches!(e, SvcError::PermissionDenied(_)));
    }

    #[test]
    fn bare_not_found_substring_is_not_found() {
        // Exercises the `"not found"` branch directly — the existing coverage only
        // hit the sibling `"could not be found"` branch via a realistic systemd
        // message. `"could not be found"` does NOT contain the literal substring
        // `"not found"` (there's a `"be "` in between), so these are genuinely
        // two different matches in `classify_stderr`, not one test standing in
        // for both.
        let e = classify_stderr("Unit foo.service not found\n", "foo.service");
        match e {
            SvcError::NotFound(name) => assert_eq!(name, "foo.service"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn not_found_with_no_target_unit_uses_trimmed_stderr_as_message() {
        // `unit == ""` is the `list-units`-call shape: there's no single target,
        // so the message falls back to the trimmed stderr text itself.
        let e = classify_stderr("  some unit not found somewhere  \n", "");
        match e {
            SvcError::NotFound(name) => assert_eq!(name, "some unit not found somewhere"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn permission_denied_is_checked_before_not_found_when_both_match() {
        // Adversarial stderr matching both branches' substrings: permission
        // classification must win (it's checked first in `classify_stderr`) since
        // "needs root" is the more actionable message for the caller than
        // "unit not found".
        let e = classify_stderr("Access denied: unit not found\n", "x.service");
        assert!(
            matches!(e, SvcError::PermissionDenied(_)),
            "permission-denied branch takes precedence"
        );
    }
}
