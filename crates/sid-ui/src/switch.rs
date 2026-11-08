//! Switches — a setting that is on or off *right now*.
//!
//! A [`crate::Checkbox`] is a choice you confirm later (a form field, "show hidden" in a
//! filter strip); a switch takes effect the moment it moves — Settings' "Reduce motion".
//! Shape: a 36x20 pill track, a 16px thumb, the accent track when on. The thumb slides
//! over [`crate::motion::FAST`] when the value changes, and simply sits in place under
//! reduced motion.
//!
//! [`SwitchPaint::resolve`] is the colour decision, pure over (on, disabled, palette).

use std::rc::Rc;

use gpui::{
    Animation, AnimationExt as _, App, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, rgb,
};

use crate::bridge::{contrast_ink, mix};
use crate::checkbox::is_toggle_key;
use crate::copy::sentence_case;
use crate::metrics::{MIN_HIT, Radius, Rounded as _};
use crate::motion;
use crate::scale::scaled;
use crate::styled::{StyledExt as _, h_flex};
use crate::theme::{self, Theme};
use crate::typography::Typography as _;

/// Track width, in design pixels.
pub const TRACK_W: f32 = 36.;
/// Track height.
pub const TRACK_H: f32 = 20.;
/// Thumb diameter.
pub const THUMB: f32 = 16.;
/// The gap between the thumb and the track's edge.
const INSET: f32 = (TRACK_H - THUMB) / 2.;

/// The thumb's left offset inside the track for a given value, in design pixels.
pub fn thumb_left(on: bool) -> f32 {
    if on { TRACK_W - THUMB - INSET } else { INSET }
}

/// The resolved colours for one (on, disabled, palette).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchPaint {
    /// Track fill.
    pub track: u32,
    /// Track hairline.
    pub border: u32,
    /// Thumb fill.
    pub thumb: u32,
    /// Label ink.
    pub label: u32,
}

impl SwitchPaint {
    /// The colour decision.
    pub fn resolve(on: bool, disabled: bool, theme: &Theme) -> Self {
        let paint = if on {
            SwitchPaint {
                track: theme.accent,
                border: theme.accent,
                thumb: contrast_ink(theme, theme.accent),
                label: theme.fg,
            }
        } else {
            SwitchPaint {
                track: theme.selection,
                border: mix(theme.border, theme.muted, 0.35),
                thumb: theme.muted,
                label: theme.fg,
            }
        };
        if disabled {
            SwitchPaint {
                track: mix(paint.track, theme.bg, 0.6),
                border: theme.border,
                thumb: theme.faint,
                label: theme.faint,
            }
        } else {
            paint
        }
    }
}

type ToggleHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A labelled on/off switch.
///
/// ```ignore
/// Switch::new("reduce-motion", reduced)
///     .label("reduce motion")
///     .on_toggle(cx.listener(|this, on: &bool, _, cx| this.set_reduced_motion(*on, cx)))
/// ```
#[derive(IntoElement)]
pub struct Switch {
    id: SharedString,
    on: bool,
    label: Option<SharedString>,
    disabled: bool,
    tab_index: isize,
    on_toggle: Option<ToggleHandler>,
}

impl Switch {
    /// A switch showing `on`. `id` must be unique within the window.
    pub fn new(id: impl Into<SharedString>, on: bool) -> Self {
        Self {
            id: id.into(),
            on,
            label: None,
            disabled: false,
            tab_index: 0,
            on_toggle: None,
        }
    }

    /// The words beside the track — part of the hit target. Sentence-cased.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Switch the control off.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Where this control sits in the keyboard tab order.
    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = index;
        self
    }

    /// Called with the new value on click, Space or Enter.
    pub fn on_toggle(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let on = self.on;
        let paint = SwitchPaint::resolve(on, self.disabled, &theme);
        let interactive = !self.disabled && self.on_toggle.is_some();
        let handler = self.on_toggle.filter(|_| interactive);
        let (from, to) = (thumb_left(!on), thumb_left(on));

        let thumb = div()
            .absolute()
            .top(scaled(INSET))
            .size(scaled(THUMB))
            .rounded_full()
            .bg(rgb(paint.thumb));
        // Keyed by the value, so a change starts a fresh slide from the old side; a
        // repaint at the same value finds a finished animation and stays put.
        let thumb = if motion::reduced(cx) {
            thumb.left(scaled(to)).into_any_element()
        } else {
            thumb
                .with_animation(
                    SharedString::from(format!("{}-thumb-{on}", self.id)),
                    Animation::new(motion::FAST).with_easing(motion::ease_out),
                    move |thumb, t| thumb.left(scaled(from + (to - from) * t)),
                )
                .into_any_element()
        };

        let track = div()
            .flex_none()
            .relative()
            .w(scaled(TRACK_W))
            .h(scaled(TRACK_H))
            .rounded_full()
            .border_1()
            .border_color(rgb(paint.border))
            .bg(rgb(paint.track))
            .child(thumb);

        h_flex()
            .id(self.id)
            .flex_none()
            .gap_2()
            .min_h(scaled(f32::from(MIN_HIT)))
            .px_1()
            .corners(Radius::Sm)
            .when(interactive, |this| {
                this.cursor_pointer()
                    .tab_index(self.tab_index)
                    .focus_ring(&theme)
            })
            .child(track)
            .when_some(self.label, |this, label| {
                this.child(
                    div()
                        .min_w_0()
                        .text_body(&theme)
                        .text_color(rgb(paint.label))
                        .child(sentence_case(&label)),
                )
            })
            .when_some(handler, |this, handler| {
                let on_key = handler.clone();
                this.on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    handler(!on, window, cx)
                })
                .on_key_down(move |ev: &KeyDownEvent, window, cx| {
                    if is_toggle_key(ev) {
                        cx.stop_propagation();
                        on_key(!on, window, cx);
                    }
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{cosmos, cosmos_light, dusk, void};

    #[test]
    fn the_thumb_sits_inside_the_track_at_both_ends() {
        for on in [false, true] {
            let left = thumb_left(on);
            assert!(left >= 0.);
            assert!(left + THUMB <= TRACK_W, "on={on}");
        }
        assert!(thumb_left(true) > thumb_left(false), "on slides right");
        assert_eq!(
            thumb_left(false),
            TRACK_W - THUMB - thumb_left(true),
            "symmetric"
        );
    }

    #[test]
    fn on_is_the_accent_and_the_thumb_stands_off_it() {
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let on = SwitchPaint::resolve(true, false, &t);
            assert_eq!(on.track, t.accent);
            let gap =
                (crate::bridge::brightness(on.thumb) - crate::bridge::brightness(on.track)).abs();
            assert!(gap > 0.3, "{}: thumb vs accent {gap}", t.name);
            let off = SwitchPaint::resolve(false, false, &t);
            assert_ne!(off.track, t.accent, "{}: off does not engage", t.name);
            assert_ne!(
                off.thumb, off.track,
                "{}: the thumb is visible when off",
                t.name
            );
        }
    }

    #[test]
    fn disabled_dims_everything() {
        let t = cosmos();
        let d = SwitchPaint::resolve(true, true, &t);
        assert_eq!(d.thumb, t.faint);
        assert_eq!(d.label, t.faint);
        assert_ne!(d.track, t.accent);
    }
}
