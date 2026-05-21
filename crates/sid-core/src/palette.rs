//! The command palette — an overlay for fuzzy-searching and executing actions.

use crate::action::{Action, ActionRegistry};

/// Interactive command palette backed by [`ActionRegistry`] fuzzy search.
///
/// # Examples
///
/// ```
/// use sid_core::action::{Action, ActionRegistry};
/// use sid_core::palette::CommandPalette;
///
/// let mut reg = ActionRegistry::new();
/// reg.register(Action::new("app.quit", "Quit"));
///
/// let mut palette = CommandPalette::new();
/// assert!(!palette.is_open());
///
/// palette.open();
/// assert!(palette.is_open());
///
/// palette.input("q");
/// assert_eq!(palette.query(), "q");
/// let hit = palette.current(&reg);
/// assert!(hit.is_some());
///
/// palette.close();
/// assert!(!palette.is_open());
/// ```
pub struct CommandPalette {
    open: bool,
    query: String,
    selected: usize,
}

impl CommandPalette {
    /// Create a new, closed command palette.
    pub fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
        }
    }

    /// Open the palette, clearing any previous query and selection.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::palette::CommandPalette;
    ///
    /// let mut p = CommandPalette::new();
    /// p.open();
    /// assert!(p.is_open());
    /// assert_eq!(p.query(), "");
    /// assert_eq!(p.selected_index(), 0);
    /// ```
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    /// Close the palette, clearing query and selection.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::palette::CommandPalette;
    ///
    /// let mut p = CommandPalette::new();
    /// p.open();
    /// p.close();
    /// assert!(!p.is_open());
    /// assert_eq!(p.query(), "");
    /// assert_eq!(p.selected_index(), 0);
    /// ```
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    /// Return `true` if the palette is currently open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Return the current search query string.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Return the zero-based index of the currently highlighted result.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Append `s` to the query and reset the selection to 0.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::palette::CommandPalette;
    ///
    /// let mut p = CommandPalette::new();
    /// p.input("qu");
    /// assert_eq!(p.query(), "qu");
    /// p.input("it");
    /// assert_eq!(p.query(), "quit");
    /// ```
    pub fn input(&mut self, s: &str) {
        self.query.push_str(s);
        self.selected = 0;
    }

    /// Remove the last character from the query and reset the selection to 0.
    ///
    /// No-op if the query is already empty.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// Move the selection one position downward, wrapping around.
    ///
    /// Has no effect (and does not panic) if there are no matches.
    pub fn cursor_down(&mut self, reg: &ActionRegistry) {
        let len = self.matches(reg).len().max(1);
        self.selected = (self.selected + 1) % len;
    }

    /// Move the selection one position upward, wrapping around.
    ///
    /// Has no effect (and does not panic) if there are no matches.
    pub fn cursor_up(&mut self, reg: &ActionRegistry) {
        let len = self.matches(reg).len().max(1);
        self.selected = (self.selected + len - 1) % len;
    }

    /// Return actions matching the current query, ranked by fuzzy score.
    pub fn matches<'a>(&self, reg: &'a ActionRegistry) -> Vec<&'a Action> {
        reg.fuzzy(&self.query)
    }

    /// Return the currently selected action, or `None` if there are no matches.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    /// use sid_core::palette::CommandPalette;
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("app.quit", "Quit"));
    ///
    /// let mut p = CommandPalette::new();
    /// p.input("quit");
    /// assert!(p.current(&reg).is_some());
    ///
    /// // No match → None
    /// let mut p2 = CommandPalette::new();
    /// p2.input("zzzzz");
    /// assert!(p2.current(&reg).is_none());
    /// ```
    pub fn current<'a>(&self, reg: &'a ActionRegistry) -> Option<&'a Action> {
        let matches = self.matches(reg);
        matches.get(self.selected).copied()
    }
}

impl Default for CommandPalette {
    fn default() -> Self {
        Self::new()
    }
}
