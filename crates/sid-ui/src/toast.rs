//! Notices — the replacement for the inline `✗ {e}` error line.
//!
//! Every error surface in sid was the same three calls typed again: a `div()`, a
//! `text_color(danger)`, and a message that sometimes opened with a literal `✗` (six
//! sites, in three different fonts, because a Dingbats codepoint renders in whatever the
//! ambient font happens to have). Nothing framed it, so a validation failure at the
//! bottom of a form looked like a caption, and a store failure looked like the same
//! caption.
//!
//! A [`Toast`] is that message with a box around it: a tinted strip, a hairline in its
//! own tone, and a monochrome mark from [`Icon`] instead of a dingbat.
//!
//! # Where the colour comes from
//!
//! [`ToastTone::paint`] is the whole decision, pure over (tone, palette), unit-tested in
//! all four palettes. Two rules make it legible everywhere:
//!
//! - **The tone colours the frame, not the sentence.** Fill, hairline and mark carry the
//!   tone; the message itself is `fg`. An error message is a *sentence* the user has to
//!   read while fixing a field — `danger` ink on a `danger` wash is a hue that reads as
//!   loud and a contrast that reads as blurred, and cosmos-light's dark red on its own
//!   pale tint was the worst of the four.
//! - **`Info` has no hue at all.** `accent` means "engage" (`.interface-design/
//!   system.md`), so an informational notice painted in the accent reads as an error on
//!   cosmos, where the accent *is* red. A neutral notice is a raised `selection` strip —
//!   the same answer [`crate::BadgeTone::Neutral`] gives.
//!
//! # No timer
//!
//! A toast here does not expire, and nothing schedules its removal: the two things sid
//! shows notices *for* are a validation miss and a failed write, both of which must stay
//! on screen while the user fixes the thing they are about. An autohiding,
//! corner-anchored queue needs a host that owns a clock and a list; when a transient
//! notice actually exists, that host is the commit that adds it.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, IntoElement, ParentElement, Refineable as _, RenderOnce,
    SharedString, StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _, rgb,
};

use crate::bridge::mix;
use crate::button::{ButtonSize, IconButton};
use crate::icon::Icon;
use crate::styled::h_flex;
use crate::styled::v_flex;
use crate::theme::{self, Theme};
use crate::typography::Typography;

/// The dismiss handler, shared so the builder can move it into the close button without
/// a second boxing. Same shape as [`crate::Button`]'s.
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// What a notice is saying.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastTone {
    /// Neutral information: what just happened, what to do next. No hue — see the
    /// module docs.
    #[default]
    Info,
    /// It worked.
    Success,
    /// It worked, but something is degraded.
    Warning,
    /// It failed: a validation miss, a refused write, a dead connection.
    Danger,
}

/// Every tone, for the gallery and for exhaustive tests.
pub const ALL_TOAST_TONES: &[ToastTone] = &[
    ToastTone::Info,
    ToastTone::Success,
    ToastTone::Warning,
    ToastTone::Danger,
];

/// How far a toast's fill travels from the panel it sits on toward its tone. Low: this
/// is a strip the width of a modal, and a saturated band that size stops being a notice
/// and becomes the subject.
const TINT: f32 = 0.16;

/// How far the hairline travels — much further than the fill, so the edge states the
/// tone and the interior stays quiet.
const EDGE: f32 = 0.55;

/// The resolved colours for one (tone, palette).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToastPaint {
    /// Strip fill.
    pub fill: u32,
    /// The message's ink. Deliberately not the tone — see the module docs.
    pub ink: u32,
    /// The leading mark's colour: this is where the tone is loudest.
    pub mark: u32,
    /// Hairline colour.
    pub border: u32,
}

impl ToastTone {
    /// The palette token this tone is made of, or `None` for the hueless [`Info`].
    ///
    /// [`Info`]: ToastTone::Info
    fn token(self, theme: &Theme) -> Option<u32> {
        match self {
            ToastTone::Info => None,
            ToastTone::Success => Some(theme.success),
            ToastTone::Warning => Some(theme.warning),
            ToastTone::Danger => Some(theme.danger),
        }
    }

