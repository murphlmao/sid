//! State for the interfaces sidebar pane: a single-column list with
//! wrap-around selection. Unlike the ports/processes tables, the sidebar
//! has no user-driven sort: rows arrive pre-sorted by name from the
//! provider.
//!
//! Selection is preserved across data refreshes when an interface with the
//! same name is still present; otherwise it resets to 0.

use sid_core::adapters::sys::NetInterface;

/// State for the interfaces sidebar.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::interfaces_sidebar::InterfacesSidebarState;
/// let s = InterfacesSidebarState::new();
/// assert!(s.rows().is_empty());
/// assert!(s.selected_row().is_none());
/// ```
#[derive(Debug, Default)]
pub struct InterfacesSidebarState {
    data: Vec<NetInterface>,
    selected: usize,
}

impl InterfacesSidebarState {
    /// Construct a fresh, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the displayed interfaces.
    ///
    /// If the previously-selected interface (by name) is still present in
    /// the new data, the selection moves with it. Otherwise the selection
    /// resets to 0.
    pub fn set_data(&mut self, data: Vec<NetInterface>) {
        let prev_name = self.data.get(self.selected).map(|i| i.name.clone());
        self.data = data;
        self.selected = prev_name
            .and_then(|n| self.data.iter().position(|i| i.name == n))
            .unwrap_or(0);
        if self.selected >= self.data.len() {
            self.selected = 0;
        }
    }

    /// Borrow the displayed rows.
    pub fn rows(&self) -> &[NetInterface] {
        &self.data
    }

    /// Current selection index.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Currently-selected interface, or `None` if the list is empty.
    pub fn selected_row(&self) -> Option<&NetInterface> {
        self.data.get(self.selected)
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
