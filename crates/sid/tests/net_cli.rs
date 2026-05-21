//! End-to-end tests for the `sid net …` CLI subcommands.

use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

fn sid_bin() -> &'static str {
    env!("CARGO_BIN_EXE_sid")
}

// ---- ports ----

#[test]
fn sid_net_ports_table_includes_a_bound_port() {
    // Bind a socket so there is a guaranteed visible listening port.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let bound = listener.local_addr().unwrap().port();
    let out = Command::new(sid_bin())
        .args(["net", "ports"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&bound.to_string()),
        "expected port {bound} in `sid net ports` output:\n{stdout}",
    );
}

#[test]
fn sid_net_ports_json_parses() {
    let out = Command::new(sid_bin())
        .args(["net", "ports", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v.is_array());
}

// ---- procs ----

#[test]
fn sid_net_procs_table_includes_a_pid() {
    let out = Command::new(sid_bin())
        .args(["net", "procs", "--top", "200"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PID"),
        "table header missing PID column:\n{stdout}",
    );
    // Output must include at least one numeric row beyond the header.
    let line_count = stdout.lines().count();
    assert!(line_count >= 2, "expected multiple lines, got\n{stdout}");
}

#[test]
fn sid_net_procs_json_parses() {
    let out = Command::new(sid_bin())
        .args(["net", "procs", "--format", "json", "--top", "5"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v.is_array());
    assert!(v.as_array().unwrap().len() <= 5);
}

#[test]
fn sid_net_procs_sort_by_cpu_does_not_error() {
    let out = Command::new(sid_bin())
        .args(["net", "procs", "--sort", "cpu", "--top", "5"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- interfaces ----

#[test]
fn sid_net_interfaces_includes_lo_or_similar() {
    let out = Command::new(sid_bin())
        .args(["net", "interfaces"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("NAME"), "table header missing:\n{stdout}");
    // At least one row beyond the header.
    let lines = stdout.lines().count();
    assert!(
        lines >= 2,
        "expected interfaces beyond the header:\n{stdout}"
    );
}

#[test]
fn sid_net_interfaces_json_parses() {
    let out = Command::new(sid_bin())
        .args(["net", "interfaces", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v.is_array());
}

// ---- kill ----

#[test]
fn sid_net_kill_pid_zero_is_rejected() {
    let out = Command::new(sid_bin())
        .args(["net", "kill", "0"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "kill 0 should fail");
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("invalid") || stderr.contains("refus") || stderr.contains("not found"),
        "unexpected stderr: {stderr}",
    );
}

#[test]
fn sid_net_kill_invalid_target_is_rejected() {
    let out = Command::new(sid_bin())
        .args(["net", "kill", "not-a-pid"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("invalid"),
        "stderr: {stderr}"
    );
}

#[test]
fn sid_net_kill_subprocess_force_terminates_it() {
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    let out = std::process::Command::new(sid_bin())
        .args(["net", "kill", &pid.to_string(), "--force"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("child not reaped after sid net kill --force");
}
