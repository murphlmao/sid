//! Criterion benchmark: SysinfoProvider::list_listening_ports.
//!
//! Baseline is the time to enumerate sockets via `netstat2` and resolve
//! PID → command via the cached `sysinfo::System`. Per CLAUDE.md, CI
//! should fail if this regresses ≥10% versus the committed baseline.

use criterion::{Criterion, criterion_group, criterion_main};
use sid_core::adapters::sys::SysProvider;
use sid_sysinfo::SysinfoProvider;

fn bench_list_listening_ports(c: &mut Criterion) {
    let mut provider = SysinfoProvider::new();
    c.bench_function("list_listening_ports", |b| {
        b.iter(|| {
            let _ = provider.list_listening_ports().unwrap();
        });
    });
}

criterion_group!(benches, bench_list_listening_ports);
criterion_main!(benches);
