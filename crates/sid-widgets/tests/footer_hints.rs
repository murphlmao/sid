//! Integration tests for `Widget::footer_hint` on the six production widgets.
//!
//! Each test exercises the concrete widget's override of `footer_hint`,
//! asserting the expected chord/label pairs are present. These tests also
//! double as an adversarial check that the trait method's default impl was
//! genuinely overridden — a regression that fell back to the empty default
//! would make every assertion below fail.

use sid_core::widget::{FooterHint, Widget};
use sid_widgets::{
    DatabaseWidget, NetworkWidget, SettingsWidget, SshWidget, SystemWidget, WorkspacesWidget,
};

/// Helper: assert that a `FooterHint` with the given `(chord, label)` pair is
/// present in `hints`. Reports the actual hint list on failure to keep
/// diagnosis cheap.
fn assert_hint(hints: &[FooterHint], chord: &str, label: &str) {
    let found = hints.iter().any(|h| h.chord == chord && h.label == label);
    assert!(
        found,
        "expected hint [ {chord}: {label} ] not found in {:?}",
        hints
            .iter()
            .map(|h| format!("[ {}: {} ]", h.chord, h.label))
            .collect::<Vec<_>>()
    );
}

// ── WorkspacesWidget ─────────────────────────────────────────────────────────

#[test]
fn workspaces_footer_hints() {
    let w = WorkspacesWidget::new(vec![], None);
    let hints = w.footer_hint();
    assert_hint(&hints, "N", "new workspace");
    assert_hint(&hints, "A", "add repo");
    assert_hint(&hints, "R", "remove");
    assert_hint(&hints, "Enter", "promote");
    assert_hint(&hints, "?", "help");
    assert_eq!(
        hints.len(),
        5,
        "WorkspacesWidget should expose exactly 5 footer hints"
    );
}

// ── SshWidget ────────────────────────────────────────────────────────────────

#[test]
fn ssh_footer_hints() {
    let w = SshWidget::new();
    let hints = w.footer_hint();
    assert_hint(&hints, "N", "new host");
    assert_hint(&hints, "G", "gen key");
    assert_hint(&hints, "S", "setup remote");
    assert_hint(&hints, "K", "keys");
    assert_hint(&hints, "X", "debug");
    assert_hint(&hints, "?", "help");
    assert_eq!(
        hints.len(),
        6,
        "SshWidget should expose exactly 6 footer hints"
    );
}

// ── DatabaseWidget ───────────────────────────────────────────────────────────

#[test]
fn database_footer_hints() {
    let w = DatabaseWidget::new(vec![]);
    let hints = w.footer_hint();
    assert_hint(&hints, "N", "new");
    assert_hint(&hints, "E", "edit");
    assert_hint(&hints, "D", "delete");
    assert_hint(&hints, "T", "test");
    assert_hint(&hints, "Tab", "pane");
    assert_hint(&hints, "Ctrl+R", "run");
    assert_eq!(
        hints.len(),
        6,
        "DatabaseWidget should expose exactly 6 footer hints"
    );
}

// ── NetworkWidget ────────────────────────────────────────────────────────────

#[test]
fn network_footer_hints() {
    let w = NetworkWidget::new();
    let hints = w.footer_hint();
    assert_hint(&hints, "/", "filter");
    assert_hint(&hints, "s", "sort");
    assert_hint(&hints, "K", "kill");
    assert_hint(&hints, "⏎", "detail");
    assert_hint(&hints, "Tab", "pane");
    assert_hint(&hints, "R", "refresh");
    assert_eq!(
        hints.len(),
        6,
        "NetworkWidget should expose exactly 6 footer hints (filter / sort \
         / kill / detail / pane / refresh)"
    );
}

// ── SystemWidget ─────────────────────────────────────────────────────────────

#[test]
fn system_footer_hints() {
    let w = SystemWidget::new();
    let hints = w.footer_hint();
    assert_hint(&hints, "N", "new");
    assert_hint(&hints, "E", "edit");
    assert_hint(&hints, "D", "remove");
    assert_hint(&hints, "Enter", "open");
    assert_hint(&hints, "Tab", "pane");
    assert_eq!(
        hints.len(),
        5,
        "SystemWidget should expose exactly 5 footer hints"
    );
}

// ── SettingsWidget ───────────────────────────────────────────────────────────

#[test]
fn settings_footer_hints() {
    let w = SettingsWidget::new();
    let hints = w.footer_hint();
    assert_hint(&hints, "Tab", "next category");
    assert_hint(&hints, "Enter", "apply");
    assert_hint(&hints, "N", "import");
    assert_hint(&hints, "?", "help");
    assert_eq!(
        hints.len(),
        4,
        "SettingsWidget should expose exactly 4 footer hints"
    );
}
