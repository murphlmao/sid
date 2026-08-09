//! Text fields — [`TextInput`] and [`SearchInput`].
//!
//! # The bug this type exists to not have
//!
//! sid's own field (`crates/sid/src/ui/text_input.rs`, 946 lines) sized itself entirely
//! in percentages: its outer element declared **no width at all** and its inner box was
//! `w_full()`. Wherever a parent handed it a definite width — a modal, the System
//! toolbar — it looked fine. Wherever a parent did not, `width: 100%` of an indefinite
//! parent resolves to `auto`, `auto` resolves to the content width, and the content of
//! an empty field is nothing: the SFTP go-to-path field rendered as a **~20px stub**
//! that swallowed every click aimed at the field the user could see, and its placeholder
//! never painted at all (bug-hunt finding 3,
//! `docs/design/2026-07-26-bughunt-findings.md`).
//!
//! So the rule for the replacement, and the thing [`FieldWidth`] exists to enforce:
//!
//! > **A field declares its own width.** It may *grow* into a parent that offers one,
//! > but it never *depends* on one. Every variant puts a definite pixel floor
//! > ([`FIELD_MIN_W`]) under itself, so the worst case is a narrow field rather than an
//! > invisible one.
//!
//! The library's `Input` has the same shape of defect — its outer div is `size_full()`,
//! i.e. `width: 100%` — which is why the floor goes on sid's wrapper, and why it is
//! applied where the library cannot undo it (`Input::render` refines with its caller's
//! style last).
//!
//! # Keyboard
//!
//! Wrapping `gpui_component::input::InputState` is also what answers "support for std
//! ctrl operations": the old widget bound word *motion* and no word *deletion*, so
//! ctrl-backspace deleted a single character. The library binds the whole family on
//! Linux — `ctrl-backspace`, `ctrl-delete`, `ctrl-left`/`ctrl-right`,
//! `ctrl-shift-left`/`ctrl-shift-right`, plus `ctrl-a`/`c`/`x`/`v` and
//! `home`/`end`/`shift-home`/`shift-end`. The rule those chords follow is written down
//! and tested in [`words`].
//!
//! **Tab is sid's.** The library binds `tab`/`shift-tab` to its indent actions and then
//! installs handlers for them only in *multi-line* mode — so in a single-line field the
//! keystroke is matched, dispatched, handled by nobody, and swallowed: Tab moves focus
//! nowhere. [`TextInput`] is single-line by construction and takes those two actions on
//! its own wrapper, turning them back into [`Window::focus_next`] /
//! [`Window::focus_prev`]. Give the fields of one form increasing
//! [`TextInput::tab_index`]es, or they all sit at 0 and the order Tab visits them in is
//! whatever the frame happened to build.
//!
//! # Submitting
//!
//! [`is_field_submit`] is the same judgement `sid`'s keystroke-level helper of that name
//! makes, at the event level: **plain Enter submits, a modified Enter belongs to someone
//! else.** A field that ate every Enter chord would quietly make ctrl-Enter unavailable
//! to the screen around it — which is exactly what the DB tab runs a query with.
//! Subscribe with [`on_submit`] and the call site never has to name `gpui_component`.

pub mod words;

use gpui::{
    App, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Refineable as _, RenderOnce, SharedString, StyleRefinement, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::{
    Sizable as _,
    input::{IndentInline, Input, InputEvent, InputState, OutdentInline},
};

use crate::button::ButtonSize;
use crate::icon::Icon;
use crate::theme::{self, Theme};
use crate::typography::Typography as _;

/// The narrowest a field is ever allowed to render.
///
/// Wide enough to hold a short path, an alias or a port and still show the caret with
/// room to read what was typed; narrow enough to sit in a 620px toolbar beside a count
/// and two buttons. The number matters less than the fact that there *is* one: this is
/// the floor that turns "the parent forgot to size me" from an invisible 20px stub into
/// a small but usable field.
pub const FIELD_MIN_W: Pixels = px(160.);

/// How a field claims horizontal space.
///
/// Every variant is *definite* — see [`FieldWidth::floor`]. There is deliberately no
/// "inherit" variant: that was the old widget's only mode, and it is the bug.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum FieldWidth {
    /// Fill the line it is given, and never collapse below [`FIELD_MIN_W`]. The default:
    /// a field in a form, a modal, or a column.
    #[default]
    Fill,
    /// Take the free space of a flex row, and never collapse below [`FIELD_MIN_W`] — a
    /// toolbar filter beside a count and some buttons.
    Grow,
    /// Exactly this wide. For a field whose content has a known size (a port, a page
    /// number) that should not swing as the window resizes.
    Fixed(Pixels),
}

