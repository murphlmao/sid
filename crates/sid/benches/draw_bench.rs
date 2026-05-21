//! Criterion benchmark for the `wire::draw` render loop.
//!
//! Uses `ratatui::backend::TestBackend` so no real terminal is required.
//! Measures how long it takes to render one full frame into a fake buffer.

use criterion::{Criterion, criterion_group, criterion_main};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use sid::wire::{build_app, draw};

fn bench_draw_loop(c: &mut Criterion) {
    // Build a minimal SidApp with all six tabs wired up.
    let app = build_app(None, vec![]);

    // A fake 120×40 terminal — large enough for the tab bar + body.
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");

    c.bench_function("wire::draw one frame (120×40)", |b| {
        b.iter(|| {
            terminal
                .draw(|frame| draw(frame, &app))
                .expect("draw should not fail");
        });
    });
}

fn bench_draw_small_terminal(c: &mut Criterion) {
    let app = build_app(None, vec![]);
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");

    c.bench_function("wire::draw one frame (40×10 small)", |b| {
        b.iter(|| {
            terminal
                .draw(|frame| draw(frame, &app))
                .expect("draw should not fail");
        });
    });
}

fn bench_draw_with_start_tab(c: &mut Criterion) {
    // Bench with a non-default starting tab to catch any tab-specific overhead.
    let app = build_app(Some("settings"), vec![]);
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");

    c.bench_function("wire::draw one frame (settings tab)", |b| {
        b.iter(|| {
            terminal
                .draw(|frame| draw(frame, &app))
                .expect("draw should not fail");
        });
    });
}

criterion_group!(
    benches,
    bench_draw_loop,
    bench_draw_small_terminal,
    bench_draw_with_start_tab
);
criterion_main!(benches);
