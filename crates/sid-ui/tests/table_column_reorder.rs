//! Dragging a `FillTable` header moves the whole column, and it stays moved.
//!
//! The user report: "it doesn't move where I actually place them". Upstream's drop
//! handler permutes only the table's private `col_groups` (width, sort, a `Column`
//! copy) and asks the delegate to follow through `TableDelegate::move_column`, a no-op
//! by default. sid's delegates read labels and cells from `FillColumns` by position, so
//! after a drop the widths moved and the labels and cells did not, and the next
//! `TableState::refresh` (any resize, any header-click sort) rebuilt `col_groups` from
//! the unmoved store and undid the drop entirely. These tests drive the real gesture.

mod common;

use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, TestAppContext, Window, WindowHandle, div, point, prelude::*, px, size,
};
use sid_ui::{ColumnWidth, FillColumns, FillTable, FillTableDelegate, TestSupportExt as _};

struct Grid {
    columns: FillColumns,
    /// The column key every `perform_sort` call resolved to.
    sorted_by: Vec<String>,
}

impl TableDelegate for Grid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        2
    }
    fn column(&self, ix: usize, _: &App) -> Column {
        self.columns.column(ix).clone()
    }
    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let key = KEYS[self.columns.logical(col_ix)];
        self.sorted_by.push(key.to_string());
        self.columns.apply_sort(col_ix, sort);
    }
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        sid_ui::sortable_th(col_ix, self.columns.column(col_ix), cx)
    }
    fn render_td(
        &mut self,
        row: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Addressed by logical index, the way a delegate adopts reordering.
        let key = KEYS[self.columns.logical(col_ix)];
        div()
            .id(SharedString::from(format!("cell-{key}-{row}")))
            .test_support()
            .w_full()
            .child(format!("{key}{row}"))
    }
}

impl FillTableDelegate for Grid {
    fn fill_columns(&mut self) -> &mut FillColumns {
        &mut self.columns
    }
}

const KEYS: [&str; 4] = ["a", "b", "c", "d"];

fn grid(reorderable: bool, pin_last: bool) -> FillColumns {
    let cols = FillColumns::new([
        (Column::new("a", "A").sortable(), ColumnWidth::Fixed(100.)),
        (Column::new("b", "B").sortable(), ColumnWidth::Fixed(150.)),
        (Column::new("c", "C").sortable(), ColumnWidth::Fixed(200.)),
        (
            Column::new("d", "D").movable(!pin_last),
            ColumnWidth::grow().min_width(100.),
        ),
    ])
    .inset_cells([0]);
    if reorderable {
        cols.reorderable()
    } else {
        cols
    }
}

struct Host {
    table: Entity<TableState<Grid>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(800.))
            .h(px(300.))
            .flex()
            .flex_col()
            .child(FillTable::new(&self.table))
    }
}

type Handle = WindowHandle<gpui_kit::base::Root>;

fn open(cx: &mut TestAppContext, columns: FillColumns) -> (Handle, Entity<TableState<Grid>>) {
    let (handle, host) = common::open_window(cx, size(px(900.), px(400.)), |window, cx| {
        let table = cx.new(|cx| {
            TableState::new(
                Grid {
                    columns,
                    sorted_by: Vec::new(),
                },
                window,
                cx,
            )
        });
        cx.new(|_| Host { table })
    });
    settle(handle, cx);
    let table = host.read_with(cx, |h, _| h.table.clone());
    (handle, table)
}

fn settle(handle: Handle, cx: &mut TestAppContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, w, cx| w.render_frame(cx))
            .unwrap();
    }
    cx.run_until_parked();
}

fn header(handle: Handle, ix: usize, cx: &mut TestAppContext) -> Bounds<Pixels> {
    cx.update_window(handle.into(), |_, w, _| w.find(("col-header", ix)).bounds())
        .unwrap()
}

/// Drag header `from` and release at window `x` on the header's row.
fn drag_header(handle: Handle, from: usize, x: Pixels, cx: &mut TestAppContext) {
    let start = header(handle, from, cx).center();
    cx.update_window(handle.into(), |_, w, cx| {
        w.drag(start, point(x, start.y), cx)
    })
    .unwrap();
    settle(handle, cx);
}

/// The column keys in display order, as the store reports them.
fn keys(table: &Entity<TableState<Grid>>, cx: &mut TestAppContext) -> Vec<String> {
    table.read_with(cx, |t, _| {
        let cols = &t.delegate().columns;
        (0..cols.len())
            .map(|ix| cols.column(ix).key.to_string())
            .collect()
    })
}

