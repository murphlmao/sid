//! Custom colors — the editor for [`theme::overrides`] (Settings -> Appearance).
//!
//! One row per [`COLOR_TOKENS`] entry: a swatch that opens `gpui-component`'s colour
//! picker, the token's label, a hex field and — only while the token is overridden — a
//! reset. A valid edit is applied to the whole app at once through the same choke point
//! as a palette switch ([`theme::set_overrides`] -> [`theme::install`], then
//! [`crate::bridge::sync`]); an invalid one marks its row and changes nothing.
//!
//! The editor applies; the caller persists. [`CustomColors::on_commit`] fires with the
//! full override map whenever a change settles — a hex field submitted or left, a reset,
//! a palette swatch picked, or the picker closed after a slider drag (a drag applies on
//! every step, but is written once).
//!
//! The rows' field and picker states are entities, and this element is stateless, so
//! they live in gpui's element state (`Window::use_keyed_state`, the trick [`crate::Modal`]
//! uses): they are built fresh, from the live theme, each time the editor is mounted, and
//! dropped when it is not rendered.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, Hsla, IntoElement,
    ParentElement as _, Render, RenderOnce, Styled as _, Subscription, Window, canvas, div,
    prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{Sizable as _, Size};

use crate::button::Button;
use crate::card::Card;
use crate::grid::fit_columns;
use crate::input::TextInput;
use crate::scale::BASE_REM_PX;
use crate::styled::{StyledExt as _, focus_halo, h_flex, v_flex};
use crate::theme::{self, COLOR_GROUPS, COLOR_TOKENS, Theme, parse_hex, to_hex};
use crate::typography::Typography as _;

/// The narrowest a group column may be laid out at (design px): swatch, a label like
/// "Secondary text", a reset and the hex field, without the label eliding.
const GROUP_MIN_W: f32 = 340.;
/// Widest a group column grows, so a label and its hex field stay a glance apart.
const GROUP_MAX_W: f32 = 400.;

/// How a settled override map reaches the caller.
type CommitHandler = Rc<dyn Fn(&BTreeMap<String, String>, &mut Window, &mut App)>;

/// The custom-colors card. `id` keys the editor's state; keep it stable.
#[derive(IntoElement)]
pub struct CustomColors {
    id: ElementId,
    on_commit: Option<CommitHandler>,
}