    /// The colour decision. Pure: palette in, tokens out, no globals and no window.
    ///
    /// Mixed from `surface` rather than `bg` because a toast's home is inside a modal or
    /// a card, both of which are `surface`. The two tokens are a hair apart in every
    /// palette, so the strip still separates from the canvas when one is used there.
    pub fn paint(self, theme: &Theme) -> ToastPaint {
        match self.token(theme) {
            Some(tone) => ToastPaint {
                fill: mix(theme.surface, tone, TINT),
                ink: theme.fg,
                mark: tone,
                border: mix(theme.surface, tone, EDGE),
            },
            // One rung up from the panel, drawn from the neutral pair: a notice with no
            // hue still has to read as a *strip* on a surface-filled modal.
            None => ToastPaint {
                fill: theme.selection,
                ink: theme.fg,
                mark: theme.muted,
                border: theme.border,
            },
        }
    }

    /// The leading mark. Named icons, so the six hand-typed `✗` prefixes have one
    /// spelling and it is monochrome line art.
    pub fn icon(self) -> Icon {
        match self {
            ToastTone::Info => Icon::Info,
            ToastTone::Success => Icon::Ok,
            ToastTone::Warning => Icon::Warning,
            ToastTone::Danger => Icon::Error,
        }
    }

    /// A human label for the gallery.
    pub fn label(self) -> &'static str {
        match self {
            ToastTone::Info => "info",
            ToastTone::Success => "success",
            ToastTone::Warning => "warning",
            ToastTone::Danger => "danger",
        }
    }
}

/// A framed notice: mark, message, optional title and actions.
///
/// ```ignore
/// Toast::danger("port must be a number in 1-65535")
/// Toast::info("no OS keyring — this password is held for this session only")
/// ```
///
/// It fills its container's width, so inside a [`crate::Modal`] it spans the panel and
/// its left edge lines up with the fields above it.
#[derive(IntoElement)]
pub struct Toast {
    tone: ToastTone,
    message: SharedString,
    title: Option<SharedString>,
    actions: Vec<AnyElement>,
    on_dismiss: Option<ClickHandler>,
    style: StyleRefinement,
}

impl Toast {
    /// A notice in `tone`.
    pub fn new(tone: ToastTone, message: impl Into<SharedString>) -> Self {
        Self {
            tone,
            message: message.into(),
            title: None,
            actions: Vec::new(),
            on_dismiss: None,
            style: StyleRefinement::default(),
        }
    }

    /// A neutral notice.
    pub fn info(message: impl Into<SharedString>) -> Self {
        Self::new(ToastTone::Info, message)
    }

    /// It worked.
    pub fn success(message: impl Into<SharedString>) -> Self {
        Self::new(ToastTone::Success, message)
    }

    /// It worked, but something is degraded.
    pub fn warning(message: impl Into<SharedString>) -> Self {
        Self::new(ToastTone::Warning, message)
    }

    /// It failed. The replacement for `div().text_color(rgb(danger)).child(err)`.
    pub fn danger(message: impl Into<SharedString>) -> Self {
        Self::new(ToastTone::Danger, message)
    }

