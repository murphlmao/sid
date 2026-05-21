use std::process::{Command, Stdio};

use sid_core::adapters::sys::{Pid, Signal, SysError, SysProvider};
use sid_sysinfo::SysinfoProvider;

fn spawn_sleep() -> std::process::Child {
    Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sleep")
}

#[test]
fn kill_term_a_subprocess_exits_it() {
    let mut child = spawn_sleep();
    let pid = Pid::from_u32(child.id());
    let mut p = SysinfoProvider::new();
    p.kill_process(pid, Signal::Term).expect("kill TERM");
    let exited = wait_for_exit(&mut child, std::time::Duration::from_secs(2));
    let _ = child.wait();
    assert!(exited, "child did not exit after SIGTERM");
}

#[test]
fn kill_nonexistent_pid_returns_not_found() {
    let mut p = SysinfoProvider::new();
    let err = p
        .kill_process(Pid::from_u32(u32::MAX), Signal::Term)
        .unwrap_err();
    assert!(matches!(err, SysError::NotFound(_)), "got {err:?}");
}

#[test]
fn kill_pid_zero_is_rejected() {
    let mut p = SysinfoProvider::new();
    let err = p.kill_process(Pid::from_u32(0), Signal::Term).unwrap_err();
    assert!(matches!(err, SysError::InvalidInput(_)), "got {err:?}");
}

#[test]
fn kill_init_pid_one_returns_permission_denied_or_not_found() {
    let mut p = SysinfoProvider::new();
    match p.kill_process(Pid::from_u32(1), Signal::Hup) {
        Ok(()) => {} // root or matching container init — allow.
        Err(SysError::PermissionDenied(_)) => {}
        Err(SysError::NotFound(_)) => {}
        Err(other) => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn kill_followed_by_sigkill_eventually_reaps() {
    let mut child = spawn_sleep();
    let pid = Pid::from_u32(child.id());
    let mut p = SysinfoProvider::new();
    let _ = p.kill_process(pid, Signal::Term);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = p.kill_process(pid, Signal::Kill);
    let exited = wait_for_exit(&mut child, std::time::Duration::from_secs(2));
    let _ = child.wait();
    assert!(exited, "child did not exit after SIGTERM+SIGKILL");
}

fn wait_for_exit(child: &mut std::process::Child, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if let Ok(Some(_)) = child.try_wait() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    false
}

use proptest::prelude::*;

proptest! {
    /// Property: any sufficiently-large random pid returns NotFound (not panic, not other).
    #[test]
    fn prop_huge_pid_is_not_found(pid_raw in (u32::MAX / 2)..u32::MAX) {
        let mut p = SysinfoProvider::new();
        let err = p.kill_process(Pid::from_u32(pid_raw), Signal::Term).unwrap_err();
        prop_assert!(matches!(err, SysError::NotFound(_)));
    }
}
