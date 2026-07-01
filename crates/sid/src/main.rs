//! sid — a native GPUI desktop cockpit for developer workflow.
//!
//! Entry point: open the global store (seeding a demo set on first run) and the secret
//! backend, then open the window over the single [`app::AppState`] entity.

mod app;
mod ui;

use gpui::{Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};

fn main() {
    Application::new().run(move |cx| {
        ui::init(cx);

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
            |_window, cx| cx.new(|_cx| app::AppState::new(store, secrets, secrets_warning)),
        )
        .unwrap();
        cx.activate(true);
    });
}
