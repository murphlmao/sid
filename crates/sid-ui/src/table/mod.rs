//! Tables that fill the width they are given.
//!
//! # The problem this solves
//!
//! Every one of sid's 30 `Column::new` sites hard-codes a pixel width, because upstream
//! gives it no choice: `gpui-component`'s `Column.width` is a plain `Pixels` with
//! no `Length`/flex/grow option and no fill logic anywhere in `table/mod.rs`
//! (`docs/design/2026-07-26-ui-overhaul-plan.md` §2.4). The consequence, measured: the
//! System tab declares **652px of columns in a 2000px window** — 67% of the widest
//! surface in the app is dead space, the single loudest complaint about the UI — and the
//! Network tab truncates IPv6 socket addresses inside a 120px column while 1350px sits
//! unused beside it.
//!
//! `TableDelegate::render_last_empty_col` only lets a delegate *paint* that leftover
//! strip. It cannot distribute it.
//!
//! # The shape of the fix
//!
//! Three layers, narrow to wide:
//!
//! 1. [`ColumnWidth`] + [`resolve_widths`] ([`column_width`]) — declared intent to
//!    resolved pixels. Pure: no `gpui`, no entity, no window, and therefore exhaustively
//!    unit-tested in microseconds. This is where all the thinking is.
//! 2. [`FillColumns`] ([`columns`]) — the delegate's column store. Writes resolved
//!    pixels back into `Column.width` (a public field) and mirrors sort state so a
//!    refresh cannot lose the header indicator.
//! 3. [`FillTable`] — the element. Measures the space the table actually got and feeds
//!    it to layer 2. Branch-free wiring, gated by observation (a `sid-cap.sh` capture),
//!    not by unit tests.
//!
//! Nothing here forks or patches the library: the table, its header, its sort cycle, its
//! scrollbars, its selection and its context menus are all still upstream's. The only
//! thing added is *when* and *to what* `Column.width` is set.
//!
//! # How the viewport is measured
//!
//! There is no resize event or layout hook to subscribe to — gpui is immediate-mode, and
//! `TableState`'s own measured `bounds` field is private. The mechanism is the one
//! upstream itself uses to capture those bounds (`table/state.rs:1413-1419`): a
//! zero-cost [`canvas`] laid `absolute().size_full()` over the table, whose prepaint
//! closure receives the resolved `Bounds` and writes them somewhere. Here it forwards
//! the width to [`FillColumns::sync`] and calls `TableState::refresh` — which re-runs
//! `prepare_col_groups` and re-reads every `Column.width` from the delegate — but only
//! when a width actually moved. A steady viewport therefore costs one comparison per
//! frame and never schedules a repaint, which is what keeps the measure/refresh pair
//! from becoming a render loop.
//!
//! # Why a cell must not carry an `ElementId`
//!
//! `TableDelegate::render_td` is called for **every visible cell on every frame** — 178
//! calls per frame on the System tab's process table at 1920x1080, confirmed with
//! `GPUI_MEASUREMENTS=1` (`gpui-component`'s own per-cell timer). Building those cells is
//! cheap: 0.10-0.20ms of a frame that costs 6-25ms. What is *not* cheap is what an
//! `ElementId` on each of them makes `gpui` do afterwards.
//!
//! An id turns a `Div` into a `Stateful<Div>`, and `Interactivity` then runs
//! `Window::with_element_state` in **both** prepaint and paint. Each of those calls
//! (`gpui-0.2.2/src/window.rs:2628-2641`) **clones the whole `GlobalElementId` twice** —
//! once for the map key, once to push onto `accessed_element_states` — and
//! `GlobalElementId` is a `SmallVec<[ElementId; 32]>` whose every `Name`/`NamedInteger`
//! component is a refcounted `SharedString`. Two hash-map operations over that key
//! follow, and the clones are dropped at frame end. Four id-path clones per cell per
//! frame, times ~180 cells, is ~700 of them — for divs that have **no** click, hover,
//! tooltip, scroll, drag or focus state to remember. In a release scroll profile that
//! machinery (`ArcCow::hash`, `ElementId::hash`, `drop_in_place<ElementId>`,
//! `hashbrown::remove_entry`, `SmallVec::extend`/`drop`, plus its share of `malloc`/
//! `memcpy`) is **10-15% of the frame**, and it is 100% waste.
//!
//! So: **no `.id()` on a table cell.** The interactive thing *inside* the cell — a
//! `ConfirmButton`, keyed by pid or unit name so its armed state survives a re-sort —
//! carries its own id and keeps its own state. The wrapper never needed one. Row hover
//! and row click are upstream's, on the row, and are unaffected.
//!
//! (An id is not free elsewhere either, but everywhere else the count is bounded: six
//! header cells, one per row, one per action button. Cells are the only place the count
//! is *rows x columns*.)
//!
//! # Using it from a delegate
//!
//! ```ignore
//! struct ProcessesDelegate { columns: FillColumns, /* .. */ }
//!
//! FillColumns::new([
//!     (Column::new("pid", "PID").sortable(), ColumnWidth::Fixed(80.)),
//!     (Column::new("name", "Name").sortable(), ColumnWidth::grow().min_width(220.)),
//!     (Column::new("user", "User").sortable(), ColumnWidth::Min(120.)),
//! ])
//! ```
//!
//! then `columns_count` -> `self.columns.len()`, `column` -> `self.columns.column(ix)`,
//! `perform_sort` -> `self.columns.apply_sort(col_ix, sort)`, plus a two-line
//! [`FillTableDelegate`] impl. Render with [`FillTable`] instead of `Table`.

