//! Tables that fill the width they are given.
//!
//! # The problem this solves
//!
//! Every one of sid's 30 `Column::new` sites hard-codes a pixel width, because upstream
//! gives it no choice: `gpui-component-0.5.1`'s `Column.width` is a plain `Pixels` with
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
use gpui_component::table::{Table, TableDelegate, TableState};

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
/// the invalidator is drawing (`gpui-0.2.2/src/window.rs:1367`, `if not_drawing()`), which
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

impl<D: FillTableDelegate> RenderOnce for FillTable<D> {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let measured = self.state.clone();
        div()
            .relative()
            .size_full()
            .child(
                Table::new(&self.state)
                    .stripe(self.stripe)
                    .bordered(self.bordered),
            )
            .child(
                // The viewport probe. See the module docs: this is upstream's own
                // bounds-capture mechanism, pointed at the column widths.
                canvas(
                    move |bounds, window, cx| {
                        let width = f32::from(bounds.size.width);
                        let moved = measured.update(cx, |table, cx| {
                            let moved = table.delegate_mut().fill_columns().sync(width);
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
