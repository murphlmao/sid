//! Buttons activate on click and on Enter/Space; a disabled button does neither.

mod common;

use gpui_kit::AppContext as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px, size};
use sid_ui::Button;

struct Probe {
    live: usize,
    dead: usize,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Button::new("live", "Live")
                    .tab_index(1)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.live += 1;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("dead", "Dead")
                    .tab_index(2)
                    .disabled(true)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dead += 1;
                        cx.notify();
                    })),
            )
    }
}

fn open(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::WindowHandle<gpui_kit::base::Root>,
    gpui_kit::Entity<Probe>,
) {
    common::open_window(cx, size(px(400.), px(300.)), |_, cx| {
        cx.new(|_| Probe { live: 0, dead: 0 })
    })
}

#[gpui_kit::test]
fn click_activates_a_live_button(cx: &mut TestAppContext) {
    let (handle, probe) = open(cx);
    cx.update_window(handle.into(), |_, window, cx| window.click("live", cx))
        .unwrap();
    assert_eq!(probe.read_with(cx, |p, _| p.live), 1);
}

#[gpui_kit::test]
fn enter_and_space_activate_the_focused_button(cx: &mut TestAppContext) {
    let (handle, probe) = open(cx);
    // Bare `Tab` is bound by the app shell, not by sid-ui, so move focus the way the shell
    // does: `Window::focus_next`. The disabled button is not a tab stop, so it lands on
    // the live one.
    cx.update_window(handle.into(), |_, window, cx| {
        window.focus_next(cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("live").focused(),
            Some(true),
            "focus_next lands on it"
        );
    })
    .unwrap();
    common::press(handle, "enter", cx);
    assert_eq!(probe.read_with(cx, |p, _| p.live), 1, "enter");
    common::press(handle, "space", cx);
    assert_eq!(probe.read_with(cx, |p, _| p.live), 2, "space");
}

#[gpui_kit::test]
fn a_disabled_button_ignores_click_and_keys(cx: &mut TestAppContext) {
    let (handle, probe) = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("dead", cx);
    })
    .unwrap();
    // Focus whatever is focusable (nothing but the live button), then try the keys.
    cx.update_window(handle.into(), |_, window, cx| window.focus_next(cx))
        .unwrap();
    for key in ["enter", "space"] {
        common::press(handle, key, cx);
    }
    assert_eq!(probe.read_with(cx, |p, _| p.dead), 0);
}
