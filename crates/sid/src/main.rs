//! sid — a native GPUI desktop cockpit for developer workflow.
//!
//! Entry point: GPU pre-flight first (gpui's GPU-context init failure is an
//! uncatchable panic, so it's probed in a subprocess and self-healed *before*
//! `Application::new()` — see `sid_gpu`), then open the global store (seeding a
//! demo set on first run) and the secret backend, then open the window over the
//! single [`app::AppState`] entity.

mod app;
// W2: DbKind -> client/descriptor wiring, constructed by `ui::db_tab::DbTabState` (W3).
// `.client()`/`.descriptor()`/`.kinds()` are now all exercised for real: `.descriptor()`
// by the W4 connection form, `.client()` by the W5 query pane's Run/next-page flow.
mod db_registry;
// Workspaces tab (track U): `sid_git::Git2Provider` -> `sid_core::git::GitProvider`
// wiring, constructed by `ui::workspaces_tab::WorkspacesTabState`'s git fetches.
mod git_registry;
// The keyboard-driven system (2026-07-02 plan): the `Action` enum + default binding
// registry. Pure/gpui-light — see that module's doc comment.
mod keymap;
mod ssh_connect;
mod ui;

use gpui::{AnyView, Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};
use sid_core::gpu::{GpuPreflight as _, RenderPath};

fn main() {
    // ---- GPU pre-flight protocol modes -------------------------------------
    // Intercepted before ANYTHING else: the probe child must never touch the
    // store (its parent may hold redb's single-writer lock), secrets, or theme.
    match std::env::args().nth(1).as_deref() {
        Some("--gpu-probe") => run_gpu_probe(),
        Some("--gpu-report") => run_gpu_report(),
        _ => {}
    }

    // Startup logging (default `warn`; `RUST_LOG` overrides): blade/gpui report
    // real diagnostics through `log` — most importantly the exact failing
    // Vulkan call when GPU init breaks — and sid previously dropped every
    // record on the floor. `try_init` so a future double-init can't panic.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init()
        .ok();

    // GPU pre-flight (the fix for gpui's uncatchable "Unable to init GPU
    // context" startup panic): crash-test init in a subprocess, self-heal by
    // pinning a working driver manifest or falling back to software rendering,
    // and only hard-fail — with a human diagnosis instead of a panic — when no
    // rendering path exists at all. Cached by environment fingerprint, so
    // healthy machines pay nothing after their first launch.
    let preflight = sid_gpu::LinuxGpuPreflight::from_env(env!("CARGO_PKG_VERSION"));
    let render_soft_reason = match preflight.ensure_renderable() {
        Ok(ok) => {
            for (key, value) in &ok.env_pins {
                // SAFETY: no other thread exists yet — this runs before gpui/
                // tokio start anything, env_logger spawns none, and `sid_gpu`
                // guarantees no surviving threads (its subprocess I/O is
                // file+poll, threadless) — which is edition 2024's soundness
                // condition for `set_var`. The pins must be exported before
                // `Application::new()` below: the renderer reads them during
                // platform init.
                unsafe { std::env::set_var(key, value) };
            }
            match ok.path {
                RenderPath::Software { reason } => Some(reason),
                RenderPath::Hardware => None,
            }
        }
        Err(diagnosis) => {
            eprintln!("sid: cannot start: {}", diagnosis.summary);
            eprintln!("sid: {}", diagnosis.remedy);
            eprintln!();
            eprintln!("{}", diagnosis.detail);
            // sid may have been launched from a desktop entry where stderr is
            // invisible — best-effort notification, skipped if unavailable.
            sid_gpu::send_failure_notification(&diagnosis);
            std::process::exit(1);
        }
    };

    // If this marker survives to the next startup, GPU bring-up below died —
    // either the context panic in `Application::new()` or the renderer/surface
    // failure inside `cx.open_window()`. `ensure_renderable` then distrusts its
    // cached verdict and re-probes, so a machine that breaks *without* a
    // driver-fingerprint change still self-heals on the following launch.
    preflight.mark_launch_attempt();

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
            // Install the persisted theme as the process-wide palette global before
            // any window renders; the settings screen swaps it live via the same
            // `theme::install` + a refresh.
            // `SID_THEME` overrides the persisted theme for this run only (never
            // written back) — the capture harness's hook for shooting every palette
            // against a hermetic store, same convention as `SID_START_TAB`.
            let theme_name = std::env::var("SID_THEME").unwrap_or_else(|_| {
                store
                    .settings()
                    .map(|s| s.theme)
                    .unwrap_or_else(|_| "cosmos".into())
            });
            sid_ui::theme::install(&theme_name, cx);
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Wayland app_id (X11 WM_CLASS): without it the window has an
                    // EMPTY class, so compositor windowrules, taskbars, and the
                    // capture harness (scripts/sid-cap.sh) can't target sid.
                    app_id: Some("sid".into()),
                    ..Default::default()
                },
                |window, cx| {
                    // Project the active sid palette onto gpui-component's own theming
                    // system (a separate layer the borrowed `Input`/`Table`/`PopupMenu`
                    // widgets read) BEFORE first paint. Without this every borrowed
                    // widget renders in the library's stock shadcn palette — see
                    // `sid_ui::bridge`.
                    sid_ui::bridge::sync(Some(window), cx);
                    // `gpui-component`'s `Input`/`Table` (W5) reach for a
                    // `gpui_component::Root` ancestor at render time — without it,
                    // rendering panics (`root.rs`'s `window.root::<Root>().expect(..)`).
                    // The window's first layer must be `Root`, not `AppState` directly.
                    // `SID_GALLERY=1` mounts `sid-ui`'s component gallery instead of
                    // the app shell — the observation gate for the component crate
                    // (every widget, every variant, every state, on one canvas, in
                    // whichever palette `SID_THEME` selects). Dev-only by env gate:
                    // nothing in the app navigates to it.
                    let view: AnyView = if sid_ui::gallery::requested() {
                        cx.new(|_| sid_ui::gallery::Gallery::new()).into()
                    } else {
                        cx.new(|cx| {
                            app::AppState::new(
                                store,
                                seed_lists,
                                secrets,
                                secrets_degraded,
                                secrets_status,
                                render_soft_reason,
                                cx,
                            )
                        })
                        .into()
                    };
                    cx.new(|cx| gpui_component::Root::new(view, window, cx))
                },
            );
            // Only NOW is the marker's job done. `Application::new()` above proves
            // the GPU *context* only (gpui wayland/client.rs `BladeContext::new()`);
            // the renderer and its Vulkan surface are built in here —
            // `BladeRenderer::new(gpu_context, &raw_window, config)?` in gpui's
            // wayland/window.rs, over blade surface configuration that hard-`panic!`s
            // on an unexpected format/color-space pair. Clearing before this point
            // stamped "init is fine" over a crash that hadn't happened yet: the next
            // launch trusted the stamp, skipped the re-probe, and crash-looped in
            // silence. So the marker covers GPU context init AND window/surface
            // creation — everything that can die before a frame exists. Anything that
            // fails past this line is a *runtime* failure, and re-probing only heals
            // init failures, so holding the marker any longer would send unrelated
            // crashes through a pointless GPU probe on the next launch.
            if window.is_ok() {
                preflight.clear_launch_attempt();
            }
            window.unwrap();
            cx.activate(true);
        });
}

