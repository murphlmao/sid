//! sid — a native GPUI desktop cockpit for developer workflow.
//!
//! Entry point: open the global store (seeding a demo set on first run) and the secret
//! backend, then open the window over the single [`app::AppState`] entity.

mod app;
// W2: DbKind -> client/descriptor wiring, constructed by `ui::db_tab::DbTabState` (W3).
// `.client()`/`.descriptor()`/`.kinds()` are now all exercised for real: `.descriptor()`
// by the W4 connection form, `.client()` by the W5 query pane's Run/next-page flow.
mod db_registry;
mod ssh_connect;
mod ui;

use gpui::{Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};
use gpui_component::{Theme, ThemeMode};

fn main() {
    Application::new()
        // Bundled icon/font assets `gpui-component`'s widgets reference (e.g. `Table`
        // column sort chevrons) — required by W5's SQL editor + results table.
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx| {
            ui::init(cx);
            gpui_component::init(cx);

            let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
            let store = app::open_store();
            // Secret backend bootstrap: keyring if it passes a startup probe, else an
            // in-memory fallback. The warning is surfaced in the app's error line (and
            // echoed to stderr for headless debugging).
            let (secrets, secrets_warning) = app::open_secrets();
            if let Some(warning) = &secrets_warning {
                eprintln!("sid: {warning}");
            }
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    Theme::change(ThemeMode::Dark, Some(window), cx);
                    // `gpui-component`'s `Input`/`Table` (W5) reach for a
                    // `gpui_component::Root` ancestor at render time — without it,
                    // rendering panics (`root.rs`'s `window.root::<Root>().expect(..)`).
                    // The window's first layer must be `Root`, not `AppState` directly.
                    let view = cx.new(|_cx| app::AppState::new(store, secrets, secrets_warning));
                    cx.new(|cx| gpui_component::Root::new(view, window, cx))
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
