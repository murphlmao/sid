//! Modals — the scrim, the panel, and the one place both are decided.
//!
//! sid had three modals (the host form, the DB connection form, the connect-time
//! password prompt) and **six** copies of what a modal *is*: `app.rs` repeated the same
//! eighteen-line `deferred(anchored(..))` scrim three times, and each form re-typed the
//! same panel — `w(px(460.))`, `p_4()`, `rounded_lg()`, a `surface` fill, a hairline, a
//! title row, a hint line, a right-aligned button pair. Nothing enforced that they
//! matched, and they had already drifted (two widths, two hint strings, three different
//! button spellings).
//!
//! This module is that shape, typed once, in two pieces:
//!
//! - [`overlay`] — the **host**: a viewport-filling, click-occluding scrim that centres
//!   its child and paints above everything else. It owns [`crate::bridge::SCRIM`], so no
//!   call site needs the one exempt hex literal in the design system.
//! - [`Modal`] — the **panel**: a header (title, dismiss), a body that scrolls when the
//!   fields outrun the window, and a footer carrying the keyboard contract on the left
//!   and the action buttons on the right.
//!
//! The split is deliberate. A modal in sid is an `Entity` its owner holds (`AppState`
//! subscribes to it and closes it on an event); the entity renders the panel, and the
//! owner wraps it in the host. That keeps `Modal` a plain [`RenderOnce`] element with no
//! lifecycle of its own.
//!
//! # What the panel deliberately does not own
//!
//! - **Escape.** Each form binds `escape` in its own `key_context` and emits its own
//!   cancel event (see `sid::ui::init`). Routing Esc through the element would mean the
//!   element owning a key context, an action and a focus handle — i.e. being the form.
//! - **Focus restore.** The owner refocuses `AppState::root_focus` when it drops the
//!   entity (`.interface-design/system.md`: *"Modals close on Esc and MUST refocus
//!   `AppState::root_focus`"*), which is the only place that still has a live handle to
//!   restore to: by the time the panel is gone, its fields' handles are gone with it, and
//!   a stale focus target silently kills all further keyboard dispatch.
//! - **Dismiss on scrim click.** [`overlay`] does not install one. Every modal sid has
//!   holds typed input, and losing a half-filled host form to a stray click on the
//!   background is worse than needing Esc. The dismiss affordance is the header's close
//!   button, which is explicit.
//!
//! # Why this is not `gpui_component::Dialog`
//!
//! 0.5.1's `Dialog` is a good widget for a different architecture: it is pushed onto a
//! `Root` dialog layer via `window.open_dialog(..)`, mints its own focus handle on open,
//! binds `escape`/`enter` in its own `Dialog` key context, renders its own OK/Cancel
//! buttons from a `DialogButtonProps`, and paints a `BoxShadow` — which the design
//! system forbids outright ("Borders + surface shifts only. No shadows."). Adopting it
//! would mean giving up the `Entity` + `EventEmitter` ownership every sid form is built
//! on, re-deriving Tab traversal against a foreign focus handle, and losing the
//! `root_focus` restore. The parts of it worth having — the centred box, the scrim, the
//! occlusion — are the twenty lines below.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement, Pixels,
    Refineable as _, RenderOnce, SharedString, Size, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, anchored, deferred, div, point, prelude::FluentBuilder as _,
    px, rgba,
};

use crate::bridge::SCRIM;
use crate::button::{ButtonSize, IconButton};
use crate::elevation::Elevation;
use crate::icon::Icon;
use crate::kbd::Kbd;
use crate::styled::{StyledExt as _, h_flex, v_flex};
use crate::theme::{self, Theme};
use crate::typography::Typography;

/// The dismiss handler, shared so the builder can move it into the header button
/// without a second boxing. Same shape as [`crate::Button`]'s.
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The default panel width. The host form's, which every other modal had copied by eye.
const DEFAULT_WIDTH: f32 = 460.;

/// The clearance the scrim keeps between a panel and the window edge. A modal that
/// touches the edge reads as a docked pane, not as something floating over the app.
const GUTTER: f32 = 24.;

/// The share of the window height a panel may take before its body starts scrolling.
/// The remainder is what keeps the scrim visible above and below, so the modal still
/// reads as *over* the app on a short window.
const HEIGHT_SHARE: f32 = 0.9;

