use sid_core::adapters::sys::{Pid, ProcessInfo, SysError};
use sysinfo::{ProcessRefreshKind, RefreshKind, UpdateKind};

/// Refresh + collect the list of processes. Cleaned up between calls by
/// `sysinfo::System::refresh_specifics` (sysinfo prunes dead processes
/// itself on each refresh).
pub(crate) fn list_processes(sys: &mut sysinfo::System) -> Result<Vec<ProcessInfo>, SysError> {
    // Refresh only what we need: process list + CPU + memory + command-line + user.
    sys.refresh_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_user(UpdateKind::Always)
                .with_cmd(UpdateKind::Always),
        ),
    );

    let mut out = Vec::with_capacity(sys.processes().len());
    for (pid, proc) in sys.processes() {
        let cmd_vec: Vec<String> = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let cmd = cmd_vec.join(" ");
        out.push(ProcessInfo {
            pid: Pid::from_u32(pid.as_u32()),
            name: proc.name().to_string_lossy().into_owned(),
            cmd,
            cpu_pct: proc.cpu_usage(),
            rss_bytes: proc.memory(),
            started_unix_secs: proc.start_time() as i64,
            parent: proc.parent().map(|p| Pid::from_u32(p.as_u32())),
            user: proc.user_id().map(|u| u.to_string()),
        });
    }
    Ok(out)
}
