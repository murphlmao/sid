//! `SysinfoProvider` — composed `SysProvider` implementation.
//!
//! Internally a `SysinfoProvider` holds:
//!   - a `sysinfo::System` for processes + interfaces
//!   - a per-call `netstat2` iterator for listening ports
//!   - no persistent state for kill (each `kill_process` call is independent)
//!
//! Ported from the `sid-poc` `sid-sysinfo` crate. The POC's `sid-core::sys_probe`
//! polling/broadcast service is deliberately **not** ported here — the Network tab
//! (inc-1) calls `list_*` directly on the runtime on refresh; a broadcast service is
//! over-built until a second consumer needs one.
//!
//! ## Refresh contract
//!
//! - `list_processes`: refreshes the process list, CPU, memory, user, and
//!   command line for every visible process (`sysinfo`'s `ProcessRefreshKind`
//!   wired to the matching subset).
//! - `list_listening_ports`: enumerates sockets via `netstat2` on each call,
//!   then refreshes *only* the socket-owning PIDs on the cached
//!   `sysinfo::System` (name-only, no dead-process pruning) to map PID →
//!   command name. It must do its own refresh: a consumer such as the Network
//!   tab never calls `list_processes`, so the handle would otherwise know about
//!   no process and every row's Process column would be blank.
//! - `list_interfaces`: builds a fresh `sysinfo::Networks` each call. This
//!   is intentional — sysinfo's `Networks` is cheap relative to a full
//!   `System` refresh.
//! - `kill_process`: no `sysinfo` state read or written; signal delivery
//!   goes through `nix::sys::signal::kill` directly.
//! - `overview`: refreshes CPU usage + memory on the same cached `System` used by
//!   `list_processes` (see `overview.rs`'s module doc for the CPU% warm-up caveat).

use std::collections::HashMap;

use sid_core::sys::{
    Connection, ListeningPort, NetInterface, Pid, ProcUsage, ProcessInfo, Signal, SysError,
    SysProvider, SystemOverview,
};

pub mod default_route;
mod interfaces;
mod kill;
mod overview;
mod ports;
mod processes;

/// Composed `SysProvider` impl backed by `sysinfo` (processes + interfaces),
/// `netstat2` (listening ports), and `nix` (signal delivery).
///
/// # Examples
///
/// ```
/// use sid_sysinfo::SysinfoProvider;
/// let _p = SysinfoProvider::new();
/// ```
pub struct SysinfoProvider {
    /// Cached sysinfo handle. `&mut self` access makes refreshes serialized.
    /// `None` when sysinfo couldn't initialise on this host; see `unavailable`.
    inner: Option<sysinfo::System>,
    /// Why `inner` is `None`; surfaced as the error of every sysinfo-backed probe.
    unavailable: Option<String>,
    /// Cached uid -> username table (refreshed rarely, not per poll).
    users: processes::UserNames,
}

impl SysinfoProvider {
    /// Construct a fresh `SysinfoProvider`. Performs an initial empty refresh
    /// so that later CPU% deltas are meaningful (sysinfo computes CPU% as a
    /// delta vs. the previous refresh).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_sysinfo::SysinfoProvider;
    /// let _p = SysinfoProvider::new();
    /// ```
    pub fn new() -> Self {
        let (inner, unavailable) = match sysinfo::System::new() {
            Ok(mut s) => {
                // Prime the CPU sampling baseline.
                s.refresh_cpu_usage();
                (Some(s), None)
            }
            Err(e) => (None, Some(e.to_string())),
        };
        Self {
            inner,
            unavailable,
            users: processes::UserNames::new(),
        }
    }

    /// Why system information is unavailable on this host, if it is.
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    fn sys(&mut self) -> Result<&mut sysinfo::System, SysError> {
        let reason = self.unavailable.clone();
        self.inner.as_mut().ok_or_else(|| unavail(reason))
    }
}

fn unavail(reason: Option<String>) -> SysError {
    SysError::Other(format!(
        "System information isn't available: {}",
        reason.as_deref().unwrap_or("unknown reason")
    ))
}

impl Default for SysinfoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SysProvider for SysinfoProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        let reason = self.unavailable.clone();
        let sys = self.inner.as_mut().ok_or_else(|| unavail(reason))?;
        processes::list_processes(sys, &mut self.users)
    }

    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
        ports::list_listening_ports(self.sys()?)
    }

    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
        interfaces::list_interfaces(self.sys()?)
    }

    fn overview(&mut self) -> Result<SystemOverview, SysError> {
        overview::overview(self.sys()?)
    }

    fn kill_process(&mut self, pid: Pid, sig: Signal) -> Result<(), SysError> {
        kill::kill_process(pid, sig)
    }

    fn default_route_iface_name(&mut self) -> Result<Option<String>, SysError> {
        default_route::read_default_route_iface()
    }

    fn list_connections(&mut self) -> Result<Vec<Connection>, SysError> {
        ports::list_connections(self.sys()?)
    }

    fn process_usage(&mut self, pids: &[Pid]) -> Result<HashMap<Pid, ProcUsage>, SysError> {
        ports::process_usage(self.sys()?, pids)
    }
}

// Compile-time assertion: `SysinfoProvider` is `Send + Sync`. The Network tab holds it
// behind `Arc<Mutex<dyn SysProvider>>` for cross-task sharing on the shared runtime,
// which requires both. `sysinfo::System` and `netstat2` provide auto-impls today; if a
// future release drops them, this assertion fails the build and we either wrap the
// inner state in a `Mutex` or follow the `unsafe impl Send + Sync` pattern used
// elsewhere in the tree (with a `// SAFETY:` comment).
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SysinfoProvider>();
};
