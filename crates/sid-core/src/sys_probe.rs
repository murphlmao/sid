//! `SysProbe` — periodic poller around a [`SysProvider`].
//!
//! The probe is the canonical concurrency hand-off point between the async
//! polling task and the synchronous render loop:
//!
//!  - The provider lives inside `Arc<Mutex<dyn SysProvider>>` so the poll
//!    task, kill-action job, and any other consumer can all reach it.
//!  - Snapshots will be broadcast over a Tokio broadcast channel (added in
//!    Task 10) so any number of subscribers (widgets, CLI processes,
//!    detached views) can receive them without blocking each other.
//!  - The CLAUDE.md loom directive applies: any code path that locks the
//!    mutex from multiple tasks is exercised under `#[cfg(loom)]`.
//!
//! This module only contains the skeleton in this task; the polling loop
//! and broadcast channel are added in the next plan step.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::adapters::sys::{ListeningPort, NetInterface, ProcessInfo, SysProvider};

/// A single point-in-time snapshot of all three lists produced by a
/// [`SysProvider`].
///
/// Snapshots are intentionally cheap to clone (everything is heap-owned
/// `Vec`/`String`), so broadcasting one through a Tokio channel is just a
/// reference bump plus a `Vec` clone per subscriber.
///
/// # Examples
///
/// ```
/// use sid_core::sys_probe::SysSnapshot;
///
/// let s = SysSnapshot::empty();
/// assert!(s.processes.is_empty());
/// assert!(s.listening_ports.is_empty());
/// assert!(s.interfaces.is_empty());
/// ```
#[derive(Clone, Debug, Default)]
pub struct SysSnapshot {
    /// Process list captured at this tick.
    pub processes: Vec<ProcessInfo>,
    /// Listening sockets captured at this tick.
    pub listening_ports: Vec<ListeningPort>,
    /// Network interfaces captured at this tick.
    pub interfaces: Vec<NetInterface>,
    /// Time the snapshot was assembled, seconds since UNIX epoch.
    /// `0` for snapshots produced before the clock was readable (effectively
    /// only the default constructor's output).
    pub captured_at_unix_secs: i64,
}

impl SysSnapshot {
    /// Construct an empty snapshot. Useful for tests and the
    /// "no probe ran yet" initial render.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::sys_probe::SysSnapshot;
    /// let s = SysSnapshot::empty();
    /// assert!(s.processes.is_empty());
    /// ```
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Periodic poller around a [`SysProvider`].
///
/// In this task the probe only stores its provider handle and configured
/// interval — the polling loop and broadcast channel come online in the
/// next plan step.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use std::time::Duration;
///
/// use sid_core::adapters::sys::{
///     ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
/// };
/// use sid_core::sys_probe::SysProbe;
///
/// struct Noop;
/// impl SysProvider for Noop {
///     fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> { Ok(vec![]) }
///     fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
///     fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
///     fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> { Ok(()) }
/// }
///
/// let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(Noop));
/// let probe = SysProbe::new(provider, Duration::from_secs(2));
/// assert_eq!(probe.interval(), Duration::from_secs(2));
/// ```
pub struct SysProbe {
    pub(crate) provider: Arc<Mutex<dyn SysProvider>>,
    pub(crate) interval: Duration,
}

impl SysProbe {
    /// Construct a new probe. `interval` is the duration between polls; the
    /// caller drives polling by calling the async `run()` method on a Tokio
    /// task in a later step.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use std::time::Duration;
    ///
    /// use sid_core::adapters::sys::{
    ///     ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
    /// };
    /// use sid_core::sys_probe::SysProbe;
    ///
    /// struct Noop;
    /// impl SysProvider for Noop {
    ///     fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> { Ok(vec![]) }
    ///     fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
    ///     fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
    ///     fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> { Ok(()) }
    /// }
    ///
    /// let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(Noop));
    /// let probe = SysProbe::new(provider, Duration::from_millis(500));
    /// assert_eq!(probe.interval(), Duration::from_millis(500));
    /// ```
    pub fn new(provider: Arc<Mutex<dyn SysProvider>>, interval: Duration) -> Self {
        Self { provider, interval }
    }

    /// Borrow a clone of the provider handle. Used by one-shot consumers
    /// (e.g., the kill action) that need to call methods directly outside
    /// the poll loop.
    pub fn provider(&self) -> Arc<Mutex<dyn SysProvider>> {
        Arc::clone(&self.provider)
    }

    /// Configured poll interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Mutate the poll interval. Effective on the next tick.
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::adapters::sys::{
        ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
    };

    struct CountingProvider {
        calls: AtomicU32,
    }
    impl CountingProvider {
        fn new() -> Self {
            Self {
                calls: AtomicU32::new(0),
            }
        }
    }
    impl SysProvider for CountingProvider {
        fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![])
        }
        fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
            Ok(vec![])
        }
        fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
            Ok(vec![])
        }
        fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> {
            Ok(())
        }
    }

    #[test]
    fn snapshot_empty_has_no_rows() {
        let s = SysSnapshot::empty();
        assert!(s.processes.is_empty());
        assert!(s.listening_ports.is_empty());
        assert!(s.interfaces.is_empty());
        assert_eq!(s.captured_at_unix_secs, 0);
    }

    #[test]
    fn probe_constructs_and_exposes_interval() {
        let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(CountingProvider::new()));
        let probe = SysProbe::new(provider, Duration::from_secs(2));
        assert_eq!(probe.interval(), Duration::from_secs(2));
    }

    #[test]
    fn set_interval_updates_field() {
        let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(CountingProvider::new()));
        let mut probe = SysProbe::new(provider, Duration::from_secs(2));
        probe.set_interval(Duration::from_millis(500));
        assert_eq!(probe.interval(), Duration::from_millis(500));
    }

    #[test]
    fn provider_handle_is_cloneable_arc() {
        let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(CountingProvider::new()));
        let probe = SysProbe::new(Arc::clone(&provider), Duration::from_secs(1));
        let handle = probe.provider();
        // Locking through the cloned handle goes to the same provider.
        let mut guard = handle.lock().unwrap();
        let _ = guard.list_processes();
        drop(guard);
        // Original provider also reflects the call.
        let mut guard = provider.lock().unwrap();
        let _ = guard.list_processes();
        // Two clones of Arc + one outside = strong count of 3.
        drop(guard);
        assert!(Arc::strong_count(&provider) >= 2);
    }
}
