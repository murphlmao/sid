//! The radio mark — a ring that gains a core when chosen.
//!
//! Both save-to pickers (`host_form.rs:551`, `db_conn_form.rs:650`) hand-rolled the same
//! seventeen lines, and both were right about the important part: the mark is **drawn,
//! not glyphed**. The `●`/`○` pair it replaces renders in whatever the ambient font
//! happens to have, at whatever weight, and in most families the two are visibly
//! different sizes — so the picker jittered as the selection moved between rows.
//!
//! # What this is not
//!
//! It is the *mark*, not the row. The label, the click target and the selected fill
//! belong to the [`crate::Row`] this leads — `Row::new(id).selected(..).leading(Radio::
//! new(..))` — because a radio option in sid is a whole row with a title and a note
//! under it, and a control that owned only its own 12px box would leave the other 400px
//! of the row unclickable.

use gpui::{App, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window, div, rgb};

use gpui::prelude::FluentBuilder as _;

use crate::theme::{self, Theme};

/// The resolved colours of one radio mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadioPaint {
    /// The ring.
    pub edge: u32,
    /// The filled core, or `None` when the option is not chosen.
    pub core: Option<u32>,
}

/// The colour decision, as a pure function of (selected, enabled, palette).
///
/// A chosen option is the accent, ring and core both — that is the one place a picker
/// spends the accent, and it is what makes "which one is on" answerable at a glance.
/// An available option is a `border` ring, the same hairline every other bounded control
/// in sid draws. An **unavailable** one drops to `faint`: still visibly a ring, so the
/// user can see the option exists and is off, rather than seeing a gap where a control
/// should be.
pub fn radio_paint(selected: bool, enabled: bool, theme: &Theme) -> RadioPaint {
    match (selected, enabled) {
        (true, _) => RadioPaint {
            edge: theme.accent,
            core: Some(theme.accent),
        },
        (false, true) => RadioPaint {
            edge: theme.border,
            core: None,
        },
        (false, false) => RadioPaint {
            edge: theme.faint,
            core: None,
        },
    }
}

/// A radio mark. See the module docs for why it is only the mark.
#[derive(IntoElement)]
pub struct Radio {
    selected: bool,
    enabled: bool,
}

impl Radio {
    /// A mark in the given state, available to be chosen.
    pub fn new(selected: bool) -> Self {
        Self {
            selected,
            enabled: true,
        }
    }

    /// Whether this option can be chosen at all. A disabled option keeps its ring —
    /// see [`radio_paint`].
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl RenderOnce for Radio {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let paint = radio_paint(self.selected, self.enabled, &theme);
        div()
            .size_3()
            // `flex_none`: the mark is a fixed 12px box leading a row that shrinks, and
            // a mark that shrank with the row would go oval before the label elided.
            .flex_none()
            .rounded_full()
            .border_1()
            .border_color(rgb(paint.edge))
            .flex()
            .items_center()
            .justify_center()
            .when_some(paint.core, |mark, core| {
                mark.child(div().size_1p5().rounded_full().bg(rgb(core)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::brightness;
    use crate::theme::{cosmos, cosmos_light, dusk, void};

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    #[test]
    fn only_a_chosen_option_has_a_core() {
        // The whole read of the control: exactly one filled ring in a group.
        for t in palettes() {
            assert_eq!(radio_paint(true, true, &t).core, Some(t.accent));
            assert_eq!(radio_paint(true, false, &t).core, Some(t.accent));
            assert_eq!(radio_paint(false, true, &t).core, None);
            assert_eq!(radio_paint(false, false, &t).core, None);
        }
    }

    #[test]
    fn a_chosen_option_is_the_accent_and_nothing_else_is() {
        // A picker is the one place a screen spends the accent on state rather than on
        // an action; if an unchosen ring could also be accent, "which one is on" stops
        // being answerable at a glance.
        for t in palettes() {
            assert_eq!(radio_paint(true, true, &t).edge, t.accent, "{}", t.name);
            assert_ne!(radio_paint(false, true, &t).edge, t.accent, "{}", t.name);
            assert_ne!(radio_paint(false, false, &t).edge, t.accent, "{}", t.name);
        }
    }

    #[test]
    fn a_disabled_option_is_dimmer_but_never_invisible() {
        // "You cannot pick this" and "there is nothing here" have to look different, or
        // the form reads as half-rendered.
        for t in palettes() {
            let off = radio_paint(false, false, &t);
            let on = radio_paint(false, true, &t);
            assert_eq!(off.edge, t.faint, "{}", t.name);
            assert_ne!(off.edge, t.bg, "{}: the ring dissolved", t.name);
            let dimmer = (brightness(off.edge) - brightness(t.bg)).abs();
            let normal = (brightness(on.edge) - brightness(t.bg)).abs();
            assert!(
                dimmer >= normal || off.edge != on.edge,
                "{}: disabled reads the same as available",
                t.name
            );
        }
    }

    #[test]
    fn every_ring_separates_from_the_surface_it_is_drawn_on() {
        // A radio row sits on a card, not on the canvas — the ring has to be visible
        // against `surface` and `selection` too, because the chosen row is filled.
        for t in palettes() {
            for selected in [true, false] {
                for enabled in [true, false] {
                    let edge = radio_paint(selected, enabled, &t).edge;
                    for backdrop in [t.bg, t.surface, t.selection] {
                        assert_ne!(
                            edge, backdrop,
                            "{}: {selected}/{enabled} ring is invisible on {backdrop:06x}",
                            t.name
                        );
                    }
                }
            }
        }
    }
}