/// Paint order: the scrim must land above every element in the tree it is deferred out
/// of, including other deferred popovers (menus, the GPU badge's popover).
const OVERLAY_PRIORITY: usize = 1;

/// The narrowest panel that still prints the footer's keyboard hint.
///
/// Calibrated against the widest contract sid states — `Esc cancels · Enter connects`
/// (~210px of chips and words) beside a Cancel/Connect button pair (~160px) inside the
/// footer's 32px of padding. Below this the two meet in the middle, which is what a
/// 300px panel in the gallery looked like: `EnterCancel`.
const KEY_HINT_MIN_WIDTH: f32 = 440.;

/// Whether a panel this wide has room for the footer's keyboard hint beside its buttons.
///
/// The hint is the most droppable thing in the panel — Escape and Enter keep working,
/// and the buttons still say what they do — so a narrow modal loses it rather than
/// overlapping its own footer. A heuristic, deliberately: an element cannot measure the
/// button labels a caller has not built yet.
pub fn shows_key_hint(width: Pixels) -> bool {
    width >= px(KEY_HINT_MIN_WIDTH)
}

/// The box a panel gets inside a given window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelGeometry {
    /// The panel's width.
    pub width: Pixels,
    /// The tallest the panel may render; past it, the body scrolls.
    pub max_height: Pixels,
}

/// Fit a panel of `preferred` width into `viewport`.
///
/// The decision, and the two failures it exists to prevent:
///
/// - **A 460px panel in a 400px window** used to render 30px off each edge, with the
///   `save to:` radio labels clipped. The panel gives up its own width before it gives
///   up the [`GUTTER`].
/// - **A tall form in a short window** (the DB connection form on Postgres is a name, an
///   engine selector, five descriptor fields, a save-to picker, a notice and a footer)
///   used to run off the bottom of the window, taking the Save button with it. Capping
///   the height is what makes the body scrollable instead of unreachable.
///
/// In a window narrower than the gutters themselves the panel takes the full width: a
/// clamped-to-nothing modal would be invisible, which is a worse answer than a tight one.
pub fn panel_geometry(preferred: Pixels, viewport: Size<Pixels>) -> PanelGeometry {
    let available = viewport.width - px(GUTTER * 2.);
    let width = if available > px(0.) {
        preferred.min(available)
    } else {
        viewport.width
    };
    PanelGeometry {
        width,
        max_height: viewport.height * HEIGHT_SHARE,
    }
}

/// The scrim host: fills the window, swallows clicks aimed at the app underneath, and
/// centres `child` over it.
///
/// `anchored` pins it at the window origin regardless of where in the tree it is built;
/// `deferred` paints it after (and therefore above) everything else in the frame.
///
/// ```ignore
/// // in AppState::render
/// .children(self.form.clone().map(|form| modal::overlay(window, form)))
/// ```
///
/// The `use<C>` bound is load-bearing: without it the returned element would capture the
/// `&Window` lifetime and pin the borrow for the rest of the caller's `render`, which in
/// `app.rs` means no other overlay may be built afterwards. Nothing here holds the
/// window — only its measured size.
pub fn overlay<C: IntoElement>(window: &Window, child: C) -> impl IntoElement + use<C> {
    let viewport = window.viewport_size();
    deferred(
        anchored().position(point(px(0.), px(0.))).child(
            div()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .w(viewport.width)
                .h(viewport.height)
                .bg(rgba(SCRIM))
                .child(child),
        ),
    )
    .with_priority(OVERLAY_PRIORITY)
}