impl FieldWidth {
    /// The definite pixel width this variant guarantees, whatever its parent does.
    ///
    /// This is the whole decision, as a number: [`Fixed`] guarantees its own width and
    /// everything else guarantees [`FIELD_MIN_W`]. A variant that could answer "none"
    /// would be the old bug with a new spelling.
    ///
    /// [`Fixed`]: FieldWidth::Fixed
    pub const fn floor(self) -> Pixels {
        match self {
            FieldWidth::Fill | FieldWidth::Grow => FIELD_MIN_W,
            FieldWidth::Fixed(width) => width,
        }
    }

    /// Apply this width to an element: the definite floor, then how it may grow.
    ///
    /// The floor is a `min_width` rather than a `width` for [`Fill`]/[`Grow`], so a
    /// parent that *does* offer a width still wins. Declaring a width and inheriting one
    /// are not in conflict; only the first is compulsory.
    ///
    /// [`Fill`]: FieldWidth::Fill
    /// [`Grow`]: FieldWidth::Grow
    pub fn declare<S: Styled>(self, element: S) -> S {
        match self {
            FieldWidth::Fill => element.w_full().min_w(FIELD_MIN_W),
            FieldWidth::Grow => element.flex_1().min_w(FIELD_MIN_W),
            FieldWidth::Fixed(width) => element.w(width).flex_none(),
        }
    }
}

/// Build a single-line field's state.
///
/// The state is an entity and outlives any one frame, so it belongs to the view:
/// construct it once (in the view's constructor, or lazily on first render) and hand a
/// reference to [`TextInput`] each frame.
///
/// ```ignore
/// let filter = sid_ui::input::field(window, cx, "filter processes");
/// ```
pub fn field(
    window: &mut Window,
    cx: &mut App,
    placeholder: impl Into<SharedString>,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

/// Whether `event` is a **plain**-Enter submit.
///
/// The mirror of `sid`'s keystroke-level `is_field_submit`, and the same contract:
/// unmodified Enter is the field's, every modified Enter is left for whatever else wants
/// it. `secondary` is the library's name for platform-Enter (ctrl-Enter on Linux), which
/// the DB tab spends on "run this query"; a field that treated it as a submit would take
/// it away.
pub fn is_field_submit(event: &InputEvent) -> bool {
    matches!(event, InputEvent::PressEnter { secondary: false })
}

/// Run `handler` when `field` is submitted with a plain Enter.
///
/// Keep the returned [`Subscription`] alive on the view — dropping it unsubscribes,
/// which is how a field silently stops submitting.
///
/// ```ignore
/// self._filter_sub = Some(input::on_submit(&filter, window, cx, |this, _, window, cx| {
///     this.apply_filter(window, cx);
/// }));
/// ```
pub fn on_submit<V: 'static>(
    field: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<V>,
    handler: impl Fn(&mut V, &Entity<InputState>, &mut Window, &mut Context<V>) + 'static,
) -> Subscription {
    cx.subscribe_in(
        field,
        window,
        move |view, field: &Entity<InputState>, event: &InputEvent, window, cx| {
            if is_field_submit(event) {
                handler(view, field, window, cx);
            }
        },
    )
}

/// Everything about a field except which text it is holding.
///
/// Split out from [`TextInput`] so the builder's decisions — what a search field is,
/// what a bare field is not — can be asserted on without standing up an `InputState`,
/// which needs a live `Window`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FieldSpec {
    width: FieldWidth,
    size: ButtonSize,
    leading: Option<Icon>,
    clearable: bool,
    disabled: bool,
    tab_index: isize,
}

