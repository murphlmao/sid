//! `Tree` keyboard wiring: arrows move the selection, Right expands, Left collapses.
//!
//! The arithmetic (`tree::navigate`) has pure unit tests; this proves the real chain —
//! key binding -> focused tree -> `on_key` -> state -> re-rendered rows.

mod common;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    TestAppContext, Window, div, prelude::*, px, size,
};
use sid_ui::{TreeNode, TreeState};

struct Host {
    tree: Entity<TreeState>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(400.)).h(px(300.)).child(self.tree.clone())
    }
}

fn selected(tree: &Entity<TreeState>, cx: &mut TestAppContext) -> Option<String> {
    tree.read_with(cx, |t, _| t.selected().map(|s| s.to_string()))
}

fn row_exists(
    handle: gpui_kit::WindowHandle<gpui_kit::base::Root>,
    ix: usize,
    cx: &mut TestAppContext,
) -> bool {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find((SharedString::from("t"), ix))
            .is_some_and(|r| r.visible())
    })
    .unwrap()
}

#[gpui_kit::test]
fn arrows_move_selection_and_left_right_fold(cx: &mut TestAppContext) {
    let (handle, host) = common::open_window(cx, size(px(500.), px(400.)), |window, cx| {
        let tree = cx.new(|cx| {
            let mut t = TreeState::new("t", cx);
            t.set_roots(
                vec![
                    TreeNode::branch(
                        "a",
                        "a",
                        vec![TreeNode::leaf("a1", "a1"), TreeNode::leaf("a2", "a2")],
                    ),
                    TreeNode::leaf("b", "b"),
                ],
                cx,
            );
            t
        });
        window.focus(&tree.read(cx).focus_handle().clone(), cx);
        cx.new(|_| Host { tree })
    });
    let tree = host.read_with(cx, |h, _| h.tree.clone());
    cx.run_until_parked();
    assert!(
        row_exists(handle, 0, cx) && row_exists(handle, 1, cx),
        "a and b rendered"
    );
    assert!(!row_exists(handle, 2, cx), "collapsed: no third row");

    common::press(handle, "down", cx);
    assert_eq!(
        selected(&tree, cx).as_deref(),
        Some("a"),
        "Down from nothing lands on the first row"
    );

    common::press(handle, "right", cx);
    assert!(
        tree.read_with(cx, |t, _| t.is_expanded("a")),
        "Right expands"
    );
    assert!(row_exists(handle, 2, cx), "expansion paints the child rows");

    common::press(handle, "down", cx);
    assert_eq!(selected(&tree, cx).as_deref(), Some("a1"));
    common::press(handle, "down", cx);
    common::press(handle, "down", cx);
    assert_eq!(selected(&tree, cx).as_deref(), Some("b"));
    common::press(handle, "up", cx);
    assert_eq!(selected(&tree, cx).as_deref(), Some("a2"));

    common::press(handle, "left", cx);
    assert_eq!(
        selected(&tree, cx).as_deref(),
        Some("a"),
        "Left on a leaf climbs to its parent"
    );
    common::press(handle, "left", cx);
    assert!(
        !tree.read_with(cx, |t, _| t.is_expanded("a")),
        "Left on an open branch collapses"
    );
    assert!(
        !row_exists(handle, 2, cx),
        "collapse removes the child rows"
    );
}