/// A modal panel: header, body, footer.
///
/// ```ignore
/// Modal::new("host-form", "Add host")
///     .submit_hint("saves")
///     .on_dismiss(cx.listener(|_, _, _, cx| cx.emit(HostFormEvent::Cancel)))
///     .child(self.field("alias", &self.alias, cx))
///     .footer(Button::new("host-form-cancel", "Cancel").ghost())
///     .footer(Button::new("host-form-save", "Save").primary())
/// ```
#[derive(IntoElement)]
pub struct Modal {
    id: SharedString,
    title: SharedString,
    submit_verb: Option<SharedString>,
    width: Pixels,
    on_dismiss: Option<ClickHandler>,
    footer: Vec<AnyElement>,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

impl Modal {
    /// A panel titled `title`. `id` must be unique within the window — the body's scroll
    /// state and the dismiss button are keyed off it.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            submit_verb: None,
            width: px(DEFAULT_WIDTH),
            on_dismiss: None,
            footer: Vec::new(),
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    /// Advertise the keyboard contract as `Esc cancels · Enter {verb}` along the footer's
    /// left edge, in real key chips rather than the prose each form used to spell for
    /// itself.
    ///
    /// The verb is the caller's because it is the caller's action: the forms save, the
    /// password prompt connects.
    pub fn submit_hint(mut self, verb: impl Into<SharedString>) -> Self {
        self.submit_verb = Some(verb.into());
        self
    }

    /// Override the panel width. Rarely needed — a modal that wants to be wider than
    /// the default usually wants fewer fields.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Add a close button to the header, wired to `handler`.
    ///
    /// Wire it to the same event Escape emits: two spellings of "dismiss" that behave
    /// differently is exactly the drift this crate exists to stop.
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// A control in the footer, right-aligned. Repeatable; they render in the order
    /// added, so the primary action goes last (rightmost).
    pub fn footer(mut self, action: impl IntoElement) -> Self {
        self.footer.push(action.into_any_element());
        self
    }
}

/// A panel is a box its content may need to resize — a form that grows a field list, a
/// prompt that is narrower than the default. Applied last, so a call site wins over the
/// panel's own geometry.
impl Styled for Modal {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Modal {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Modal {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let geometry = panel_geometry(self.width, window.viewport_size());
        let body_id = SharedString::from(format!("{}-body", self.id));
        let dismiss_id = SharedString::from(format!("{}-dismiss", self.id));
        let hint = self.submit_verb.filter(|_| shows_key_hint(geometry.width));

        // Title and close button only. The keyboard contract used to live up here beside
        // the title and, at a 300px panel, squeezed "Add host" down to "Add …" — the
        // hint is 60% of the header's width and the least important thing in it. It
        // belongs next to the buttons it describes anyway.
        let header = h_flex()
            .w_full()
            .flex_none()
            .gap_3()
            .px_4()
            .py_3()
            .hairline_b(&theme)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .clamp_one_line()
                    .text_title(&theme)
                    .child(self.title),
            )
            .when_some(self.on_dismiss, |this, on_dismiss| {
                this.child(
                    IconButton::new(dismiss_id, Icon::Close, "close")
                        .size(ButtonSize::Sm)
                        .on_click(move |ev, window, cx| on_dismiss(ev, window, cx)),
                )
            });

        // `min_h_0` is what lets the body shrink below its content height inside the
        // panel's `max_h`; without it flexbox refuses, the panel overflows, and the
        // footer leaves the window. No `flex_1`: the panel hugs short content instead of
        // always standing 90% of the window tall.
        let body = v_flex()
            .id(body_id)
            .w_full()
            .min_h_0()
            .gap_3()
            .p_4()
            .overflow_y_scroll()
            .children(self.children);

        let mut panel = v_flex()
            .w(geometry.width)
            .max_h(geometry.max_height)
            .elevation(Elevation::Overlay, &theme)
            .rounded_lg()
            // The panel states its own type: a modal is mounted from wherever the app
            // happened to be painting, and the fields inside it must not inherit that.
            .text_body(&theme)
            .child(header)
            .child(body)
            // Footer: the keyboard contract on the left, the buttons on the right — so
            // "Enter saves" sits beside the Save button rather than a panel away from it.
            .when(!self.footer.is_empty() || hint.is_some(), |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .flex_none()
                        .justify_between()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .hairline_t(&theme)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .when_some(hint, |this, verb| this.child(key_hint(&theme, verb))),
                        )
                        .child(h_flex().flex_none().gap_2().children(self.footer)),
                )
            });
        panel.style().refine(&self.style);
        panel
    }
}

