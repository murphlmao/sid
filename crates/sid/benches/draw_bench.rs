//! Criterion benchmark for the `wire::draw` render loop.
//!
//! Uses `ratatui::backend::TestBackend` so no real terminal is required.
//! Measures how long it takes to render one full frame into a fake buffer.

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use sid::wire::{NoopSystemctlClient, NoopTerminalSpawner, SidApp, build_app, draw};
use sid_store::{OpenStore, RedbStore, Store};
use tempfile::tempdir;

fn build_bench_sid_app(start_tab: Option<&str>) -> SidApp {
    let dir = tempdir().expect("tempdir");
    let db_file = dir.path().join("bench.redb");
    let store = Arc::new(RedbStore::open(&db_file).expect("open redb"));
    // Leak tempdir so it survives the benchmark loop.
    std::mem::forget(dir);
    let secrets: Arc<dyn sid_core::adapters::secrets::SecretStore> = Arc::new(
        sid_secrets::PlainStore::new(Arc::clone(&store) as Arc<dyn Store>),
    );
    SidApp {
        app: build_app(start_tab, vec![]),
        store,
        session_id: "bench-sess".into(),
        sys_probe: None,
        systemctl: Arc::new(NoopSystemctlClient),
        spawner: Arc::new(NoopTerminalSpawner),
        postgres: sid_db_clients::PostgresClient::factory(),
        sqlite: sid_db_clients::SqliteClient::factory(),
        secrets,
    }
}

fn bench_draw_loop(c: &mut Criterion) {
    let sid_app = build_bench_sid_app(None);
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");

    c.bench_function("wire::draw one frame (120×40)", |b| {
        b.iter(|| {
            terminal
                .draw(|frame| draw(frame, &sid_app))
                .expect("draw should not fail");
        });
    });
}

fn bench_draw_small_terminal(c: &mut Criterion) {
    let sid_app = build_bench_sid_app(None);
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");

    c.bench_function("wire::draw one frame (40×10 small)", |b| {
        b.iter(|| {
            terminal
                .draw(|frame| draw(frame, &sid_app))
                .expect("draw should not fail");
        });
    });
}

fn bench_draw_with_start_tab(c: &mut Criterion) {
    let sid_app = build_bench_sid_app(Some("settings"));
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");

    c.bench_function("wire::draw one frame (settings tab)", |b| {
        b.iter(|| {
            terminal
                .draw(|frame| draw(frame, &sid_app))
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
