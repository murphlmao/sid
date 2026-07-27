//! The button — the affordance sid did not have.
//!
//! The audit counted 113 interactive sites and zero shared button constructors; the
//! ceiling of an affordance in sid was `text_color(accent) + hover(bg)`, i.e. a coloured
//! word. This type is the replacement, and its house rule is one sentence: **a button
//! reads as a button** — a fill or a border, a hover shift, a pressed shift, a focus
//! ring, and a disabled state that still looks like a control.
//!
//! # Where the pixels come from
//!
//! [`ButtonVariant::paint`] is the whole colour decision, as a pure function of
//! (variant, state, palette). It is unit-tested against all four palettes; the render
//! path below is declarative glue over it. Nothing here reads a literal colour.
//!
//! # Why the box comes from `gpui-component` but the ink does not
//!
//! The library's `Button` carries the parts that are tedious and easy to get wrong —
//! sizing, corner radii, the tab-focus ring, tooltip plumbing, hover/active state
//! transitions — so sid wraps it rather than re-deriving them. But 0.5.1 has a bug at
//! `button/button.rs:518`: the hovered style sets `text_color(crate::red_400())`
//! unconditionally, a hard-coded Tailwind red on *every* variant. It cannot be
//! overridden from outside (gpui permits exactly one hover style per element and the
//! library installs it internally), so a hovered label would leave the palette.
//!
//! The fix is structural rather than a patch: sid never uses the library's `label`/
//! `icon` slots. Label and icon go in as **children with their own explicit token
//! colour**, which the parent's hover refinement cannot reach. That also gives sid the
//! behaviour it wants anyway — on hover the *fill* shifts and the ink holds still.
//!
//! For the same reason sid resolves `disabled`/`loading` itself instead of handing them
//! to the library: its disabled style is `fill.opacity(0.15)` with no border, which
//! dissolves the control instead of dimming it. Here a disabled button keeps its
//! hairline (see [`ButtonVariant::paint`]) and simply stops responding — no click
//! handler is installed, and it leaves the tab order.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, IntoElement, ParentElement, Refineable as _, RenderOnce,
    SharedString, StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _, px, rgb,
    transparent_black,
};
use gpui_component::{
    Sizable as _, Size,
    button::{Button as ComponentButton, ButtonCustomVariant, ButtonVariants as _},
    spinner::Spinner,
};

use crate::bridge::{contrast_ink, hover_of, mix, pressed_of};
use crate::icon::Icon;
use crate::styled::h_flex;
use crate::theme::{self, Theme};

/// A click handler, as both button types store it: shared so the builder can move it
/// into the library's own `on_click` slot without a second boxing.
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// How loudly a button asks to be pressed.
///
/// One accent, used sparingly (`.interface-design/system.md`): [`Primary`] is the
/// screen's single "engage" action, [`Danger`] is destructive, [`Secondary`] is the
/// default control chrome, and [`Ghost`] is the quiet one for dense rows.
///
/// [`Primary`]: ButtonVariant::Primary
/// [`Danger`]: ButtonVariant::Danger
/// [`Secondary`]: ButtonVariant::Secondary
/// [`Ghost`]: ButtonVariant::Ghost
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// The screen's one engage action: accent fill.
    Primary,
    /// The default: a raised `surface` chip with a hairline.
    #[default]
    Secondary,
    /// Quiet: no fill, but still a hairline and a hover fill — a ghost is a *button*
    /// with the volume down, never a coloured word.
    Ghost,
    /// Destructive: `danger` fill.
    Danger,
}

/// Every variant, for the gallery and for exhaustive tests.
pub const ALL_BUTTON_VARIANTS: &[ButtonVariant] = &[
    ButtonVariant::Primary,
    ButtonVariant::Secondary,
    ButtonVariant::Ghost,
    ButtonVariant::Danger,
];

/// The two sizes sid uses. `Sm` is for row-level and toolbar actions, `Md` for a
/// screen's primary action and for anything inside a form or a modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 24px tall, `text_xs`, 14px icon.
    Sm,
    /// 32px tall, `text_sm`, 16px icon.
    #[default]
    Md,
}

impl ButtonSize {
    /// The `gpui-component` size that carries this size's box (height, padding,
    /// corner radius).
    fn component(self) -> Size {
        match self {
            ButtonSize::Sm => Size::Small,
            ButtonSize::Md => Size::Medium,
        }
    }

