//! Tooltips on things that are not [`crate::Button`]s.
//!
//! [`crate::Button`] and [`crate::IconButton`] take a tooltip in their constructor —
//! type-required, so a glyph-only control cannot ship without a name. Everything else
//! that hides its label (a tab collapsed to its icon on a narrow window) needs the same
//! promise from a plain `div()`, and the library's own `ManagedTooltipExt` is
//! `pub(crate)`.
//!
//! `gpui`'s `InteractiveElement::tooltip` is public and does the job. What it wants is an
//! `AnyView` builder on an element that carries an id (a tooltip needs a hitbox), which
//! means naming `gpui_component::tooltip::Tooltip` — a frontend-crate import that belongs
//! here rather than in a tab module (CLAUDE.md rule 1).

use gpui::{SharedString, StatefulInteractiveElement};
use gpui_component::tooltip::Tooltip;

/// `.tip("what this does")` on any interactive element.
pub trait Tipped: StatefulInteractiveElement + Sized {
    /// Name this element on hover.
    ///
    /// `gpui` debug-asserts on a second `tooltip()` call for the same element, so this
    /// is once per element — the same rule the library's buttons follow.
    fn tip(self, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        self.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
    }

    /// Name this element on hover *and* show its shortcut as a keycap — `"ctrl-t"`,
    /// `"f5"`, in `gpui`'s keystroke syntax. A keyboard-first app teaches its keys where
    /// the mouse already is. An unparseable chord falls back to the plain tip.
    fn tip_with_key(self, text: impl Into<SharedString>, keys: &str) -> Self {
        let text = text.into();
        // Parsed the way `Kbd` parses a spec, so a tooltip's keycap and a cheat-sheet
        // row agree on what `secondary-` means; the first stroke of a chord is shown.
        let stroke = crate::kbd::chips(keys)
            .into_iter()
            .find_map(|chip| match chip {
                crate::kbd::Chip::Stroke(stroke) => Some(stroke),
                _ => None,
            });
        self.tooltip(move |window, cx| {
            Tooltip::new(text.clone())
                .key_binding(stroke.clone().map(gpui_component::kbd::Kbd::new))
                .build(window, cx)
        })
    }
}

impl<E: StatefulInteractiveElement> Tipped for E {}
