//! A primary sub-view strip is comfortably tall, and a click anywhere in a segment's box
//! (not just on its label) selects it.

mod common;

use gpui_kit::AppContext as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    Context, IntoElement, Render, TestAppContext, Window, div, point, prelude::*, px, size,
};
use sid_ui::{SegmentSelect, SegmentedControl};

struct Probe {
    picked: Option<usize>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().p_4().child(
            SegmentedControl::new("seg")
                .large()
                .segments(["Ports", "Connections", "Services"])
                .selected(self.picked.unwrap_or(0))
                .on_select(cx.listener(|this, ev: &SegmentSelect, _, cx| {
                    this.picked = Some(ev.index);
                    cx.notify();
                })),
        )
    }
}

#[gpui_kit::test]
fn the_whole_segment_box_is_the_click_target(cx: &mut TestAppContext) {
    let (handle, probe) = common::open_window(cx, size(px(600.), px(200.)), |_, cx| {
        cx.new(|_| Probe { picked: None })
    });
    for ix in [2usize, 1, 0] {
        let id = format!("seg-{ix}");
        let b = cx
            .update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window.find(id.clone()).bounds()
            })
            .unwrap();
        assert!(
            f32::from(b.size.height) >= 32.,
            "segment {ix} is {:?} tall",
            b.size.height
        );
        // The four corners, inset 2px: padding and edges, nowhere near the label.
        for (dx, dy) in [
            (2., 2.),
            (f32::from(b.size.width) - 2., f32::from(b.size.height) - 2.),
        ] {
            cx.update_window(handle.into(), |_, window, cx| {
                window.click_at(id.clone(), point(px(dx), px(dy)), cx)
            })
            .unwrap();
            assert_eq!(
                probe.read_with(cx, |p, _| p.picked),
                Some(ix),
                "{id} ({dx},{dy})"
            );
            // Reset to a different segment so the next click has to change it.
            probe.update(cx, |p, _| p.picked = Some(usize::MAX));
        }
    }
}
