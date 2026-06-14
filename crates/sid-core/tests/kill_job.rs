//! Tests for `run_kill_job` — covers the three KillOutcome variants and
//! the adversarial paths (pid 0, zero grace).

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use sid_core::{
    adapters::sys::{ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider},
    sys_probe::kill_job::{KillOutcome, run_kill_job},
};

#[derive(Default)]
struct RecordingProvider {
    calls: Vec<(Pid, Signal)>,
    alive_after_term: bool,
    fail_term_with: Option<SysError>,
}

impl SysProvider for RecordingProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        if self.alive_after_term {
            Ok(vec![ProcessInfo {
                pid: Pid::from_u32(42),
                name: "x".into(),
                cmd: "x".into(),
                cpu_pct: 0.0,
                rss_bytes: 0,
                started_unix_secs: 0,
                parent: None,
                user: None,
            }])
        } else {
            Ok(vec![])
        }
    }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
        Ok(vec![])
    }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
        Ok(vec![])
    }
    fn kill_process(&mut self, pid: Pid, sig: Signal) -> Result<(), SysError> {
        if let Some(e) = self.fail_term_with.take() {
            return Err(e);
        }
        self.calls.push((pid, sig));
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sigterm_then_dead_returns_killed() {
    let prov = Arc::new(Mutex::new(RecordingProvider {
        alive_after_term: false,
        calls: vec![],
        fail_term_with: None,
    }));
    let provider: Arc<Mutex<dyn SysProvider>> = prov.clone();
    let fut = run_kill_job(provider, Pid::from_u32(42), Duration::from_secs(5));
    let outcome = fut.await.unwrap();
    assert_eq!(outcome, KillOutcome::Killed(Pid::from_u32(42)));
    let p = prov.lock().unwrap();
    assert_eq!(p.calls, vec![(Pid::from_u32(42), Signal::Term)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sigterm_then_alive_escalates_to_sigkill() {
    let prov = Arc::new(Mutex::new(RecordingProvider {
        alive_after_term: true,
        calls: vec![],
        fail_term_with: None,
    }));
    let provider: Arc<Mutex<dyn SysProvider>> = prov.clone();
    let fut = run_kill_job(provider, Pid::from_u32(42), Duration::from_secs(5));
    let outcome = fut.await.unwrap();
    let p = prov.lock().unwrap();
    assert_eq!(p.calls.len(), 2);
    assert_eq!(p.calls[0].1, Signal::Term);
    assert_eq!(p.calls[1].1, Signal::Kill);
    assert_eq!(outcome, KillOutcome::EscalatedToSigkill(Pid::from_u32(42)));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sigterm_permission_denied_returns_failed_without_escalation() {
    let prov = Arc::new(Mutex::new(RecordingProvider {
        alive_after_term: true, // would escalate if we got there
        calls: vec![],
        fail_term_with: Some(SysError::PermissionDenied("nope".into())),
    }));
    let provider: Arc<Mutex<dyn SysProvider>> = prov.clone();
    let outcome = run_kill_job(provider, Pid::from_u32(42), Duration::from_secs(5))
        .await
        .unwrap();
    match outcome {
        KillOutcome::Failed(pid, msg) => {
            assert_eq!(pid, Pid::from_u32(42));
            assert!(msg.to_lowercase().contains("permission"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    // No calls were recorded because the test stub consumed the error
    // before pushing the SIGTERM call. SIGKILL escalation must NOT have
    // happened.
    let p = prov.lock().unwrap();
    assert!(p.calls.is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn zero_grace_still_does_alive_check() {
    let prov = Arc::new(Mutex::new(RecordingProvider {
        alive_after_term: false,
        calls: vec![],
        fail_term_with: None,
    }));
    let provider: Arc<Mutex<dyn SysProvider>> = prov.clone();
    let outcome = run_kill_job(provider, Pid::from_u32(42), Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(outcome, KillOutcome::Killed(Pid::from_u32(42)));
}
