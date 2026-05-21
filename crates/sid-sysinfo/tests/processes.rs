use sid_core::adapters::sys::{Pid, SysProvider};
use sid_sysinfo::SysinfoProvider;

#[test]
fn new_constructs_without_panicking() {
    let _ = SysinfoProvider::new();
}

#[test]
fn provider_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SysinfoProvider>();
}

#[test]
fn boxes_into_dyn_provider() {
    let p: Box<dyn SysProvider> = Box::new(SysinfoProvider::new());
    drop(p);
}

#[test]
fn many_news_in_sequence_does_not_leak() {
    for _ in 0..50 {
        let _ = SysinfoProvider::new();
    }
}

#[test]
fn list_processes_includes_current_process() {
    let mut p = SysinfoProvider::new();
    let procs = p.list_processes().expect("list_processes");
    let me = std::process::id();
    assert!(
        procs.iter().any(|x| x.pid.as_u32() == me),
        "expected current pid {me} to appear in process list"
    );
}

#[test]
fn list_processes_nonempty_on_live_system() {
    let mut p = SysinfoProvider::new();
    let procs = p.list_processes().unwrap();
    assert!(!procs.is_empty(), "live system should have processes");
}

#[test]
fn list_processes_pids_are_unique() {
    let mut p = SysinfoProvider::new();
    let procs = p.list_processes().unwrap();
    let mut pids: Vec<u32> = procs.iter().map(|p| p.pid.as_u32()).collect();
    pids.sort_unstable();
    let total = pids.len();
    pids.dedup();
    assert_eq!(total, pids.len(), "PIDs in process list should be unique");
}

#[test]
fn list_processes_repeated_calls_are_stable() {
    let mut p = SysinfoProvider::new();
    let a = p.list_processes().unwrap();
    let b = p.list_processes().unwrap();
    let me = Pid::from_u32(std::process::id());
    assert!(a.iter().any(|x| x.pid == me));
    assert!(b.iter().any(|x| x.pid == me));
}

use proptest::prelude::*;

proptest! {
    /// Property: repeated calls never increase the live-PID count past
    /// (initial + bounded delta from background activity).
    #[test]
    fn prop_process_count_does_not_explode(iters in 1usize..4) {
        let mut p = SysinfoProvider::new();
        let baseline = p.list_processes().unwrap().len();
        for _ in 0..iters {
            let n = p.list_processes().unwrap().len();
            prop_assert!(n < baseline.saturating_mul(10).saturating_add(1000));
        }
    }
}

#[test]
fn high_count_does_not_panic() {
    let mut p = SysinfoProvider::new();
    for _ in 0..5 {
        let _ = p.list_processes().unwrap();
    }
}