    /// The square edge of an [`IconButton`] at this size.
    fn square(self) -> gpui::Pixels {
        match self {
            ButtonSize::Sm => px(24.),
            ButtonSize::Md => px(32.),
        }
    }

    /// The label's own text size. Set on the label child rather than inherited: the
    /// library's `Medium` maps to `text_base` (16px), which is a size sid's type scale
    /// does not use.
    fn text_size(self) -> gpui::Pixels {
        match self {
            ButtonSize::Sm => px(12.),
            ButtonSize::Md => px(14.),
        }
    }

    /// The leading icon's size.
    fn icon(self) -> Size {
        match self {
            ButtonSize::Sm => Size::Small,
            ButtonSize::Md => Size::Medium,
        }
    }
}

/// What a button is currently doing. Derived from the `disabled`/`loading` flags by
/// [`ButtonState::resolve`] — never set directly, so the precedence between them is
/// decided in exactly one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    /// Interactive.
    Normal,
    /// Work in flight: a spinner replaces the leading icon, clicks are dropped.
    Loading,
    /// Inert: dimmed, no hover/pressed shift, out of the tab order.
    Disabled,
}

impl ButtonState {
    /// Resolve the two flags. **Disabled wins**: a control that is switched off is not
    /// also busy, and showing a spinner on it would promise work that will never
    /// happen.
    pub fn resolve(disabled: bool, loading: bool) -> Self {
        match (disabled, loading) {
            (true, _) => ButtonState::Disabled,
            (false, true) => ButtonState::Loading,
            (false, false) => ButtonState::Normal,
        }
    }

    /// Whether clicks, hover shifts, the pointer cursor and tab focus apply.
    pub fn is_interactive(self) -> bool {
        matches!(self, ButtonState::Normal)
    }

    /// Whether the leading slot shows a spinner instead of the icon.
    pub fn shows_spinner(self) -> bool {
        matches!(self, ButtonState::Loading)
    }
}

/// The resolved colours for one (variant, state, palette). `None` means "no fill" /
/// "no border" — a ghost's rest state is genuinely transparent, not "the canvas colour"
/// (which would punch a hole through any card it sits on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonPaint {
    /// Rest fill.
    pub fill: Option<u32>,
    /// Label and icon colour. Explicit, so no ancestor hover style can move it.
    pub ink: u32,
    /// Hairline colour.
    pub border: Option<u32>,
    /// Fill under the pointer.
    pub hover_fill: Option<u32>,
    /// Fill while held down.
    pub pressed_fill: Option<u32>,
}

impl ButtonVariant {
    /// The colour decision. Pure: palette in, tokens out, no globals and no window.
    ///
    /// Rest/hover/pressed follow one rule for every variant — **hover steps toward the
    /// palette's lightest ink, pressed steps toward its darkest** — so "raised" and
    /// "pushed in" read the same way on cosmos, void, dusk and cosmos-light without a
    /// per-theme special case.
    pub fn paint(self, state: ButtonState, theme: &Theme) -> ButtonPaint {
        let normal = self.normal_paint(theme);
        match state {
            // Loading keeps its colours: the spinner is the state signal, and dimming
            // a control that is *working* reads as broken rather than busy.
            ButtonState::Normal | ButtonState::Loading => ButtonPaint {
                hover_fill: if state.is_interactive() {
                    normal.hover_fill
                } else {
                    normal.fill
                },
                pressed_fill: if state.is_interactive() {
                    normal.pressed_fill
                } else {
                    normal.fill
                },
                ..normal
            },
            // Disabled: the fill fades almost into the canvas, the ink drops to the
            // faintest visible tone, and the hairline STAYS — that outline is the only
            // thing left saying "this is a control that is currently off".
            ButtonState::Disabled => {
                let fill = normal.fill.map(|f| mix(f, theme.bg, 0.8));
                ButtonPaint {
                    fill,
                    ink: theme.faint,
                    border: Some(theme.border),
                    hover_fill: fill,
                    pressed_fill: fill,
                }
            }
        }
    }

