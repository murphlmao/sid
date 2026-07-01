//! sid — a native GPUI desktop cockpit for developer workflow.
//!
//! Entry point: open the global store (seeding a demo set on first run), then open the
//! window over the single [`app::AppState`] entity.

mod app;

use gpui::{Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};

fn main() {
    Application::new().run(|cx| {
        let store = app::open_store();
        // Secret backend bootstrap: keyring if it passes a startup probe, else an
        // in-memory fallback. Not yet surfaced in the UI (P3.2/A4) — logged for now.
        let (_secrets, secrets_warning) = app::open_secrets();
        if let Some(warning) = secrets_warning {
            eprintln!("sid: {warning}");
        }
        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_cx| app::AppState::new(store)),
        )
        .unwrap();
        cx.activate(true);
    });
}
