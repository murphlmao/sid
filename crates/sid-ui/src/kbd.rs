//! Keybinding chips.
//!
//! The Settings screen renders 16 keybinding values in accent red. `.interface-design/
//! system.md` says accent means "engage"; a static reference table is the least
//! engageable content in the app, so the eye is dragged to it and, after 16 of them,
//! learns to ignore red everywhere. Keybindings are *orientation*: a muted chip.
//!
//! The chip itself is `gpui_component::kbd::Kbd`, which already draws the right box and
//! — more usefully — carries the platform key-name table (`Esc`, `Page Down`, `⌘` on
//! macOS). What it needs is a `gpui::Keystroke`, and sid's keymap hands out display
//! strings like `Ctrl+K`. [`chips`] is that seam: normalise, parse, and fall back to
//! rendering the text verbatim rather than dropping a binding the parser did not know.

use gpui::{
    App, IntoElement, Keystroke, ParentElement, RenderOnce, SharedString, Styled, Window, div, rgb,
};
use gpui_component::kbd::Kbd as ComponentKbd;

use crate::styled::h_flex;
use crate::theme;

/// One chip's worth of a keybinding spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chip {
    /// A stroke the parser understood — rendered through the library's platform-aware
    /// formatter.
    Stroke(Keystroke),
    /// Text the parser did not understand (`double-click`, `—`, a chord sid invented),
    /// rendered verbatim. A binding sid cannot parse is still a binding the user has to
    /// read.
    Literal(SharedString),
}

/// Rewrite a display-style spec into the syntax [`Keystroke::parse`] accepts.
///
/// sid's keymap prints `Ctrl+Shift+T`; gpui parses `ctrl-shift-t`. The `+` separator is
/// only translated *between* parts, so a trailing `+` — the plus key itself, as in
/// `Ctrl++` — survives as the key rather than becoming a stray `-`.
fn normalize(stroke: &str) -> String {
    let lower = stroke.to_lowercase();
    if !lower.contains('+') {
        return lower;
    }
    let mut parts: Vec<&str> = lower.split('+').collect();
    // A trailing empty part means the spec ended in `+`: that final `+` is the key.
    if parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
        if let Some(last) = parts.last_mut() {
            if last.is_empty() {
                *last = "+";
            } else {
                parts.push("+");
            }
        }
    }
    parts.join("-")
}

/// Split a spec into chips: whitespace separates the strokes of a chord sequence
/// (`ctrl-k ctrl-s` is two chips), and each stroke is parsed independently.
pub fn chips(spec: &str) -> Vec<Chip> {
    spec.split_whitespace()
        .map(|stroke| match Keystroke::parse(&normalize(stroke)) {
            Ok(parsed) => Chip::Stroke(parsed),
            Err(_) => Chip::Literal(stroke.to_string().into()),
        })
        .collect()
}

/// A keybinding, rendered as one muted chip per stroke.
///
/// ```ignore
/// Kbd::new("Ctrl+K")        // one chip
/// Kbd::new("ctrl-k ctrl-s") // two chips, in sequence
/// ```
#[derive(IntoElement)]
pub struct Kbd {
    spec: SharedString,
}

impl Kbd {
    /// A keybinding chip row. `spec` may be written in either gpui syntax
    /// (`ctrl-shift-t`) or display syntax (`Ctrl+Shift+T`).
    pub fn new(spec: impl Into<SharedString>) -> Self {
        Self { spec: spec.into() }
    }
}

impl RenderOnce for Kbd {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        h_flex()
            .gap_1()
            .children(chips(&self.spec).into_iter().map(|chip| {
                match chip {
                    Chip::Stroke(stroke) => ComponentKbd::new(stroke).into_any_element(),
                    // The same box the library draws, minus the parsing — `bg` fill and
                    // all, so a parsed and an unparsed chip sitting side by side are
                    // indistinguishable.
                    Chip::Literal(text) => div()
                        .flex_none()
                        .px_1()
                        .py_0p5()
                        .min_w_5()
                        .text_center()
                        .text_xs()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.bg))
                        .text_color(rgb(theme.muted))
                        .child(text)
                        .into_any_element(),
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strokes(spec: &str) -> Vec<String> {
        chips(spec)
            .into_iter()
            .map(|chip| match chip {
                Chip::Stroke(s) => ComponentKbd::format(&s),
                Chip::Literal(text) => text.to_string(),
            })
            .collect()
    }

    #[test]
    fn display_syntax_round_trips() {
        // What sid's keymap actually prints today. `Ctrl+K` must come back out as
        // `Ctrl+K`, not as an unparsed literal that skips the platform key table.
        assert_eq!(chips("Ctrl+K").len(), 1);
        assert!(matches!(chips("Ctrl+K")[0], Chip::Stroke(_)));
        if cfg!(not(target_os = "macos")) {
            assert_eq!(strokes("Ctrl+K"), vec!["Ctrl+K"]);
            assert_eq!(strokes("Ctrl+Shift+T"), vec!["Ctrl+Shift+T"]);
            assert_eq!(strokes("Escape"), vec!["Esc"]);
        }
    }

    #[test]
    fn gpui_syntax_parses_unchanged() {
        assert!(matches!(chips("ctrl-k")[0], Chip::Stroke(_)));
        assert!(matches!(chips("shift-f5")[0], Chip::Stroke(_)));
    }

    #[test]
    fn a_chord_sequence_becomes_one_chip_per_stroke() {
        let parsed = chips("ctrl-k ctrl-s");
        assert_eq!(parsed.len(), 2);
        assert!(parsed.iter().all(|c| matches!(c, Chip::Stroke(_))));
    }

    #[test]
    fn an_unparseable_spec_is_shown_verbatim_not_dropped() {
        // A binding sid cannot parse is still a binding the user has to read — and the
        // literal keeps its original casing, because that is what the caller wrote.
        let parsed = chips("double-click");
        assert_eq!(parsed, vec![Chip::Literal("double-click".into())]);
        assert_eq!(strokes("Right-Click"), vec!["Right-Click"]);
    }

    #[test]
    fn an_empty_spec_renders_nothing() {
        assert!(chips("").is_empty());
        assert!(chips("   ").is_empty());
    }

    #[test]
    fn normalize_translates_the_display_separator() {
        assert_eq!(normalize("Ctrl+K"), "ctrl-k");
        assert_eq!(normalize("Ctrl+Shift+T"), "ctrl-shift-t");
        assert_eq!(normalize("ctrl-k"), "ctrl-k");
        assert_eq!(normalize("F5"), "f5");
    }

    #[test]
    fn a_trailing_plus_is_the_plus_key_not_a_separator() {
        // `Ctrl++` means ctrl and the `+` key. Naive `+`->`-` replacement turns it
        // into `ctrl--`, i.e. ctrl-minus: the wrong binding, silently.
        assert_eq!(normalize("Ctrl++"), "ctrl-+");
        assert!(matches!(chips("Ctrl++")[0], Chip::Stroke(_)));
    }
}
