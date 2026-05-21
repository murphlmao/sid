use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use proptest::prelude::*;
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

// ── minimal test double ──────────────────────────────────────────────────────

struct Dummy {
    id: WidgetId,
    title: &'static str,
}

impl Widget for Dummy {
    fn id(&self) -> &WidgetId {
        &self.id
    }
    fn title(&self) -> &str {
        self.title
    }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        EventOutcome::Consumed
    }
    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }
    fn load_state(&mut self, _: &[u8]) {}
}

// ── existing test ─────────────────────────────────────────────────────────────

#[test]
fn dummy_widget_reports_metadata() {
    let d = Dummy {
        id: WidgetId::new("dummy"),
        title: "Dummy",
    };
    assert_eq!(d.id().as_str(), "dummy");
    assert_eq!(d.title(), "Dummy");
}

// ── WidgetId basic tests ──────────────────────────────────────────────────────

#[test]
fn widget_id_new_and_as_str_roundtrip() {
    let id = WidgetId::new("git-log");
    assert_eq!(id.as_str(), "git-log");
}

#[test]
fn widget_id_display_equals_inner_string() {
    let id = WidgetId::new("terminal");
    assert_eq!(format!("{id}"), "terminal");
}

#[test]
fn widget_id_equality_is_string_equality() {
    let a = WidgetId::new("a");
    let b = WidgetId::new("a");
    let c = WidgetId::new("b");
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn widget_id_clone_equals_original() {
    let id = WidgetId::new("cloneme");
    let cloned = id.clone();
    assert_eq!(id, cloned);
}

// ── WidgetId Hash consistency ────────────────────────────────────────────────

fn hash_of(id: &WidgetId) -> u64 {
    let mut h = DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

#[test]
fn hash_consistency_same_id_same_hash() {
    let id = WidgetId::new("consistency-check");
    let h1 = hash_of(&id);
    let h2 = hash_of(&id);
    assert_eq!(h1, h2, "hash must be deterministic for the same WidgetId");
}

#[test]
fn hash_consistency_clone_same_hash() {
    let id = WidgetId::new("clone-hash");
    let clone = id.clone();
    assert_eq!(
        hash_of(&id),
        hash_of(&clone),
        "equal WidgetIds must have equal hashes"
    );
}

#[test]
fn hash_different_ids_different_hash_usually() {
    // Not guaranteed by Hash contract, but "a" vs "b" should differ.
    let a = hash_of(&WidgetId::new("aaaaaa"));
    let b = hash_of(&WidgetId::new("bbbbbb"));
    assert_ne!(a, b);
}

// ── WidgetId serde round-trip (proptest) ─────────────────────────────────────

proptest! {
    #[test]
    fn widget_id_serde_json_roundtrip(s in "\\PC{1,256}") {
        let id = WidgetId::new(s.clone());
        let json = serde_json::to_string(&id).unwrap();
        let restored: WidgetId = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(id, restored);
    }
}

// ── EventOutcome tests ───────────────────────────────────────────────────────

#[test]
fn event_outcome_consumed_is_not_bubble() {
    assert_ne!(EventOutcome::Consumed, EventOutcome::Bubble);
}

#[test]
fn event_outcome_eq_is_reflexive() {
    assert_eq!(EventOutcome::Consumed, EventOutcome::Consumed);
    assert_eq!(EventOutcome::Bubble, EventOutcome::Bubble);
}

#[test]
fn event_outcome_copy() {
    let x = EventOutcome::Consumed;
    let y = x; // copy, not move
    assert_eq!(x, y);
}

#[test]
fn event_outcome_debug_is_informative() {
    assert!(format!("{:?}", EventOutcome::Consumed).contains("Consumed"));
    assert!(format!("{:?}", EventOutcome::Bubble).contains("Bubble"));
}

// ── RenderTarget tests ───────────────────────────────────────────────────────

struct FixedArea {
    w: u16,
    h: u16,
}

impl RenderTarget for FixedArea {
    fn width(&self) -> u16 {
        self.w
    }
    fn height(&self) -> u16 {
        self.h
    }
}

#[test]
fn render_target_reports_dimensions() {
    let area = FixedArea { w: 80, h: 24 };
    assert_eq!(area.width(), 80);
    assert_eq!(area.height(), 24);
}

#[test]
fn render_target_zero_dimensions() {
    let area = FixedArea { w: 0, h: 0 };
    assert_eq!(area.width(), 0);
    assert_eq!(area.height(), 0);
}

#[test]
fn render_target_max_dimensions() {
    let area = FixedArea {
        w: u16::MAX,
        h: u16::MAX,
    };
    assert_eq!(area.width(), u16::MAX);
    assert_eq!(area.height(), u16::MAX);
}

// ── Widget default methods ───────────────────────────────────────────────────

#[test]
fn widget_save_state_default_is_empty() {
    let d = Dummy {
        id: WidgetId::new("x"),
        title: "X",
    };
    assert!(d.save_state().is_empty());
}

#[test]
fn widget_load_state_default_is_noop() {
    let mut d = Dummy {
        id: WidgetId::new("x"),
        title: "X",
    };
    // Should not panic with arbitrary bytes
    d.load_state(&[0x00, 0x01, 0xFF]);
}

// ── adversarial WidgetId tests ───────────────────────────────────────────────

#[test]
fn widget_id_empty_string() {
    let id = WidgetId::new("");
    assert_eq!(id.as_str(), "");
    assert_eq!(format!("{id}"), "");
    // Hash and clone should still work
    let _ = hash_of(&id);
    let _ = id.clone();
}

#[test]
fn widget_id_very_long_string() {
    let long = "z".repeat(10_000);
    let id = WidgetId::new(long.clone());
    assert_eq!(id.as_str(), long.as_str());
    assert_eq!(format!("{id}"), long);
    let _ = hash_of(&id);
}

#[test]
fn widget_id_unicode_emoji() {
    let id = WidgetId::new("🦀-widget");
    assert_eq!(id.as_str(), "🦀-widget");
    assert_eq!(format!("{id}"), "🦀-widget");
    let json = serde_json::to_string(&id).unwrap();
    let back: WidgetId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, back);
}

#[test]
fn widget_id_multi_codepoint_grapheme_cluster() {
    // "é" as e + combining acute accent (U+0301) — two codepoints, one grapheme
    let s = "cafe\u{0301}";
    let id = WidgetId::new(s);
    assert_eq!(id.as_str(), s);
    let json = serde_json::to_string(&id).unwrap();
    let back: WidgetId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, back);
}

#[test]
fn widget_id_control_chars_survive() {
    let s = "widget\x00\n\r\t";
    let id = WidgetId::new(s);
    assert_eq!(id.as_str(), s);
    let _ = hash_of(&id);
}
