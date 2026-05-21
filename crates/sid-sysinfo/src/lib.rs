//! `SysinfoProvider` — composed `SysProvider` implementation.
//!
//! Internally a `SysinfoProvider` holds:
//!   - a `sysinfo::System` for processes + interfaces
//!   - a per-call `netstat2` iterator for listening ports
//!   - no persistent state for kill (each `kill_process` call is independent)
//!
//! All access to the inner `sysinfo::System` is serialized via `&mut self`;
//! the `SysProbe` in `sid-core` wraps the provider in `Arc<Mutex<…>>` for
//! cross-task sharing.

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
};

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
}
