//! Checkboxes — the boolean sid used to spell as a `☑` / `☐` glyph.
//!
//! The SFTP sidebar's "hidden" toggle was a text button whose label swapped between two
//! Dingbats codepoints (rendered in whatever font happened to carry them), which is the
//! kind of control that reads as a placeholder. This is the real thing: an 18px box on
//! the [`Radius::Xs`] corner, the accent when on, a check (or a dash for a mixed state)
//! drawn from [`Icon`], a label beside it that is part of the hit target, and the whole
//! control keyboard-operable — Space or Enter toggles it when it has focus.
//!
//! [`CheckboxPaint::resolve`] is the colour decision, pure over (state, disabled,
//! palette) and unit-tested in all four palettes.

use std::rc::Rc;

use crate::TestSupportExt as _;

use gpui::{
    App, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, rgb,
};

use crate::bridge::{contrast_ink, mix};
use crate::copy::sentence_case;
use crate::icon::Icon;
use crate::metrics::{MIN_HIT, Radius, Rounded as _};
use crate::scale::scaled;
use crate::styled::{StyledExt as _, h_flex};
use crate::theme::{self, Theme};
use crate::typography::Typography as _;

/// The box's edge, in design pixels.
pub const CHECKBOX_PX: f32 = 18.;

/// A checkbox's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CheckState {
    /// Unchecked.
    #[default]
    Off,
    /// Checked.
    On,
    /// Some of a group is checked — drawn as a dash.
    Mixed,
}

impl CheckState {
    /// `true` → `On`, `false` → `Off`.
    pub fn from_bool(on: bool) -> Self {
        if on { CheckState::On } else { CheckState::Off }
    }

    /// What one press turns this into. A mixed box becomes fully on — the convention
    /// every platform follows, because "select all" is what a user pressing a
    /// half-ticked header means.
    pub fn toggled(self) -> Self {
        match self {
            CheckState::On => CheckState::Off,
            CheckState::Off | CheckState::Mixed => CheckState::On,
        }
    }

    /// Whether the box shows the accent.
    pub fn is_lit(self) -> bool {
        !matches!(self, CheckState::Off)
    }
}

/// The resolved colours for one (state, disabled, palette).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckboxPaint {
    /// The box's fill.
    pub fill: u32,
    /// The box's hairline.
    pub border: u32,
    /// The check / dash ink, when there is one.
    pub mark: Option<u32>,
    /// The label's ink.
    pub label: u32,
}

impl CheckboxPaint {
    /// The colour decision.
    pub fn resolve(state: CheckState, disabled: bool, theme: &Theme) -> Self {
        let lit = state.is_lit();
        let (fill, border) = if lit {
            (theme.accent, theme.accent)
        } else {
            // A visible frame on the recess: `border` alone is a hairline meant to
            // recede, which on an empty box is the whole control.
            (theme.well, mix(theme.border, theme.muted, 0.45))
        };
        let mark = lit.then(|| contrast_ink(theme, theme.accent));
        if disabled {
            CheckboxPaint {
                fill: mix(fill, theme.bg, 0.6),
                border: theme.border,
                mark: mark.map(|_| theme.faint),
                label: theme.faint,
            }
        } else {
            CheckboxPaint {
                fill,
                border,
                mark,
                label: theme.fg,
            }
        }
    }
}

/// A toggle handler: receives the state the control is changing *to*.
type ToggleHandler = Rc<dyn Fn(CheckState, &mut Window, &mut App)>;

/// A labelled checkbox.
///
/// ```ignore
/// Checkbox::new("sftp-hidden", CheckState::from_bool(self.show_hidden))
///     .label("show hidden files")
///     .on_toggle(cx.listener(|this, state: &CheckState, _, cx| { .. }))
/// ```
#[derive(IntoElement)]
pub struct Checkbox {
    id: SharedString,
    state: CheckState,
    label: Option<SharedString>,
    disabled: bool,
    tab_index: isize,
    on_toggle: Option<ToggleHandler>,
}

