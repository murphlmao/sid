//! The delegate's column store: `Column`s plus the [`ColumnWidth`] each one declared.
//!
//! [`FillColumns`] is the seam between the pure resolver in [`super::column_width`] and
//! `gpui-component`'s table. Upstream's `Column.width` is a bare `Pixels` with no flex
//! option (`gpui-component-0.5.1/src/table/column.rs:28`) — but the field is public and
//! the delegate owns the `Vec<Column>` it hands out, so a fill-width table is a matter
//! of keeping those pixel values in step with the measured viewport. That is all this
//! type does: hold the declaration, and write resolved pixels back on demand.
//!
//! It deliberately needs no `App`, `Window` or entity, so its behaviour is unit-tested
//! at the same millisecond speed as the resolver underneath it.

use std::rc::Rc;

use gpui::{App, Edges, Pixels, Subscription, px};
use gpui_component::Size;
use gpui_component::table::{Column, ColumnSort};

use super::column_width::{ColumnWidth, resolve_widths};
use crate::scale::UiScale;

/// What the columns do **not** get out of the measured width, in logical pixels.
///
/// | Part | Why |
/// |---|---|
/// | 2px | the bordered table's own left+right hairline (`table/mod.rs:127-130`) |
/// | 12px | the strip `TableDelegate::render_last_empty_col` paints after the last column (`w_3`, `table/delegate.rs:157-163`) |
/// | 4px | slack — see below |
///
/// **The slack is load-bearing, not padding.** The columns must come out *strictly*
/// narrower than the row's content box, because at an exact fit (or wider) upstream's
/// horizontal `virtual_list` mis-measures itself: `virtual_list.rs:454-472` sets
/// `size.height = known_dimensions.width` in its horizontal branch, so the moment taffy
/// resolves a definite width for it — which is exactly when its content stops fitting —
/// every row's cell strip becomes ~1900px tall inside a 32px `items_center`
/// `overflow_hidden` row and is clipped out of existence. The table renders its header,
/// its stripes and its scrollbars, and **not one cell**.
///
/// Confirmed by capture at 2000x1200: a 14px reserve (the exact fit) blanks the process
/// table; 15px renders it. 4px is that threshold with room for sub-pixel rounding at
/// fractional scale factors, and is invisible next to the border it sits under.
///
/// Override with [`FillColumns::chrome`] if a delegate replaces
/// `render_last_empty_col` or renders the table unbordered — but keep some slack.
pub const TABLE_CHROME: f32 = REAL_CHROME + CHROME_SLACK;

/// The chrome that physically exists: the table's 2px border plus the 12px trailing
/// strip. Not the reserve — see [`TABLE_CHROME`] for why the reserve must exceed it.
/// `px_2` at 100% zoom, in logical pixels.
const INSET: f32 = 8.0;

const REAL_CHROME: f32 = 14.0;

/// How far inside the row's content box the columns are kept. See [`TABLE_CHROME`].
const CHROME_SLACK: f32 = 4.0;

/// A width difference too small to be worth a relayout, in logical pixels. Sub-pixel
/// churn would re-`refresh` the table on every frame for no visible change.
const REDRAW_EPSILON: f32 = 0.5;

/// A table's columns and their declared widths, owned by the delegate.
///
/// Build it once in the delegate's constructor, return [`FillColumns::column`] and
/// [`FillColumns::len`] from `TableDelegate::column`/`columns_count`, and let
/// [`super::FillTable`] drive [`FillColumns::sync`] from the measured viewport.
///
/// # Order
///
/// `columns`, `declared` and `inset` stay in **declaration order** forever and are
/// indexed by declaration index. A user's reorder touches only `order`, which maps a
/// display position to a declaration index, so a column's width, inset and sort state
/// cannot be separated from it by a move: there is nothing parallel to permute.
pub struct FillColumns {
    columns: Vec<Column>,
    declared: Vec<ColumnWidth>,
    chrome: f32,
    /// Columns whose text inset is carried by `Column::paddings` instead of a `px_2` div
    /// in every cell. See [`FillColumns::inset_cells`].
    inset: Vec<bool>,
    /// Display position -> declaration index.
    order: Vec<usize>,
    /// See [`FillColumns::reorderable`]. Read by [`super::FillTable`].
    pub(super) reorderable: bool,
    /// The order changed outside a drag ([`FillColumns::apply_order`]); the next `sync`
    /// reports it so the table re-reads its columns.
    reordered: bool,
    /// [`super::FillTable`]'s subscription to this table's `MoveColumn` events. Kept
    /// here so it lives exactly as long as the store it updates: a delegate that
    /// rebuilds its `FillColumns` (the Database grid, every page) drops the old one,
    /// and the next render subscribes the new one.
    pub(super) moves: Option<Subscription>,
    /// See [`FillColumns::on_reorder`].
    pub(super) on_reorder: Option<Rc<SaveOrder>>,
}

