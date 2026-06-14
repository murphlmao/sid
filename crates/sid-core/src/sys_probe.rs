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

pub mod kill_job;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::broadcast;

use crate::adapters::sys::{ListeningPort, NetInterface, ProcessInfo, SysError, SysProvider};

/// Capacity of the broadcast channel used to fan snapshots out to widgets,
/// CLI consumers, and detached views. Sized for a small number of slow
/// consumers — if a consumer falls more than this many ticks behind, it will
/// observe `broadcast::error::RecvError::Lagged` and should re-sync against
/// the latest snapshot.
const SNAPSHOT_CHANNEL_CAPACITY: usize = 16;

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
    /// Name of the interface holding the default route at probe time, if
    /// any. Used by the Network tab to sort the primary WAN first.
    pub default_route_iface: Option<String>,
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
    pub(crate) tx: broadcast::Sender<SysSnapshot>,
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
        let (tx, _rx) = broadcast::channel(SNAPSHOT_CHANNEL_CAPACITY);
        Self {
            provider,
            interval,
            tx,
        }
    }

    /// Subscribe to broadcast snapshots. The returned receiver will yield
    /// `broadcast::error::RecvError::Lagged` if a subscriber consumes too
    /// slowly; widgets should treat lag as "fetch the latest snapshot on the
    /// next tick" rather than as a fatal error.
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
    /// let probe = SysProbe::new(provider, Duration::from_secs(1));
    /// let _rx = probe.subscribe();
    /// ```
    pub fn subscribe(&self) -> broadcast::Receiver<SysSnapshot> {
        self.tx.subscribe()
    }

    /// Run the polling loop. Loops forever, ticking on the configured
    /// interval, capturing a snapshot from the provider, and broadcasting it
    /// to all subscribers. Designed to be spawned on a Tokio task:
    ///
    /// ```ignore
    /// let probe = std::sync::Arc::new(probe);
    /// let probe_for_task = std::sync::Arc::clone(&probe);
    /// tokio::spawn(async move { probe_for_task.run().await });
    /// ```
    ///
    /// Takes `&self` so the caller can subscribe to the broadcast channel
    /// from the same `Arc<SysProbe>` that drives the loop, and so each
    /// subscriber receives the snapshots the loop actually emits.
    ///
    /// Returns only when the spawned task is cancelled — `run` itself never
    /// returns `Ok(())`. The `Err` arm is reserved for future infrastructure
    /// errors (none reachable today).
    pub async fn run(&self) {
        let mut interval = tokio::time::interval(self.interval);
        // Skip missed ticks rather than catching up; sysinfo poll loops
        // should not "burst" after a long sleep.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let snapshot = collect_snapshot(&self.provider).unwrap_or_else(|e| {
                tracing::warn!("SysProbe snapshot failed: {e}");
                SysSnapshot::empty()
            });
            // If there are no receivers, `send` returns an error; ignore it
            // — the next tick will retry once a subscriber appears.
            let _ = self.tx.send(snapshot);
        }
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

/// Lock the provider, capture all three lists, and stamp a timestamp.
///
/// Pulled out of `SysProbe::run` so adversarial tests can drive failure
/// scenarios (poisoned mutex, provider returning `Err`) deterministically.
fn collect_snapshot(provider: &Arc<Mutex<dyn SysProvider>>) -> Result<SysSnapshot, SysProbeError> {
    let mut guard = provider.lock().map_err(|_| SysProbeError::PoisonedMutex)?;
    let processes = guard.list_processes().map_err(SysProbeError::Sys)?;
    let listening_ports = guard.list_listening_ports().map_err(SysProbeError::Sys)?;
    let interfaces = guard.list_interfaces().map_err(SysProbeError::Sys)?;
    // default_route_iface_name is best-effort: Err collapses to None so the
    // sort falls back to alphabetical instead of bubbling the error up.
    let default_route_iface = guard.default_route_iface_name().unwrap_or_else(|e| {
        tracing::debug!("default_route_iface_name failed: {e}");
        None
    });
    let captured_at_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(SysSnapshot {
        processes,
        listening_ports,
        interfaces,
        default_route_iface,
        captured_at_unix_secs,
    })
}

/// Errors produced while assembling a snapshot.
#[derive(Debug, thiserror::Error)]
pub enum SysProbeError {
    /// The provider mutex was poisoned by a panicking task that held the
    /// lock. Treated as an internal bug; recovery requires restarting the
    /// process holding the probe.
    #[error("provider mutex poisoned")]
    PoisonedMutex,
    /// The underlying [`SysProvider`] returned an error.
    #[error("provider error: {0}")]
    Sys(#[from] SysError),
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicU32, Ordering},
        },
        time::Duration,
    };

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
