use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use sid_core::adapters::systemctl::UnitBus;
use sid_system::parse::parse_list_units;

fn build_200_unit_sample() -> String {
    let mut s = String::new();
    for i in 0..200 {
        s.push_str(&format!(
            "svc-{i:03}.service                          loaded active   running  service {i} description here\n"
        ));
    }
    s
}

fn bench_parse_list_units(c: &mut Criterion) {
    let sample = build_200_unit_sample();
    c.bench_function("parse_list_units_200", |b| {
        b.iter(|| {
            let _ = parse_list_units(black_box(&sample), UnitBus::System).unwrap();
        })
    });
}

criterion_group!(benches, bench_parse_list_units);
criterion_main!(benches);