    /// The rest colours, before any state adjustment.
    fn normal_paint(self, theme: &Theme) -> ButtonPaint {
        match self {
            ButtonVariant::Primary => filled(theme, theme.accent),
            ButtonVariant::Danger => filled(theme, theme.danger),
            // `selection`, not `surface`: a secondary button spends most of its life
            // *inside* a card, and a surface-filled control on a surface-filled card is
            // an outline with nothing in it. `selection` is one rung above both the
            // canvas and a card in every palette, so the chip reads as raised wherever
            // it lands.
            ButtonVariant::Secondary => ButtonPaint {
                fill: Some(theme.selection),
                ink: theme.fg,
                border: Some(theme.border),
                hover_fill: Some(hover_of(theme, theme.selection)),
                pressed_fill: Some(pressed_of(theme, theme.selection)),
            },
            ButtonVariant::Ghost => ButtonPaint {
                fill: None,
                ink: theme.fg,
                border: Some(theme.border),
                hover_fill: Some(theme.selection),
                pressed_fill: Some(pressed_of(theme, theme.selection)),
            },
        }
    }

    /// A human label for the gallery.
    pub fn label(self) -> &'static str {
        match self {
            ButtonVariant::Primary => "primary",
            ButtonVariant::Secondary => "secondary",
            ButtonVariant::Ghost => "ghost",
            ButtonVariant::Danger => "danger",
        }
    }
}

/// A solid swatch of `fill`, labelled in whichever ink stays readable on it.
fn filled(theme: &Theme, fill: u32) -> ButtonPaint {
    ButtonPaint {
        fill: Some(fill),
        ink: contrast_ink(theme, fill),
        border: Some(fill),
        hover_fill: Some(hover_of(theme, fill)),
        pressed_fill: Some(pressed_of(theme, fill)),
    }
}

/// A labelled button.
///
/// ```ignore
/// Button::new("refresh", "refresh")
///     .icon(Icon::Refresh)
///     .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
/// ```
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    size: ButtonSize,
    icon: Option<Icon>,
    tooltip: Option<SharedString>,
    disabled: bool,
    loading: bool,
    full_width: bool,
    style: StyleRefinement,
    on_click: Option<ClickHandler>,
}

impl Button {
    /// A `Secondary` `Md` button. `id` must be unique within the window.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            icon: None,
            tooltip: None,
            disabled: false,
            loading: false,
            full_width: false,
            style: StyleRefinement::default(),
            on_click: None,
        }
    }

    /// Set the variant.
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// The screen's one engage action.
    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }

    /// Destructive.
    pub fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }

    /// Quiet — for dense rows.
    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }

    /// Set the size.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// The row/toolbar size.
    pub fn small(self) -> Self {
        self.size(ButtonSize::Sm)
    }

    /// A leading icon, from the registry. Replaced by a spinner while loading.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A tooltip. Optional here — the label already says what this does. Required by
    /// [`IconButton`], which has no label.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Switch the button off: dimmed, unclickable, out of the tab order.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Show a spinner and drop clicks while work is in flight.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Stretch to the container's width (form footers, empty-state actions).
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// The click handler. Not installed at all when the button is disabled or loading,
    /// so "inert" is a structural property rather than a guard inside the callback.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// The resolved state of this button.
    fn state(&self) -> ButtonState {
        ButtonState::resolve(self.disabled, self.loading)
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let state = self.state();
        let paint = self.variant.paint(state, &theme);
        let size = self.size;
        let interactive = state.is_interactive();

        let button = shell(cx, self.id, &paint, size, interactive)
            .when(self.full_width, |this| this.w_full())
            .child(
                h_flex()
                    .gap_1p5()
                    .when(state.shows_spinner(), |this| {
                        this.child(
                            Spinner::new()
                                .with_size(size.icon())
                                .color(rgb(paint.ink).into()),
                        )
                    })
                    .when_some(
                        self.icon.filter(|_| !state.shows_spinner()),
                        |this, icon| {
                            this.child(icon.el().with_size(size.icon()).text_color(rgb(paint.ink)))
                        },
                    )
                    .child(
                        // Explicit colour and size on the label itself: this is what
                        // the library's hover style cannot reach (see module docs).
                        div()
                            .flex_none()
                            .text_size(size.text_size())
                            .text_color(rgb(paint.ink))
                            .child(self.label),
                    ),
            )
            .when_some(self.tooltip, |this, tip| this.tooltip(tip))
            .when_some(self.on_click.filter(|_| interactive), |this, on_click| {
                this.on_click(move |ev, window, cx| consume(ev, window, cx, &on_click))
            });
        refined(button, self.style)
    }
}

