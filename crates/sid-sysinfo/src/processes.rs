use sid_core::sys::{Pid, ProcessInfo, SysError};
use sysinfo::{ProcessRefreshKind, RefreshKind, ThreadKind, UpdateKind};

/// Whether a `sysinfo` entry is a *process* worth listing, given its
/// [`sysinfo::Process::thread_kind`].
///
/// On Linux `sysinfo` walks `/proc/<pid>/task/*` as well as `/proc/<pid>`, so a plain
/// `System::processes()` is a **task** list, not a process list: measured on the dev box,
/// 374 processes come back as 1457 rows — 1213 of them threads of something already in
/// the table (twelve identical `tokio-rt-worker` rows under one PID's command line). That
/// is wrong as a process monitor *and* it is the multiplier on every per-row cost in the
/// tick: the clone, the sort, the live-pid set, and the `/proc` reads behind them.
///
/// `Some(ThreadKind::Userland)` is exactly "a thread of a process that is itself in this
/// list", so that is what's dropped. `Some(ThreadKind::Kernel)` is kept — a kernel thread
/// (`kworker/*`, `kswapd0`) has no separate parent row to fold into, and an ops cockpit
/// that hid them would be hiding real CPU consumers.
///
/// # Examples
///
/// ```ignore
/// assert!(is_listable(None));                            // a process
/// assert!(is_listable(Some(ThreadKind::Kernel)));        // a kernel thread
/// assert!(!is_listable(Some(ThreadKind::Userland)));     // a thread of a process
/// ```
pub(crate) fn is_listable(thread_kind: Option<ThreadKind>) -> bool {
    !matches!(thread_kind, Some(ThreadKind::Userland))
}

/// What each refresh re-reads per process.
///
/// CPU and memory are the whole point of the poll, so they refresh every time. The
/// command line and the owning user are **immutable for the life of a PID** in every case
/// this tab cares about, and re-reading them is what made the poll expensive:
/// `UpdateKind::Always` costs one `/proc/<pid>/cmdline` open+read and one uid lookup for
/// every task on the machine, every 2 seconds. Measured on the dev box (1457 tasks, debug
/// build): `Always` 55ms per refresh, `OnlyIfNotSet` 28ms; release, 78ms vs 38ms — a ~49%
/// cut for data that cannot have changed. `OnlyIfNotSet` still reads both the first time
/// a PID is seen, so new processes arrive complete.
fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet)
}

/// Refresh + collect the list of processes. Cleaned up between calls by
/// `sysinfo::System::refresh_specifics` (sysinfo prunes dead processes
/// itself on each refresh).
///
/// Threads of a listed process are filtered out — see [`is_listable`].
pub(crate) fn list_processes(sys: &mut sysinfo::System) -> Result<Vec<ProcessInfo>, SysError> {
    sys.refresh_specifics(RefreshKind::nothing().with_processes(refresh_kind()));

    let mut out = Vec::with_capacity(sys.processes().len());
    for (pid, proc) in sys.processes() {
        if !is_listable(proc.thread_kind()) {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The row-count invariant the Systems tab depends on: userland threads are folded
    /// away, processes and kernel threads are not. Pinned as a pure predicate because the
    /// alternative — asserting a row count against the live machine — is a flake.
    #[test]
    fn only_userland_threads_are_folded_away() {
        assert!(is_listable(None), "a process is always listable");
        assert!(
            is_listable(Some(ThreadKind::Kernel)),
            "a kernel thread has no parent row to fold into"
        );
        assert!(
            !is_listable(Some(ThreadKind::Userland)),
            "a thread of a listed process must not be its own row"
        );
    }

    /// The live-machine contract: the returned list is a *process* list, so no entry may
    /// be a userland thread, and it must be strictly smaller than the raw task table that
    /// `sysinfo` hands back. Loose bounds only — this runs on any dev box or CI runner.
    #[test]
    fn list_processes_returns_processes_not_tasks() {
        let mut sys = sysinfo::System::new();
        let procs = list_processes(&mut sys).expect("list_processes should not error");
        assert!(!procs.is_empty(), "a real host always has some process");

        let tasks = sys.processes().len();
        let threads = sys
            .processes()
            .values()
            .filter(|p| p.thread_kind() == Some(ThreadKind::Userland))
            .count();
        assert_eq!(
            procs.len(),
            tasks - threads,
            "every non-userland-thread task should be listed exactly once"
        );
    }

    /// `OnlyIfNotSet` must still populate the command line the first time a PID is seen —
    /// the whole point is that it is read once, not never. This test's own process always
    /// has a command line, so it is a fair fixed point.
    #[test]
    fn the_command_line_is_populated_on_first_sight() {
        let mut sys = sysinfo::System::new();
        let procs = list_processes(&mut sys).expect("list_processes should not error");
        let me = std::process::id();
        let mine = procs
            .iter()
            .find(|p| p.pid.as_u32() == me)
            .expect("the test process should list itself");
        assert!(
            !mine.cmd.is_empty(),
            "cmd must be read on the refresh that first sees a PID"
        );
        assert!(mine.user.is_some(), "user must be read on first sight too");
    }
}
