//! Header cells whose **whole width** toggles the column's sort.
//!
//! # The bug this fixes
//!
//! In `gpui-component-0.5.1` the only element wired to sorting is the chevron glyph:
//! `render_sort_icon` (`table/state.rs:744-785`) is the sole caller of
//! `TableState::perform_sort`. The header *cell* around it gets a different handler —
//! `render_th` (`:791-820`) attaches `on_col_head_click` (`:327-341`), which only ever
//! sets column *selection*. Clicking the word "PID" therefore paints the column maroon
//! and leaves the rows exactly as they were; only a hit on the glyph sorts.
//!
//! The glyph is not 12px either. `render_th` lays the delegate's label and the icon in
//! one `h_flex().justify_between()`, and upstream's default label is `div().size_full()`
//! — width 100% of the row — so flex-shrink squeezes the icon down to the **~6x8px**
//! target measured in `docs/design/2026-07-26-bughunt-findings.md` §8. Every table in
//! the app inherits it, because every table uses the default `render_th`.
//!
//! # Why this is not a fork
//!
//! Upstream's `TableState::perform_sort` is private (`state.rs:500`) and `col_groups` is
//! a private field, so the sort cycle cannot be *called* from outside. It does not need
//! to be. Everything it does is reachable through public API:
//!
//! | `TableState::perform_sort` step | reached here by |
//! |---|---|
//! | cycle `Ascending -> Default -> Descending` | [`next_sort`], a pure function |
//! | write the new sort onto the columns | [`FillColumns::apply_sort`] on the delegate's own store |
//! | reorder the rows | `TableDelegate::perform_sort`, a public trait method |
//! | re-read the columns into the header | `TableState::refresh` -> `prepare_col_groups` (`state.rs:278-290`), which clones `sort` straight off the delegate's `Column`s |
//!
//! and `TableDelegate::render_th` hands the delegate a full `&mut Context<TableState<D>>`
//! (`table/delegate.rs:47-56`), which is exactly the handle `cx.listener` needs. So the
//! whole fix is a delegate-side header cell: sid keeps the library's table, header,
//! chevron, cycle, comparators and scrollbars, and only supplies a bigger click target.
//!
//! # Why the column no longer highlights
//!
//! The listener ends in `cx.stop_propagation()`. gpui fires click handlers from a
//! `MouseUpEvent` in the bubble phase, dispatched innermost-first
//! (`gpui-0.2.2/src/window.rs:3703-3709` iterates `.rev()` and breaks as soon as
//! `propagate_event` clears), so stopping here means upstream's cell-level
//! `on_col_head_click` never runs. A click on a *sortable* header sorts and does not
//! select — which is the behaviour the bug report asked for. Non-sortable headers get no
//! listener at all and keep upstream's selection behaviour untouched.
//!
//! # Using it
//!
//! One method on the delegate, replacing the default `render_th`:
//!
//! ```ignore
//! fn render_th(
//!     &mut self,
//!     col_ix: usize,
//!     _window: &mut Window,
//!     cx: &mut Context<TableState<Self>>,
//! ) -> impl IntoElement {
//!     sortable_th(col_ix, self.columns.column(col_ix), cx)
//! }
//! ```
//!
//! Every table rendered by [`super::FillTable`] can adopt it; a table still on a
//! hand-rolled `Vec<Column>` inherits it when it migrates to [`FillColumns`], because
//! the mirroring step above needs [`FillColumns::apply_sort`] to survive the refresh.

use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::table::{Column, ColumnSort, TableState};

use super::FillTableDelegate;
use crate::typography::Typography;

/// The sort a click on this header should apply next, or `None` for "do nothing".
///
/// The one decision in the whole file, kept pure so it can be checked without a window.
/// It has to agree with `TableState::perform_sort` (`table/state.rs:511-515`) exactly:
/// the chevron still runs upstream's copy of this cycle, and a table where the glyph and
/// the label disagreed about the next state would be worse than the bug it replaces.
///
/// `None` in means the column is not sortable — `Column::sort` is `Option`, and `None` is
/// what suppresses the chevron — so `None` comes back out and nothing happens.
pub fn next_sort(current: Option<ColumnSort>) -> Option<ColumnSort> {
    match current? {
        ColumnSort::Ascending => Some(ColumnSort::Default),
        ColumnSort::Descending => Some(ColumnSort::Ascending),
        ColumnSort::Default => Some(ColumnSort::Descending),
    }
}

