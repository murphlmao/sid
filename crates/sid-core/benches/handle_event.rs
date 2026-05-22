//! Criterion bench for `App::handle_event` on the noop path.
//!
//! CLAUDE.md mandates a 1 µs budget for this hot path. The bench feeds an
//! unbound `F(12)` chord so dispatch:
//! 1. Skips the palette intercept (palette closed).
//! 2. Looks up the chord in the keybind map (miss).
//! 3. Forwards to the active widget (stub that returns Bubble).
//! 4. Drains zero pending actions.
//!
//! This exercises every non-action arm of the dispatch in a few hundred
//! cycles per call. Regressions on this bench indicate someone added an
//! unconditional allocation or a hash-table miss in the hot path.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::action::ActionRegistry;
use sid_core::app::App;
use sid_core::context::WidgetCtx;
use sid_core::event::{Event, KeyChord};
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabKind, TabManager};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

struct Stub {
    id: WidgetId,
}

impl Widget for Stub {
    fn id(&self) -> &WidgetId {
        &self.id
    }
    fn title(&self) -> &str {
        "stub"
    }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome {
        EventOutcome::Bubble
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn make_app() -> App {
    let tabs = TabManager::new(vec![Tab {
        id: TabId::new("a"),
        title: "A".into(),
        layout: Layout::Single(Box::new(Stub {
            id: WidgetId::new("w"),
        })),
        hotkey: None,
        kind: TabKind::Core,
    }]);
    App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new())
}

fn bench_app_handle_event_noop(c: &mut Criterion) {
    let mut app = make_app();
    let chord = Event::Key(KeyChord::new(KeyCode::F(12), KeyModifiers::NONE));
    c.bench_function("app_handle_event_noop", |b| {
        b.iter(|| {
            let _ = app.handle_event(black_box(&chord));
        });
    });
}

criterion_group!(benches, bench_app_handle_event_noop);
criterion_main!(benches);
