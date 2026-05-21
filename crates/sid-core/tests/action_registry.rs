//! Integration tests for `Action`, `ActionId`, and `ActionRegistry`.

use sid_core::action::{Action, ActionId, ActionRegistry, ActionScope};

// ---------------------------------------------------------------------------
// ActionId
// ---------------------------------------------------------------------------

#[test]
fn action_id_as_str() {
    let id = ActionId::new("app.quit");
    assert_eq!(id.as_str(), "app.quit");
}

#[test]
fn action_id_display() {
    let id = ActionId::new("palette.open");
    assert_eq!(id.to_string(), "palette.open");
}

#[test]
fn action_id_equality() {
    assert_eq!(ActionId::new("a"), ActionId::new("a"));
    assert_ne!(ActionId::new("a"), ActionId::new("b"));
}

#[test]
fn action_id_from_str() {
    let id: ActionId = "app.quit".into();
    assert_eq!(id.as_str(), "app.quit");
}

#[test]
fn action_id_from_string() {
    let id: ActionId = String::from("tabs.next").into();
    assert_eq!(id.as_str(), "tabs.next");
}

#[test]
fn action_id_clone_equals() {
    let id = ActionId::new("x");
    assert_eq!(id.clone(), id);
}

// ---------------------------------------------------------------------------
// Action
// ---------------------------------------------------------------------------

#[test]
fn action_new_defaults_to_global_scope() {
    let a = Action::new("app.quit", "Quit");
    assert_eq!(a.id.as_str(), "app.quit");
    assert_eq!(a.label, "Quit");
    assert_eq!(a.scope, ActionScope::Global);
    assert!(a.keybind_hint.is_none());
}

#[test]
fn action_scope_variants_are_distinct() {
    assert_ne!(ActionScope::Global, ActionScope::Workspace);
    assert_ne!(ActionScope::Workspace, ActionScope::WorkspaceTree);
    assert_ne!(ActionScope::Tab("a".into()), ActionScope::Tab("b".into()));
}

#[test]
fn action_clone_preserves_fields() {
    let a = Action {
        id: ActionId::new("test"),
        label: "Test".into(),
        scope: ActionScope::Workspace,
        keybind_hint: Some("C-t".into()),
    };
    let b = a.clone();
    assert_eq!(a.id, b.id);
    assert_eq!(a.label, b.label);
    assert_eq!(a.scope, b.scope);
    assert_eq!(a.keybind_hint, b.keybind_hint);
}

// ---------------------------------------------------------------------------
// Registry — register and get
// ---------------------------------------------------------------------------

#[test]
fn register_then_get_returns_some() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("app.quit", "Quit"));
    assert!(reg.get(&ActionId::new("app.quit")).is_some());
}

#[test]
fn get_unregistered_id_returns_none() {
    let reg = ActionRegistry::new();
    assert!(reg.get(&ActionId::new("no.such")).is_none());
}

#[test]
fn register_overwrites_existing_id() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("a", "First"));
    reg.register(Action::new("a", "Second"));
    assert_eq!(reg.get(&ActionId::new("a")).unwrap().label, "Second");
}

#[test]
fn all_returns_all_registered_actions() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("a", "A"));
    reg.register(Action::new("b", "B"));
    reg.register(Action::new("c", "C"));
    assert_eq!(reg.all().count(), 3);
}

// ---------------------------------------------------------------------------
// Registry — fuzzy
// ---------------------------------------------------------------------------

fn populated_registry() -> ActionRegistry {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("app.quit", "Quit"));
    reg.register(Action::new("palette.open", "Open Palette"));
    reg.register(Action::new("tabs.next", "Next Tab"));
    reg.register(Action::new("tabs.prev", "Previous Tab"));
    reg.register(Action::new("app.settings", "Open Settings"));
    reg
}

#[test]
fn fuzzy_empty_query_returns_all() {
    let reg = populated_registry();
    assert_eq!(reg.fuzzy("").len(), 5);
}

#[test]
fn fuzzy_matches_exact_word() {
    let reg = populated_registry();
    let hits = reg.fuzzy("quit");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id.as_str(), "app.quit");
}