impl FieldSpec {
    /// A bare field: fills its line, no glyph, no clear.
    const fn plain() -> Self {
        Self {
            width: FieldWidth::Fill,
            size: ButtonSize::Md,
            leading: None,
            clearable: false,
            disabled: false,
            tab_index: 0,
        }
    }

    /// A filter: takes a row's slack, wears the search glyph, offers a clear.
    ///
    /// `Grow` rather than `Fill` because a filter almost always lives in a toolbar
    /// beside a count and some buttons and should take the space they do not.
    const fn search() -> Self {
        Self {
            width: FieldWidth::Grow,
            leading: Some(Icon::Search),
            clearable: true,
            ..Self::plain()
        }
    }
}

/// A single-line text field.
///
/// ```ignore
/// TextInput::new(&self.alias).width(FieldWidth::Fill).tab_index(1)
/// ```
#[derive(IntoElement)]
pub struct TextInput {
    state: Entity<InputState>,
    spec: FieldSpec,
    style: StyleRefinement,
}

impl TextInput {
    /// A `Fill`-width field over `state`.
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            spec: FieldSpec::plain(),
            style: StyleRefinement::default(),
        }
    }

    /// How this field claims width. See [`FieldWidth`].
    pub fn width(mut self, width: FieldWidth) -> Self {
        self.spec.width = width;
        self
    }

    /// Take a flex row's free space — the toolbar-filter shape.
    pub fn grow(self) -> Self {
        self.width(FieldWidth::Grow)
    }

    /// An exact width.
    pub fn fixed(self, width: Pixels) -> Self {
        self.width(FieldWidth::Fixed(width))
    }

    /// Set the size. The same two rungs [`crate::Button`] uses, so a filter and the
    /// refresh button beside it line up instead of missing each other by 8px.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.spec.size = size;
        self
    }

    /// The row/toolbar size.
    pub fn small(self) -> Self {
        self.size(ButtonSize::Sm)
    }

    /// A leading glyph inside the box, from the registry.
    pub fn leading(mut self, icon: Icon) -> Self {
        self.spec.leading = Some(icon);
        self
    }

    /// Show a clear affordance while the field has content.
    pub fn clearable(mut self, clearable: bool) -> Self {
        self.spec.clearable = clearable;
        self
    }

    /// Switch the field off: dimmed, unclickable, out of the tab order.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.spec.disabled = disabled;
        self
    }

    /// This field's place in the window's tab order. Fields in one form must be given
    /// increasing indices — left at the default, every field is index 0.
    pub fn tab_index(mut self, index: isize) -> Self {
        self.spec.tab_index = index;
        self
    }
}

impl Styled for TextInput {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for TextInput {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        paint(&self.state, self.spec, self.style, &theme)
    }
}

/// A single-line field with a leading search glyph and a clear affordance.
///
/// The filter row's field, spelled once: `SearchInput::new(&state)` instead of a
/// `TextInput` plus two setters that every screen has to remember in the same order.
#[derive(IntoElement)]
pub struct SearchInput {
    state: Entity<InputState>,
    spec: FieldSpec,
    style: StyleRefinement,
}

impl SearchInput {
    /// A `Grow`-width search field.
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            spec: FieldSpec::search(),
            style: StyleRefinement::default(),
        }
    }

    /// How this field claims width.
    pub fn width(mut self, width: FieldWidth) -> Self {
        self.spec.width = width;
        self
    }

    /// Set the size.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.spec.size = size;
        self
    }

    /// The row/toolbar size.
    pub fn small(self) -> Self {
        self.size(ButtonSize::Sm)
    }

    /// Switch the field off.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.spec.disabled = disabled;
        self
    }

    /// This field's place in the window's tab order.
    pub fn tab_index(mut self, index: isize) -> Self {
        self.spec.tab_index = index;
        self
    }
}

impl Styled for SearchInput {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for SearchInput {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        paint(&self.state, self.spec, self.style, &theme)
    }
}