/// Everything a user sees about a column agrees with the store at every position:
/// the width the table draws (`col_groups`), the header label, and the body cells.
fn assert_coherent(handle: Handle, table: &Entity<TableState<Grid>>, cx: &mut TestAppContext) {
    let keys = keys(table, cx);
    for (ix, key) in keys.iter().enumerate() {
        let (drawn, stored) = table.read_with(cx, |t, _| {
            (t.col_width(ix), t.delegate().columns.column(ix).width)
        });
        assert_eq!(
            drawn,
            Some(stored),
            "position {ix} ({key}): the table draws a width the store does not hold"
        );
        let head = header(handle, ix, cx);
        for row in 0..2 {
            let cell = cx
                .update_window(handle.into(), |_, w, _| {
                    w.find(SharedString::from(format!("cell-{key}-{row}")))
                        .bounds()
                })
                .unwrap();
            assert!(
                cell.left() >= head.left() - px(0.5) && cell.right() <= head.right() + px(0.5),
                "position {ix}: cell {key}{row} at {:?}..{:?} is not under its header {:?}..{:?}",
                cell.left(),
                cell.right(),
                head.left(),
                head.right()
            );
        }
    }
}

fn refresh(handle: Handle, table: &Entity<TableState<Grid>>, cx: &mut TestAppContext) {
    table.update(cx, |t, cx| t.refresh(cx));
    settle(handle, cx);
}

#[gpui_kit::test]
fn a_dropped_column_lands_in_the_gap_and_stays_there(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, grid(true, false));
    assert_eq!(keys(&table, cx), ["a", "b", "c", "d"]);

    // Release just right of C's centre: the gap after C. A goes there.
    let c = header(handle, 2, cx);
    drag_header(handle, 0, c.center().x + px(10.), cx);
    assert_eq!(keys(&table, cx), ["b", "c", "a", "d"]);
    assert_coherent(handle, &table, cx);

    // A refresh rebuilds the table's columns from the store, which is what every
    // resize and every header-click sort does. The drop must survive it.
    refresh(handle, &table, cx);
    assert_eq!(keys(&table, cx), ["b", "c", "a", "d"]);
    assert_coherent(handle, &table, cx);
}

#[gpui_kit::test]
fn dragging_past_either_end_moves_to_that_end(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, grid(true, false));

    let last = header(handle, 3, cx);
    drag_header(handle, 0, last.right() - px(2.), cx);
    assert_eq!(keys(&table, cx), ["b", "c", "d", "a"]);
    assert_coherent(handle, &table, cx);

    let first = header(handle, 0, cx);
    drag_header(handle, 2, first.left() + px(2.), cx);
    assert_eq!(keys(&table, cx), ["d", "b", "c", "a"]);
    assert_coherent(handle, &table, cx);
}

#[gpui_kit::test]
fn dropping_a_column_on_itself_changes_nothing(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, grid(true, false));
    let b = header(handle, 1, cx);
    drag_header(handle, 1, b.center().x + px(5.), cx);
    assert_eq!(keys(&table, cx), ["a", "b", "c", "d"]);
    // And the release did not land as a sort click on the header it started on.
    assert!(table.read_with(cx, |t, _| t.delegate().sorted_by.is_empty()));
    assert_coherent(handle, &table, cx);
}

#[gpui_kit::test]
fn sort_follows_the_column_through_a_move(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, grid(true, false));
    // Sort by B (a click anywhere on its header), then move B to the front.
    cx.update_window(handle.into(), |_, w, cx| {
        w.click(("col-header", 1usize), cx)
    })
    .unwrap();
    settle(handle, cx);
    let a = header(handle, 0, cx);
    drag_header(handle, 1, a.left() + px(2.), cx);
    assert_eq!(keys(&table, cx), ["b", "a", "c", "d"]);
    let sorts = table.read_with(cx, |t, _| {
        let cols = &t.delegate().columns;
        (0..cols.len())
            .map(|ix| cols.column(ix).sort)
            .collect::<Vec<_>>()
    });
    assert_eq!(
        sorts,
        [
            Some(ColumnSort::Descending),
            Some(ColumnSort::Default),
            Some(ColumnSort::Default),
            None
        ]
    );

    // A click on the header now at position 2 sorts by the column drawn there: C.
    cx.update_window(handle.into(), |_, w, cx| {
        w.click(("col-header", 2usize), cx)
    })
    .unwrap();
    settle(handle, cx);
    assert_eq!(
        table.read_with(cx, |t, _| t.delegate().sorted_by.clone()),
        ["b", "c"]
    );
    assert_coherent(handle, &table, cx);
}

#[gpui_kit::test]
fn a_pinned_column_cannot_be_dragged(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, grid(true, true));
    let a = header(handle, 0, cx);
    drag_header(handle, 3, a.left() + px(2.), cx);
    assert_eq!(keys(&table, cx), ["a", "b", "c", "d"]);
    // The other columns still move, on either side of it.
    let d = header(handle, 3, cx);
    drag_header(handle, 0, d.right() - px(2.), cx);
    assert_eq!(keys(&table, cx), ["b", "c", "d", "a"]);
    assert_coherent(handle, &table, cx);
}

#[gpui_kit::test]
fn a_table_that_has_not_opted_in_does_not_offer_the_drag(cx: &mut TestAppContext) {
    // Its `render_td` may still match on display position, so a move would put every
    // cell under the wrong header. No gesture at all beats a gesture that lies.
    let (handle, table) = open(cx, grid(false, false));
    let c = header(handle, 2, cx);
    drag_header(handle, 0, c.center().x + px(10.), cx);
    assert_eq!(keys(&table, cx), ["a", "b", "c", "d"]);
    assert_coherent(handle, &table, cx);
}
