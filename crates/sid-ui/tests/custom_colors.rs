//! The custom-colors editor end to end: a valid hex repaints the whole theme, an invalid
//! one changes nothing, and "Reset all" puts the palette back.

mod common;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, IntoElement, Render, TestAppContext, Window, WindowHandle,
    base::Root, px, size,
};
use sid_ui::{CustomColors, theme};

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        CustomColors::new("custom-colors")
    }
}

fn act(
    handle: WindowHandle<Root>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut Window, &mut gpui_kit::App),
) {
    cx.update_window(handle.into(), |_, window, cx| f(window, cx))
        .unwrap();
    cx.run_until_parked();
}

fn type_into(handle: WindowHandle<Root>, id: &'static str, text: &str, cx: &mut TestAppContext) {
    act(handle, cx, |w, cx| {
        w.click(id, cx);
        w.press("ctrl-a", cx);
        w.input(text, cx);
        w.press("enter", cx);
    });
}

fn accent(cx: &mut TestAppContext) -> u32 {
    cx.update(|cx| theme::active(cx).accent)
}

#[gpui_kit::test]
fn a_valid_hex_repaints_an_invalid_one_does_not_and_reset_all_restores(cx: &mut TestAppContext) {
    let (handle, _) = common::open_window(cx, size(px(1400.), px(900.)), |_, cx| cx.new(|_| Host));
    let palette = accent(cx);

    type_into(handle, "accent", "not a colour", cx);
    assert_eq!(accent(cx), palette, "an invalid entry changes nothing");
    assert!(cx.update(|cx| theme::overrides(cx).is_empty()));

    type_into(handle, "accent", "112233", cx);
    assert_eq!(accent(cx), 0x112233, "a valid entry is live");
    assert_eq!(
        cx.update(|cx| theme::overrides(cx).get("accent").cloned()),
        Some("#112233".to_string()),
        "stored normalised"
    );

    act(handle, cx, |w, cx| w.click("custom-colors-reset-all", cx));
    assert_eq!(accent(cx), palette);
    assert!(cx.update(|cx| theme::overrides(cx).is_empty()));
}