/// The persistence hook [`FillColumns::on_reorder`] stores.
type SaveOrder = dyn Fn(Option<Vec<String>>, &mut App);

impl FillColumns {
    /// Declare the columns, each paired with how it wants to be sized.
    ///
    /// Every column comes out **not resizable**, whatever it declared. The width of a
    /// column here is a function of the viewport ([`FillColumns::sync`] re-resolves all
    /// of them every frame), so a drag would be reverted before it painted — and
    /// upstream draws a 1px hairline at the right edge of each resizable column *in the
    /// header only*, which is the "empty last column" the header used to show past the
    /// [`TABLE_CHROME`] reserve while no body row drew anything there.
    pub fn new(declared: impl IntoIterator<Item = (Column, ColumnWidth)>) -> Self {
        let (columns, declared): (Vec<_>, Vec<_>) = declared
            .into_iter()
            .map(|(column, width)| (column.resizable(false), width))
            .unzip();
        Self {
            order: (0..columns.len()).collect(),
            columns,
            declared,
            chrome: TABLE_CHROME,
            inset: Vec::new(),
            reorderable: false,
            reordered: false,
            moves: None,
            on_reorder: None,
        }
    }

    /// Override the [`TABLE_CHROME`] reserve — for a delegate that overrides
    /// `render_last_empty_col` or renders the table unbordered.
    /// Inset these columns' body text by `px_2`, the way every sid text cell used to do
    /// with a wrapper `div` -- but through `Column::paddings`, which upstream's cell
    /// wrapper already applies, so a cell can be a bare [`CellText`](super::CellText)
    /// with no node of its own (`docs/design/2026-11-07-perf-pass.md`, table section).
    ///
    /// Only the **left** edge grows: the header shares `Column::paddings` and parks its
    /// sort chevron against the right padding, so growing that edge would move every
    /// chevron. [`sortable_th`](super::sortable_th) cancels the left growth with a
    /// negative margin, so header text stays where it was. The cost is the right edge of
    /// an ellipsised cell, which now ends at the column's default 8px padding rather than
    /// 16px.
    ///
    /// Columns that hold a widget (a kill button) are simply left out of `cols`.
    /// `cols` are declaration indices.
    pub fn inset_cells(mut self, cols: impl IntoIterator<Item = usize>) -> Self {
        self.inset = vec![false; self.columns.len()];
        for ix in cols {
            if let Some(slot) = self.inset.get_mut(ix) {
                *slot = true;
            }
        }
        self.apply_insets(UiScale::DEFAULT);
        self
    }

    /// The left padding an inset column carries at `scale`: upstream's default cell
    /// padding plus `px_2` (0.5rem, which is zoomed).
    fn inset_padding(scale: UiScale) -> Edges<Pixels> {
        let mut pad = Size::default().table_cell_padding();
        pad.left += scale.scale_px(px(INSET));
        pad
    }

    fn apply_insets(&mut self, scale: UiScale) -> bool {
        let pad = Self::inset_padding(scale);
        let mut changed = false;
        for (column, inset) in self.columns.iter_mut().zip(&self.inset) {
            if *inset && column.paddings != Some(pad) {
                column.paddings = Some(pad);
                changed = true;
            }
        }
        changed
    }

    /// Let the user drag headers to reorder the columns.
    ///
    /// Off by default because it is a promise about the delegate, not a style: after a
    /// move, the `col_ix` upstream passes to `render_td` and `perform_sort` is a
    /// **display position**, and the only correct way to read it is through
    /// [`FillColumns::logical`]. A delegate that still matches on `col_ix` directly
    /// would draw every cell under the wrong header, so tables that have not made that
    /// change get no drag at all ([`super::FillTable`] turns the gesture off).
    ///
    /// A column declared `Column::movable(false)` (an action column) cannot be picked
    /// up, though other columns can still be dropped on either side of it.
    pub fn reorderable(mut self) -> Self {
        self.reorderable = true;
        self
    }

