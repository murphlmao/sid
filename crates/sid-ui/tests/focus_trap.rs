//! A `Modal` traps Tab: no number of presses walks focus out to the page behind the scrim,
//! and Shift-Tab wraps from the first stop to the last.
//!
//! This replaces the A/B screenshot run the 2026-09-09 cohesion loop used as its only
//! evidence ("25 Tabs stay inside the host form").

mod common;

use gpui_kit::component::input::InputState;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, Render, TestAppContext,
    Window, div, prelude::*, px, size,
};
use sid_ui::{Button, Modal, TextInput, modal};

struct Form {
    fields: [Entity<InputState>; 3],
}

impl Render for Form {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut m = Modal::new("trap", "Add host");
        for (ix, state) in self.fields.iter().enumerate() {
            m = m.child(TextInput::new(state).tab_index(ix as isize + 1));
        }
        let m = m
            .footer(Button::new("trap-cancel", "Cancel").tab_index(4))
            .footer(Button::new("trap-save", "Save").tab_index(5));
        div()
            .size_full()
            // The page behind the scrim: a tab stop that must never receive focus.
            .child(Button::new("outside", "Behind the scrim").tab_index(0))
            .child(modal::overlay(window, m))
    }
}

fn open(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::WindowHandle<gpui_kit::base::Root>,
    Entity<Form>,
) {
    common::open_window(cx, size(px(900.), px(700.)), |window, cx| {
        let fields = std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx)));
        let form = cx.new(|_| Form { fields });
        form.read(cx).fields[0].read(cx).focus_handle(cx).focus(window, cx);
        form
    })
}

/// Which of the trap's stops holds focus: a field index, or `None` if none does.
fn focused_field(
    handle: gpui_kit::WindowHandle<gpui_kit::base::Root>,
    form: &Entity<Form>,
    cx: &mut TestAppContext,
) -> Option<usize> {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        form.read(cx)
            .fields
            .iter()
            .position(|f| f.focus_handle(cx).is_focused(window))
    })
    .unwrap()
}

/// Whether anything outside the modal holds focus.
fn page_has_focus(handle: gpui_kit::WindowHandle<gpui_kit::base::Root>, cx: &mut TestAppContext) -> bool {
    cx.update_window(handle.into(), |_, window, _| {
        window.find("outside").focused() == Some(true)
    })
    .unwrap()
}

#[gpui_kit::test]
fn tab_never_leaves_the_modal(cx: &mut TestAppContext) {
    let (handle, form) = open(cx);
    cx.run_until_parked();
    assert_eq!(focused_field(handle, &form, cx), Some(0), "setup: first field");
    let mut seen_fields = std::collections::BTreeSet::new();
    for press in 1..=25 {
        common::press(handle, "tab", cx);
        assert!(!page_has_focus(handle, cx), "Tab #{press} escaped to the page");
        if let Some(f) = focused_field(handle, &form, cx) {
            seen_fields.insert(f);
        }
    }
    assert_eq!(seen_fields.len(), 3, "the Tab ring still visits every field: {seen_fields:?}");
}

#[gpui_kit::test]
fn shift_tab_wraps_from_the_first_stop(cx: &mut TestAppContext) {
    let (handle, form) = open(cx);
    cx.run_until_parked();
    assert_eq!(focused_field(handle, &form, cx), Some(0));
    common::press(handle, "shift-tab", cx);
    assert!(!page_has_focus(handle, cx), "Shift-Tab escaped to the page");
    assert_eq!(
        focused_field(handle, &form, cx),
        None,
        "wrapped off the first field onto the footer, not another field"
    );
    // ...and the footer's last stop is where it landed.
    let on_save = cx
        .update_window(handle.into(), |_, window, _| {
            window.find("trap-save").focused() == Some(true)
        })
        .unwrap();
    assert!(on_save, "Shift-Tab from the first stop lands on the last (Save)");
    for _ in 0..25 {
        common::press(handle, "shift-tab", cx);
        assert!(!page_has_focus(handle, cx));
    }
}
