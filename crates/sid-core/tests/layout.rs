use proptest::prelude::*;
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::layout::{Dir, Layout};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

// ── minimal widget stub ──────────────────────────────────────────────────────

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
}

// ── existing tests ────────────────────────────────────────────────────────────

#[test]
fn single_layout_holds_one_widget() {
    let layout: Layout = Layout::Single(Box::new(W::new("only")));
    let titles: Vec<String> = layout
        .iter_widgets()
        .map(|w| w.title().to_string())
        .collect();
    assert_eq!(titles, vec!["only".to_string()]);
}

#[test]
fn split_layout_iterates_in_order() {
    let layout = Layout::Split {
        dir: Dir::Horizontal,
        ratio: 0.5,
        a: Box::new(Layout::Single(Box::new(W::new("a")))),
        b: Box::new(Layout::Single(Box::new(W::new("b")))),
    };
    let titles: Vec<String> = layout
        .iter_widgets()
        .map(|w| w.title().to_string())
        .collect();
    assert_eq!(titles, vec!["a".to_string(), "b".to_string()]);
}

// ── Dir tests ─────────────────────────────────────────────────────────────────

#[test]
fn dir_horizontal_and_vertical_are_distinct() {
    assert_ne!(Dir::Horizontal, Dir::Vertical);
}

#[test]
fn dir_copy_and_eq() {
    let h = Dir::Horizontal;
    let h2 = h;
    assert_eq!(h, h2);
}

#[test]
fn dir_debug_is_informative() {
    assert!(format!("{:?}", Dir::Horizontal).contains("Horizontal"));
    assert!(format!("{:?}", Dir::Vertical).contains("Vertical"));
}

// ── iter_widgets order: more complex trees ───────────────────────────────────

