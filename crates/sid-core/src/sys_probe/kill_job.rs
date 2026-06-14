//! Async job that drives the SIGTERM → grace → SIGKILL escalation against a
//! [`SysProvider`] without blocking the UI thread.
//!
//! Spawned by the widget through the [`crate::tab::TabManager`]-adjacent
//! `JobQueue` (or directly from the binary's tokio runtime in CLI mode). The
//! returned future resolves to a [`KillOutcome`] which the caller turns into
//! a user-visible toast.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::adapters::sys::{Pid, Signal, SysError, SysProvider};

/// Terminal outcome of `run_kill_job`. Surfaced as a toast by the widget.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::sys::Pid;
/// use sid_core::sys_probe::kill_job::KillOutcome;
///
/// let o = KillOutcome::Killed(Pid::from_u32(1));
/// match o {
///     KillOutcome::Killed(_) => {}
///     KillOutcome::EscalatedToSigkill(_) => {}
///     KillOutcome::Failed(_, _) => {}
/// }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KillOutcome {
    /// SIGTERM delivered and the process exited within the grace period.
    Killed(Pid),
    /// SIGTERM delivered, the process remained alive, SIGKILL was sent. The
    /// SIGKILL itself is fire-and-forget — we do not re-poll after it.
    EscalatedToSigkill(Pid),
    /// SIGTERM failed (permission denied, ESRCH, invalid pid). The error
    /// message is the `SysError`'s `Display`.
    Failed(Pid, String),
}

/// Send SIGTERM to `pid`, wait `grace`, then either return `Killed` (if the
/// process is gone) or send SIGKILL and return `EscalatedToSigkill`.
///
/// If SIGTERM itself fails — typically permission-denied for a process owned
/// by another user, or `NotFound` for an already-dead pid — the future
/// resolves to `Failed` and does not escalate.
///
/// Designed to be `tokio::spawn`'d from a JobQueue or driven directly from a
/// tokio runtime in CLI mode. The signature is `async`, not blocking; the
/// only sync operation is the brief mutex lock around each provider call.
///
/// # Errors
///
/// Returns `Err(SysError)` only when the alive-check between SIGTERM and the
/// SIGKILL decision fails (e.g., provider's `list_processes` returns Err).
/// Both the SIGTERM and SIGKILL paths translate their own errors into
/// `KillOutcome` variants instead of bubbling.
pub async fn run_kill_job(
    provider: Arc<Mutex<dyn SysProvider>>,
    pid: Pid,
    grace: Duration,
) -> Result<KillOutcome, SysError> {
    // 1. SIGTERM. If it fails outright, surface as `Failed` and stop.
    {
        let mut guard = provider.lock().expect("provider mutex poisoned");
        if let Err(e) = guard.kill_process(pid, Signal::Term) {
            return Ok(KillOutcome::Failed(pid, format!("{e}")));
        }
    }
    // 2. Wait the grace period before deciding to escalate.
    tokio::time::sleep(grace).await;
    // 3. Re-check whether the process is alive.
    let still_alive = {
        let mut guard = provider.lock().expect("provider mutex poisoned");
        let procs = guard.list_processes()?;
        procs.iter().any(|p| p.pid == pid)
    };
    if !still_alive {
        return Ok(KillOutcome::Killed(pid));
    }
    // 4. SIGKILL. Fire-and-forget; we don't re-poll because SIGKILL is
    //    uncatchable and the next probe tick will see the process gone.
    {
        let mut guard = provider.lock().expect("provider mutex poisoned");
        let _ = guard.kill_process(pid, Signal::Kill);
    }
    Ok(KillOutcome::EscalatedToSigkill(pid))
}
