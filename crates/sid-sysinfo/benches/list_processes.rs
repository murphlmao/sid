//! Criterion benchmark: SysinfoProvider::list_processes.
//!
//! Baseline is the time to refresh the process list once. Per CLAUDE.md,
//! CI should fail if this regresses ≥10% versus the committed baseline.

use criterion::{Criterion, criterion_group, criterion_main};
use sid_core::adapters::sys::SysProvider;
use sid_sysinfo::SysinfoProvider;

fn bench_list_processes(c: &mut Criterion) {
    let mut provider = SysinfoProvider::new();
    c.bench_function("list_processes", |b| {
        b.iter(|| {
            let _ = provider.list_processes().unwrap();
        });
    });
}

criterion_group!(benches, bench_list_processes);
criterion_main!(benches);
