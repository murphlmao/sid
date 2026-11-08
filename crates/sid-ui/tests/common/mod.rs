//! Shared harness for the headless UI interaction tests (see `crates/sid-ui/CLAUDE.md`,
//! "UI interaction tests"). Each test file is its own crate, so unused helpers are fine.
#![allow(dead_code)]

use gpui_kit::{
    App, Bounds, Entity, Pixels, Point, Render, Size, TestAppContext, Window, WindowBounds,
    WindowHandle, WindowOptions, base::Root,
};

/// Opens `build`'s view in a headless window of `size`, under the base `Root` the
/// library widgets require, with sid's palette installed and bridged to the library theme
/// (the same order `main.rs` uses: init, install, `bridge::sync` before first paint).
pub fn open_window<V: Render>(
    cx: &mut TestAppContext,
    size: Size<Pixels>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> (WindowHandle<Root>, Entity<V>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        sid_ui::theme::install("cosmos", cx);
        let bounds = Bounds {
            origin: Point::default(),
            size,
        };
        let (window, content) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                sid_ui::bridge::sync(Some(window), cx);
                build(window, cx)
            },
        )
        .expect("open test window");
        (window.downcast::<Root>().expect("Base Root"), content)
    })
}

/// Sends one key through the real keymap and lets deferred focus work settle.
pub fn press(handle: WindowHandle<Root>, key: &str, cx: &mut TestAppContext) {
    use gpui_kit::AppContext as _;
    use gpui_kit::test::TestWindowExt as _;
    cx.update_window(handle.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}