#[test]
fn three_wide_split_left_to_right() {
    // a | (b | c) — expect a, b, c
    let layout = Layout::Split {
        dir: Dir::Horizontal,
        ratio: 0.33,
        a: Box::new(Layout::Single(Box::new(W::new("a")))),
        b: Box::new(Layout::Split {
            dir: Dir::Horizontal,
            ratio: 0.5,
            a: Box::new(Layout::Single(Box::new(W::new("b")))),
            b: Box::new(Layout::Single(Box::new(W::new("c")))),
        }),
    };
    let ids: Vec<&str> = layout.iter_widgets().map(|w| w.id().as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[test]
fn asymmetric_split_right_branch_deeper() {
    // a | (b | (c | d)) — expect a, b, c, d
    let layout = Layout::Split {
        dir: Dir::Vertical,
        ratio: 0.25,
        a: Box::new(Layout::Single(Box::new(W::new("a")))),
        b: Box::new(Layout::Split {
            dir: Dir::Vertical,
            ratio: 0.33,
            a: Box::new(Layout::Single(Box::new(W::new("b")))),
            b: Box::new(Layout::Split {
                dir: Dir::Vertical,
                ratio: 0.5,
                a: Box::new(Layout::Single(Box::new(W::new("c")))),
                b: Box::new(Layout::Single(Box::new(W::new("d")))),
            }),
        }),
    };
    let ids: Vec<&str> = layout.iter_widgets().map(|w| w.id().as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c", "d"]);
}

// ── iter_widgets_mut count matches iter_widgets ───────────────────────────────

#[test]
fn iter_widgets_mut_count_matches_immutable() {
    let mut layout = Layout::Split {
        dir: Dir::Horizontal,
        ratio: 0.5,
        a: Box::new(Layout::Single(Box::new(W::new("x")))),
        b: Box::new(Layout::Split {
            dir: Dir::Vertical,
            ratio: 0.5,
            a: Box::new(Layout::Single(Box::new(W::new("y")))),
            b: Box::new(Layout::Single(Box::new(W::new("z")))),
        }),
    };
    let immut_count = layout.iter_widgets().count();
    let mut_count = layout.iter_widgets_mut().count();
    assert_eq!(immut_count, mut_count);
}

// ── adversarial: deeply nested Split (10 levels) ────────────────────────────

/// Build a balanced binary split tree of the given depth.
/// At depth 0 returns a Single. Returns `(layout, expected_leaf_count)`.
fn build_balanced(depth: u32) -> (Layout, usize) {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    fn go(depth: u32) -> (Layout, usize) {
        if depth == 0 {
            let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let s: &'static str = Box::leak(format!("leaf-{id}").into_boxed_str());
            (Layout::Single(Box::new(W::new(s))), 1)
        } else {
            let (la, ca) = go(depth - 1);
            let (lb, cb) = go(depth - 1);
            (
                Layout::Split {
                    dir: Dir::Horizontal,
                    ratio: 0.5,
                    a: Box::new(la),
                    b: Box::new(lb),
                },
                ca + cb,
            )
        }
    }
    go(depth)
}

#[test]
fn deeply_nested_split_10_levels_correct_leaf_count() {
    // 10 levels of binary splits → 2^10 = 1024 leaves
    let (layout, expected) = build_balanced(10);
    let count = layout.iter_widgets().count();
    assert_eq!(count, expected);
    assert_eq!(count, 1024);
}

#[test]
fn deeply_nested_split_10_levels_mut_correct_leaf_count() {
    let (mut layout, expected) = build_balanced(10);
    let count = layout.iter_widgets_mut().count();
    assert_eq!(count, expected);
    assert_eq!(count, 1024);
}

#[test]
fn layout_with_100_widgets_linear_chain() {
    static LCOUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(10000);

    fn next_leaf() -> Layout {
        let id = LCOUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let s: &'static str = Box::leak(format!("lc-{id}").into_boxed_str());
        Layout::Single(Box::new(W::new(s)))
    }

    let mut layout = next_leaf();
    for _ in 1..100 {
        layout = Layout::Split {
            dir: Dir::Vertical,
            ratio: 0.5,
            a: Box::new(next_leaf()),
            b: Box::new(layout),
        };
    }
    assert_eq!(layout.iter_widgets().count(), 100);
}

#[test]
fn single_degenerate_layout_count_is_one() {
    let layout = Layout::Single(Box::new(W::new("only")));
    assert_eq!(layout.iter_widgets().count(), 1);
    assert_eq!(layout.iter_widgets().next().unwrap().id().as_str(), "only");
}

// ── iter_count_matches_leaf_count for balanced trees 0-8 ────────────────────

#[test]
fn iter_count_matches_leaf_count_balanced_trees() {
    for depth in 0u32..=8 {
        let (layout, expected) = build_balanced(depth);
        let got = layout.iter_widgets().count();
        assert_eq!(
            got, expected,
            "depth {depth}: expected {expected} leaves, got {got}"
        );
    }
}

// ── proptest: linear chain count invariant ───────────────────────────────────

proptest! {
    #[test]
    fn prop_linear_chain_count(n in 1usize..=50usize) {
        static PROP_COUNTER: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(20000);

        fn next_leaf() -> Layout {
            let id = PROP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let s: &'static str = Box::leak(format!("p-{id}").into_boxed_str());
            Layout::Single(Box::new(W::new(s)))
        }

        let mut layout = next_leaf();
        for _ in 1..n {
            layout = Layout::Split {
                dir: Dir::Vertical,
                ratio: 0.5,
                a: Box::new(next_leaf()),
                b: Box::new(layout),
            };
        }
        let got = layout.iter_widgets().count();
        prop_assert_eq!(got, n, "linear chain should have {} leaves", n);
    }
}

proptest! {
    #[test]
    fn prop_mut_count_equals_immut_count(n in 1usize..=50usize) {
        static PROP_COUNTER2: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(30000);

        fn next_leaf() -> Layout {
            let id = PROP_COUNTER2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let s: &'static str = Box::leak(format!("pm-{id}").into_boxed_str());
            Layout::Single(Box::new(W::new(s)))
        }

        let mut layout = next_leaf();
        for _ in 1..n {
            layout = Layout::Split {
                dir: Dir::Horizontal,
                ratio: 0.5,
                a: Box::new(next_leaf()),
                b: Box::new(layout),
            };
        }
        let immut_count = layout.iter_widgets().count();
        let mut_count = layout.iter_widgets_mut().count();
        prop_assert_eq!(immut_count, mut_count);
    }
}

// ── mixed Horizontal / Vertical directions preserved correctly ───────────────

#[test]
fn split_dir_is_preserved_in_iteration_order() {
    // Vertical split — a is top, b is bottom; iteration: a, b
    let v_split = Layout::Split {
        dir: Dir::Vertical,
        ratio: 0.5,
        a: Box::new(Layout::Single(Box::new(W::new("top")))),
        b: Box::new(Layout::Single(Box::new(W::new("bot")))),
    };
    let ids: Vec<&str> = v_split.iter_widgets().map(|w| w.id().as_str()).collect();
    assert_eq!(ids, vec!["top", "bot"]);
}
