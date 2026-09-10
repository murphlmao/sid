//! Inline notices — the "that didn't work" that lives *in* a form.
//!
//! Distinct from [`crate::Toast`], and the distinction is where the message belongs, not
//! how big it is: a toast is an event that floats over the screen and can be dismissed;
//! a notice is a **property of the thing it sits under**, and it stays until that thing
//! changes. The kill error under a process row, the connection error under a DB form's
//! footer, the "no rows" caveat under a paged result — none of those make sense
//! anywhere but where they are.
//!
//! The message wraps to two lines rather than clamping to one — an OS error can run
//! past what a form's width can hold on one line, and the old one-line clamp taught it
//! to end in an ellipsis instead. A caller with more to say than the sentence itself
//! (what, and separately why) can add a [`detail`](InlineNotice::detail) line, in the
//! Meta role and always `muted` regardless of tone — mirroring [`crate::Toast`]'s
//! title-then-message shape, but as a *second* line under the message rather than
//! above it, since a notice's message is the headline here.
//!
//! # What this replaces
//!
//! Four lines, retyped in three tabs (`systems_tab.rs:1126`, `network_tab.rs:2221`,
//! `db_tab.rs:1183`), each file-private so the next tab copied it again rather than
//! importing it. The copies had already drifted: all three hand-typed `text_xs()`
//! instead of naming a role, so a notice rendered at whatever size the surrounding
//! panel's cascade happened to leave, and the `db_tab` copy grew a fourth spelling
//! ([`caveat_line`]) that the other two do not have.

use gpui::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window,
    prelude::FluentBuilder as _, rgb,
};

use crate::icon::Icon;
use crate::styled::{h_flex, v_flex};
use crate::theme::{self, Theme};
use crate::typography::Typography as _;

/// What an inline notice is saying.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NoticeTone {
    /// Something failed and the user has to do something about it.
    #[default]
    Error,
    /// Something to know about what is on screen. Not a failure.
    Caveat,
}

/// Every tone, for the gallery and for exhaustive tests.
pub const ALL_NOTICE_TONES: &[NoticeTone] = &[NoticeTone::Error, NoticeTone::Caveat];

impl NoticeTone {
    /// This tone's ink.
    ///
    /// A caveat is deliberately **not** the danger colour. A page-local sort or a
    /// truncated result set is not a failure — nothing went wrong and nothing needs
    /// fixing — and painting it red trains the eye to skip the real errors that share
    /// this slot.
    pub const fn ink(self, theme: &Theme) -> u32 {
        match self {
            NoticeTone::Error => theme.danger,
            NoticeTone::Caveat => theme.muted,
        }
    }

    /// This tone's leading glyph.
    pub const fn icon(self) -> Icon {
        match self {
            NoticeTone::Error => Icon::Error,
            NoticeTone::Caveat => Icon::Info,
        }
    }

    /// A human label for the gallery.
    pub const fn label(self) -> &'static str {
        match self {
            NoticeTone::Error => "error",
            NoticeTone::Caveat => "caveat",
        }
    }
}

/// A notice: a glyph from the registry, then the message, wrapped to at most two
/// lines, with an optional `detail` line under it.
///
/// ```ignore
/// v_flex().children(self.kill_error.clone().map(error_line))
/// ```
#[derive(IntoElement)]
pub struct InlineNotice {
    tone: NoticeTone,
    message: SharedString,
    detail: Option<SharedString>,
}

impl InlineNotice {
    /// A notice in `tone`.
    pub fn new(tone: NoticeTone, message: impl Into<SharedString>) -> Self {
        Self {
            tone,
            message: message.into(),
            detail: None,
        }
    }

