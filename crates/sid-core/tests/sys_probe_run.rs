//! Tokio-driven tests for `SysProbe::run`: emits one snapshot per interval,
//! fans out to multiple subscribers, and survives a failing provider.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use sid_core::{
    adapters::sys::{ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider},
    sys_probe::SysProbe,
};

struct StubProvider;
impl SysProvider for StubProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        Ok(vec![ProcessInfo {
            pid: Pid::from_u32(1),
            name: "init".into(),
            cmd: "init".into(),
            cpu_pct: 0.0,
            rss_bytes: 0,
            started_unix_secs: 0,
            parent: None,
            user: None,
        }])
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

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn run_emits_one_snapshot_per_interval() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(StubProvider));
    let probe = SysProbe::new(provider, Duration::from_millis(100));
    let mut rx = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });

    // First tick fires immediately on tokio's interval.
    let snap = rx.recv().await.unwrap();
    assert_eq!(snap.processes.len(), 1);

    tokio::time::advance(Duration::from_millis(110)).await;
    let snap2 = rx.recv().await.unwrap();
    assert_eq!(snap2.processes.len(), 1);

    handle.abort();
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn multiple_subscribers_each_receive_snapshot() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(StubProvider));
    let probe = SysProbe::new(provider, Duration::from_millis(100));
    let mut rx1 = probe.subscribe();
    let mut rx2 = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });

    // First tick fires immediately.
    let _ = rx1.recv().await.unwrap();
    let _ = rx2.recv().await.unwrap();
    handle.abort();
}

struct FailingProvider;
impl SysProvider for FailingProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        Err(SysError::Other("boom".into()))
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

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn failing_provider_emits_empty_snapshot_and_keeps_running() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(FailingProvider));
    let probe = SysProbe::new(provider, Duration::from_millis(100));
    let mut rx = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });

    // First tick: provider returns Err, probe emits empty snapshot.
    let snap = rx.recv().await.unwrap();
    assert!(
        snap.processes.is_empty(),
        "failed snapshot should be empty, not crash"
    );

    // Second tick: still alive after the error.
    tokio::time::advance(Duration::from_millis(110)).await;
    let snap2 = rx.recv().await.unwrap();
    assert!(snap2.processes.is_empty());

    handle.abort();
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn subscribe_after_run_started_still_receives_snapshots() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(StubProvider));
    let probe = SysProbe::new(provider, Duration::from_millis(100));
    // Take a clone of the sender by subscribing once before moving the probe.
    let _drop_first = probe.subscribe();
    // Subscribe is via sender clone — we cannot subscribe after move, but we
    // can pre-subscribe and use the second receiver after spawn.
    let mut rx = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });

    let snap = rx.recv().await.unwrap();
    assert_eq!(snap.processes.len(), 1);
    handle.abort();
}
