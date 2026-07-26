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

use gpui::px;
use gpui_component::table::{Column, ColumnSort};

use super::column_width::{ColumnWidth, resolve_widths};

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
pub struct FillColumns {
    columns: Vec<Column>,
    declared: Vec<ColumnWidth>,
    chrome: f32,
}

impl FillColumns {
    /// Declare the columns, each paired with how it wants to be sized.
    pub fn new(declared: impl IntoIterator<Item = (Column, ColumnWidth)>) -> Self {
        let (columns, declared): (Vec<_>, Vec<_>) = declared.into_iter().unzip();
        Self {
            columns,
            declared,
            chrome: TABLE_CHROME,
        }
    }

    /// Override the [`TABLE_CHROME`] reserve — for a delegate that overrides
    /// `render_last_empty_col` or renders the table unbordered.
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

    /// The column at `ix`, for `TableDelegate::column`.
    ///
    /// Panics on an out-of-range index, exactly as upstream's own `render_th` does — a
    /// delegate whose `columns_count` disagrees with its store is a bug worth a loud
    /// failure rather than a silently blank column.
    pub fn column(&self, ix: usize) -> &Column {
        &self.columns[ix]
    }

    /// Resize the columns to a measured viewport width, in logical pixels.
    ///
    /// Returns whether anything actually moved — the caller uses that to decide whether
    /// the table needs a `TableState::refresh`, so that a steady viewport costs nothing
    /// after the frame it settles on.
    pub fn sync(&mut self, viewport: f32) -> bool {
        let resolved = resolve_widths(&self.declared, viewport - self.chrome);
        let mut changed = false;
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
    pub fn apply_sort(&mut self, col_ix: usize, sort: ColumnSort) {
        for (ix, column) in self.columns.iter_mut().enumerate() {
            if column.sort.is_none() {
                continue;
            }
            column.sort = Some(if ix == col_ix {
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
    fn sync_writes_resolved_widths_into_the_columns() {
        let mut cols = processes();
        cols.sync(2000.);
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
            cols.sync(viewport);
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
            cols.sync(viewport);
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
        assert!(cols.sync(2000.));
    }

    #[test]
    fn sync_reports_no_change_at_an_unchanged_viewport() {
        // This is what keeps the measure-and-refresh loop from running every frame.
        let mut cols = processes();
        cols.sync(2000.);
        assert!(!cols.sync(2000.));
        assert!(!cols.sync(2000.));
    }

    #[test]
    fn sync_reports_a_change_when_the_viewport_changes() {
        let mut cols = processes();
        cols.sync(2000.);
        assert!(cols.sync(1200.));
        assert!(cols.sync(2000.));
    }

    #[test]
    fn sync_ignores_a_sub_pixel_viewport_wobble() {
        let mut cols = processes();
        cols.sync(2000.);
        assert!(!cols.sync(2000.2));
    }

    #[test]
    fn a_custom_chrome_reserve_is_honoured() {
        let mut cols = processes().chrome(0.);
        cols.sync(2000.);
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
    fn the_store_reports_its_own_size() {
        assert_eq!(processes().len(), 6);
        assert!(!processes().is_empty());
        assert!(FillColumns::new([]).is_empty());
    }
}
