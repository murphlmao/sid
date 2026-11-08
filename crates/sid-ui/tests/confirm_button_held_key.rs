//! A held key (keyboard auto-repeat) or a fast double activation must not fire the armed
//! second press of a `ConfirmButton`.

mod common;

use std::time::Instant;

use gpui_kit::AppContext as _;
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px, size};
use sid_ui::{Confirm, ConfirmArm, ConfirmButton};

#[derive(Default)]
struct Probe {
    arm: ConfirmArm<u32>,
    fired: usize,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let armed = self.arm.is_armed(1, Instant::now());
        div().child(
            ConfirmButton::new("kill", "kill")
                .armed(armed)
                .on_press(cx.listener(|this, _, _, cx| {
                    if this.arm.press(1, Instant::now()) == Confirm::Fire {
                        this.fired += 1;
                    }
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
    let (handle, probe) = common::open_window(cx, size(px(300.), px(120.)), |_, cx| {
        cx.new(|_| Probe::default())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.focus_next(cx);
        window.render_frame(cx);
    })
    .unwrap();
    (handle, probe)
}

#[gpui_kit::test]
fn a_held_enter_arms_but_never_fires(cx: &mut TestAppContext) {
    let (handle, probe) = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.key_down("enter", false, cx);
        for _ in 0..20 {
            window.key_down("enter", true, cx);
        }
        window.key_up("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    probe.read_with(cx, |p, _| {
        assert_eq!(p.fired, 0, "auto-repeat must not confirm");
        assert!(
            p.arm.is_armed(1, Instant::now()),
            "the first press armed it"
        );
    });
}

#[gpui_kit::test]
fn a_rapid_second_activation_does_not_confirm(cx: &mut TestAppContext) {
    let (handle, probe) = open(cx);
    common::press(handle, "enter", cx);
    common::press(handle, "enter", cx);
    assert_eq!(probe.read_with(cx, |p, _| p.fired), 0);
}
