//! Cursor math for list panes that may carry a synthetic "+ add new" first row.
//!
//! Pure logic — no rendering. Widgets translate their row count into a
//! `ListCursor` and ask it which *item* (if any) is selected. Index 0 is the
//! synthetic add-new row when `add_new` is true; item indices are offset by 1.

/// What the cursor currently points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorTarget {
    /// The synthetic "+ add new" row.
    AddNew,
    /// A real item, by index into the widget's backing vec.
    Item(usize),
    /// List is empty and there is no add-new row.
    Nothing,
}

/// Cursor over `len` items plus an optional synthetic first row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListCursor {
    /// Number of real items.
    pub len: usize,
    /// Whether the synthetic "+ add new" row is shown (governed by the
    /// `show_add_new_row` behavior toggle, hydrated by the binary).
    pub add_new: bool,
    /// Raw cursor position over the combined rows.
    pub pos: usize,
}

impl ListCursor {
    /// Total navigable rows (items + synthetic row).
    pub fn total(&self) -> usize {
        self.len + usize::from(self.add_new)
    }

    /// Build a cursor clamped into range. Position clamps to the last row;
    /// an empty list with no add-new row pins `pos` to 0.
    pub fn new(len: usize, add_new: bool, pos: usize) -> Self {
        let total = len + usize::from(add_new);
        Self {
            len,
            add_new,
            pos: pos.min(total.saturating_sub(1)),
        }
    }

    /// What the cursor points at.
    pub fn target(&self) -> CursorTarget {
        if self.total() == 0 {
            CursorTarget::Nothing
        } else if self.add_new && self.pos == 0 {
            CursorTarget::AddNew
        } else {
            CursorTarget::Item(self.pos - usize::from(self.add_new))
        }
    }

    /// Move down one row, saturating at the bottom.
    pub fn down(&mut self) {
        if self.pos + 1 < self.total() {
            self.pos += 1;
        }
    }

    /// Move up one row, saturating at the top.
    pub fn up(&mut self) {
        self.pos = self.pos.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_no_addnew_is_nothing() {
        assert_eq!(ListCursor::new(0, false, 0).target(), CursorTarget::Nothing);
    }

    #[test]
    fn empty_with_addnew_targets_addnew() {
        assert_eq!(ListCursor::new(0, true, 0).target(), CursorTarget::AddNew);
    }

    #[test]
    fn addnew_offsets_item_indices() {
        let c = ListCursor::new(3, true, 2);
        assert_eq!(c.target(), CursorTarget::Item(1));
    }

    #[test]
    fn no_addnew_indices_are_identity() {
        let c = ListCursor::new(3, false, 2);
        assert_eq!(c.target(), CursorTarget::Item(2));
    }

    #[test]
    fn pos_clamps_into_range_and_motion_saturates() {
        let mut c = ListCursor::new(2, true, 99);
        assert_eq!(c.pos, 2); // clamped to last row
        c.down();
        assert_eq!(c.pos, 2); // saturates
        c.up();
        c.up();
        c.up();
        assert_eq!(c.pos, 0); // saturates at top
        assert_eq!(c.target(), CursorTarget::AddNew);
    }

    #[test]
    fn toggling_addnew_off_keeps_selection_valid() {
        // Simulates the behavior toggle flipping mid-session: rebuild with same pos.
        let on = ListCursor::new(3, true, 3); // Item(2)
        let off = ListCursor::new(3, false, on.pos);
        assert_eq!(off.target(), CursorTarget::Item(2)); // pos clamped 3 -> 2
    }
}
