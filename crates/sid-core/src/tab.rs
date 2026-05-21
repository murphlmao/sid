//! Tab and TabManager — the top-level navigation structure of the sid cockpit.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::layout::Layout;

/// Stable, unique identifier for a tab.
///
/// # Examples
///
/// ```
/// use sid_core::tab::TabId;
///
/// let id = TabId::new("git");
/// assert_eq!(id.as_str(), "git");
/// assert_eq!(id.to_string(), "git");
/// ```
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct TabId(String);

impl TabId {
    /// Create a new `TabId` from any string-like value.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::tab::TabId;
    ///
    /// let id = TabId::new("workspace");
    /// assert_eq!(id.as_str(), "workspace");
    /// ```
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Return the inner string slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::tab::TabId;
    ///
    /// let id = TabId::new("db");
    /// assert_eq!(id.as_str(), "db");
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A single tab in the sid cockpit. Each tab owns a [`Layout`] of widgets.
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub layout: Layout,
    pub hotkey: Option<char>,
}

/// Manages an ordered list of tabs, tracking which is currently active.
///
/// # Examples
///
/// ```
/// use sid_core::tab::{Tab, TabId, TabManager};
/// use sid_core::layout::{Dir, Layout};
/// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
/// # struct W { id: WidgetId }
/// # impl Widget for W {
/// #     fn id(&self) -> &WidgetId { &self.id }
/// #     fn title(&self) -> &str { "t" }
/// #     fn render(&self, _: &mut dyn RenderTarget) {}
/// #     fn handle_event(&mut self, _: &sid_core::event::Event, _: &mut sid_core::context::WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
/// # }
///
/// let tabs = vec![
///     Tab { id: TabId::new("a"), title: "A".into(), layout: Layout::Single(Box::new(W { id: WidgetId::new("w") })), hotkey: None },
///     Tab { id: TabId::new("b"), title: "B".into(), layout: Layout::Single(Box::new(W { id: WidgetId::new("w2") })), hotkey: None },
/// ];
/// let mut mgr = TabManager::new(tabs);
/// assert_eq!(mgr.active().id.as_str(), "a");
/// mgr.next();
/// assert_eq!(mgr.active().id.as_str(), "b");
/// ```
pub struct TabManager {
    tabs: Vec<Tab>,
    active_idx: usize,
}

impl TabManager {
    /// Create a new `TabManager`.
    ///
    /// # Panics
    ///
    /// Panics if `tabs` is empty — a cockpit with no tabs is invalid.
    pub fn new(tabs: Vec<Tab>) -> Self {
        assert!(!tabs.is_empty(), "TabManager requires at least one tab");
        Self {
            tabs,
            active_idx: 0,
        }
    }

    /// Return a reference to the currently active tab.
    pub fn active(&self) -> &Tab {
        &self.tabs[self.active_idx]
    }

    /// Return a mutable reference to the currently active tab.
    pub fn active_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_idx]
    }

    /// Return the full ordered slice of tabs.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Return the zero-based index of the active tab.
    pub fn active_index(&self) -> usize {
        self.active_idx
    }

    /// Advance to the next tab, wrapping around to the first.
    ///
    /// # Examples
    ///
    /// ```
    /// # use sid_core::tab::{Tab, TabId, TabManager};
    /// # use sid_core::layout::Layout;
    /// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// # struct W { id: WidgetId }
    /// # impl Widget for W {
    /// #     fn id(&self) -> &WidgetId { &self.id }
    /// #     fn title(&self) -> &str { "t" }
    /// #     fn render(&self, _: &mut dyn RenderTarget) {}
    /// #     fn handle_event(&mut self, _: &sid_core::event::Event, _: &mut sid_core::context::WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    /// # }
    /// # fn make_tab(s: &'static str) -> Tab {
    /// #     Tab { id: TabId::new(s), title: s.into(), layout: Layout::Single(Box::new(W { id: WidgetId::new(s) })), hotkey: None }
    /// # }
    /// let mut mgr = TabManager::new(vec![make_tab("a"), make_tab("b")]);
    /// mgr.next();
    /// assert_eq!(mgr.active_index(), 1);
    /// mgr.next(); // wraps
    /// assert_eq!(mgr.active_index(), 0);
    /// ```
    pub fn next(&mut self) {
        self.active_idx = (self.active_idx + 1) % self.tabs.len();
    }

    /// Move to the previous tab, wrapping around to the last.
    pub fn prev(&mut self) {
        self.active_idx = (self.active_idx + self.tabs.len() - 1) % self.tabs.len();
    }

    /// Jump to a specific index, clamping to the last tab if out of range.
    pub fn jump(&mut self, idx: usize) {
        self.active_idx = idx.min(self.tabs.len() - 1);
    }

    /// Switch to the tab with the given id. Returns `true` if found, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use sid_core::tab::{Tab, TabId, TabManager};
    /// # use sid_core::layout::Layout;
    /// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// # struct W { id: WidgetId }
    /// # impl Widget for W {
    /// #     fn id(&self) -> &WidgetId { &self.id }
    /// #     fn title(&self) -> &str { "t" }
    /// #     fn render(&self, _: &mut dyn RenderTarget) {}
    /// #     fn handle_event(&mut self, _: &sid_core::event::Event, _: &mut sid_core::context::WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    /// # }
    /// # fn make_tab(s: &'static str) -> Tab {
    /// #     Tab { id: TabId::new(s), title: s.into(), layout: Layout::Single(Box::new(W { id: WidgetId::new(s) })), hotkey: None }
    /// # }
    /// let mut mgr = TabManager::new(vec![make_tab("a"), make_tab("b"), make_tab("c")]);
    /// assert!(mgr.switch_to(&TabId::new("c")));
    /// assert_eq!(mgr.active_index(), 2);
    /// assert!(!mgr.switch_to(&TabId::new("nope")));
    /// ```
    pub fn switch_to(&mut self, id: &TabId) -> bool {
        if let Some(i) = self.tabs.iter().position(|t| &t.id == id) {
            self.active_idx = i;
            true
        } else {
            false
        }
    }
}
