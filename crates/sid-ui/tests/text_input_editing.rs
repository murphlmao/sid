//! Word-wise editing keys on `TextInput`, and the masked-field privacy contract.

mod common;

use gpui_kit::component::input::InputState;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, Window, WindowHandle,
    base::Root, div, prelude::*, px, size,
};
use sid_ui::TextInput;

struct Fields {
    plain: Entity<InputState>,
    secret: Entity<InputState>,
}

impl Render for Fields {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(TextInput::new(&self.plain).test_id("plain").tab_index(1))
            .child(TextInput::new(&self.secret).test_id("secret").tab_index(2))
    }
}

fn open(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<Fields>) {
    common::open_window(cx, size(px(500.), px(300.)), |window, cx| {
        let plain = cx.new(|cx| InputState::new(window, cx).default_value("one two three"));
        let secret = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("hunter2")
                .masked(true)
        });
        cx.new(|_| Fields { plain, secret })
    })
}

fn on_plain(
    handle: WindowHandle<Root>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut Window, &mut gpui_kit::App),
) {
    cx.update_window(handle.into(), |_, window, cx| f(window, cx))
        .unwrap();
    cx.run_until_parked();
}

fn value(handle: WindowHandle<Root>, id: &'static str, cx: &mut TestAppContext) -> Option<String> {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.find(id).value().map(str::to_owned)
    })
    .unwrap()
}

/// Focus the plain field with the caret at the end of "one two three".
fn focused(cx: &mut TestAppContext) -> WindowHandle<Root> {
    let (handle, _) = open(cx);
    on_plain(handle, cx, |w, cx| {
        w.click("plain", cx);
        w.press("end", cx);
    });
    handle
}

#[gpui_kit::test]
fn ctrl_backspace_deletes_a_word_not_a_character(cx: &mut TestAppContext) {
    let handle = focused(cx);
    on_plain(handle, cx, |w, cx| w.press("ctrl-backspace", cx));
    assert_eq!(value(handle, "plain", cx).as_deref(), Some("one two "));
    on_plain(handle, cx, |w, cx| w.press("ctrl-backspace", cx));
    assert_eq!(value(handle, "plain", cx).as_deref(), Some("one "));
}

#[gpui_kit::test]
fn ctrl_shift_arrows_select_by_word(cx: &mut TestAppContext) {
    let handle = focused(cx);
    on_plain(handle, cx, |w, cx| {
        w.press("ctrl-shift-left", cx);
        w.press("ctrl-shift-left", cx);
        // Typing replaces the selection: it was exactly "two three".
        w.input("X", cx);
    });
    assert_eq!(value(handle, "plain", cx).as_deref(), Some("one X"));
}

#[gpui_kit::test]
fn select_all_then_typing_replaces_everything(cx: &mut TestAppContext) {
    let handle = focused(cx);
    on_plain(handle, cx, |w, cx| {
        w.press("ctrl-a", cx);
        w.input("z", cx);
    });
    assert_eq!(value(handle, "plain", cx).as_deref(), Some("z"));
}

#[gpui_kit::test]
fn a_masked_field_never_exposes_its_text(cx: &mut TestAppContext) {
    let (handle, _) = open(cx);
    // Whatever the harness reads for the secret, it is not the secret...
    assert_eq!(
        value(handle, "secret", cx),
        None,
        "masked value is withheld"
    );
    // ...and editing it keeps it that way, while the plain field beside it still reports.
    on_plain(handle, cx, |w, cx| {
        w.click("secret", cx);
        w.press("end", cx);
        w.input("!", cx);
        w.press("ctrl-a", cx);
        w.press("ctrl-c", cx);
    });
    assert_eq!(value(handle, "secret", cx), None);
    assert_eq!(value(handle, "plain", cx).as_deref(), Some("one two three"));
}
