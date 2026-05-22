//! Criterion bench for settings sub-view dispatch.
//!
//! 200 µs budget per the interaction spec. Settings dispatch runs on
//! every key in the sub-views; gating prevents regressions if the
//! Outcome match grows.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::event::{Event, KeyChord};
use sid_widgets::settings::behavior_toggles::BehaviorTogglesView;

fn bench_dispatch(c: &mut Criterion) {
    let mut v = BehaviorTogglesView::defaults();
    let ev = Event::Key(KeyChord::new(KeyCode::Right, KeyModifiers::NONE));
    c.bench_function("settings_behavior_toggle_dispatch", |b| {
        b.iter(|| {
            let out = v.handle_event(black_box(&ev));
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_dispatch);
criterion_main!(benches);