/// A square, icon-only button. **The tooltip is a constructor argument**, so an
/// unlabelled control that says nothing about itself does not compile — the fix for the
/// naked `»` / `✎` / `⟳` glyphs the SSH and Workspaces rows shipped.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: Icon,
    tooltip: SharedString,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    loading: bool,
    style: StyleRefinement,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    /// A `Ghost` `Md` icon button. There is no tooltip-less constructor: an icon is not
    /// a label, and every icon-only control in sid must name itself on hover.
    pub fn new(id: impl Into<ElementId>, icon: Icon, tooltip: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            tooltip: tooltip.into(),
            variant: ButtonVariant::Ghost,
            size: ButtonSize::default(),
            disabled: false,
            loading: false,
            style: StyleRefinement::default(),
            on_click: None,
        }
    }

    /// Set the variant.
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Destructive.
    pub fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }

    /// The screen's one engage action.
    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }

    /// Set the size.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// The row/toolbar size.
    pub fn small(self) -> Self {
        self.size(ButtonSize::Sm)
    }

    /// Switch the button off.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Show a spinner in place of the icon.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// The click handler. Not installed when disabled or loading.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Styled for IconButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let state = ButtonState::resolve(self.disabled, self.loading);
        let paint = self.variant.paint(state, &theme);
        let size = self.size;
        let interactive = state.is_interactive();
        let edge = size.square();

        let button = shell(cx, self.id, &paint, size, interactive)
            .w(edge)
            .h(edge)
            .p_0()
            .when(state.shows_spinner(), |this| {
                this.child(
                    Spinner::new()
                        .with_size(size.icon())
                        .color(rgb(paint.ink).into()),
                )
            })
            .when(!state.shows_spinner(), |this| {
                this.child(
                    self.icon
                        .el()
                        .with_size(size.icon())
                        .text_color(rgb(paint.ink)),
                )
            })
            .tooltip(self.tooltip)
            .when_some(self.on_click.filter(|_| interactive), |this, on_click| {
                this.on_click(move |ev, window, cx| consume(ev, window, cx, &on_click))
            });
        refined(button, self.style)
    }
}

/// Run a button's handler and **stop the click there**.
///
/// A button's click belongs to the button. Without this, a button inside a clickable
/// [`crate::Row`] fires both — gpui dispatches click listeners in the bubble phase,
/// child first, and `gpui-component`'s own `Button` does not stop propagation on a
/// normal click. On the SSH row that would mean "connect" opening one session from the
/// button and a second from the row underneath it.
fn consume(ev: &ClickEvent, window: &mut Window, cx: &mut App, handler: &ClickHandler) {
    cx.stop_propagation();
    handler(ev, window, cx);
}

/// The shared box: the library's button with every colour it uses replaced by a sid
/// token, and its own label/icon slots left empty (callers pass coloured children).
fn shell(
    cx: &mut App,
    id: ElementId,
    paint: &ButtonPaint,
    size: ButtonSize,
    interactive: bool,
) -> ComponentButton {
    let colour = |token: Option<u32>| token.map_or_else(transparent_black, |c| rgb(c).into());
    ComponentButton::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(colour(paint.fill))
                .foreground(rgb(paint.ink).into())
                .border(colour(paint.border))
                .hover(colour(paint.hover_fill))
                .active(colour(paint.pressed_fill))
                // Design law: depth is borders + surface shifts, never shadows.
                .shadow(false),
        )
        .with_size(size.component())
        .tab_stop(interactive)
        .when(interactive, |this| this.cursor_pointer())
}

