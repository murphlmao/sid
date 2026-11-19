//! Shared subprocess runner: every CLI call is bounded, and a call that outlives its
//! limit is killed (`kill_on_drop`) instead of leaving a view spinning forever.

use std::process::Output;
use std::time::Duration;

use tokio::process::Command;

/// Why a bounded call produced no `Output`.
pub(crate) enum RunError {
    /// The binary could not be started (`NotFound` = not on `PATH`).
    Spawn(std::io::Error),
    /// The call outlived its limit and was killed.
    TimedOut(Duration),
}

/// Run `cmd`, giving up (and killing the child) after `limit`.
pub(crate) async fn run_within(cmd: &mut Command, limit: Duration) -> Result<Output, RunError> {
    match tokio::time::timeout(limit, cmd.kill_on_drop(true).output()).await {
        Ok(res) => res.map_err(RunError::Spawn),
        Err(_) => Err(RunError::TimedOut(limit)),
    }
}
