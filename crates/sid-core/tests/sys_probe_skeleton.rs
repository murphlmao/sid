//! Smoke tests for the `SysProbe` skeleton.

use std::{
    sync::{Arc, Mutex, atomic::AtomicU32},
    time::Duration,
};

use sid_core::{
    adapters::sys::{ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider},
    sys_probe::{SysProbe, SysSnapshot},
};

struct CountingProvider {
    processes_calls: AtomicU32,
}

impl CountingProvider {
    fn new() -> Self {
        Self {
            processes_calls: AtomicU32::new(0),
        }
    }
}

impl SysProvider for CountingProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        self.processes_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
fn sys_probe_constructs_with_provider() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(CountingProvider::new()));
    let probe = SysProbe::new(provider, Duration::from_secs(2));
    assert_eq!(probe.interval(), Duration::from_secs(2));
}

#[test]
fn snapshot_has_all_three_lists() {
    let s = SysSnapshot::empty();
    assert!(s.processes.is_empty());
    assert!(s.listening_ports.is_empty());
    assert!(s.interfaces.is_empty());
}

#[test]
fn provider_handle_can_be_locked_through_clone() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(CountingProvider::new()));
    let probe = SysProbe::new(provider, Duration::from_millis(100));
    let handle = probe.provider();
    let mut g = handle.lock().unwrap();
    let v = g.list_processes().unwrap();
    assert!(v.is_empty());
}
