use nix::errno::Errno;
use sid_core::adapters::sys::{Pid, Signal, SysError};

/// Send a signal to a process via `nix::sys::signal::kill`.
///
/// Maps platform errors to the typed `SysError` variants:
/// - `ESRCH` → `NotFound`
/// - `EPERM`/`EACCES` → `PermissionDenied`
/// - `EINVAL` → `InvalidInput`
/// - anything else → `Other`
///
/// PID 0 is rejected explicitly (POSIX kill(2) treats 0 as "the whole
/// process group", which is a footgun we don't want to expose).
pub(crate) fn kill_process(pid: Pid, sig: Signal) -> Result<(), SysError> {
    if pid.as_u32() == 0 {
        return Err(SysError::InvalidInput(
            "refusing to send signal to pid 0 (process-group semantics)".into(),
        ));
    }
    // POSIX kill(2) treats negative pids as process-group broadcasts. A naive
    // `u32 as i32` cast turns large pids (e.g. u32::MAX → -1) into "signal
    // every process the caller can signal" — including this process. Reject
    // anything that won't fit in a positive i32.
    let raw_pid: i32 = match i32::try_from(pid.as_u32()) {
        Ok(v) if v > 0 => v,
        _ => return Err(SysError::NotFound(format!("pid {}", pid.as_u32()))),
    };
    let nix_sig = match sig {
        Signal::Term => nix::sys::signal::Signal::SIGTERM,
        Signal::Kill => nix::sys::signal::Signal::SIGKILL,
        Signal::Int => nix::sys::signal::Signal::SIGINT,
        Signal::Hup => nix::sys::signal::Signal::SIGHUP,
    };
    let nix_pid = nix::unistd::Pid::from_raw(raw_pid);
    match nix::sys::signal::kill(nix_pid, nix_sig) {
        Ok(()) => Ok(()),
        Err(Errno::ESRCH) => Err(SysError::NotFound(format!("pid {}", pid.as_u32()))),
        Err(Errno::EPERM) | Err(Errno::EACCES) => Err(SysError::PermissionDenied(format!(
            "cannot signal pid {} (likely owned by another user)",
            pid.as_u32()
        ))),
        Err(Errno::EINVAL) => Err(SysError::InvalidInput(format!(
            "invalid signal for pid {}",
            pid.as_u32()
        ))),
        Err(e) => Err(SysError::Other(format!("kill({}): {e}", pid.as_u32()))),
    }
}