    /// The declaration index of the column drawn at display position `col_ix`: its
    /// position in the list given to [`FillColumns::new`], whatever the user has done
    /// to the order since.
    ///
    /// Every `col_ix` a delegate receives from upstream (`render_td`, `perform_sort`,
    /// `context_menu` cells, ...) goes through this before it is matched on. Header
    /// code does not need it: [`FillColumns::column`] already takes a display position.
    pub fn logical(&self, col_ix: usize) -> usize {
        self.order[col_ix]
    }

    /// The column keys (`Column::key`) in display order, for persisting a user's order.
    pub fn order(&self) -> Vec<String> {
        self.order
            .iter()
            .map(|&ix| self.columns[ix].key.to_string())
            .collect()
    }

    /// The order worth persisting: the keys in display order, or `None` when the table
    /// is in its declared order, so a table dragged back to its default leaves no entry.
    pub fn saved_order(&self) -> Option<Vec<String>> {
        let declared = self.order.iter().enumerate().all(|(pos, &ix)| pos == ix);
        (!declared).then(|| self.order())
    }

    /// Call `save` with [`FillColumns::saved_order`] after every column the user drags.
    ///
    /// This is the only reorder signal a delegate should listen to. A subscriber to the
    /// table's own `TableEvent::MoveColumn` can run *before* this store has followed
    /// the move (subscribers run in the order they subscribed, and [`super::FillTable`]
    /// subscribes on first paint), and would save the old order. `save` runs after the
    /// store has moved, from the drop's event flush, so it may do I/O; it must not
    /// update the table it came from.
    pub fn on_reorder(&mut self, save: impl Fn(Option<Vec<String>>, &mut App) + 'static) {
        self.on_reorder = Some(Rc::new(save));
    }

    /// Restore an order saved by [`FillColumns::order`] or [`FillColumns::saved_order`].
    ///
    /// Lenient, because a saved order outlives the declaration it was taken from: keys
    /// this table no longer has are ignored, a key named twice counts once, and columns
    /// the saved order does not mention (added since) follow in declaration order. A
    /// table that is not [`reorderable`](Self::reorderable) ignores it, for the reason
    /// given there. The next [`FillColumns::sync`] reports a change, so a mounted
    /// [`super::FillTable`] refreshes itself.
    pub fn apply_order(&mut self, saved: &[String]) {
        if !self.reorderable {
            return;
        }
        let mut order = Vec::with_capacity(self.columns.len());
        for key in saved {
            if let Some(ix) = self.columns.iter().position(|c| c.key.as_ref() == key)
                && !order.contains(&ix)
            {
                order.push(ix);
            }
        }
        for ix in 0..self.columns.len() {
            if !order.contains(&ix) {
                order.push(ix);
            }
        }
        if order != self.order {
            self.order = order;
            self.reordered = true;
        }
    }

    /// Follow a move the table has just made to its own copy of the columns
    /// (`TableEvent::MoveColumn`): the same remove-then-insert upstream applies to
    /// `col_groups`, applied to the one vector that maps positions to columns.
    pub(super) fn move_column(&mut self, from: usize, to: usize) {
        let len = self.order.len();
        if !self.reorderable || from >= len || to >= len {
            return;
        }
        let ix = self.order.remove(from);
        self.order.insert(to, ix);
    }

    pub fn chrome(mut self, chrome: f32) -> Self {
        self.chrome = chrome;
        self
    }

    /// The number of columns, for `TableDelegate::columns_count`.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Whether the table declares no columns at all.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// The column at display position `ix`, for `TableDelegate::column` and `render_th`.
    ///
    /// Panics on an out-of-range index, exactly as upstream's own `render_th` does — a
    /// delegate whose `columns_count` disagrees with its store is a bug worth a loud
    /// failure rather than a silently blank column.
    pub fn column(&self, ix: usize) -> &Column {
        &self.columns[self.order[ix]]
    }

    /// Resize the columns to a measured viewport width, in logical pixels, at the app
    /// zoom currently in force.
    ///
    /// Returns whether anything actually moved — the caller uses that to decide whether
    /// the table needs a `TableState::refresh`, so that a steady viewport costs nothing
    /// after the frame it settles on.
    ///
    /// The `viewport` is a *real* measurement and is already zoomed; the declarations are
    /// authored at 100% and are not. So the floors and the chrome reserve get scaled and
    /// the viewport does not — get that backwards and zooming in truncates every cell
    /// while the table keeps its original column widths. See [`ColumnWidth::scaled`].
    pub fn sync(&mut self, viewport: f32, scale: UiScale) -> bool {
        let declared: Vec<ColumnWidth> = self.declared.iter().map(|d| d.scaled(scale)).collect();
        let chrome = f32::from(scale.scale_px(px(self.chrome)));
        // Resolved in declaration order: a width depends on the declarations and the
        // viewport, never on where the user has put the column.
        let resolved = resolve_widths(&declared, viewport - chrome);
        let mut changed = self.apply_insets(scale) | std::mem::take(&mut self.reordered);
        for (column, width) in self.columns.iter_mut().zip(resolved) {
            if (f32::from(column.width) - width).abs() >= REDRAW_EPSILON {
                column.width = px(width);
                changed = true;
            }
        }
        changed
    }

