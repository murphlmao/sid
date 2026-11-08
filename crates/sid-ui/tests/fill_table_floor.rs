//! A `FillTable` panel keeps the header filter-floor behaviour when it is too narrow.
//!
//! When a panel cannot hold its 240px filter and the refresh button, the *filter* yields,
//! shrinking toward `PANEL_FILTER_FLOOR` (120px), and the refresh button keeps its content
//! width and stays inside the card (`card::header_actions`). This used to be verified by
//! `sid-cap.sh` at 700px; `bounds()` makes it an assertion at any width.
//!
//! KNOWN GAP (reported, not asserted): since v2 the panel is ~675px at a 700px window (no
//! sidebar), so the filter never shrinks there. And at widths between the actions' own
//! content width (~373px) and ~460px the title (`flex_1`, basis 0) gets only the leftover
//! and is crushed to a few px while the filter keeps 240px: the filter does NOT yield "before
//! the title" in that band. The test below pins the half that does hold (<=340px).

mod common;

use gpui_kit::component::table::{Column, TableDelegate, TableState};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, TestAppContext,
    Window, div, prelude::*, px, size,
};
use sid_ui::{
    Button, Card, ColumnWidth, FillColumns, FillTable, FillTableDelegate, PANEL_FILTER_FLOOR,
    TextInput, scaled,
};

struct Rows {
    columns: FillColumns,
}

impl TableDelegate for Rows {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        3
    }
    fn column(&self, ix: usize, _: &App) -> Column {
        self.columns.column(ix).clone()
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        format!("r{row}c{col}")
    }
}

impl FillTableDelegate for Rows {
    fn fill_columns(&mut self) -> &mut FillColumns {
        &mut self.columns
    }
}

struct Panel {
    table: Entity<TableState<Rows>>,
    filter: Entity<gpui_kit::component::input::InputState>,
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // A panel too narrow for its actions.
        div().w(px(340.)).h(px(400.)).flex().flex_col().child(
            Card::panel("Ports")
                .count(13)
                .flex_1()
                .min_h_0()
                // Wrapped in a div exactly as `network_tab::network_filter_field` does; that div needs
                // `min_w_0` too, or its own min-content (240) keeps the filter from yielding.
                .action(
                    div().flex().min_w_0().child(
                        TextInput::new(&self.filter)
                            .test_id("filter")
                            .small()
                            .fixed(px(240.))
                            .flex_shrink_1()
                            .min_w(scaled(f32::from(PANEL_FILTER_FLOOR))),
                    ),
                )
                .action(Button::new("refresh", "Refresh"))
                .child(FillTable::new(&self.table)),
        )
    }
}

#[gpui_kit::test]
fn the_filter_shrinks_to_its_floor_before_anything_else(cx: &mut TestAppContext) {
    let (handle, _) = common::open_window(cx, size(px(700.), px(500.)), |window, cx| {
        let table = cx.new(|cx| {
            TableState::new(
                Rows {
                    columns: FillColumns::new([
                        (Column::new("a", "A"), ColumnWidth::Fixed(80.)),
                        (Column::new("b", "B"), ColumnWidth::grow().min_width(120.)),
                    ]),
                },
                window,
                cx,
            )
        });
        let filter = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx));
        cx.new(|_| Panel { table, filter })
    });
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let filter = window.find("filter").bounds();
        let refresh = window.find("refresh").bounds();
        let (w, floor) = (f32::from(filter.size.width), f32::from(PANEL_FILTER_FLOOR));
        assert!(
            w < 240.,
            "the filter must give up width at 340px (got {w}px)"
        );
        assert!(
            w >= floor - 0.5,
            "the filter may not go under its floor (got {w}px)"
        );
        assert!(
            refresh.size.width > px(40.),
            "the refresh button keeps its content width (got {:?})",
            refresh.size.width
        );
        assert!(
            filter.right() <= refresh.left() + px(0.5),
            "filter and refresh must not overlap"
        );
        assert!(
            refresh.right() <= px(340.),
            "the actions stay inside the card (refresh ends at {:?})",
            refresh.right()
        );
    })
    .unwrap();
}
