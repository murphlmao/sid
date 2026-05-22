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

/// Kind of tab — distinguishes pinned cockpit tabs from dynamically-opened
/// detail tabs.
///
/// `Core` tabs are the fixed six the cockpit boots with; they cannot be
/// closed at runtime. `Detail` tabs are dynamically opened by user action
/// (e.g., pressing Enter on a workspace) and carry a `parent_idx` pointing
/// back at the core tab that spawned them, so `close_active` can snap focus
/// back to it.
///
/// # Examples
///
/// ```
/// use sid_core::tab::TabKind;
///
/// assert_eq!(TabKind::Core, TabKind::Core);
/// assert_ne!(TabKind::Core, TabKind::Detail { parent_idx: 0 });
/// ```
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum TabKind {
    /// One of the pinned cockpit tabs (workspaces, ssh, database, network,
    /// system, settings). Pinned tabs cannot be closed at runtime.
    Core,
    /// Dynamically opened, closable. Carries the index of the core tab
    /// that spawned this detail tab so `close_active` can snap `active_idx`
    /// back to the spawning tab.
    Detail {
        /// Index of the spawning core tab (always `< 6` in v1 — the six
        /// cockpit tabs occupy the first six positions of `TabManager::tabs`).
        parent_idx: usize,
    },
}