mod column_width;
mod columns;
mod header;

pub use column_width::{ColumnWidth, DEFAULT_GROW_MIN, resolve_widths};
pub use columns::{FillColumns, TABLE_CHROME};
pub use header::{next_sort, sortable_th};

use gpui::{
    App, Entity, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window, canvas, div,
};
use gpui_component::Size;
use gpui_component::table::{DataTable, TableDelegate, TableState};

use crate::scale::UiScale;

/// How many rows a table body paints, given the data it has and the rows its viewport
/// has room for.
///
/// sid's answer is `data_rows`, always: a table stops at its data, and the floor below
/// the last row is the panel's own plain surface, with no separators and no zebra. The
/// library's answer, whenever striping is on, is `data_rows.max(viewport_rows)` — it
/// fills the leftover with ruled, striped **fake** rows down to the panel floor
/// (`table/state.rs`'s `calculate_extra_rows_needed`), which is why the Network tab's
/// PORTS panel showed 26 rows of chrome under 10 rows of data at the 2026-09-10 gate.
/// Rows that hold nothing, are not hoverable and cannot be clicked read as data that
/// failed to load.
///
/// `viewport_rows` is deliberately not consulted: it is the *only* thing that could
/// buy a row past the data, and it may not. It stays in the signature because the
/// comparison is what [`FillTable`] wires — striping is the single lever the library
/// exposes over the fill (with it off, `render_rows_count` is exactly `rows_count`), so
/// the element hands `stripe` on only when the data already reaches the floor and the
/// fill therefore has nothing to paint.
pub fn rows_to_paint(data_rows: usize, viewport_rows: usize) -> usize {
    let _ = viewport_rows;
    data_rows
}

/// A [`TableDelegate`] that sizes its columns with [`FillColumns`] and can therefore be
/// rendered by [`FillTable`].
///
/// Two lines to implement; the store it exposes is the same one the delegate already
/// answers `columns_count`/`column` from.
pub trait FillTableDelegate: TableDelegate {
    /// The delegate's column store, so the element can resize it to the viewport.
    fn fill_columns(&mut self) -> &mut FillColumns;
}

/// A `gpui_component` [`Table`] whose columns are resized to the width it is given.
///
/// A drop-in replacement for `Table` at the call site — same `Entity<TableState<D>>`,
/// same options — for any delegate that also implements [`FillTableDelegate`].
#[derive(IntoElement)]
pub struct FillTable<D: FillTableDelegate> {
    state: Entity<TableState<D>>,
    stripe: bool,
    bordered: bool,
}

impl<D: FillTableDelegate> FillTable<D> {
    /// Wrap a table state. Defaults match `Table`'s: unstriped, bordered.
    pub fn new(state: &Entity<TableState<D>>) -> Self {
        Self {
            state: state.clone(),
            stripe: false,
            bordered: true,
        }
    }

    /// Alternate row fills, for tables dense enough to need row tracking.
    pub fn stripe(mut self, stripe: bool) -> Self {
        self.stripe = stripe;
        self
    }

    /// Draw the table's outer hairline. Turning this off also removes 2px of the
    /// [`TABLE_CHROME`] reserve — tell the delegate's store with [`FillColumns::chrome`].
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }
}

