//! List/pane focus state + drill-in stack for split-layout widgets.
//!
//! `V` is the widget's own view enum (e.g. workspace-detail's
//! `OpsMenu | Commits | Diff`). The widget pushes views as the user drills in;
//! `←` pops one level and finally returns focus to the list. This is the
//! single source of truth for "context-aware Tab": when `focus()` is `List`,
//! the widget must return `EventOutcome::Bubble` for Tab so the global
//! tab-strip cycling sees it; when `Pane`, the widget consumes Tab itself.

/// Which side owns key events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitFocus {
    /// Left list: ↑↓ select, → or Enter dives in, Tab bubbles to tab strip.
    List,
    /// Right pane: keys go to the top view of the stack, ← pops.
    Pane,
}

/// Focus + drill-in stack.
#[derive(Debug, Clone)]
pub struct SplitView<V> {
    stack: Vec<V>,
    focus: SplitFocus,
}

impl<V> Default for SplitView<V> {
    fn default() -> Self {
        Self {
            stack: Vec::new(),
            focus: SplitFocus::List,
        }
    }
}

impl<V> SplitView<V> {
    /// Current focus side.
    pub fn focus(&self) -> SplitFocus {
        self.focus
    }

    /// Top of the drill-in stack, if any.
    pub fn top(&self) -> Option<&V> {
        self.stack.last()
    }

    /// Depth of the drill-in stack (for breadcrumb rendering).
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Enter the pane, pushing `view` onto the stack.
    pub fn push(&mut self, view: V) {
        self.stack.push(view);
        self.focus = SplitFocus::Pane;
    }

    /// Replace the whole stack with a single root view and focus the pane.
    /// (Used when list selection changes: the ops menu re-roots.)
    pub fn reroot(&mut self, view: V) {
        self.stack.clear();
        self.stack.push(view);
        self.focus = SplitFocus::Pane;
    }

    /// Pop one level; when the stack empties, focus returns to the list.
    /// Returns `true` if a pop happened (the caller consumed the key).
    pub fn pop(&mut self) -> bool {
        if self.stack.pop().is_some() {
            if self.stack.is_empty() {
                self.focus = SplitFocus::List;
            }
            true
        } else if self.focus == SplitFocus::Pane {
            self.focus = SplitFocus::List;
            true
        } else {
            false
        }
    }

    /// Drop everything and focus the list (e.g. list contents reloaded).
    pub fn reset(&mut self) {
        self.stack.clear();
        self.focus = SplitFocus::List;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum V {
        Ops,
        Commits,
        Diff,
    }

    #[test]
    fn starts_on_list_with_empty_stack() {
        let s: SplitView<V> = SplitView::default();
        assert_eq!(s.focus(), SplitFocus::List);
        assert_eq!(s.top(), None);
    }

    #[test]
    fn push_enters_pane_and_pop_unwinds_to_list() {
        let mut s = SplitView::default();
        s.push(V::Ops);
        s.push(V::Commits);
        s.push(V::Diff);
        assert_eq!(s.depth(), 3);
        assert!(s.pop()); // Diff -> Commits
        assert_eq!(s.top(), Some(&V::Commits));
        assert!(s.pop());
        assert!(s.pop()); // stack empty -> back to list
        assert_eq!(s.focus(), SplitFocus::List);
        assert!(!s.pop()); // on list: ← is not ours, let the widget bubble it
    }

    #[test]
    fn reroot_replaces_stack() {
        let mut s = SplitView::default();
        s.push(V::Ops);
        s.push(V::Diff);
        s.reroot(V::Ops);
        assert_eq!(s.depth(), 1);
        assert_eq!(s.focus(), SplitFocus::Pane);
    }

    #[test]
    fn pop_with_pane_focus_but_empty_stack_returns_to_list() {
        let mut s: SplitView<V> = SplitView::default();
        s.push(V::Ops);
        s.pop();
        // second pop on list focus is a no-op
        assert!(!s.pop());
        assert_eq!(s.focus(), SplitFocus::List);
    }
}