/// `Esc cancels · Enter {verb}` — the keyboard contract as chips, not prose.
///
/// The three forms each spelled this by hand (`"esc cancels · enter saves"`), so the
/// separator, the casing and the key names were three independent decisions. `Kbd`
/// carries the platform key-name table, so this reads `Esc`/`Enter` on Linux and
/// `⎋`/`⏎` on macOS without a second string.
fn key_hint(theme: &Theme, verb: SharedString) -> impl IntoElement + use<> {
    h_flex()
        .flex_none()
        .gap_1()
        .child(Kbd::new("escape"))
        .child(div().text_meta(theme).child("cancels"))
        .child(div().text_meta(theme).child("·"))
        .child(Kbd::new("enter"))
        .child(div().text_meta(theme).child(verb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn viewport(w: f32, h: f32) -> Size<Pixels> {
        size(px(w), px(h))
    }

    #[test]
    fn a_roomy_window_gives_the_panel_exactly_what_it_asked_for() {
        let g = panel_geometry(px(DEFAULT_WIDTH), viewport(1920., 1080.));
        assert_eq!(g.width, px(DEFAULT_WIDTH));
        assert_eq!(g.max_height, px(1080. * HEIGHT_SHARE));
    }

    #[test]
    fn a_narrow_window_shrinks_the_panel_and_keeps_the_gutter() {
        // The failure: a 460px panel in a 400px window rendered 30px off each edge with
        // the save-to labels clipped.
        let g = panel_geometry(px(DEFAULT_WIDTH), viewport(400., 800.));
        assert_eq!(g.width, px(400. - GUTTER * 2.));
        assert!(g.width < px(DEFAULT_WIDTH));
    }

    #[test]
    fn the_panel_never_touches_the_window_edge() {
        for w in [300., 480., 520., 1024., 2000.] {
            let g = panel_geometry(px(DEFAULT_WIDTH), viewport(w, 800.));
            assert!(
                g.width <= px(w - GUTTER * 2.),
                "{w}px window: panel {:?} leaves no gutter",
                g.width
            );
        }
    }

    #[test]
    fn a_window_narrower_than_the_gutters_still_renders_a_panel() {
        // Pathological, but a modal clamped to zero width is invisible, and an invisible
        // modal is a hang: the app is waiting on input the user cannot see.
        let g = panel_geometry(px(DEFAULT_WIDTH), viewport(40., 40.));
        assert_eq!(g.width, px(40.));
        assert!(g.width > px(0.));
    }

    #[test]
    fn the_scrim_always_shows_above_and_below_the_panel() {
        // The height cap is what keeps a modal reading as *over* the app rather than as
        // a docked pane, and it is what makes the body scroll instead of pushing the
        // footer off-window.
        for h in [400., 720., 1080., 2160.] {
            let g = panel_geometry(px(DEFAULT_WIDTH), viewport(1920., h));
            assert!(g.max_height < px(h), "{h}px window: panel fills it");
            assert!(g.max_height > px(h / 2.), "{h}px window: panel is cramped");
        }
    }

    #[test]
    fn a_wider_panel_is_honoured_when_it_fits() {
        let g = panel_geometry(px(720.), viewport(1920., 1080.));
        assert_eq!(g.width, px(720.));
    }

    #[test]
    fn the_keyboard_hint_yields_before_the_footer_collides() {
        // The failure, seen in the gallery at a 300px panel: the hint and the button
        // row overlapped into `EnterCancel`.
        assert!(shows_key_hint(px(DEFAULT_WIDTH)), "the form width shows it");
        assert!(shows_key_hint(px(KEY_HINT_MIN_WIDTH)), "inclusive bound");
        assert!(!shows_key_hint(px(380.)), "a one-field prompt drops it");
        assert!(!shows_key_hint(px(300.)));
    }

    #[test]
    fn a_window_too_narrow_for_the_panel_also_drops_the_hint() {
        // The hint decision reads the *resolved* width, not the requested one: a 460px
        // form in a 400px window is a 352px panel, and 352px has no room either.
        let g = panel_geometry(px(DEFAULT_WIDTH), viewport(400., 800.));
        assert!(!shows_key_hint(g.width));
    }

    #[test]
    fn a_modal_takes_a_style_refinement() {
        let mut modal = Modal::new("m", "Title").w(px(300.));
        assert!(modal.style().size.width.is_some());
    }
}