/// Ask for exactly one more frame, because the widths this frame just resolved cannot
/// reach the screen inside it.
///
/// The measure happens in **prepaint**, which is after taffy has already laid the columns
/// out at their previous widths; `TableState::refresh` re-reads them, but nothing repaints
/// unless something asks. Inside a draw, nothing will: `Window::refresh` is a no-op while
/// the invalidator is drawing (`gpui-pre-0.3.4/src/window.rs:2179`, `if not_drawing()`), which
/// is the same rule that makes a `cx.notify()` from `render` disappear. A table on a poll
/// timer never noticed — the next tick brought a frame along. A **quiescent** one did: the
/// Workspaces fleet table settled its data before its first paint, so nothing followed the
/// measure and every column sat at `gpui-component`'s 100px default until a stray click
/// woke it. That tab worked around it with a 120ms timer of its own
/// (`ui::workspaces_tab::AppState::settle_fleet_layout`), which this makes deletable.
///
/// [`Window::on_next_frame`] is the escape hatch: the callback runs from
/// `on_request_frame`, outside the draw, where `refresh` takes. One frame, only after a
/// `sync` that actually moved a width — a steady viewport still costs one comparison and
/// schedules nothing, so this cannot become a render loop.
fn settle(window: &mut Window) {
    window.on_next_frame(|window, _| window.refresh());
}

/// The rows the table's body has room for, from the same measurement the library uses
/// to decide how many fake rows to paint (`state.rs`: the vertical scroll handle's base
/// bounds, divided by the row height). Zero until the body has been laid out once —
/// which is also when the library's own fill is zero, so the two agree on frame one.
///
/// The row height is `Size::default()`'s because [`FillTable`] exposes no size knob; a
/// table that grew one would have to read the same source the library does.
fn viewport_rows<D: FillTableDelegate>(state: &Entity<TableState<D>>, cx: &App) -> usize {
    let height = state
        .read(cx)
        .vertical_scroll_handle
        .0
        .borrow()
        .base_handle
        .bounds()
        .size
        .height;
    let row_height = f32::from(Size::default().table_row_height());
    (f32::from(height) / row_height).floor().max(0.) as usize
}

impl<D: FillTableDelegate> RenderOnce for FillTable<D> {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // The phantom-row gate. See [`rows_to_paint`]: the library ties its striped
        // fake-row fill to the same flag as the zebra, so the zebra is only asked for
        // when the data already reaches the floor and the fill has nothing to add.
        let viewport = viewport_rows(&self.state, cx);
        let data = self.state.read(cx).delegate().rows_count(cx);
        let stripe = self.stripe && rows_to_paint(data, viewport) >= viewport;

        let measured = self.state.clone();
        div()
            .relative()
            .size_full()
            .child(
                DataTable::new(&self.state)
                    .stripe(stripe)
                    .bordered(self.bordered),
            )
            .child(
                // The viewport probe. See the module docs: this is upstream's own
                // bounds-capture mechanism, pointed at the column widths.
                canvas(
                    move |bounds, window, cx| {
                        let width = f32::from(bounds.size.width);
                        // App zoom, read straight off the window that is being measured —
                        // the same lever every rem-based length in the app resolves
                        // against, so a table can never disagree with the chrome around
                        // it about what 150% means.
                        let scale = UiScale::from_rem_size(window.rem_size());
                        let moved = measured.update(cx, |table, cx| {
                            let moved = table.delegate_mut().fill_columns().sync(width, scale);
                            if moved {
                                table.refresh(cx);
                            }
                            moved
                        });
                        if moved {
                            settle(window);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fewer_rows_than_the_viewport_paints_only_the_data() {
        // Network's PORTS panel at the 2026-09-10 gate: 10 rows of data under 26 rows
        // of ruled, striped chrome. A table stops at its data; the floor below the
        // last row is the panel's own surface.
        assert_eq!(rows_to_paint(10, 36), 10);
        assert_eq!(rows_to_paint(0, 36), 0);
        assert_eq!(rows_to_paint(35, 36), 35);
    }

    #[test]
    fn a_table_that_reaches_the_floor_paints_every_row_it_has() {
        // The other side: nothing is clipped, and a full table has no leftover for the
        // library to fill, which is where its zebra is still worth having.
        assert_eq!(rows_to_paint(500, 36), 500);
        assert_eq!(rows_to_paint(36, 36), 36);
    }
}
