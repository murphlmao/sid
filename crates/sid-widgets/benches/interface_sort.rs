//! Criterion bench for `sort_interfaces` — the score-based sort used by
//! the Network tab's interfaces sidebar.
//!
//! 50 µs budget at n=100 per the interaction spec. Sort runs on every
//! SysSnapshot apply (~ once per second); gating prevents regressions if
//! the score function gets fancier later.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sid_core::adapters::sys::NetInterface;
use sid_widgets::network::interfaces_sidebar::sort_interfaces;

fn make_ifaces(n: usize) -> Vec<NetInterface> {
    (0..n)
        .map(|i| NetInterface {
            name: match i % 5 {
                0 => format!("wlan{i}"),
                1 => format!("eth{i}"),
                2 => format!("docker{i}"),
                3 => format!("veth_{i}"),
                _ => format!("tun{i}"),
            },
            addrs: vec![],
            rx_bytes: 0,
            tx_bytes: 0,
            is_up: i % 2 == 0,
        })
        .collect()
}

fn bench_sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("interface_sort");
    for n in [5usize, 20, 100] {
        let data = make_ifaces(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let mut d = data.clone();
                sort_interfaces(&mut d, Some("wlan0"));
                black_box(d.len())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_sort);
criterion_main!(benches);
