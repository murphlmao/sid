//! Criterion benches for the per-tab render hot path.
//!
//! Each bench builds one widget at its trivial default state and renders it
//! into a 120×40 ratatui TestBackend, then repeats in `iter`. Per the
//! interaction spec the per-tab budget is 8 ms at 120 Hz; anything beyond
//! that surfaces a tab-switch hesitation.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use sid_ui::themes::cosmos;
use sid_widgets::{
    DatabaseWidget, NetworkWidget, SettingsWidget, SshWidget, SystemWidget, WorkspacesWidget,
};

fn bench_workspaces_render(c: &mut Criterion) {
    let w = WorkspacesWidget::new(vec![], None);
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_workspaces", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme))
                .unwrap();
            black_box(())
        });
    });
}

fn bench_ssh_render(c: &mut Criterion) {
    let w = SshWidget::default();
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_ssh", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme))
                .unwrap();
            black_box(())
        });
    });
}

fn bench_database_render(c: &mut Criterion) {
    let w = DatabaseWidget::new(vec![]);
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_database", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme))
                .unwrap();
            black_box(())
        });
    });
}

fn bench_network_render(c: &mut Criterion) {
    let w = NetworkWidget::new();
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_network", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme))
                .unwrap();
            black_box(())
        });
    });
}

fn bench_system_render(c: &mut Criterion) {
    let w = SystemWidget::default();
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_system", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme))
                .unwrap();
            black_box(())
        });
    });
}

fn bench_settings_render(c: &mut Criterion) {
    let w = SettingsWidget::new();
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_settings", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme))
                .unwrap();
            black_box(())
        });
    });
}

criterion_group!(
    benches,
    bench_workspaces_render,
    bench_ssh_render,
    bench_database_render,
    bench_network_render,
    bench_system_render,
    bench_settings_render,
);
criterion_main!(benches);
