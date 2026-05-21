//! State for the processes table: sort column, sort direction, selection
//! cursor, and the underlying data slice.
//!
//! Mirrors [`super::ports_table`] but for `ProcessInfo` rows. Sort
//! comparators handle NaN CPU values defensively — sysinfo can return
//! `f32::NAN` for processes that died between two snapshots.

use std::cmp::Ordering;

use sid_core::adapters::sys::ProcessInfo;

pub use super::ports_table::SortDir;

/// Sortable columns for the processes table.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::processes_table::ProcessesSortBy;
/// assert_ne!(ProcessesSortBy::Pid, ProcessesSortBy::Cpu);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessesSortBy {
    /// Process id.
    Pid,
    /// Short process name.
    Name,
    /// CPU percent (NaN values sort last under Asc, first under Desc — we
    /// treat NaN as `Ordering::Equal` to keep sorts total).
    Cpu,
    /// Resident set size in bytes.
    Rss,
    /// Start time (seconds since UNIX epoch).
    Started,
}

/// State for the processes pane.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::processes_table::ProcessesTableState;
/// let s = ProcessesTableState::new();
/// assert_eq!(s.selected_index(), 0);
/// assert!(s.rows().is_empty());
/// ```
#[derive(Debug, Default)]
pub struct ProcessesTableState {
    data: Vec<ProcessInfo>,
    sort_by: Option<ProcessesSortBy>,
    sort_dir: SortDir,
    selected: usize,
}

impl ProcessesTableState {
    /// Construct a fresh, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the underlying data. If a sort column is set, the new data is
    /// sorted before being stored. If the previously-selected index is now
    /// out of bounds, the selection resets to 0.
    pub fn set_data(&mut self, mut data: Vec<ProcessInfo>) {
        if let Some(by) = self.sort_by {
            sort_rows(&mut data, by, self.sort_dir);
        }
        self.data = data;
        if self.selected >= self.data.len() {
            self.selected = 0;
        }
    }

    /// Borrow the currently-displayed rows.
    pub fn rows(&self) -> &[ProcessInfo] {
        &self.data
    }

    /// Current selection index. Always within `0..=data.len()`; pair with
    /// [`Self::selected_row`] for the empty-table case.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Currently-selected row, or `None` if the table is empty.
    pub fn selected_row(&self) -> Option<&ProcessInfo> {
        self.data.get(self.selected)
    }

    /// Set sort column and direction. The current data is sorted in place.
    pub fn set_sort(&mut self, by: ProcessesSortBy, dir: SortDir) {
        self.sort_by = Some(by);
        self.sort_dir = dir;
        sort_rows(&mut self.data, by, dir);
    }

    /// Current sort column, if any.
    pub fn sort_by(&self) -> Option<ProcessesSortBy> {
        self.sort_by
    }

    /// Current sort direction.
    pub fn sort_dir(&self) -> SortDir {
        self.sort_dir
    }

    /// Advance selection by one, wrapping around the end. No-op when empty.
    pub fn select_next(&mut self) {
        if self.data.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.data.len();
    }

    /// Move selection back by one, wrapping around the start. No-op when
    /// empty.
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

fn sort_rows(data: &mut [ProcessInfo], by: ProcessesSortBy, dir: SortDir) {
    data.sort_by(|a, b| {
        let ord = match by {
            ProcessesSortBy::Pid => a.pid.as_u32().cmp(&b.pid.as_u32()),
            ProcessesSortBy::Name => a.name.cmp(&b.name),
            // NaN guard: `partial_cmp` returns None for NaN; treat as Equal.
            ProcessesSortBy::Cpu => a.cpu_pct.partial_cmp(&b.cpu_pct).unwrap_or(Ordering::Equal),
            ProcessesSortBy::Rss => a.rss_bytes.cmp(&b.rss_bytes),
            ProcessesSortBy::Started => a.started_unix_secs.cmp(&b.started_unix_secs),
        };
        match dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });
}
