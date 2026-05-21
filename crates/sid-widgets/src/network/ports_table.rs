//! State for the listening-ports table: sort column, sort direction,
//! selection cursor, and the underlying data slice.
//!
//! The state is intentionally separated from any ratatui-side rendering so
//! tests can drive the full behaviour without touching a buffer or a
//! `RenderTarget`. The widget assembly (in `widget.rs`) does the actual
//! drawing using a frozen view of this state.

use sid_core::adapters::sys::ListeningPort;

/// Sortable columns for the ports table.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::ports_table::PortsSortBy;
/// assert_ne!(PortsSortBy::Port, PortsSortBy::Pid);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortsSortBy {
    /// Port number.
    Port,
    /// Owning process id (rows without a pid sort first under Asc).
    Pid,
    /// Command name (lexicographic).
    Command,
    /// Transport protocol (TCP / UDP).
    Protocol,
}

/// Sort direction. Defaults to ascending.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::ports_table::SortDir;
/// assert_eq!(SortDir::default(), SortDir::Asc);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortDir {
    /// Ascending — lowest first.
    #[default]
    Asc,
    /// Descending — highest first.
    Desc,
}

/// State for the ports pane.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::ports_table::PortsTableState;
/// let s = PortsTableState::new();
/// assert_eq!(s.selected_index(), 0);
/// assert!(s.rows().is_empty());
/// ```
#[derive(Debug, Default)]
pub struct PortsTableState {
    data: Vec<ListeningPort>,
    sort_by: Option<PortsSortBy>,
    sort_dir: SortDir,
    selected: usize,
}

impl PortsTableState {
    /// Construct a fresh, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the underlying data. If a sort column is set, the new data is
    /// sorted before being stored. If the previously-selected index is now
    /// out of bounds, the selection resets to 0.
    pub fn set_data(&mut self, mut data: Vec<ListeningPort>) {
        if let Some(by) = self.sort_by {
            sort_rows(&mut data, by, self.sort_dir);
        }
        self.data = data;
        if self.selected >= self.data.len() {
            self.selected = 0;
        }
    }

    /// Borrow the currently-displayed rows.
    pub fn rows(&self) -> &[ListeningPort] {
        &self.data
    }

    /// Current selection index. Always within `0..=data.len()`; for an empty
    /// table, returns 0 even though no row is selectable — callers should
    /// pair with [`Self::selected_row`] to disambiguate.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Currently-selected row, or `None` if the table is empty.
    pub fn selected_row(&self) -> Option<&ListeningPort> {
        self.data.get(self.selected)
    }

    /// Set the sort column and direction. The current data is sorted in
    /// place.
    pub fn set_sort(&mut self, by: PortsSortBy, dir: SortDir) {
        self.sort_by = Some(by);
        self.sort_dir = dir;
        sort_rows(&mut self.data, by, dir);
    }

    /// Current sort column, if any.
    pub fn sort_by(&self) -> Option<PortsSortBy> {
        self.sort_by
    }

    /// Current sort direction.
    pub fn sort_dir(&self) -> SortDir {
        self.sort_dir
    }

    /// Advance selection by one, wrapping to the first row at end of list.
    /// No-op on an empty table.
    pub fn select_next(&mut self) {
        if self.data.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.data.len();
    }

    /// Move selection back by one, wrapping to the last row at start of list.
    /// No-op on an empty table.
    pub fn select_prev(&mut self) {
        if self.data.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.data.len() - 1
        } else {
            self.selected - 1
        };
    }
}

fn sort_rows(data: &mut [ListeningPort], by: PortsSortBy, dir: SortDir) {
    data.sort_by(|a, b| {
        let ord = match by {
            PortsSortBy::Port => a.port.cmp(&b.port),
            PortsSortBy::Pid => a.pid.map(|p| p.as_u32()).cmp(&b.pid.map(|p| p.as_u32())),
            PortsSortBy::Command => a.command.cmp(&b.command),
            PortsSortBy::Protocol => format!("{:?}", a.protocol).cmp(&format!("{:?}", b.protocol)),
        };
        match dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });
}
