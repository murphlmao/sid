//! Badges and pills — the orientation chips.
//!
//! Origin (`global` / `workspace`), counts, and states (`connected`, `degraded`,
//! `failed`) are all the same shape: a short word in a small box that the eye can skip.
//! `.interface-design/system.md` is explicit that these are *orientation*, not
//! engagement — "Orientation badges (origin, counts) are `faint`/`muted`. One accent,
//! used sparingly" — which is why [`BadgeTone::Neutral`] is the default and why the
//! accent tone exists but is not the easy thing to type.
//!
//! The box comes from `gpui_component::tag::Tag` (a bordered `text_xs` chip whose
//! geometry already matches the design system's `rounded_md` corner spec); every colour
//! comes from [`BadgeTone::paint`], a pure function over sid tokens. The library's own
//! `Badge` is a different widget entirely — a notification dot positioned over another
//! element — and is not what this is.

use gpui::{
    App, IntoElement, ParentElement, RenderOnce, SharedString, Window, div,
    prelude::FluentBuilder as _, rgb, transparent_black,
};
use gpui_component::{Sizable as _, tag::Tag};

use crate::bridge::{contrast_ink, mix};
use crate::theme::{self, Theme};

/// What a badge is saying.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BadgeTone {
    /// Orientation: origin chips, counts, metadata. Reads as furniture, on purpose.
    #[default]
    Neutral,
    /// Engage / active. Sparingly — one accent per screen.
    Accent,
    /// Healthy, connected, passing.
    Success,
    /// Degraded, connecting, needs attention.
    Warning,
    /// Failed, destructive, disconnected-with-an-error.
    Danger,
}

/// Every tone, for the gallery and for exhaustive tests.
pub const ALL_BADGE_TONES: &[BadgeTone] = &[
    BadgeTone::Neutral,
    BadgeTone::Accent,
    BadgeTone::Success,
    BadgeTone::Warning,
    BadgeTone::Danger,
];

/// How much of the tone a badge spends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BadgeFill {
    /// A washed tint of the tone with the tone as ink — the default, and quiet enough
    /// to repeat down a list.
    #[default]
    Soft,
    /// The tone at full strength. For the one chip that must be seen.
    Solid,
    /// Border and ink only. For chips that sit on an already-busy surface.
    Outline,
}

/// Every fill, for the gallery and for exhaustive tests.
pub const ALL_BADGE_FILLS: &[BadgeFill] = &[BadgeFill::Soft, BadgeFill::Solid, BadgeFill::Outline];

/// The resolved colours for one (tone, fill, palette).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadgePaint {
    /// Fill, or `None` for [`BadgeFill::Outline`].
    pub fill: Option<u32>,
    /// Label colour.
    pub ink: u32,
    /// Hairline colour. Always present: a badge is a box.
    pub border: u32,
}

/// How far a soft fill travels from the canvas toward its tone. High enough that the
/// chip is visible as a box, low enough that the tone stays legible as *ink* on top
/// of it.
const SOFT_TINT: f32 = 0.22;

/// How far a soft badge's hairline travels — further than the fill, so the edge reads
/// before the interior does.
const SOFT_EDGE: f32 = 0.45;

impl BadgeTone {
    /// The token this tone is made of. `Neutral` has no hue: it is drawn from the
    /// surface/muted pair instead, which is what "furniture" means here.
    fn token(self, theme: &Theme) -> Option<u32> {
        match self {
            BadgeTone::Neutral => None,
            BadgeTone::Accent => Some(theme.accent),
            BadgeTone::Success => Some(theme.success),
            BadgeTone::Warning => Some(theme.warning),
            BadgeTone::Danger => Some(theme.danger),
        }
    }

