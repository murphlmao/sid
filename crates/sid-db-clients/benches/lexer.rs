use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use sid_db_clients::lexer::tokenize;

const SMALL: &str = "SELECT id, name FROM users WHERE id = 1";
const LARGE: &str = include_str!("../tests/fixtures/large_query.sql");

fn bench_small(c: &mut Criterion) {
    c.bench_function("lexer/small", |b| {
        b.iter(|| {
            let toks = tokenize(black_box(SMALL));
            black_box(toks);
        })
    });
}

fn bench_large(c: &mut Criterion) {
    c.bench_function("lexer/large", |b| {
        b.iter(|| {
            let toks = tokenize(black_box(LARGE));
            black_box(toks);
        })
    });
}

criterion_group!(benches, bench_small, bench_large);
criterion_main!(benches);
