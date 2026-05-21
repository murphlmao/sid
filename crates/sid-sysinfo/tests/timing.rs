//! Timing smoke test: 20 back-to-back polls of every `SysProvider` method
//! must complete inside a generous budget. Catches accidental quadratic
//! refresh regressions if `list_listening_ports` or `list_interfaces` ever
//! grow a hidden full `System` refresh.

use std::time::Instant;

use sid_core::adapters::sys::SysProvider;
use sid_sysinfo::SysinfoProvider;

#[test]
fn twenty_polls_complete_within_five_seconds() {
    let mut p = SysinfoProvider::new();
    let start = Instant::now();
    for _ in 0..20 {
        let _ = p.list_processes().expect("list_processes");
        let _ = p.list_listening_ports().expect("list_listening_ports");
        let _ = p.list_interfaces().expect("list_interfaces");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_secs_f64() < 5.0,
        "20 polls took {:?}, expected < 5s",
        elapsed
    );
}