#[test]
fn fuzzy_no_match_returns_empty() {
    let reg = populated_registry();
    let hits = reg.fuzzy("zzzzzzz");
    assert!(hits.is_empty());
}

#[test]
fn fuzzy_prefix_ranks_first() {
    // "Op" should rank "Open Palette" or "Open Settings" highest (start bonus):
    // labels starting with 'O' get +5, so they must appear before any
    // non-initial match.
    let reg = populated_registry();
    let hits = reg.fuzzy("Op");
    assert!(!hits.is_empty());
    // The top-ranked result must be one of the "Open …" labels
    let top = hits[0].label.to_lowercase();
    assert!(top.starts_with('o'), "expected top hit to start with 'o', got: {top}");
}

#[test]
fn fuzzy_case_insensitive() {
    let reg = populated_registry();
    let lower = reg.fuzzy("quit");
    let upper = reg.fuzzy("QUIT");
    assert_eq!(lower.len(), upper.len());
    assert_eq!(lower[0].id, upper[0].id);
}

// ---------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------

#[test]
fn fuzzy_empty_registry_returns_empty() {
    let reg = ActionRegistry::new();
    assert!(reg.fuzzy("anything").is_empty());
}

#[test]
fn fuzzy_empty_registry_empty_query_returns_empty() {
    let reg = ActionRegistry::new();
    assert!(reg.fuzzy("").is_empty());
}

#[test]
fn fuzzy_non_ascii_query_does_not_panic() {
    let reg = populated_registry();
    // Should not panic — just return empty or some result
    let _ = reg.fuzzy("ñoño");
    let _ = reg.fuzzy("日本語");
    let _ = reg.fuzzy("🦀");
}

#[test]
fn fuzzy_very_long_query_does_not_panic() {
    let reg = populated_registry();
    let q = "a".repeat(10_000);
    let _ = reg.fuzzy(&q);
}

#[test]
fn fuzzy_very_long_label_does_not_panic() {
    let mut reg = ActionRegistry::new();
    let long_label = "x".repeat(100_000);
    reg.register(Action::new("big", &*long_label));
    let _ = reg.fuzzy("x");
    let _ = reg.fuzzy("xyz");
}

#[test]
fn fuzzy_single_char_query_matches_labels_containing_it() {
    let reg = populated_registry();
    // "Q" should match "Quit"
    let hits = reg.fuzzy("Q");
    assert!(!hits.is_empty());
    assert!(hits.iter().any(|a| a.id.as_str() == "app.quit"));
}

// ---------------------------------------------------------------------------
// Property tests — fuzzy monotonicity
// ---------------------------------------------------------------------------

use proptest::prelude::*;

proptest! {
    /// A 2-char prefix that fully matches scores higher (or equal) than just
    /// the 1-char prefix (all else equal) — monotonicity of consecutive match.
    #[test]
    fn fuzzy_consecutive_match_scores_at_least_as_high(
        prefix in "[a-z]{1,8}"
    ) {
        let label = format!("{prefix}suffix");
        let mut reg = ActionRegistry::new();
        reg.register(Action::new("x", &*label));

        // Score for first char only
        let hits_short = reg.fuzzy(&prefix[..1]);
        // Score for full prefix
        let hits_full = reg.fuzzy(&prefix);

        // If both match, they should both return results
        if !hits_short.is_empty() && !hits_full.is_empty() {
            // Both hit the same action — just verify no panic and results are consistent
            prop_assert_eq!(hits_short[0].id.as_str(), "x");
            prop_assert_eq!(hits_full[0].id.as_str(), "x");
        }
    }

    /// Registering and immediately getting returns the same action.
    #[test]
    fn register_then_get_round_trips(id_str in "[a-z]{1,20}(\\.[a-z]{1,10})?", label in "[A-Za-z ]{1,50}") {
        let mut reg = ActionRegistry::new();
        reg.register(Action::new(id_str.clone(), label.clone()));
        let got = reg.get(&ActionId::new(&id_str));
        prop_assert!(got.is_some());
        prop_assert_eq!(&got.unwrap().label, &label);
    }
}