    /// A second line under the message: the "why", once the sentence itself is the
    /// whole first line and there is still a reason or a recommendation to give.
    /// Always `muted`, independent of `tone` — the elaboration is not itself the
    /// failure.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A failure, in the danger tone — the shape three tabs each retyped.
pub fn error_line(message: impl Into<SharedString>) -> InlineNotice {
    InlineNotice::new(NoticeTone::Error, message)
}

/// An advisory, in `muted` — see [`NoticeTone::ink`] for why this is not red.
pub fn caveat_line(message: impl Into<SharedString>) -> InlineNotice {
    InlineNotice::new(NoticeTone::Caveat, message)
}

impl RenderOnce for InlineNotice {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let ink = self.tone.ink(&theme);
        h_flex()
            .w_full()
            .min_w_0()
            .items_start()
            .gap_1p5()
            .py_1()
            // The role sets the measurement, the tone sets the ink. The copies this
            // replaces set a raw `text_xs()`, so a notice rendered at whatever size the
            // panel around it had cascaded.
            .text_meta(&theme)
            .text_color(rgb(ink))
            .child(self.tone.icon().small().text_color(rgb(ink)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        // An error message is arbitrary text from an OS call — it can be
                        // a sentence or it can be 400 characters of `ssh: handshake
                        // failed:`. `min_w_0` on a `flex_1` child is the landmines-doc
                        // fix trio's other half: without it gpui reports this element's
                        // min-content width as the whole string and it never shrinks to
                        // the row's real width, so it never gets a wrap width to wrap
                        // *at*. Two lines, then a real ellipsis for what still overflows.
                        gpui::div()
                            .min_w_0()
                            .line_clamp(2)
                            .text_ellipsis()
                            .child(self.message),
                    )
                    .when_some(self.detail, |this, detail| {
                        this.child(
                            gpui::div()
                                .min_w_0()
                                .text_meta(&theme)
                                .line_clamp(2)
                                .text_ellipsis()
                                .child(detail),
                        )
                    }),
            )
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
    fn an_error_is_the_danger_token_and_a_caveat_is_never() {
        // The distinction the DB tab discovered and the other two copies never got: a
        // page-local sort is not a failure, and painting it red teaches the eye to skip
        // the failures that use the same slot.
        for t in palettes() {
            assert_eq!(NoticeTone::Error.ink(&t), t.danger, "{}", t.name);
            assert_eq!(NoticeTone::Caveat.ink(&t), t.muted, "{}", t.name);
            assert_ne!(NoticeTone::Caveat.ink(&t), t.danger, "{}", t.name);
        }
    }

    #[test]
    fn every_tone_stays_readable_on_the_canvas_and_on_a_card() {
        // A notice lives inside a form, which lives on a card, which sits on the canvas.
        // It has to survive both backdrops in all four palettes.
        for t in palettes() {
            for &tone in ALL_NOTICE_TONES {
                for backdrop in [t.bg, t.surface] {
                    let delta = (brightness(tone.ink(&t)) - brightness(backdrop)).abs();
                    assert!(
                        delta > 0.15,
                        "{}/{}: contrast {delta:.2} on {backdrop:06x}",
                        t.name,
                        tone.label()
                    );
                }
            }
        }
    }

    #[test]
    fn the_two_tones_are_told_apart_by_their_glyph_as_well_as_their_colour() {
        // Colour alone is not a signal — the same message in two colours is one message
        // to anyone reading it at a glance or with a colour-vision difference.
        assert_eq!(NoticeTone::Error.icon(), Icon::Error);
        assert_eq!(NoticeTone::Caveat.icon(), Icon::Info);
        assert_ne!(NoticeTone::Error.icon(), NoticeTone::Caveat.icon());
    }

    #[test]
    fn the_constructors_pick_their_tone() {
        assert_eq!(error_line("x").tone, NoticeTone::Error);
        assert_eq!(caveat_line("x").tone, NoticeTone::Caveat);
        assert_eq!(error_line("boom").message.as_ref(), "boom");
        assert_eq!(NoticeTone::default(), NoticeTone::Error);
    }

    #[test]
    fn detail_defaults_to_none_and_the_builder_sets_it() {
        // No API break: a caller that never calls `.detail(..)` gets exactly the old
        // one-`Option`-field shape, just unset.
        assert_eq!(error_line("boom").detail, None);
        assert_eq!(
            error_line("boom").detail("why").detail.as_deref(),
            Some("why")
        );
    }

    #[test]
    fn tone_labels_are_unique_so_the_gallery_cannot_double_up() {
        let mut names: Vec<&str> = ALL_NOTICE_TONES.iter().map(|t| t.label()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
    }
}