impl Checkbox {
    /// A checkbox in `state`. `id` must be unique within the window.
    pub fn new(id: impl Into<SharedString>, state: CheckState) -> Self {
        Self {
            id: id.into(),
            state,
            label: None,
            disabled: false,
            tab_index: 0,
            on_toggle: None,
        }
    }

    /// The words beside the box — part of the hit target. Sentence-cased by the control.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Switch the control off: dimmed, no pointer, out of the tab order.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Where this control sits in the keyboard tab order.
    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = index;
        self
    }

    /// Called with the new state on click, Space or Enter.
    pub fn on_toggle(
        mut self,
        handler: impl Fn(CheckState, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let paint = CheckboxPaint::resolve(self.state, self.disabled, &theme);
        let next = self.state.toggled();
        let interactive = !self.disabled && self.on_toggle.is_some();
        let handler = self.on_toggle.filter(|_| interactive);
        let mark = match self.state {
            CheckState::On => Some(Icon::Check),
            CheckState::Mixed => Some(Icon::Remove),
            CheckState::Off => None,
        };

        let check_box = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(scaled(CHECKBOX_PX))
            .corners(Radius::Xs)
            .border_1()
            .border_color(rgb(paint.border))
            .bg(rgb(paint.fill))
            .when_some(mark.zip(paint.mark), |this, (icon, ink)| {
                this.child(icon.el().size(scaled(14.)).text_color(rgb(ink)))
            });

        h_flex()
            .id(self.id)
            .test_support()
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
            .child(check_box)
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
                    handler(next, window, cx)
                })
                .on_key_down(move |ev: &KeyDownEvent, window, cx| {
                    if is_toggle_key(ev) {
                        cx.stop_propagation();
                        on_key(next, window, cx);
                    }
                })
            })
    }
}

/// Space or Enter, unmodified — the keys that operate a focused checkbox or switch.
pub(crate) fn is_toggle_key(ev: &KeyDownEvent) -> bool {
    let k = &ev.keystroke;
    (k.key == "space" || k.key == "enter") && !k.modifiers.modified()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{cosmos, cosmos_light, dusk, void};

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    #[test]
    fn a_press_toggles_and_a_mixed_box_turns_fully_on() {
        assert_eq!(CheckState::Off.toggled(), CheckState::On);
        assert_eq!(CheckState::On.toggled(), CheckState::Off);
        assert_eq!(CheckState::Mixed.toggled(), CheckState::On);
        assert_eq!(CheckState::from_bool(true), CheckState::On);
        assert_eq!(CheckState::from_bool(false), CheckState::Off);
    }

    #[test]
    fn on_is_the_accent_and_off_is_a_visible_empty_frame() {
        for t in palettes() {
            let on = CheckboxPaint::resolve(CheckState::On, false, &t);
            assert_eq!(on.fill, t.accent, "{}", t.name);
            assert!(on.mark.is_some(), "{}: a lit box carries its mark", t.name);
            let off = CheckboxPaint::resolve(CheckState::Off, false, &t);
            assert_eq!(off.mark, None);
            assert_ne!(
                off.border, off.fill,
                "{}: the empty box still has an edge",
                t.name
            );
            assert_ne!(
                off.border, t.border,
                "{}: and it is firmer than a hairline",
                t.name
            );
        }
    }

    #[test]
    fn the_mark_is_readable_on_the_accent() {
        for t in palettes() {
            let on = CheckboxPaint::resolve(CheckState::On, false, &t);
            let mark = on.mark.expect("lit");
            let gap = (crate::bridge::brightness(mark) - crate::bridge::brightness(on.fill)).abs();
            assert!(gap > 0.3, "{}: mark vs accent {gap}", t.name);
        }
    }

    #[test]
    fn disabled_dims_the_label_and_the_mark() {
        for t in palettes() {
            let off = CheckboxPaint::resolve(CheckState::On, true, &t);
            assert_eq!(off.label, t.faint);
            assert_eq!(off.mark, Some(t.faint));
            assert_ne!(
                off.fill, t.accent,
                "{}: a disabled box does not engage",
                t.name
            );
        }
    }
}