/// `sid --gpu-probe`: the crash-test child `sid_gpu`'s pre-flight spawns.
///
/// Constructing `Application` eagerly initializes the platform GPU context
/// (gpui-0.2.2 `src/platform.rs` `current_platform` → Wayland/X11 client →
/// `BladeContext::new().expect(..)`) — on a broken machine that is an
/// uncatchable panic, i.e. a nonzero exit, which is exactly the signal the
/// parent wants. On a healthy one, blade's `Adapter: "..."` log line lands on
/// stderr (the forced `blade_graphics=info` filter below) for the parent to
/// parse, and we exit 0 before any window opens.
fn run_gpu_probe() -> ! {
    env_logger::Builder::new()
        .parse_filters("warn,blade_graphics=info")
        .init();
    let _app = Application::new();
    std::process::exit(0);
}

/// `sid --gpu-report`: the support tool — run the full pre-flight live (never
/// mutating its cache) and print what a bug report needs: manifests, kernel
/// modules, the probe ladder's per-rung outcomes, and the diagnosis if all fail.
///
/// Exits nonzero when nothing can render, so it doubles as a scriptable "can
/// sid start on this machine?" health check instead of always claiming success.
fn run_gpu_report() -> ! {
    env_logger::Builder::new()
        .parse_filters("warn,blade_graphics=info")
        .init();
    let preflight = sid_gpu::LinuxGpuPreflight::from_env(env!("CARGO_PKG_VERSION"));
    let report = preflight.report();
    print!("{}", report.text);
    std::process::exit(if report.renderable { 0 } else { 1 });
}
