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
//! - `list_listening_ports`: enumerates sockets via `netstat2` on each call
//!   and uses the cached `sysinfo::System` only to map PID → command name.
//!   No `sysinfo` refresh is performed inside `list_listening_ports` itself,
//!   so PID-to-command attribution lags a `list_processes` call.
//! - `list_interfaces`: builds a fresh `sysinfo::Networks` each call. This
//!   is intentional — sysinfo's `Networks` is cheap relative to a full
//!   `System` refresh.
//! - `kill_process`: no `sysinfo` state read or written; signal delivery
//!   goes through `nix::sys::signal::kill` directly.

use sid_core::sys::{ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider};

pub mod default_route;
mod interfaces;
mod kill;
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
    inner: sysinfo::System,
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
        let mut inner = sysinfo::System::new();
        // Prime the CPU sampling baseline.
        inner.refresh_cpu_usage();
        Self { inner }
    }
}

impl Default for SysinfoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SysProvider for SysinfoProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        processes::list_processes(&mut self.inner)
    }

    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
        ports::list_listening_ports(&self.inner)
    }

    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
        interfaces::list_interfaces(&mut self.inner)
    }

    fn kill_process(&mut self, pid: Pid, sig: Signal) -> Result<(), SysError> {
        kill::kill_process(pid, sig)
    }

    fn default_route_iface_name(&mut self) -> Result<Option<String>, SysError> {
        default_route::read_default_route_iface()
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