    /// The colour decision. Pure: palette in, tokens out.
    pub fn paint(self, fill: BadgeFill, theme: &Theme) -> BadgePaint {
        match (self.token(theme), fill) {
            // Neutral: the design system's own answer for orientation chips.
            (None, BadgeFill::Soft) => BadgePaint {
                fill: Some(theme.surface),
                ink: theme.muted,
                border: theme.border,
            },
            (None, BadgeFill::Solid) => BadgePaint {
                fill: Some(theme.selection),
                ink: theme.fg,
                border: theme.border,
            },
            (None, BadgeFill::Outline) => BadgePaint {
                fill: None,
                ink: theme.muted,
                border: theme.border,
            },
            // Toned: a wash of the tone over the canvas, with the tone itself as ink.
            (Some(tone), BadgeFill::Soft) => BadgePaint {
                fill: Some(mix(theme.bg, tone, SOFT_TINT)),
                ink: tone,
                border: mix(theme.bg, tone, SOFT_EDGE),
            },
            (Some(tone), BadgeFill::Solid) => BadgePaint {
                fill: Some(tone),
                ink: contrast_ink(theme, tone),
                border: tone,
            },
            (Some(tone), BadgeFill::Outline) => BadgePaint {
                fill: None,
                ink: tone,
                border: tone,
            },
        }
    }

    /// A human label for the gallery.
    pub fn label(self) -> &'static str {
        match self {
            BadgeTone::Neutral => "neutral",
            BadgeTone::Accent => "accent",
            BadgeTone::Success => "success",
            BadgeTone::Warning => "warning",
            BadgeTone::Danger => "danger",
        }
    }
}

/// A small labelled chip.
///
/// ```ignore
/// Badge::new("global")                       // an origin chip
/// Badge::new("3").tone(BadgeTone::Warning)   // a count that wants attention
/// Badge::new("connected").pill().tone(BadgeTone::Success)
/// ```
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: BadgeTone,
    fill: BadgeFill,
    pill: bool,
}

