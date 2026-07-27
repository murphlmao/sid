//! `sudo` stderr → [`PrivError`] classification.
//!
//! The single most important distinction this module draws is **AuthFailed versus
//! everything else**: it is what decides whether the password prompt stays open for
//! another try or is torn down with a final message. Getting it wrong in either
//! direction is a bad bug — a re-prompt loop the user can never satisfy, or a "wrong
//! password" dismissal when the password was fine.
//!
//! Substring matching only, never panicking, and only ever consulted for a **non-zero
//! exit**: `sudo` writes to stderr on success too (`unable to resolve host`, lecture
//! text), and treating that as a failure would break working setups. Same shape as
//! `sid-svcctl`'s `classify_stderr`.

use sid_core::privfs::PrivError;

/// Markers that mean "this user may not elevate here, and no password will change
/// that". Checked before the authentication markers: a `sudoers` rejection is the more
/// actionable message, and it is the one that must stop the re-prompt loop.
const NOT_PERMITTED: &[&str] = &[
    "is not in the sudoers file",
    "is not allowed to execute",
    "is not allowed to run sudo",
    "may not run sudo",
    "This incident has been reported",
    "This incident will be reported",
];

/// Markers that mean "the secret was wrong or missing" — the caller re-prompts.
const AUTH_FAILED: &[&str] = &[
    "incorrect password attempt",
    "Sorry, try again",
    "no password was provided",
    "a password is required",
    "Authentication failure",
    "authentication failure",
];

/// Markers that mean "the elevation mechanism cannot be driven from here" — a
/// configuration problem, not a credential problem. Checked first: these messages
/// mention passwords and would otherwise be misread as authentication failures.
const UNAVAILABLE: &[&str] = &[
    "a terminal is required to read the password",
    "no tty present and no askpass program specified",
    "no askpass program specified",
    "unable to execute",
    "command not found",
];

/// Classify `sudo`'s stderr for a non-zero exit.
///
/// `fallback` describes the operation ("read /etc/fstab") and is used when stderr says
/// nothing recognizable — a bare non-zero exit with empty stderr must still produce a
/// message a human can act on.
pub(crate) fn classify_stderr(stderr: &str, fallback: &str) -> PrivError {
    let trimmed = stderr.trim();
    let matches_any = |markers: &[&str]| markers.iter().any(|m| stderr.contains(m));

    if matches_any(UNAVAILABLE) {
        PrivError::Unavailable(trimmed.to_string())
    } else if matches_any(NOT_PERMITTED) {
        PrivError::NotPermitted(trimmed.to_string())
    } else if matches_any(AUTH_FAILED) {
        PrivError::AuthFailed
    } else if trimmed.is_empty() {
        PrivError::Io(fallback.to_string())
    } else {
        PrivError::Io(trimmed.to_string())
    }
}

