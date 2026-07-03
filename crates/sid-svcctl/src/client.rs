//! `tokio::process`-backed `systemctl` invocations.
//!
//! Uses `tokio::process::Command` (never `std::process`) so the shell-out is a real
//! non-blocking `.await` rather than a synchronous subprocess wait — see
//! `crates/sid/src/ui/network_tab.rs`'s module doc for why: every call here already
//! only ever runs inside a `ssh_runtime().spawn(async move { .. })` block, never
//! inline in `render`, and `tokio::process` lets that spawned task yield to the
//! runtime while `systemctl`/`systemd` do their IPC round-trip instead of parking a
//! whole worker thread on `wait(2)`.
//!
//! Request/response only — JSON parsing lives in [`crate::parse`], stderr
//! classification in [`crate::classify`].

use sid_core::svc::{SvcAction, SvcError, SvcScope};
use tokio::process::Command;

use crate::classify::classify_stderr;

fn scope_flag(scope: SvcScope) -> &'static str {
    match scope {
        SvcScope::System => "--system",
        SvcScope::User => "--user",
    }
}

fn action_flag(action: SvcAction) -> &'static str {
    match action {
        SvcAction::Start => "start",
        SvcAction::Stop => "stop",
        SvcAction::Restart => "restart",
        SvcAction::Kill => "kill",
    }
}

/// Run `systemctl [--user|--system] --no-pager --no-ask-password list-units
/// --type=service --all --output=json` and return raw stdout (JSON array text).
pub(crate) async fn list_units_json(scope: SvcScope) -> Result<String, SvcError> {
    let out = Command::new("systemctl")
        .args([
            scope_flag(scope),
            "--no-pager",
            "--no-ask-password",
            "list-units",
            "--type=service",
            "--all",
            "--output=json",
        ])
        .output()
        .await
        .map_err(|e| SvcError::Other(format!("spawn systemctl: {e}")))?;
    if !out.status.success() {
        return Err(classify_stderr(&String::from_utf8_lossy(&out.stderr), ""));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `systemctl [--user|--system] --no-pager --no-ask-password <action> <unit>`.
///
/// `--no-ask-password` makes a system-scope action attempted without root fail
/// immediately with a classifiable stderr message instead of systemd/polkit spawning
/// an interactive password agent — sid owns its own window and must never let a
/// external prompt steal focus or block waiting for input that never arrives.
pub(crate) async fn run_action(
    scope: SvcScope,
    action: SvcAction,
    unit: &str,
) -> Result<(), SvcError> {
    let out = Command::new("systemctl")
        .args([
            scope_flag(scope),
            "--no-pager",
            "--no-ask-password",
            action_flag(action),
            unit,
        ])
        .output()
        .await
        .map_err(|e| SvcError::Other(format!("spawn systemctl: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    Err(classify_stderr(&String::from_utf8_lossy(&out.stderr), unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_flags_are_distinct() {
        assert_eq!(scope_flag(SvcScope::System), "--system");
        assert_eq!(scope_flag(SvcScope::User), "--user");
    }

    #[test]
    fn action_flags_match_systemctl_verbs() {
        assert_eq!(action_flag(SvcAction::Start), "start");
        assert_eq!(action_flag(SvcAction::Stop), "stop");
        assert_eq!(action_flag(SvcAction::Restart), "restart");
        assert_eq!(action_flag(SvcAction::Kill), "kill");
    }
}