    /// Record a sort the table has just performed, so the header keeps its indicator.
    ///
    /// This is not bookkeeping for its own sake. `TableState` cycles sort state on its
    /// *runtime* copies of the columns (`table/state.rs:517-525`), and
    /// `TableState::refresh` rebuilds those copies from the delegate
    /// (`prepare_col_groups`, `state.rs:278-290`) — so any refresh after a sort snaps
    /// the header chevron back to whatever the delegate originally declared while the
    /// rows stay sorted the way the user asked. Every table this model drives refreshes
    /// on viewport change, which would make that pre-existing mismatch happen constantly;
    /// mirroring the sort into the delegate's own columns is what stops it.
    ///
    /// Matches upstream's cycle exactly: the sorted column takes `sort`, every other
    /// *sortable* column falls back to `ColumnSort::Default`, and columns that were
    /// never sortable stay that way (a `None` sort is what suppresses the chevron).
    ///
    /// `col_ix` is a display position, as upstream hands it to `perform_sort`.
    pub fn apply_sort(&mut self, col_ix: usize, sort: ColumnSort) {
        let sorted = self.order.get(col_ix).copied();
        for (ix, column) in self.columns.iter_mut().enumerate() {
            if column.sort.is_none() {
                continue;
            }
            column.sort = Some(if Some(ix) == sorted {
                sort
            } else {
                ColumnSort::Default
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The System tab's declaration, which is also the shape the plan calls for:
    /// numbers fixed, `Name` the grower, `User` floored.
    fn processes() -> FillColumns {
        FillColumns::new([
            (
                Column::new("cpu", "CPU%").descending(),
                ColumnWidth::Fixed(70.),
            ),
            (
                Column::new("mem", "Mem").sortable(),
                ColumnWidth::Fixed(90.),
            ),
            (
                Column::new("pid", "PID").sortable(),
                ColumnWidth::Fixed(80.),
            ),
            (
                Column::new("name", "Name").sortable(),
                ColumnWidth::grow().min_width(220.),
            ),
            (
                Column::new("user", "User").sortable(),
                ColumnWidth::Min(120.),
            ),
            (Column::new("kill", ""), ColumnWidth::Fixed(72.)),
        ])
    }

    fn widths(cols: &FillColumns) -> Vec<f32> {
        (0..cols.len())
            .map(|ix| f32::from(cols.column(ix).width))
            .collect()
    }

    #[test]
    fn a_fill_table_declares_no_resizable_column() {
        // The stray header column, root cause: upstream paints a 1px hairline at the
        // right edge of every *resizable* column, in the header only
        // (`state.rs::render_resize_handle`). On Network that put dividers at x=1889 —
        // the last column's edge — and x=1906, the table's own border, with the
        // `TABLE_CHROME` reserve between them reading as a 17px empty column no body
        // row draws. And the gesture those hairlines advertise cannot work in a fill
        // table: `sync` recomputes every width from the viewport on the next frame, so
        // a dragged column snaps back before it paints. A handle that cannot move
        // anything is chrome, and this is chrome the body does not have.
        let cols = processes();
        for ix in 0..cols.len() {
            assert!(
                !cols.column(ix).resizable,
                "column {ix} offers a resize the next sync would revert"
            );
        }
    }

    #[test]
    fn sync_writes_resolved_widths_into_the_columns() {
        let mut cols = processes();
        cols.sync(2000., UiScale::DEFAULT);
        // The five non-growers hold their declared 432px; Name takes the rest.
        let name = 220. + (2000. - TABLE_CHROME - 652.);
        assert_eq!(widths(&cols), vec![70., 90., 80., name, 120., 72.]);
    }

    #[test]
    fn the_columns_stay_strictly_inside_the_rows_content_box() {
        // A regression pin for the nastiest failure mode this file has. The row's
        // content box is the viewport less 2px of border and the 12px trailing strip;
        // the columns must land *strictly* inside it, never flush against it. At an
        // exact fit upstream's horizontal `virtual_list` mis-measures itself and clips
        // every cell out of its row — header, stripes and scrollbars render, zero cells
        // do. See `TABLE_CHROME`. Verified by capture at 2000x1200.
        let mut cols = processes();
        for viewport in [900., 1440., 2000., 3440.] {
            cols.sync(viewport, UiScale::DEFAULT);
            let total: f32 = widths(&cols).iter().sum();
            assert!(
                total < viewport - REAL_CHROME,
                "{viewport}px: columns total {total}, content box is {}",
                viewport - REAL_CHROME
            );
        }
    }

    #[test]
    fn the_columns_fill_the_viewport_less_the_table_chrome() {
        // The property the whole commit exists for, checked where it meets the library:
        // wider than this and the table grows a horizontal scrollbar; narrower and the
        // dead space is back.
        let mut cols = processes();
        for viewport in [900., 1440., 2000., 3440.] {
            cols.sync(viewport, UiScale::DEFAULT);
            let total: f32 = widths(&cols).iter().sum();
            assert!(
                (total - (viewport - TABLE_CHROME)).abs() < 0.01,
                "{viewport}px: columns total {total}"
            );
        }
    }

    #[test]
    fn sync_reports_the_first_measurement_as_a_change() {
        // The declared widths are whatever the delegate typed; the first real viewport
        // is nearly always a different answer, and the table has to be told.
        let mut cols = processes();
        assert!(cols.sync(2000., UiScale::DEFAULT));
    }

    #[test]
    fn sync_reports_no_change_at_an_unchanged_viewport() {
        // This is what keeps the measure-and-refresh loop from running every frame.
        let mut cols = processes();
        cols.sync(2000., UiScale::DEFAULT);
        assert!(!cols.sync(2000., UiScale::DEFAULT));
        assert!(!cols.sync(2000., UiScale::DEFAULT));
    }

    #[test]
    fn sync_reports_a_change_when_the_viewport_changes() {
        let mut cols = processes();
        cols.sync(2000., UiScale::DEFAULT);
        assert!(cols.sync(1200., UiScale::DEFAULT));
        assert!(cols.sync(2000., UiScale::DEFAULT));
    }

    #[test]
    fn sync_ignores_a_sub_pixel_viewport_wobble() {
        let mut cols = processes();
        cols.sync(2000., UiScale::DEFAULT);
        assert!(!cols.sync(2000.2, UiScale::DEFAULT));
    }

    #[test]
    fn a_custom_chrome_reserve_is_honoured() {
        let mut cols = processes().chrome(0.);
        cols.sync(2000., UiScale::DEFAULT);
        let total: f32 = widths(&cols).iter().sum();
        assert!((total - 2000.).abs() < 0.01, "{total}");
    }

    #[test]
    fn apply_sort_records_the_sort_on_the_clicked_column() {
        // Without this the next refresh — which a viewport change triggers — would
        // reset the header chevron while the rows stayed sorted.
        let mut cols = processes();
        cols.apply_sort(3, ColumnSort::Ascending);
        assert_eq!(cols.column(3).sort, Some(ColumnSort::Ascending));
    }

    #[test]
    fn apply_sort_clears_the_other_sortable_columns() {
        let mut cols = processes();
        cols.apply_sort(3, ColumnSort::Ascending);
        for ix in [0, 1, 2, 4] {
            assert_eq!(
                cols.column(ix).sort,
                Some(ColumnSort::Default),
                "column {ix}"
            );
        }
    }

    #[test]
    fn apply_sort_never_makes_an_unsortable_column_sortable() {
        // `Column::sort == None` is what suppresses the chevron; the trailing action
        // column must not sprout one.
        let mut cols = processes();
        cols.apply_sort(3, ColumnSort::Ascending);
        assert_eq!(cols.column(5).sort, None);
        cols.apply_sort(5, ColumnSort::Ascending);
        assert_eq!(cols.column(5).sort, None);
    }

    #[test]
    fn an_inset_column_carries_px_2_in_its_left_padding_only() {
        let cols = processes().inset_cells(0..=4);
        let default = Size::default().table_cell_padding();
        let pad = cols.column(0).paddings.expect("inset column has paddings");
        assert_eq!(pad.left, default.left + px(8.));
        assert_eq!(
            pad.right, default.right,
            "the chevron lives against this edge"
        );
        assert_eq!(pad.top, default.top);
        // The widget column is left alone.
        assert!(cols.column(5).paddings.is_none());
    }

    #[test]
    fn insets_follow_the_app_zoom() {
        let mut cols = processes().inset_cells(0..=4);
        let big = UiScale::from_percent(150);
        assert!(cols.sync(2000., big), "a new inset is a change");
        let default = Size::default().table_cell_padding();
        assert_eq!(
            cols.column(0).paddings.unwrap().left,
            default.left + big.scale_px(px(8.))
        );
    }

    fn keys(cols: &FillColumns) -> Vec<String> {
        cols.order()
    }

    #[test]
    fn a_move_carries_width_inset_and_sort_with_the_column() {
        let mut cols = processes().inset_cells(0..=4).reorderable();
        cols.sync(2000., UiScale::DEFAULT);
        cols.apply_sort(3, ColumnSort::Ascending);
        let name = cols.column(3).clone();

        cols.move_column(3, 0);
        assert_eq!(keys(&cols), ["name", "cpu", "mem", "pid", "user", "kill"]);
        assert_eq!(cols.logical(0), 3, "display 0 is the 4th declared column");
        let moved = cols.column(0);
        assert_eq!(moved.width, name.width);
        assert_eq!(moved.paddings, name.paddings);
        assert_eq!(moved.sort, Some(ColumnSort::Ascending));
        assert!(
            !cols.sync(2000., UiScale::DEFAULT),
            "a width never depends on where its column sits"
        );

        // A sort arriving afterwards names a display position, and hits what is drawn there.
        cols.apply_sort(1, ColumnSort::Descending);
        assert_eq!(cols.column(1).key.as_ref(), "cpu");
        assert_eq!(cols.column(1).sort, Some(ColumnSort::Descending));
        assert_eq!(cols.column(0).sort, Some(ColumnSort::Default));
    }

    #[test]
    fn a_saved_order_round_trips() {
        let mut cols = processes().reorderable();
        cols.move_column(0, 4);
        cols.move_column(5, 0);
        let saved = cols.order();
        let mut fresh = processes().reorderable();
        fresh.apply_order(&saved);
        assert_eq!(fresh.order(), saved);
        assert_eq!(
            (0..6).map(|ix| fresh.logical(ix)).collect::<Vec<_>>(),
            (0..6).map(|ix| cols.logical(ix)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn only_a_changed_order_is_worth_saving() {
        let mut cols = processes().reorderable();
        assert_eq!(cols.saved_order(), None);
        cols.move_column(5, 0);
        assert_eq!(
            cols.saved_order().as_deref(),
            Some(&cols.order()[..]),
            "a moved table saves its full order"
        );
        cols.move_column(0, 5);
        assert_eq!(
            cols.saved_order(),
            None,
            "back at its default, nothing to keep"
        );
    }

    #[test]
    fn a_saved_order_is_merged_not_trusted() {
        // Unknown keys dropped, a repeat counts once, and columns the saved order never
        // heard of (added since) follow in declaration order.
        let mut cols = processes().reorderable();
        let saved = ["user", "gone", "pid", "user"].map(String::from);
        cols.apply_order(&saved);
        assert_eq!(keys(&cols), ["user", "pid", "cpu", "mem", "name", "kill"]);
        cols.apply_order(&[]);
        assert_eq!(keys(&cols), ["cpu", "mem", "pid", "name", "user", "kill"]);
    }

    #[test]
    fn applying_an_order_is_reported_by_the_next_sync() {
        // So a mounted table re-reads its columns without the caller knowing to ask.
        let mut cols = processes().reorderable();
        cols.sync(2000., UiScale::DEFAULT);
        cols.apply_order(&["kill".to_string()]);
        assert!(cols.sync(2000., UiScale::DEFAULT));
        assert!(!cols.sync(2000., UiScale::DEFAULT));
        // The same order again is not a change.
        cols.apply_order(&cols.order());
        assert!(!cols.sync(2000., UiScale::DEFAULT));
    }

    #[test]
    fn a_store_that_is_not_reorderable_keeps_its_declared_order() {
        // Its delegate may still match `render_td` on display position.
        let mut cols = processes();
        cols.move_column(0, 3);
        cols.apply_order(&["kill".to_string()]);
        assert_eq!(keys(&cols), ["cpu", "mem", "pid", "name", "user", "kill"]);
    }

    #[test]
    fn the_store_reports_its_own_size() {
        assert_eq!(processes().len(), 6);
        assert!(!processes().is_empty());
        assert!(FillColumns::new([]).is_empty());
    }
}