/// Classify the failure to *spawn* `sudo` at all. Distinct from
/// [`classify_stderr`]: there is no stderr, because nothing ran.
pub(crate) fn classify_spawn_error(err: &std::io::Error) -> PrivError {
    // Never `AuthFailed`: nothing ran, so nothing was learned about the credential.
    PrivError::Unavailable(format!("could not run sudo: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact text sudo 1.9.x prints when a password fed on stdin is wrong: it says
    /// "Sorry, try again." then reads again, gets EOF, and gives up with the count.
    const WRONG_PASSWORD: &str = "Sorry, try again.\nsudo: 1 incorrect password attempt\n";

    #[test]
    fn a_wrong_password_is_an_authentication_failure() {
        assert!(matches!(
            classify_stderr(WRONG_PASSWORD, "read /etc/fstab"),
            PrivError::AuthFailed
        ));
    }

    #[test]
    fn three_incorrect_attempts_is_still_an_authentication_failure() {
        assert!(matches!(
            classify_stderr("sudo: 3 incorrect password attempts\n", "x"),
            PrivError::AuthFailed
        ));
    }

    #[test]
    fn an_empty_password_is_an_authentication_failure() {
        // `sudo -S` with nothing but EOF on stdin.
        assert!(matches!(
            classify_stderr("sudo: no password was provided\n", "x"),
            PrivError::AuthFailed
        ));
    }

    #[test]
    fn a_pam_authentication_failure_line_is_an_authentication_failure() {
        assert!(matches!(
            classify_stderr("sudo: pam_authenticate: Authentication failure\n", "x"),
            PrivError::AuthFailed
        ));
    }

    #[test]
    fn a_non_sudoer_is_not_permitted_and_carries_sudos_own_words() {
        let e = classify_stderr(
            "murphy is not in the sudoers file.  This incident will be reported.\n",
            "read /etc/fstab",
        );
        match e {
            PrivError::NotPermitted(msg) => assert!(
                msg.contains("not in the sudoers file"),
                "message lost sudo's explanation: {msg}"
            ),
            other => panic!("expected NotPermitted, got {other:?}"),
        }
    }

    #[test]
    fn a_user_barred_from_this_command_is_not_permitted() {
        let e = classify_stderr(
            "Sorry, user murphy is not allowed to execute '/usr/bin/cat /etc/fstab' \
             as root on box.\n",
            "x",
        );
        assert!(matches!(e, PrivError::NotPermitted(_)), "got {e:?}");
    }

    #[test]
    fn a_sudoers_rejection_outranks_the_apology_that_precedes_it() {
        // "Sorry, user X is not allowed..." contains "Sorry, " — the same opening as
        // "Sorry, try again." Without ordering, a non-sudoer would be told they mistyped
        // and re-prompted forever.
        let e = classify_stderr("Sorry, user murphy is not allowed to execute 'x'.\n", "x");
        assert!(
            matches!(e, PrivError::NotPermitted(_)),
            "the sudoers branch must win, got {e:?}"
        );
    }

    #[test]
    fn a_requiretty_configuration_is_unavailable_not_a_wrong_password() {
        // Mentions "password", but no password will ever get through — re-prompting
        // would be an infinite loop.
        let e = classify_stderr(
            "sudo: a terminal is required to read the password; either use the -S option \
             to read from standard input or configure an askpass helper\n",
            "x",
        );
        assert!(matches!(e, PrivError::Unavailable(_)), "got {e:?}");
    }

    #[test]
    fn a_missing_askpass_helper_is_unavailable() {
        let e = classify_stderr(
            "sudo: no tty present and no askpass program specified\n",
            "x",
        );
        assert!(matches!(e, PrivError::Unavailable(_)), "got {e:?}");
    }

    #[test]
    fn a_helper_sudo_cannot_execute_is_unavailable() {
        let e = classify_stderr("sudo: unable to execute /usr/bin/cat: No such file\n", "x");
        assert!(matches!(e, PrivError::Unavailable(_)), "got {e:?}");
    }

    #[test]
    fn the_inner_commands_own_failure_is_an_io_error() {
        // Authentication succeeded; `cat` then failed on its own terms. The user must
        // see this, and must NOT be asked for their password again.
        let e = classify_stderr(
            "cat: /etc/nope: No such file or directory\n",
            "read /etc/nope",
        );
        match e {
            PrivError::Io(msg) => assert!(msg.contains("No such file"), "got {msg}"),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn a_read_only_filesystem_is_an_io_error_not_an_auth_failure() {
        let e = classify_stderr("mv: cannot move '/etc/x': Read-only file system\n", "x");
        assert!(matches!(e, PrivError::Io(_)), "got {e:?}");
    }

    #[test]
    fn a_silent_failure_falls_back_to_describing_the_operation() {
        let e = classify_stderr("   \n", "read /etc/fstab");
        match e {
            PrivError::Io(msg) => assert!(
                msg.contains("read /etc/fstab"),
                "an empty stderr must still say what failed: {msg}"
            ),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn classification_trims_the_message_it_carries() {
        let e = classify_stderr("  cat: /etc/nope: No such file  \n\n", "x");
        match e {
            PrivError::Io(msg) => assert_eq!(msg, "cat: /etc/nope: No such file"),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_sudo_binary_is_unavailable() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "No such file or directory");
        let e = classify_spawn_error(&err);
        match e {
            PrivError::Unavailable(msg) => assert!(
                msg.contains("sudo"),
                "the message must name what is missing: {msg}"
            ),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn any_other_spawn_failure_is_also_unavailable() {
        // Nothing ran, so nothing about the user's credentials was learned — this can
        // never be an AuthFailed.
        let err = std::io::Error::other("cgroup limit");
        assert!(matches!(
            classify_spawn_error(&err),
            PrivError::Unavailable(_)
        ));
    }
}