/// The one render path both field types take.
fn paint(
    state: &Entity<InputState>,
    spec: FieldSpec,
    style: StyleRefinement,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let field = Input::new(state)
        .with_size(spec.size.component())
        .disabled(spec.disabled)
        .cleanable(spec.clearable)
        .tab_index(spec.tab_index)
        .when_some(spec.leading, |this, icon: Icon| {
            this.prefix(icon.small().text_color(rgb(theme.faint)))
        })
        // Fills the wrapper, which is the element that declared the definite width.
        .flex_1()
        .min_w_0()
        // The library paints an input `ThemeColor::background`, which the bridge maps to
        // the *canvas* — right for the library's own layout, wrong for sid, where a
        // field is a recess (`Elevation::Well`). `Input::render` applies its caller's
        // refinement last, so this wins.
        .bg(rgb(theme.well))
        // ...and the same for its type: the library sizes input text off its own `Size`
        // ladder, which has a 16px rung sid's scale does not.
        .text_role(spec.size.role(), theme);
    wrapper(spec.width, style).child(field)
}

/// The element that declares the width and owns Tab.
///
/// Both jobs live here rather than on the `Input` because both are about the field's
/// relationship with the screen around it: how much room it takes, and what happens when
/// the user leaves it.
fn wrapper(width: FieldWidth, style: StyleRefinement) -> Div {
    let mut wrapper = width
        .declare(div().flex().flex_row().items_center())
        // The library binds `tab`/`shift-tab` to its indent actions and installs
        // handlers for them only in multi-line mode, so in a single-line field the
        // keystroke is matched, dispatched, handled by nobody and swallowed. Taking them
        // here turns Tab back into what it means in a form.
        .on_action(|_: &IndentInline, window: &mut Window, _cx: &mut App| window.focus_next())
        .on_action(|_: &OutdentInline, window: &mut Window, _cx: &mut App| window.focus_prev());
    // Applied last, so a `.mt_2()` typed at the call site wins over the wrapper's box —
    // the same contract `Button` and `Card` offer.
    wrapper.style().refine(&style);
    wrapper
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AbsoluteLength, DefiniteLength, Length};

    /// Read back a `Div`'s refined style — the same trick `styled.rs` uses to assert on
    /// a helper without standing up a renderer.
    fn style_of(mut d: Div) -> StyleRefinement {
        d.style().clone()
    }

    /// The pixel value of a length that is *definite*, or `None` for `auto`, a
    /// percentage, or nothing at all. A percentage is deliberately not a number here:
    /// the whole bug was treating one as if it were.
    fn definite_px(length: Option<Length>) -> Option<f32> {
        match length {
            Some(Length::Definite(DefiniteLength::Absolute(AbsoluteLength::Pixels(p)))) => {
                Some(f32::from(p))
            }
            _ => None,
        }
    }

    #[test]
    fn every_width_variant_declares_a_definite_floor() {
        // THE regression this module exists for. The old field declared no width and its
        // inner box was `w_full()`; in a parent with no definite width of its own that
        // collapsed to padding + border — a ~20px stub that ate the clicks aimed at the
        // field the user could see. No variant here may resolve to "whatever the parent
        // says", so every one of them has to name a number.
        for width in [
            FieldWidth::Fill,
            FieldWidth::Grow,
            FieldWidth::Fixed(px(90.)),
        ] {
            let style = style_of(width.declare(div()));
            let floor = definite_px(style.min_size.width).or(definite_px(style.size.width));
            assert_eq!(
                floor,
                Some(f32::from(width.floor())),
                "{width:?} does not put a definite pixel floor under itself"
            );
        }
    }

    #[test]
    fn a_flexible_field_still_grows_into_a_parent_that_offers_width() {
        // Declaring a width must not mean refusing one: a filter in a wide toolbar
        // should be wide. `Fill` asks for the line, `Grow` asks for the row's slack, and
        // the floor is only what happens when neither is on offer.
        let fill = style_of(FieldWidth::Fill.declare(div()));
        assert_eq!(
            fill.size.width,
            Some(gpui::relative(1.).into()),
            "Fill takes the line when there is one"
        );
        assert_eq!(definite_px(fill.min_size.width), Some(160.));

        let grow = style_of(FieldWidth::Grow.declare(div()));
        assert_eq!(grow.flex_grow, Some(1.), "Grow takes a row's slack");
        assert_eq!(definite_px(grow.min_size.width), Some(160.));
    }

    #[test]
    fn a_fixed_field_neither_grows_nor_shrinks() {
        let style = style_of(FieldWidth::Fixed(px(90.)).declare(div()));
        assert_eq!(definite_px(style.size.width), Some(90.));
        assert_eq!(style.flex_grow, Some(0.), "fixed means fixed");
        assert_eq!(style.flex_shrink, Some(0.));
    }

    #[test]
    fn the_floor_is_the_same_number_for_every_flexible_variant() {
        // Two fields side by side must reach their floor together, or a narrowing window
        // collapses one of them first and the row reads as broken rather than tight.
        assert_eq!(FieldWidth::Fill.floor(), FIELD_MIN_W);
        assert_eq!(FieldWidth::Grow.floor(), FIELD_MIN_W);
        assert_eq!(FieldWidth::Fixed(px(42.)).floor(), px(42.));
        assert_eq!(FieldWidth::default(), FieldWidth::Fill);
    }

    #[test]
    fn the_floor_is_wide_enough_to_be_a_field() {
        // A floor that is merely non-zero would satisfy the test above and still ship
        // the bug in miniature. 160px holds a short path and a caret.
        assert!(
            f32::from(FIELD_MIN_W) >= 120.,
            "{FIELD_MIN_W:?} is a stub, not a field"
        );
    }

    #[test]
    fn the_wrapper_keeps_its_declared_floor_through_a_call_sites_style() {
        // A field is a box a layout has to be able to place, so a call site's `.mt_2()`
        // must survive — and the width the field declared must survive the call site.
        let style = style_of(wrapper(FieldWidth::Fill, style_of(div().mt_2())));
        assert!(style.margin.top.is_some(), "the call site's margin");
        assert_eq!(
            definite_px(style.min_size.width),
            Some(160.),
            "an unrelated refinement must not drop the floor"
        );
    }

    #[test]
    fn a_call_site_that_means_it_can_still_override_the_floor() {
        let narrower = style_of(wrapper(FieldWidth::Fill, style_of(div().min_w(px(60.)))));
        assert_eq!(definite_px(narrower.min_size.width), Some(60.));
    }

    #[test]
    fn only_an_unmodified_enter_submits() {
        // The same contract as `sid`'s keystroke-level helper: a field that ate every
        // Enter chord would quietly take ctrl-Enter away from the screen around it —
        // which is what the DB tab runs a query with.
        assert!(is_field_submit(&InputEvent::PressEnter {
            secondary: false
        }));
        assert!(!is_field_submit(&InputEvent::PressEnter {
            secondary: true
        }));
    }

    #[test]
    fn nothing_else_is_a_submit() {
        for event in [InputEvent::Change, InputEvent::Focus, InputEvent::Blur] {
            assert!(!is_field_submit(&event));
        }
    }

    #[test]
    fn a_search_field_grows_and_carries_its_glyph_and_its_clear() {
        // The three things a call site would otherwise have to remember, in one name.
        let search = FieldSpec::search();
        assert_eq!(search.width, FieldWidth::Grow);
        assert_eq!(search.leading, Some(Icon::Search));
        assert!(search.clearable);
    }

    #[test]
    fn a_plain_field_is_bare_until_it_is_asked() {
        let plain = FieldSpec::plain();
        assert_eq!(plain.width, FieldWidth::Fill);
        assert_eq!(plain.leading, None);
        assert!(!plain.clearable);
        assert!(!plain.disabled);
        assert_eq!(plain.tab_index, 0);
    }

    #[test]
    fn the_two_field_shapes_differ_only_where_they_should() {
        // A search field is a plain field plus three decisions — if it ever differs in a
        // fourth, that difference should be a deliberate line in this test.
        let plain = FieldSpec::plain();
        let search = FieldSpec::search();
        assert_eq!(plain.size, search.size);
        assert_eq!(plain.disabled, search.disabled);
        assert_eq!(plain.tab_index, search.tab_index);
        assert_eq!(
            search,
            FieldSpec {
                width: FieldWidth::Grow,
                leading: Some(Icon::Search),
                clearable: true,
                ..plain
            }
        );
    }
}
