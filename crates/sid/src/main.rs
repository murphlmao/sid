//! sid — a native GPUI desktop cockpit for developer workflow.
//!
//! Entry point: open the global store (seeding a demo set on first run) and the secret
//! backend, then open the window over the single [`app::AppState`] entity.

mod app;
// W2: DbKind -> client/descriptor wiring, constructed by `ui::db_tab::DbTabState` (W3).
// `.client()`/`.descriptor()`/`.kinds()` are now all exercised for real: `.descriptor()`
// by the W4 connection form, `.client()` by the W5 query pane's Run/next-page flow.
mod db_registry;
// The keyboard-driven system (2026-07-02 plan): the `Action` enum + default binding
// registry. Pure/gpui-light — see that module's doc comment.
mod keymap;
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
            // `seed_lists` is `open_store`'s already-done hosts/workspaces reads (see
            // `app::SeedLists`) — threaded to `AppState::new` below so it doesn't
            // immediately re-read the same two tables (perf audit finding #7).
            let (store, seed_lists) = app::open_store();
            // Secret backend bootstrap: resolves keyring vs. in-memory from the
            // persisted `Settings.secret_keyring_enabled` toggle (see
            // `app::open_secrets`; round-D §A dropped the encrypted-file backend from
            // the chain). The status text (which backend is live, plus any warning) is
            // echoed to stderr for headless debugging, and shown in-app only when
            // degraded (the tab strip's warning badge — see `app::AppState::new`).
            let (secrets, secrets_degraded, secrets_status) = app::open_secrets(&store);
            eprintln!("sid: {secrets_status}");
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Wayland app_id (X11 WM_CLASS): without it the window has an
                    // EMPTY class, so compositor windowrules, taskbars, and the
                    // capture harness (scripts/sid-cap.sh) can't target sid.
                    app_id: Some("sid".into()),
                    ..Default::default()
                },
                |window, cx| {
                    Theme::change(ThemeMode::Dark, Some(window), cx);
                    // `gpui-component`'s `Input`/`Table` (W5) reach for a
                    // `gpui_component::Root` ancestor at render time — without it,
                    // rendering panics (`root.rs`'s `window.root::<Root>().expect(..)`).
                    // The window's first layer must be `Root`, not `AppState` directly.
                    let view = cx.new(|cx| {
                        app::AppState::new(
                            store,
                            seed_lists,
                            secrets,
                            secrets_degraded,
                            secrets_status,
                            cx,
                        )
                    });
                    cx.new(|cx| gpui_component::Root::new(view, window, cx))
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
