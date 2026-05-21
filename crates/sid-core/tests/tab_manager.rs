//! Integration tests for `Tab`, `TabId`, and `TabManager`.

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabManager};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

// ---------------------------------------------------------------------------
// Test widget stub
// ---------------------------------------------------------------------------

struct W {
    id: WidgetId,
    title: &'static str,
}

impl W {
    fn new(s: &'static str) -> Self {
        Self {
            id: WidgetId::new(s),
            title: s,
        }
    }
}

impl Widget for W {
    fn id(&self) -> &WidgetId {
        &self.id
    }
    fn title(&self) -> &str {
        self.title
    }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome {
        EventOutcome::Bubble
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn make_tab(s: &'static str) -> Tab {
    Tab {
        id: TabId::new(s),
        title: s.into(),
        layout: Layout::Single(Box::new(W::new(s))),
        hotkey: None,
    }
}

fn three_tabs() -> TabManager {
    TabManager::new(vec![make_tab("a"), make_tab("b"), make_tab("c")])
}

// ---------------------------------------------------------------------------
// Basic construction and active
// ---------------------------------------------------------------------------

#[test]
fn starts_at_index_zero() {
    let mgr = three_tabs();
    assert_eq!(mgr.active_index(), 0);
    assert_eq!(mgr.active().id.as_str(), "a");
}

#[test]
fn tabs_slice_length_matches_input() {
    let mgr = three_tabs();
    assert_eq!(mgr.tabs().len(), 3);
}

// ---------------------------------------------------------------------------
// next / prev cycling
// ---------------------------------------------------------------------------

#[test]
fn next_advances_index() {
    let mut mgr = three_tabs();
    mgr.next();
    assert_eq!(mgr.active_index(), 1);
    mgr.next();
    assert_eq!(mgr.active_index(), 2);
}

#[test]
fn next_wraps_around() {
    let mut mgr = three_tabs();
    mgr.next();
    mgr.next();
    mgr.next(); // back to 0
    assert_eq!(mgr.active_index(), 0);
}

#[test]
fn prev_moves_backward() {
    let mut mgr = three_tabs();
    mgr.next();
    mgr.next(); // idx 2
    mgr.prev(); // back to 1
    assert_eq!(mgr.active_index(), 1);
}

#[test]
fn prev_wraps_from_zero_to_last() {
    let mut mgr = three_tabs();
    mgr.prev(); // 0 → 2
    assert_eq!(mgr.active_index(), 2);
}

#[test]
fn single_tab_next_is_noop() {
    let mut mgr = TabManager::new(vec![make_tab("only")]);
    mgr.next();
    assert_eq!(mgr.active_index(), 0);
}

#[test]
fn single_tab_prev_is_noop() {
    let mut mgr = TabManager::new(vec![make_tab("only")]);
    mgr.prev();
    assert_eq!(mgr.active_index(), 0);
}

// ---------------------------------------------------------------------------
// jump
// ---------------------------------------------------------------------------

#[test]
fn jump_sets_index() {
    let mut mgr = three_tabs();
    mgr.jump(2);
    assert_eq!(mgr.active_index(), 2);
}

#[test]
fn jump_out_of_range_clamps_to_last() {
    let mut mgr = three_tabs();
    mgr.jump(99);
    assert_eq!(mgr.active_index(), 2);
}

#[test]
fn jump_to_zero_stays_at_zero() {
    let mut mgr = three_tabs();
    mgr.next(); // move away first
    mgr.jump(0);
    assert_eq!(mgr.active_index(), 0);
}

// ---------------------------------------------------------------------------
// switch_to
// ---------------------------------------------------------------------------

#[test]
fn switch_to_known_id_returns_true() {
    let mut mgr = three_tabs();
    assert!(mgr.switch_to(&TabId::new("c")));
    assert_eq!(mgr.active_index(), 2);
}

#[test]
fn switch_to_unknown_id_returns_false() {
    let mut mgr = three_tabs();
    assert!(!mgr.switch_to(&TabId::new("nope")));
    // Active index unchanged
    assert_eq!(mgr.active_index(), 0);
}

#[test]
fn switch_to_current_tab_is_ok() {
    let mut mgr = three_tabs();
    assert!(mgr.switch_to(&TabId::new("a")));
    assert_eq!(mgr.active_index(), 0);
}

// ---------------------------------------------------------------------------
// active_mut
// ---------------------------------------------------------------------------

#[test]
fn active_mut_allows_title_mutation() {
    let mut mgr = three_tabs();
    mgr.active_mut().title = "changed".into();
    assert_eq!(mgr.active().title, "changed");
}

// ---------------------------------------------------------------------------
// TabId properties
// ---------------------------------------------------------------------------

#[test]
fn tab_id_display_equals_as_str() {
    let id = TabId::new("my-tab");
    assert_eq!(id.to_string(), id.as_str());
}

#[test]
fn tab_id_equality() {
    assert_eq!(TabId::new("x"), TabId::new("x"));
    assert_ne!(TabId::new("x"), TabId::new("y"));
}

#[test]
fn tab_id_clone_is_equal() {
    let id = TabId::new("clone-me");
    assert_eq!(id.clone(), id);
}

// ---------------------------------------------------------------------------
// Empty-TabManager panic
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "TabManager requires at least one tab")]
fn new_with_empty_vec_panics() {
    let _: TabManager = TabManager::new(vec![]);
}

// ---------------------------------------------------------------------------
// Property tests — cycling invariants
// ---------------------------------------------------------------------------

use proptest::prelude::*;

proptest! {
    /// For any tab count > 1, calling `next` then `prev` returns to the original index.
    #[test]
    fn next_then_prev_is_identity(n in 2usize..20usize, start in 0usize..20usize) {
        let tabs: Vec<Tab> = (0..n)
            .map(|i| {
                let s = Box::leak(format!("t{i}").into_boxed_str()) as &'static str;
                make_tab(s)
            })
            .collect();
        let mut mgr = TabManager::new(tabs);
        let start_idx = start % n;
        mgr.jump(start_idx);
        let before = mgr.active_index();
        mgr.next();
        mgr.prev();
        prop_assert_eq!(mgr.active_index(), before);
    }

    /// For any tab count > 1, calling `prev` then `next` returns to the original index.
    #[test]
    fn prev_then_next_is_identity(n in 2usize..20usize, start in 0usize..20usize) {
        let tabs: Vec<Tab> = (0..n)
            .map(|i| {
                let s = Box::leak(format!("p{i}").into_boxed_str()) as &'static str;
                make_tab(s)
            })
            .collect();
        let mut mgr = TabManager::new(tabs);
        let start_idx = start % n;
        mgr.jump(start_idx);
        let before = mgr.active_index();
        mgr.prev();
        mgr.next();
        prop_assert_eq!(mgr.active_index(), before);
    }

    /// jump always produces an index within bounds.
    #[test]
    fn jump_always_in_bounds(n in 1usize..20usize, idx in 0usize..usize::MAX) {
        let tabs: Vec<Tab> = (0..n)
            .map(|i| {
                let s = Box::leak(format!("j{i}").into_boxed_str()) as &'static str;
                make_tab(s)
            })
            .collect();
        let mut mgr = TabManager::new(tabs);
        // avoid usize overflow: cap at a large but safe value
        let safe_idx = idx.min(1_000_000);
        mgr.jump(safe_idx);
        prop_assert!(mgr.active_index() < n);
    }
}
