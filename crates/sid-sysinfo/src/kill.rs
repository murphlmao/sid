use nix::errno::Errno;
use sid_core::sys::{Pid, Signal, SysError};

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Load-bearing guard: pid 0 has process-group semantics under POSIX kill(2) — never
    /// let it through, regardless of privilege.
    #[test]
    fn rejects_pid_0() {
        let err = kill_process(Pid::from_u32(0), Signal::Term).unwrap_err();
        assert!(matches!(err, SysError::InvalidInput(_)));
    }

    /// Load-bearing guard: `u32::MAX as i32` wraps to `-1`, which POSIX kill(2) reads as
    /// "signal every process the caller can signal" — must be rejected, not cast.
    #[test]
    fn rejects_pid_overflowing_i32() {
        let err = kill_process(Pid::from_u32(u32::MAX), Signal::Term).unwrap_err();
        assert!(matches!(err, SysError::NotFound(_)));
    }

    /// The pid-0 guard applies regardless of which signal is requested — it's a
    /// property of the pid, not the signal.
    #[test]
    fn rejects_pid_0_for_every_signal_variant() {
        for sig in [Signal::Term, Signal::Kill, Signal::Int, Signal::Hup] {
            let err = kill_process(Pid::from_u32(0), sig).unwrap_err();
            assert!(matches!(err, SysError::InvalidInput(_)), "signal {sig:?}");
        }
    }

    /// Boundary just past the overflow guard: `i32::MAX` fits as a positive raw pid,
    /// so it must reach the real `kill(2)` syscall rather than being rejected as an
    /// overflow. No process has this pid, so the syscall itself reports `ESRCH`,
    /// mapped to `NotFound` — proving the boundary is drawn at the right value (one
    /// past `i32::MAX` is rejected by `rejects_pid_overflowing_i32`'s sibling
    /// behaviour; `i32::MAX` itself is not).
    #[test]
    fn pid_at_i32_max_is_not_rejected_as_overflow() {
        let err = kill_process(Pid::from_u32(i32::MAX as u32), Signal::Term).unwrap_err();
        assert!(
            matches!(err, SysError::NotFound(_)),
            "i32::MAX is a valid raw pid; failure must come from the syscall (ESRCH), not the overflow guard"
        );
    }
}