/// A single tab in the sid cockpit. Each tab owns a [`Layout`] of widgets.
///
/// # Examples
///
/// ```
/// use sid_core::tab::{Tab, TabId, TabKind};
/// use sid_core::layout::Layout;
/// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
/// # struct W { id: WidgetId }
/// # impl Widget for W {
/// #     fn id(&self) -> &WidgetId { &self.id }
/// #     fn title(&self) -> &str { "t" }
/// #     fn render(&self, _: &mut dyn RenderTarget) {}
/// #     fn handle_event(&mut self, _: &sid_core::event::Event, _: &mut sid_core::context::WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
/// #     fn as_any(&self) -> &dyn std::any::Any { self }
/// #     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
/// # }
/// let t = Tab {
///     id: TabId::new("workspaces"),
///     title: "Workspaces".into(),
///     layout: Layout::Single(Box::new(W { id: WidgetId::new("w") })),
///     hotkey: Some('1'),
///     kind: TabKind::Core,
/// };
/// assert_eq!(t.kind, TabKind::Core);
/// ```
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub layout: Layout,
    pub hotkey: Option<char>,
    /// Pinned-vs-dynamic discriminator. Drives `close_active` and
    /// `push_detail` validation on `TabManager`.
    pub kind: TabKind,
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
/// #     fn as_any(&self) -> &dyn std::any::Any { self }
/// #     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
/// # }
///
/// use sid_core::tab::TabKind;
/// let tabs = vec![
///     Tab { id: TabId::new("a"), title: "A".into(), layout: Layout::Single(Box::new(W { id: WidgetId::new("w") })), hotkey: None, kind: TabKind::Core },
///     Tab { id: TabId::new("b"), title: "B".into(), layout: Layout::Single(Box::new(W { id: WidgetId::new("w2") })), hotkey: None, kind: TabKind::Core },
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

    /// Return a mutable slice over the tabs.
    ///
    /// Used by the binary's wire layer to refresh a non-active tab's widget
    /// state after a store mutation (e.g. workspace added via modal).
    pub fn tabs_mut(&mut self) -> &mut [Tab] {
        &mut self.tabs
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
    /// #     fn as_any(&self) -> &dyn std::any::Any { self }
    /// #     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    /// # }
    /// # fn make_tab(s: &'static str) -> Tab {
    /// #     Tab { id: TabId::new(s), title: s.into(), layout: Layout::Single(Box::new(W { id: WidgetId::new(s) })), hotkey: None, kind: sid_core::tab::TabKind::Core }
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
    /// #     fn as_any(&self) -> &dyn std::any::Any { self }
    /// #     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    /// # }
    /// # fn make_tab(s: &'static str) -> Tab {
    /// #     Tab { id: TabId::new(s), title: s.into(), layout: Layout::Single(Box::new(W { id: WidgetId::new(s) })), hotkey: None, kind: sid_core::tab::TabKind::Core }
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

    /// Push a new detail tab onto the manager. Returns `Err` if `tab.kind`
    /// is [`TabKind::Core`] — only detail tabs can be added at runtime; the
    /// six cores are fixed by [`Self::new`].
    ///
    /// On success the new tab is appended at the end of the tab list. The
    /// active tab is *not* changed; callers that want to focus the new tab
    /// must call [`Self::switch_to`] afterwards.
    ///
    /// # Examples
    ///
    /// ```
    /// # use sid_core::tab::{Tab, TabId, TabKind, TabManager};
    /// # use sid_core::layout::Layout;
    /// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// # struct W { id: WidgetId }
    /// # impl Widget for W {
    /// #     fn id(&self) -> &WidgetId { &self.id }
    /// #     fn title(&self) -> &str { "t" }
    /// #     fn render(&self, _: &mut dyn RenderTarget) {}
    /// #     fn handle_event(&mut self, _: &sid_core::event::Event, _: &mut sid_core::context::WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    /// #     fn as_any(&self) -> &dyn std::any::Any { self }
    /// #     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    /// # }
    /// # fn mk(id: &'static str, k: TabKind) -> Tab {
    /// #     Tab { id: TabId::new(id), title: id.into(),
    /// #           layout: Layout::Single(Box::new(W { id: WidgetId::new(id) })),
    /// #           hotkey: None, kind: k }
    /// # }
    /// let mut mgr = TabManager::new(vec![mk("a", TabKind::Core)]);
    /// mgr.push_detail(mk("d1", TabKind::Detail { parent_idx: 0 })).unwrap();
    /// assert_eq!(mgr.detail_count(), 1);
    /// assert!(mgr.push_detail(mk("bad", TabKind::Core)).is_err());
    /// ```
    pub fn push_detail(&mut self, tab: Tab) -> Result<(), crate::SidError> {
        match tab.kind {
            TabKind::Detail { .. } => {
                self.tabs.push(tab);
                Ok(())
            }
            TabKind::Core => Err(crate::SidError::Other(
                "push_detail rejects TabKind::Core — only Detail tabs can be added at runtime"
                    .into(),
            )),
        }
    }

    /// Close the active tab if it is a [`TabKind::Detail`]. Returns `true`
    /// when a tab was actually removed. Snaps `active_idx` back to the
    /// saved `parent_idx` of the closed tab (clamped to the new tab-list
    /// length).
    ///
    /// No-op (returns `false`) when the active tab is [`TabKind::Core`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use sid_core::tab::{Tab, TabId, TabKind, TabManager};
    /// # use sid_core::layout::Layout;
    /// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// # struct W { id: WidgetId }
    /// # impl Widget for W {
    /// #     fn id(&self) -> &WidgetId { &self.id }
    /// #     fn title(&self) -> &str { "t" }
    /// #     fn render(&self, _: &mut dyn RenderTarget) {}
    /// #     fn handle_event(&mut self, _: &sid_core::event::Event, _: &mut sid_core::context::WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    /// #     fn as_any(&self) -> &dyn std::any::Any { self }
    /// #     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    /// # }
    /// # fn mk(id: &'static str, k: TabKind) -> Tab {
    /// #     Tab { id: TabId::new(id), title: id.into(),
    /// #           layout: Layout::Single(Box::new(W { id: WidgetId::new(id) })),
    /// #           hotkey: None, kind: k }
    /// # }
    /// let mut mgr = TabManager::new(vec![mk("ws", TabKind::Core)]);
    /// mgr.push_detail(mk("d1", TabKind::Detail { parent_idx: 0 })).unwrap();
    /// mgr.switch_to(&TabId::new("d1"));
    /// assert_eq!(mgr.active().id.as_str(), "d1");
    /// assert!(mgr.close_active());
    /// assert_eq!(mgr.active().id.as_str(), "ws");
    /// // Closing again on a Core tab is a no-op.
    /// assert!(!mgr.close_active());
    /// ```
    pub fn close_active(&mut self) -> bool {
        let parent_idx = match &self.tabs[self.active_idx].kind {
            TabKind::Core => return false,
            TabKind::Detail { parent_idx } => *parent_idx,
        };
        self.tabs.remove(self.active_idx);
        // Snap back to the parent core tab. Clamp so it stays in range even
        // if the parent was somehow out of bounds (shouldn't happen — Core
        // tabs are immutable in v1 — but defend against future changes).
        self.active_idx = parent_idx.min(self.tabs.len().saturating_sub(1));
        true
    }

    /// Number of detail (closable) tabs currently in the manager.
    ///
    /// # Examples
    ///
    /// ```
    /// # use sid_core::tab::{Tab, TabId, TabKind, TabManager};
    /// # use sid_core::layout::Layout;
    /// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// # struct W { id: WidgetId }
    /// # impl Widget for W {
    /// #     fn id(&self) -> &WidgetId { &self.id }
    /// #     fn title(&self) -> &str { "t" }
    /// #     fn render(&self, _: &mut dyn RenderTarget) {}
    /// #     fn handle_event(&mut self, _: &sid_core::event::Event, _: &mut sid_core::context::WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    /// #     fn as_any(&self) -> &dyn std::any::Any { self }
    /// #     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    /// # }
    /// # fn mk(id: &'static str, k: TabKind) -> Tab {
    /// #     Tab { id: TabId::new(id), title: id.into(),
    /// #           layout: Layout::Single(Box::new(W { id: WidgetId::new(id) })),
    /// #           hotkey: None, kind: k }
    /// # }
    /// let mgr = TabManager::new(vec![mk("a", TabKind::Core)]);
    /// assert_eq!(mgr.detail_count(), 0);
    /// ```
    pub fn detail_count(&self) -> usize {
        self.tabs
            .iter()
            .filter(|t| matches!(t.kind, TabKind::Detail { .. }))
            .count()
    }
}