    /// A headline above the message, for a notice whose *subject* is not obvious from
    /// its sentence. Most notices need none.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A control on the notice's right edge — "retry", "open settings". Repeatable.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// Make the notice dismissible, adding a close button on its right edge.
    ///
    /// Optional on purpose: a validation error is cleared by fixing the field it is
    /// about, and a close button on it invites the user to hide the reason their save
    /// did not land.
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl Styled for Toast {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let paint = self.tone.paint(&theme);
        // A one-line notice centres on its mark; a wrapped one, or one with controls
        // beside it, has to align to the top or the mark floats mid-paragraph.
        let single_line = self.on_dismiss.is_none() && self.actions.is_empty();

        let mut strip = h_flex()
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(paint.fill))
            .border_1()
            .border_color(rgb(paint.border))
            .when(!single_line, |this| this.items_start())
            .child(self.tone.icon().small().text_color(rgb(paint.mark)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .when_some(self.title, |this, title| {
                        this.child(div().text_body(&theme).child(title))
                    })
                    // The message names its own ink: a notice inside a form would
                    // otherwise inherit whatever the surrounding labels were painted in.
                    .child(
                        div()
                            .text_meta(&theme)
                            .text_color(rgb(paint.ink))
                            .child(self.message),
                    ),
            )
            .when(!self.actions.is_empty(), |this| {
                this.child(h_flex().flex_none().gap_1().children(self.actions))
            })
            .when_some(self.on_dismiss, |this, on_dismiss| {
                this.child(
                    IconButton::new("toast-dismiss", Icon::Close, "dismiss")
                        .size(ButtonSize::Sm)
                        .on_click(move |ev, window, cx| on_dismiss(ev, window, cx)),
                )
            });
        strip.style().refine(&self.style);
        strip
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
    fn every_tone_reads_as_a_framed_strip() {
        // The failure this replaces: coloured text with nothing around it. A notice has
        // to separate from the panel it sits on (`surface`) *and* have an edge.
        for t in palettes() {
            for &tone in ALL_TOAST_TONES {
                let p = tone.paint(&t);
                assert_ne!(
                    p.fill,
                    t.surface,
                    "{}/{}: invisible on a modal",
                    t.name,
                    tone.label()
                );
                assert_ne!(p.border, p.fill, "{}/{}: no hairline", t.name, tone.label());
            }
        }
    }

    #[test]
    fn the_message_stays_readable_in_every_palette() {
        // Why the ink is `fg` and not the tone: this is the assertion that fails when a
        // sentence is painted in its own wash. The threshold is the button label's.
        for t in palettes() {
            for &tone in ALL_TOAST_TONES {
                let p = tone.paint(&t);
                let delta = (brightness(p.ink) - brightness(p.fill)).abs();
                assert!(
                    delta > 0.25,
                    "{}/{}: message contrast {delta:.2} is too low",
                    t.name,
                    tone.label()
                );
            }
        }
    }

    #[test]
    fn the_mark_is_visible_on_its_own_fill() {
        for t in palettes() {
            for &tone in ALL_TOAST_TONES {
                let p = tone.paint(&t);
                let delta = (brightness(p.mark) - brightness(p.fill)).abs();
                assert!(
                    delta > 0.15,
                    "{}/{}: mark contrast {delta:.2} is too low",
                    t.name,
                    tone.label()
                );
            }
        }
    }

    #[test]
    fn a_toned_notice_carries_its_semantic_token() {
        for t in palettes() {
            assert_eq!(ToastTone::Danger.paint(&t).mark, t.danger, "{}", t.name);
            assert_eq!(ToastTone::Warning.paint(&t).mark, t.warning, "{}", t.name);
            assert_eq!(ToastTone::Success.paint(&t).mark, t.success, "{}", t.name);
        }
    }

    #[test]
    fn info_spends_no_accent() {
        // On cosmos the accent *is* red: an accent-marked "info" notice is
        // indistinguishable from an error, which is the whole reason this tone has no
        // hue.
        for t in palettes() {
            let p = ToastTone::Info.paint(&t);
            assert_ne!(p.mark, t.accent, "{}: info must not read as engage", t.name);
            assert_ne!(
                p.mark, t.danger,
                "{}: info must not read as failure",
                t.name
            );
            assert_eq!(p.fill, t.selection, "{}: a raised neutral strip", t.name);
            assert_eq!(p.border, t.border, "{}", t.name);
        }
    }

    #[test]
    fn every_mark_is_a_registry_icon_and_not_a_dingbat() {
        // The hand-typed `✗` prefixes, replaced by one named glyph each.
        assert_eq!(ToastTone::Danger.icon(), Icon::Error);
        assert_eq!(ToastTone::Warning.icon(), Icon::Warning);
        assert_eq!(ToastTone::Success.icon(), Icon::Ok);
        assert_eq!(ToastTone::Info.icon(), Icon::Info);
    }

    #[test]
    fn a_toast_takes_a_style_refinement() {
        // Notices get placed by their host: `mt_2` above a footer, `w_full` in a row.
        let mut toast = Toast::danger("boom").mt_2();
        assert!(toast.style().margin.top.is_some());
    }
}