impl CustomColors {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            on_commit: None,
        }
    }

    /// Persist `overrides` (already validated and normalised). Called once per settled
    /// change, never per keystroke or per drag step.
    pub fn on_commit(
        mut self,
        handler: impl Fn(&BTreeMap<String, String>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_commit = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for CustomColors {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let editor = window.use_keyed_state(self.id, cx, Editor::new);
        editor.update(cx, |editor, _| editor.on_commit = self.on_commit);
        editor
    }
}

/// One token's live controls.
struct TokenRow {
    field: Entity<InputState>,
    picker: Entity<ColorPickerState>,
    /// The field holds text that is not a colour. Kept (and not overwritten by a theme
    /// change) until the user edits it, resets the token, or resets all.
    invalid: bool,
}

struct Editor {
    rows: Vec<TokenRow>,
    on_commit: Option<CommitHandler>,
    /// A slider drag has been applied live but not yet committed.
    unsaved: bool,
    /// How many group columns the card's last measured width fits.
    columns: usize,
    _subscriptions: Vec<Subscription>,
}

impl Editor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let t = theme::active(cx).clone();
        let mut subscriptions = vec![cx.observe_global_in::<Theme>(window, Self::sync)];
        let rows = COLOR_TOKENS
            .iter()
            .enumerate()
            .map(|(ix, token)| {
                let color = token.get(&t);
                let field = cx.new(|cx| InputState::new(window, cx).default_value(to_hex(color)));
                let picker =
                    cx.new(|cx| ColorPickerState::new(window, cx).default_value(rgb(color)));
                subscriptions.push(cx.subscribe_in(
                    &field,
                    window,
                    move |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } | InputEvent::Blur => {
                            this.submit(ix, window, cx)
                        }
                        InputEvent::Change if this.rows[ix].invalid => {
                            this.rows[ix].invalid = false;
                            cx.notify();
                        }
                        _ => {}
                    },
                ));
                subscriptions.push(cx.subscribe_in(
                    &picker,
                    window,
                    move |this, _, event: &ColorPickerEvent, window, cx| {
                        if let ColorPickerEvent::Change(Some(color)) = event {
                            this.rows[ix].invalid = false;
                            this.set(ix, Some(token_of(*color)), window, cx);
                        }
                    },
                ));
                // Closing the picker settles a drag.
                subscriptions.push(cx.observe_in(&picker, window, |this, picker, window, cx| {
                    if this.unsaved && !picker.read(cx).is_open() {
                        this.commit(window, cx);
                    }
                }));
                TokenRow {
                    field,
                    picker,
                    invalid: false,
                }
            })
            .collect();
        Self {
            rows,
            on_commit: None,
            unsaved: false,
            columns: COLOR_GROUPS.len(),
            _subscriptions: subscriptions,
        }
    }

    /// The hex field was submitted or left.
    fn submit(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.rows[ix].field.read(cx).value();
        let Some(color) = parse_hex(&text) else {
            self.rows[ix].invalid = true;
            cx.notify();
            return;
        };
        self.rows[ix].invalid = false;
        if color == COLOR_TOKENS[ix].get(theme::active(cx)) {
            // Unchanged (or re-typed in another case): tidy the text, keep the palette's
            // value un-pinned.
            self.sync(window, cx);
        } else {
            self.set(ix, Some(color), window, cx);
        }
    }

    /// Override token `ix` with `color`, or drop its override.
    fn set(&mut self, ix: usize, color: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        let mut overrides = theme::overrides(cx).clone();
        let key = COLOR_TOKENS[ix].key.to_string();
        match color {
            Some(color) => overrides.insert(key, to_hex(color)),
            None => overrides.remove(&key),
        };
        self.apply(&overrides, window, cx);
    }

    fn reset_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rows.iter_mut().for_each(|row| row.invalid = false);
        self.apply(&BTreeMap::new(), window, cx);
    }

    /// Apply `overrides` to the whole app now; commit unless a picker is mid-drag.
    fn apply(
        &mut self,
        overrides: &BTreeMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme::set_overrides(overrides, cx);
        crate::bridge::sync(None, cx);
        cx.refresh_windows();
        if self.rows.iter().any(|row| row.picker.read(cx).is_open()) {
            self.unsaved = true;
        } else {
            self.commit(window, cx);
        }
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.unsaved = false;
        let overrides = theme::overrides(cx).clone();
        if let Some(on_commit) = self.on_commit.clone() {
            on_commit(&overrides, window, cx);
        }
    }

    /// Bring every row's field and swatch in line with the live theme — after an edit, a
    /// reset, or a palette switch made elsewhere.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = theme::active(cx).clone();
        for (row, token) in self.rows.iter().zip(&COLOR_TOKENS) {
            let color = token.get(&t);
            if !row.invalid {
                row.field.update(cx, |field, cx| {
                    let hex = to_hex(color);
                    if field.value().as_ref() != hex.as_str() {
                        field.set_value(hex, window, cx);
                    }
                });
            }
            row.picker.update(cx, |picker, cx| {
                // Never under an open picker: it is the source of the change, and writing
                // its sliders back would fight the drag in progress.
                if !picker.is_open() && picker.value().map(token_of) != Some(color) {
                    picker.set_value(rgb(color), window, cx);
                }
            });
        }
        cx.notify();
    }

    fn row(
        &self,
        ix: usize,
        t: &Theme,
        base: &Theme,
        overridden: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let token = &COLOR_TOKENS[ix];
        let row = &self.rows[ix];
        let picker_focused = row.picker.read(cx).focus_handle(cx).is_focused(window);
        let note = if row.invalid {
            Some((t.danger, "Use a hex color, like #7c6cf2"))
        } else if theme::low_contrast(t, token.key) {
            Some((t.warning, "Hard to read on the background"))
        } else {
            None
        };
        h_flex()
            .gap_2()
            .py_1()
            .child(
                // A `faint` ring so a near-black swatch still reads on a dark card, and the
                // accent when the swatch has keyboard focus (the library's trigger draws no
                // focus state of its own).
                div()
                    .flex_none()
                    .p_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if picker_focused { t.accent } else { t.faint }))
                    .when(picker_focused, |this| this.shadow(focus_halo(t)))
                    .child(
                        ColorPicker::new(&row.picker)
                            .with_size(Size::Medium)
                            .accessibility_label(token.label),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .min_w(px(0.))
                            .clamp_one_line()
                            .text_body(t)
                            .child(token.label),
                    )
                    .children(note.map(|(ink, note)| {
                        div()
                            .min_w(px(0.))
                            .clamp_one_line()
                            .text_meta(t)
                            .text_color(rgb(ink))
                            .child(note)
                    })),
            )
            .when(overridden, |this| {
                this.child(
                    // Base palette, like "Reset all": the row most in need of its reset
                    // (text set to the background colour) is the one whose own ink
                    // just vanished.
                    Button::new(("custom-color-reset", ix), "Reset")
                        .small()
                        .palette(base.clone())
                        .tooltip(format!(
                            "Back to the palette's {}",
                            token.label.to_lowercase()
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.rows[ix].invalid = false;
                            this.set(ix, None, window, cx);
                        })),
                )
            })
            .child(
                TextInput::new(&row.field)
                    .test_id(token.key)
                    .fixed(px(112.))
                    .when(row.invalid, |this| {
                        this.rounded_lg().border_1().border_color(rgb(t.danger))
                    }),
            )
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx).clone();
        let overrides = theme::overrides(cx).clone();
        let base = theme::base(cx);
        // Drawn in the base palette, not the live one: whatever the user has done to the
        // colours (text == background), the way back stays legible.
        let reset_all = Button::new("custom-colors-reset-all", "Reset all")
            .small()
            .palette(base.clone())
            .disabled(overrides.is_empty())
            .on_click(cx.listener(|this, _, window, cx| this.reset_all(window, cx)));
        let groups: Vec<_> = COLOR_GROUPS
            .iter()
            .map(|&group| {
                let rows: Vec<_> = (0..COLOR_TOKENS.len())
                    .filter(|&ix| COLOR_TOKENS[ix].group == group)
                    .map(|ix| {
                        let overridden = overrides.contains_key(COLOR_TOKENS[ix].key);
                        self.row(ix, &t, &base, overridden, window, cx)
                    })
                    .collect();
                // Capped: on a wide card an uncapped column parks each hex field a
                // thousand px from its label.
                v_flex()
                    .min_w(px(0.))
                    .max_w(px(GROUP_MAX_W))
                    .child(div().pb_1().text_label(&t).child(group))
                    .children(rows)
            })
            .collect();
        // One column per group that fits at `GROUP_MIN_W`, measured off the card's own
        // width (a probe, the way `FillTable` sizes its columns), so a mid-width window
        // gets two aligned columns rather than two plus a stretched straggler.
        let editor = cx.entity();
        let probe = canvas(
            move |bounds, window, cx| {
                let width =
                    f32::from(bounds.size.width) * BASE_REM_PX / f32::from(window.rem_size());
                let columns = fit_columns(width, GROUP_MIN_W, COLOR_GROUPS.len());
                let moved = editor.update(cx, |editor, _| {
                    std::mem::replace(&mut editor.columns, columns) != columns
                });
                if moved {
                    // `refresh` is a no-op mid-draw; the next frame picks the count up.
                    window.on_next_frame(|window, _| window.refresh());
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        Card::new()
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap_1()
                            .child(div().text_heading(&t).child("Custom colors"))
                            .child(div().text_meta(&t).child(
                                "Paint over the palette. Your colors stay when you switch palettes.",
                            )),
                    )
                    .child(div().flex_none().child(reset_all)),
            )
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .grid()
                            .grid_cols(self.columns as u16)
                            .gap_x_4()
                            .gap_y_3()
                            .children(groups),
                    )
                    .child(probe),
            )
    }
}

/// A picker colour as a `0xRRGGBB` token (alpha dropped: tokens are opaque).
fn token_of(color: Hsla) -> u32 {
    let rgba = gpui::Rgba::from(color);
    let channel = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u32;
    (channel(rgba.r) << 16) | (channel(rgba.g) << 8) | channel(rgba.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_survives_the_pickers_hsla_round_trip() {
        // The swatch is fed `rgb(token)` and hands back an `Hsla`; if that round trip
        // drifted, opening and closing the picker would quietly pin a near-identical
        // override.
        for t in [
            theme::cosmos(),
            theme::void(),
            theme::dusk(),
            theme::cosmos_light(),
        ] {
            for token in &COLOR_TOKENS {
                let color = token.get(&t);
                assert_eq!(token_of(rgb(color).into()), color, "{}", token.key);
            }
        }
    }
}