impl Badge {
    /// A neutral, soft, `rounded_md` chip.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: BadgeTone::default(),
            fill: BadgeFill::default(),
            pill: false,
        }
    }

    /// A count chip: `Badge::count(12)` is `Badge::new("12")`, spelled so call sites do
    /// not each invent their own formatting.
    pub fn count(count: usize) -> Self {
        Self::new(count.to_string())
    }

    /// Set the tone.
    pub fn tone(mut self, tone: BadgeTone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the fill.
    pub fn fill(mut self, fill: BadgeFill) -> Self {
        self.fill = fill;
        self
    }

    /// The tone at full strength.
    pub fn solid(self) -> Self {
        self.fill(BadgeFill::Solid)
    }

    /// Border and ink only.
    pub fn outline(self) -> Self {
        self.fill(BadgeFill::Outline)
    }

    /// Fully rounded ends — the "pill" shape, for status words and counts.
    pub fn pill(mut self) -> Self {
        self.pill = true;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let paint = self.tone.paint(self.fill, &theme);
        let fill = paint
            .fill
            .map_or_else(transparent_black, |fill| rgb(fill).into());

        Tag::custom(fill, rgb(paint.ink).into(), rgb(paint.border).into())
            .small()
            .map(|tag| if self.pill { tag.rounded_full() } else { tag })
            .child(div().child(self.label))
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
    fn every_tone_and_fill_keeps_its_label_readable() {
        // A chip whose word cannot be read is decoration. Soft fills are the risky
        // case: the wash must stay far enough from the tone to leave it legible.
        for t in palettes() {
            for &tone in ALL_BADGE_TONES {
                for &fill in ALL_BADGE_FILLS {
                    let p = tone.paint(fill, &t);
                    let backdrop = p.fill.unwrap_or(t.bg);
                    let delta = (brightness(p.ink) - brightness(backdrop)).abs();
                    assert!(
                        delta > 0.2,
                        "{}/{}/{fill:?}: label contrast {delta:.2} is too low",
                        t.name,
                        tone.label()
                    );
                }
            }
        }
    }

    #[test]
    fn every_badge_is_a_box() {
        // Orientation chips are skipped by the eye precisely because they are bounded;
        // one that dissolves into the canvas is just differently-coloured text. Both
        // the edge and any interior must separate from the background.
        for t in palettes() {
            for &tone in ALL_BADGE_TONES {
                for &fill in ALL_BADGE_FILLS {
                    let p = tone.paint(fill, &t);
                    assert_ne!(
                        p.border,
                        t.bg,
                        "{}/{}/{fill:?}: the hairline is invisible",
                        t.name,
                        tone.label()
                    );
                    if let Some(interior) = p.fill {
                        assert_ne!(
                            interior,
                            t.bg,
                            "{}/{}/{fill:?}: the fill is invisible",
                            t.name,
                            tone.label()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn neutral_is_furniture_not_engagement() {
        // The design system's rule, pinned: origin/count chips read `muted`, never
        // accent. A regression here is how 16 keybindings ended up painted red.
        for t in palettes() {
            let soft = BadgeTone::Neutral.paint(BadgeFill::Soft, &t);
            assert_eq!(soft.ink, t.muted, "{}: neutral ink", t.name);
            assert_eq!(soft.fill, Some(t.surface), "{}: neutral fill", t.name);
            assert_ne!(soft.ink, t.accent, "{}: neutral is never accent", t.name);
            let outline = BadgeTone::Neutral.paint(BadgeFill::Outline, &t);
            assert_eq!(outline.ink, t.muted, "{}: neutral outline ink", t.name);
        }
    }

    #[test]
    fn a_soft_badge_tints_the_canvas_toward_its_tone() {
        // Soft is a *wash*: nearer the canvas than the tone, but not the canvas.
        for t in palettes() {
            for &tone in &ALL_BADGE_TONES[1..] {
                let token = tone.token(&t).expect("toned");
                let p = tone.paint(BadgeFill::Soft, &t);
                let fill = p.fill.expect("soft has a fill");
                assert_ne!(fill, t.bg, "{}/{}: invisible chip", t.name, tone.label());
                assert_ne!(fill, token, "{}/{}: that is Solid", t.name, tone.label());
                assert_eq!(p.ink, token, "{}/{}: ink is the tone", t.name, tone.label());
            }
        }
    }

    #[test]
    fn a_solid_badge_labels_itself_in_whichever_ink_survives() {
        // dusk's amber warning needs dark ink; cosmos's dark-red danger needs light —
        // the same reason `contrast_ink` exists for buttons.
        for t in palettes() {
            for &tone in &ALL_BADGE_TONES[1..] {
                let token = tone.token(&t).expect("toned");
                let p = tone.paint(BadgeFill::Solid, &t);
                assert_eq!(p.fill, Some(token), "{}/{}", t.name, tone.label());
                assert_eq!(
                    p.ink,
                    contrast_ink(&t, token),
                    "{}/{}",
                    t.name,
                    tone.label()
                );
            }
        }
    }

    #[test]
    fn an_outline_badge_has_no_fill() {
        for t in palettes() {
            for &tone in ALL_BADGE_TONES {
                assert_eq!(
                    tone.paint(BadgeFill::Outline, &t).fill,
                    None,
                    "{}/{}",
                    t.name,
                    tone.label()
                );
            }
        }
    }

    #[test]
    fn count_renders_the_number() {
        // `Badge::count` exists so 30 call sites do not each write `format!("{n}")`.
        let mut badge = Badge::count(12);
        assert_eq!(badge.label.as_ref(), "12");
        badge = Badge::count(0);
        assert_eq!(badge.label.as_ref(), "0");
    }

    #[test]
    fn the_fluent_setters_land_where_they_claim() {
        let b = Badge::new("x").tone(BadgeTone::Danger).solid().pill();
        assert_eq!(b.tone, BadgeTone::Danger);
        assert_eq!(b.fill, BadgeFill::Solid);
        assert!(b.pill);
        let b = Badge::new("x").outline();
        assert_eq!(b.fill, BadgeFill::Outline);
        assert!(!b.pill);
    }
}