/// Apply a caller's style refinement last, so a `.mt_2()` typed at the call site wins
/// over the shell's own box.
fn refined(mut button: ComponentButton, style: StyleRefinement) -> ComponentButton {
    button.style().refine(&style);
    button
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::brightness;
    use crate::theme::{cosmos, cosmos_light, dusk, void};

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    /// The effective background a label sits on: its own fill, or the canvas when the
    /// variant has none.
    fn backdrop(paint: &ButtonPaint, theme: &Theme) -> u32 {
        paint.fill.unwrap_or(theme.bg)
    }

    #[test]
    fn disabled_beats_loading() {
        // A switched-off control is not "busy"; a spinner on it would promise work
        // that can never start.
        assert_eq!(ButtonState::resolve(true, true), ButtonState::Disabled);
        assert_eq!(ButtonState::resolve(true, false), ButtonState::Disabled);
        assert_eq!(ButtonState::resolve(false, true), ButtonState::Loading);
        assert_eq!(ButtonState::resolve(false, false), ButtonState::Normal);
    }

    #[test]
    fn only_the_normal_state_is_interactive() {
        assert!(ButtonState::Normal.is_interactive());
        assert!(!ButtonState::Loading.is_interactive());
        assert!(!ButtonState::Disabled.is_interactive());
        assert!(ButtonState::Loading.shows_spinner());
        assert!(!ButtonState::Disabled.shows_spinner());
        assert!(!ButtonState::Normal.shows_spinner());
    }

    #[test]
    fn every_variant_reads_as_a_button() {
        // The whole point of the crate: no variant may render as bare coloured text.
        // Each one must carry a fill or a hairline, and must visibly move on hover.
        for t in palettes() {
            for &v in ALL_BUTTON_VARIANTS {
                let p = v.paint(ButtonState::Normal, &t);
                assert!(
                    p.fill.is_some() || p.border.is_some(),
                    "{}/{}: no fill and no border — that is a word, not a button",
                    t.name,
                    v.label()
                );
                assert_ne!(
                    p.hover_fill,
                    p.fill,
                    "{}/{}: no hover shift",
                    t.name,
                    v.label()
                );
                assert_ne!(
                    p.pressed_fill,
                    p.hover_fill,
                    "{}/{}: pressed matches hover",
                    t.name,
                    v.label()
                );
            }
        }
    }

    #[test]
    fn hover_lifts_and_pressed_sinks_in_every_palette() {
        // One rule for all four palettes: hover steps toward the lightest ink,
        // pressed toward the darkest. Including cosmos-light, where "lighter" and
        // "darker" mean the opposite tokens.
        for t in palettes() {
            for &v in ALL_BUTTON_VARIANTS {
                let p = v.paint(ButtonState::Normal, &t);
                let hover = brightness(p.hover_fill.expect("hover fill"));
                let pressed = brightness(p.pressed_fill.expect("pressed fill"));
                assert!(
                    pressed < hover,
                    "{}/{}: pressed ({pressed:.3}) must be darker than hover ({hover:.3})",
                    t.name,
                    v.label()
                );
            }
        }
    }

    #[test]
    fn filled_variants_use_their_semantic_token() {
        for t in palettes() {
            assert_eq!(
                ButtonVariant::Primary.paint(ButtonState::Normal, &t).fill,
                Some(t.accent),
                "{}: primary is the accent",
                t.name
            );
            assert_eq!(
                ButtonVariant::Danger.paint(ButtonState::Normal, &t).fill,
                Some(t.danger),
                "{}: danger is the danger token",
                t.name
            );
            assert_eq!(
                ButtonVariant::Secondary.paint(ButtonState::Normal, &t).fill,
                Some(t.selection),
                "{}: secondary is a raised chip",
                t.name
            );
        }
    }

    #[test]
    fn a_secondary_button_survives_being_put_on_a_card() {
        // The failure this guards: `surface` as the secondary fill makes every
        // secondary button inside a `Card` (also `surface`) an empty outline. Its fill
        // has to separate from the canvas AND from a card.
        for t in palettes() {
            let fill = ButtonVariant::Secondary
                .paint(ButtonState::Normal, &t)
                .fill
                .expect("secondary is filled");
            assert_ne!(fill, t.bg, "{}: invisible on the canvas", t.name);
            assert_ne!(fill, t.surface, "{}: invisible on a card", t.name);
        }
    }

    #[test]
    fn a_ghost_has_no_fill_but_is_still_bounded() {
        // `None`, not `theme.bg`: a ghost inside a card must not punch a canvas-
        // coloured hole through the card's surface.
        for t in palettes() {
            let p = ButtonVariant::Ghost.paint(ButtonState::Normal, &t);
            assert_eq!(p.fill, None, "{}: ghost is transparent at rest", t.name);
            assert_eq!(
                p.border,
                Some(t.border),
                "{}: ghost keeps a hairline",
                t.name
            );
            assert_eq!(
                p.hover_fill,
                Some(t.selection),
                "{}: ghost hover is the selection fill",
                t.name
            );
        }
    }

    #[test]
    fn labels_stay_readable_on_every_fill_in_every_palette() {
        // The reason ink is resolved rather than fixed: cosmos-light's `fg_strong` is
        // pure black (unreadable on its dark-red accent) and dusk's accent is a bright
        // amber that needs dark ink.
        for t in palettes() {
            for &v in ALL_BUTTON_VARIANTS {
                for state in [ButtonState::Normal, ButtonState::Loading] {
                    let p = v.paint(state, &t);
                    let delta = (brightness(p.ink) - brightness(backdrop(&p, &t))).abs();
                    assert!(
                        delta > 0.25,
                        "{}/{} ({state:?}): label contrast {delta:.2} is too low",
                        t.name,
                        v.label()
                    );
                }
            }
        }
    }

    #[test]
    fn loading_keeps_its_colours_but_stops_moving() {
        // Dimming a control that is *working* reads as broken; the spinner is the
        // signal. What loading does change is that hover and press no longer respond.
        for t in palettes() {
            for &v in ALL_BUTTON_VARIANTS {
                let normal = v.paint(ButtonState::Normal, &t);
                let loading = v.paint(ButtonState::Loading, &t);
                assert_eq!(loading.fill, normal.fill, "{}/{}", t.name, v.label());
                assert_eq!(loading.ink, normal.ink, "{}/{}", t.name, v.label());
                assert_eq!(
                    loading.hover_fill,
                    loading.fill,
                    "{}/{}: loading must not shift on hover",
                    t.name,
                    v.label()
                );
                assert_eq!(
                    loading.pressed_fill,
                    loading.fill,
                    "{}/{}: loading must not shift on press",
                    t.name,
                    v.label()
                );
            }
        }
    }

    #[test]
    fn disabled_dims_but_keeps_the_outline() {
        // The hairline is the last thing standing between "a disabled control" and
        // "nothing rendered here" — the library's own disabled style drops it, which
        // is why sid resolves this state itself.
        for t in palettes() {
            for &v in ALL_BUTTON_VARIANTS {
                let normal = v.paint(ButtonState::Normal, &t);
                let off = v.paint(ButtonState::Disabled, &t);
                assert_eq!(off.ink, t.faint, "{}/{}: faint ink", t.name, v.label());
                assert_eq!(
                    off.border,
                    Some(t.border),
                    "{}/{}: a disabled button is still bounded",
                    t.name,
                    v.label()
                );
                assert_eq!(
                    off.hover_fill,
                    off.fill,
                    "{}/{}: disabled must not react to hover",
                    t.name,
                    v.label()
                );
                assert_eq!(
                    off.pressed_fill,
                    off.fill,
                    "{}/{}: disabled must not react to press",
                    t.name,
                    v.label()
                );
                if let (Some(on), Some(off)) = (normal.fill, off.fill) {
                    let toward_canvas = (brightness(off) - brightness(t.bg)).abs()
                        < (brightness(on) - brightness(t.bg)).abs();
                    assert!(
                        toward_canvas,
                        "{}/{}: the disabled fill must fade toward the canvas",
                        t.name,
                        v.label()
                    );
                }
                // Faint is the palette's faintest *visible* tone: still legible
                // against the near-canvas disabled fill.
                let backdrop = off.fill.unwrap_or(t.bg);
                let delta = (brightness(off.ink) - brightness(backdrop)).abs();
                assert!(
                    delta > 0.15,
                    "{}/{}: disabled label vanished (contrast {delta:.2})",
                    t.name,
                    v.label()
                );
            }
        }
    }

    #[test]
    fn the_small_size_is_smaller_in_every_measure() {
        assert!(ButtonSize::Sm.square() < ButtonSize::Md.square());
        assert!(ButtonSize::Sm.text_size() < ButtonSize::Md.text_size());
        assert_eq!(ButtonSize::Sm.component(), Size::Small);
        assert_eq!(ButtonSize::Md.component(), Size::Medium);
    }
}