/// A header cell that sorts when clicked anywhere on it, not just on the chevron.
///
/// A drop-in body for `TableDelegate::render_th`. Renders the same element upstream's
/// default does — `div().size_full()` around the column name, so the header is
/// pixel-identical — plus an id, a pointer cursor and the click listener. See the module
/// docs for why that div is already the full width of the cell.
///
/// Columns that are not sortable (`Column::sort == None`) get the plain label back.
pub fn sortable_th<D: FillTableDelegate>(
    col_ix: usize,
    column: &Column,
    cx: &mut Context<TableState<D>>,
) -> impl IntoElement {
    let sortable = column.sort.is_some();
    let theme = crate::theme::active(cx).clone();

    div()
        .id(("sid-th", col_ix))
        .size_full()
        .when(sortable, |this| {
            this.cursor_pointer()
                .on_click(cx.listener(move |table, _, window, cx| {
                    toggle_sort(table, col_ix, window, cx);
                    // Or the click also lands on upstream's column-selection handler.
                    cx.stop_propagation();
                }))
        })
        .text_label(&theme)
        .child(column.name.clone())
}

/// Advance `col_ix` through the sort cycle, exactly as a chevron click would.
///
/// Split out from the listener so the ordering is readable: mirror the new sort onto the
/// delegate's columns *before* `refresh`, because `refresh` rebuilds the header's copy of
/// every column from the delegate and would otherwise re-read the old indicator.
///
/// `apply_sort` runs here rather than being left to the delegate: a delegate that forgets
/// it loses only its chevron on the chevron path (upstream having already written the
/// runtime copy), but on this path it would lose the indicator outright. Delegates that
/// do call it — the ones written against `FillColumns::apply_sort`'s advice — are
/// unaffected, since applying the same sort twice is idempotent.
fn toggle_sort<D: FillTableDelegate>(
    table: &mut TableState<D>,
    col_ix: usize,
    window: &mut Window,
    cx: &mut Context<TableState<D>>,
) {
    if !table.sortable {
        return;
    }

    let current = table.delegate_mut().fill_columns().column(col_ix).sort;
    let Some(sort) = next_sort(current) else {
        return;
    };

    table.delegate_mut().fill_columns().apply_sort(col_ix, sort);
    table.delegate_mut().perform_sort(col_ix, sort, window, cx);
    table.refresh(cx);
    cx.notify();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsorted_column_sorts_descending_first() {
        // Upstream's first-click direction. Every sid table that defaults to "biggest
        // first" (CPU%, memory, port counts) depends on it.
        assert_eq!(
            next_sort(Some(ColumnSort::Default)),
            Some(ColumnSort::Descending)
        );
    }

    #[test]
    fn a_descending_column_flips_to_ascending() {
        assert_eq!(
            next_sort(Some(ColumnSort::Descending)),
            Some(ColumnSort::Ascending)
        );
    }

    #[test]
    fn an_ascending_column_returns_to_unsorted() {
        assert_eq!(
            next_sort(Some(ColumnSort::Ascending)),
            Some(ColumnSort::Default)
        );
    }

    #[test]
    fn a_column_that_is_not_sortable_yields_no_sort() {
        // `Column::sort == None` is what suppresses the chevron. The System tab's
        // trailing `kill` column is one; clicking its header must stay inert rather
        // than sorting the table by an empty string.
        assert_eq!(next_sort(None), None);
    }

    #[test]
    fn three_clicks_return_the_column_to_where_it_started() {
        // The property that makes the header honest: the cycle is closed, so a user who
        // clicks past the state they wanted gets back to it rather than into a fourth
        // state upstream's chevron does not know about.
        for start in [
            ColumnSort::Default,
            ColumnSort::Ascending,
            ColumnSort::Descending,
        ] {
            let mut sort = start;
            for _ in 0..3 {
                sort = next_sort(Some(sort)).expect("a sortable column stays sortable");
            }
            assert_eq!(sort, start, "cycle from {start:?}");
        }
    }
}
