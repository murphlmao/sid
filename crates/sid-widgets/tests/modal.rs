//! Insta snapshot tests for [`render_modal`].
//!
//! Each test builds a deterministic [`ModalSpec`] and renders it into a
//! fixed 80×24 `TestBackend` via [`render_modal_to_string`]. Layout drift
//! shows up as a snapshot diff. The plain-text snapshot deliberately
//! drops style information so the tests stay tolerant of theme tweaks
//! while still locking in geometry and field-glyph choices (bullets,
//! radio glyphs, focus prefix, button row).

use sid_widgets::modal::{Field, ModalSpec, render_modal_to_string};

const W: u16 = 80;
const H: u16 = 24;

/// Helper: a Text field with no placeholder.
fn text(label: &str, value: &str) -> Field {
    Field::Text {
        label: label.into(),
        value: value.into(),
        placeholder: None,
    }
}

#[test]
fn snapshot_empty_modal_title_only() {
    let m = ModalSpec::new("demo.empty", "Empty Modal", vec![]);
    let s = render_modal_to_string(&m, W, H);
    insta::assert_snapshot!("modal_empty_title_only", s);
}

#[test]
fn snapshot_three_text_fields_first_focused() {
    let m = ModalSpec::new(
        "demo.three.first",
        "Three Text Fields",
        vec![
            text("alias", "prod"),
            text("host", "prod.example.com"),
            text("user", "root"),
        ],
    );
    assert_eq!(m.focus, 0, "default focus must be on first field");
    let s = render_modal_to_string(&m, W, H);
    insta::assert_snapshot!("modal_three_text_first_focused", s);
}

#[test]
fn snapshot_three_text_fields_second_focused() {
    let mut m = ModalSpec::new(
        "demo.three.second",
        "Three Text Fields",
        vec![
            text("alias", "prod"),
            text("host", "prod.example.com"),
            text("user", "root"),
        ],
    );
    m.cycle_focus_forward();
    assert_eq!(m.focus, 1);
    let s = render_modal_to_string(&m, W, H);
    insta::assert_snapshot!("modal_three_text_second_focused", s);
}

#[test]
fn snapshot_password_renders_bullets_not_value() {
    let m = ModalSpec::new(
        "demo.password",
        "Set Password",
        vec![Field::Password {
            label: "passphrase".into(),
            value: "secret123".into(),
        }],
    );
    let s = render_modal_to_string(&m, W, H);
    // Adversarial assertion: the raw password must NEVER appear in the
    // rendered buffer. Bullets are nine wide for "secret123".
    assert!(
        !s.contains("secret123"),
        "raw password leaked into rendered modal:\n{s}"
    );
    let bullets: String = std::iter::repeat_n('\u{2022}', "secret123".chars().count()).collect();
    assert!(
        s.contains(&bullets),
        "expected nine bullets ({bullets:?}) in rendered modal:\n{s}"
    );
    insta::assert_snapshot!("modal_password_bullets", s);
}

#[test]
fn snapshot_choice_three_options_second_selected() {
    let m = ModalSpec::new(
        "demo.choice",
        "Pick Algorithm",
        vec![Field::Choice {
            label: "algorithm".into(),
            options: vec!["Ed25519".into(), "RSA-4096".into(), "ECDSA-256".into()],
            selected: 1,
        }],
    );
    let s = render_modal_to_string(&m, W, H);
    // Spot-check: the selected glyph (●) should appear in the output and
    // the unselected glyph ( ) should appear at least once.
    assert!(s.contains('\u{25cf}'), "selected radio glyph missing\n{s}");
    insta::assert_snapshot!("modal_choice_second_selected", s);
}

#[test]
fn snapshot_display_field_multiline_body() {
    // Build a help drawer-style modal with a single `Field::Display`. The
    // snapshot locks in the per-line layout: the renderer must paint each
    // `\n`-separated body line on its own row, never as a `\n` literal.
    let m = ModalSpec::new(
        "help.demo",
        "Help — Demo",
        vec![Field::Display {
            label: "keys".into(),
            body: [
                "Demo:",
                "  N: new",
                "  D: delete",
                "  R: rename",
                "",
                "Global:",
                "  Ctrl+Q: quit",
                "  Ctrl+F: palette",
            ]
            .join("\n"),
        }],
    )
    .with_help("Esc closes.");
    let s = render_modal_to_string(&m, W, H);
    // Adversarial check: literal `\n` must never leak.
    assert!(
        !s.contains("\\n"),
        "literal `\\n` leaked into rendered modal:\n{s}"
    );
    // Each body row should be visible.
    for row in ["N: new", "D: delete", "R: rename", "Ctrl+Q: quit"] {
        assert!(s.contains(row), "expected {row} in rendered modal:\n{s}");
    }
    insta::assert_snapshot!("modal_display_multiline_body", s);
}

#[test]
fn snapshot_all_five_field_types_mixed_focus() {
    // Build a modal that exercises every field variant in one render.
    // Focus lands on the third row (the Toggle) so the focus prefix is
    // visible on a non-text row.
    let mut m = ModalSpec::new(
        "demo.all_types",
        "All Field Types",
        vec![
            Field::Text {
                label: "alias".into(),
                value: "prod".into(),
                placeholder: None,
            },
            Field::Password {
                label: "passphrase".into(),
                value: "hunter2".into(),
            },
            Field::Toggle {
                label: "use_agent".into(),
                value: true,
            },
            Field::Choice {
                label: "algorithm".into(),
                options: vec!["Ed25519".into(), "RSA-4096".into()],
                selected: 0,
            },
            Field::Picker {
                label: "identity".into(),
                value: "~/.ssh/id_ed25519".into(),
                hint: "browse ~/.ssh".into(),
            },
        ],
    )
    .with_help("Tab moves between fields. Enter to save.");
    m.cycle_focus_forward();
    m.cycle_focus_forward();
    assert_eq!(m.focus, 2, "focus expected on the Toggle");
    let s = render_modal_to_string(&m, W, H);
    // Password value must never appear.
    assert!(!s.contains("hunter2"), "password leaked:\n{s}");
    insta::assert_snapshot!("modal_all_field_types", s);
}
