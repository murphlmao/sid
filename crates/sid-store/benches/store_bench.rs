use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use sid_core::tab::TabId;
use sid_core::widget::WidgetId;
use sid_store::{OpenStore, RedbStore, SessionRecord, SettingValue, Store, WidgetState};
use tempfile::tempdir;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_store() -> (tempfile::TempDir, RedbStore) {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    (dir, store)
}

fn make_session(id: &str) -> SessionRecord {
    SessionRecord {
        id: id.into(),
        started_at: 1_000_000,
        last_active: 2_000_000,
        ended_at: None,
        active_tab: Some(TabId::new("workspaces")),
        open_tabs: vec![TabId::new("workspaces"), TabId::new("ssh")],
    }
}

// ── Benchmarks ────────────────────────────────────────────────────────────────

/// Measure `Store::put_setting` for a 1 KB value.
fn bench_put_setting(c: &mut Criterion) {
    let (_dir, store) = make_store();
    let key = "bench.setting";
    let val = SettingValue(vec![0x42u8; 1024]);

    c.bench_function("put_setting_1kb", |b| {
        b.iter(|| {
            store.put_setting(key, &val).unwrap();
        });
    });
}

/// Measure cached read (get) after a put.
fn bench_get_setting_hit(c: &mut Criterion) {
    let (_dir, store) = make_store();
    let key = "bench.setting.read";
    let val = SettingValue(vec![0xAAu8; 1024]);
    store.put_setting(key, &val).unwrap();

    c.bench_function("get_setting_hit_1kb", |b| {
        b.iter(|| {
            let _ = store.get_setting(key).unwrap();
        });
    });
}

/// Measure `Store::save_widget_state` for a 100-byte blob.
fn bench_save_widget_state_small(c: &mut Criterion) {
    let tab = TabId::new("workspaces");
    let widget = WidgetId::new("workspaces.root");
    let blob = vec![0x01u8; 100];

    c.bench_function("save_widget_state_small_100b", |b| {
        b.iter_batched(
            make_store,
            |(_dir, store)| {
                store
                    .save_widget_state(&WidgetState {
                        tab_id: tab.clone(),
                        widget_id: widget.clone(),
                        blob: blob.clone(),
                    })
                    .unwrap();
            },
            BatchSize::SmallInput,
        );
    });
}

/// Measure `Store::save_widget_state` for a 10 KB blob.
fn bench_save_widget_state_medium(c: &mut Criterion) {
    let tab = TabId::new("workspaces");
    let widget = WidgetId::new("workspaces.root");
    let blob = vec![0x02u8; 10 * 1024];

    c.bench_function("save_widget_state_medium_10kb", |b| {
        b.iter_batched(
            make_store,
            |(_dir, store)| {
                store
                    .save_widget_state(&WidgetState {
                        tab_id: tab.clone(),
                        widget_id: widget.clone(),
                        blob: blob.clone(),
                    })
                    .unwrap();
            },
            BatchSize::SmallInput,
        );
    });
}

/// Measure `Store::load_widget_state` after a save (warm read).
fn bench_load_widget_state(c: &mut Criterion) {
    let (_dir, store) = make_store();
    let tab = TabId::new("workspaces");
    let widget = WidgetId::new("workspaces.root");
    store
        .save_widget_state(&WidgetState {
            tab_id: tab.clone(),
            widget_id: widget.clone(),
            blob: vec![0x03u8; 1024],
        })
        .unwrap();

    c.bench_function("load_widget_state_1kb", |b| {
        b.iter(|| {
            let _ = store.load_widget_state(&tab, &widget).unwrap();
        });
    });
}

/// Measure a full session round-trip: upsert + current_session + end_session.
fn bench_session_round_trip(c: &mut Criterion) {
    let (_dir, store) = make_store();
    let session = make_session("bench-sess");

    c.bench_function("session_round_trip", |b| {
        b.iter(|| {
            store.upsert_session(&session).unwrap();
            let _ = store.current_session().unwrap();
            store.end_session(&session.id, 9_999_999).unwrap();
            // Re-upsert so next iteration starts clean.
            store
                .upsert_session(&SessionRecord {
                    ended_at: None,
                    ..session.clone()
                })
                .unwrap();
        });
    });
}

criterion_group!(
    store_benches,
    bench_put_setting,
    bench_get_setting_hit,
    bench_save_widget_state_small,
    bench_save_widget_state_medium,
    bench_load_widget_state,
    bench_session_round_trip,
);
criterion_main!(store_benches);
