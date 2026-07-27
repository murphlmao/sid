//! Row actions — a real, bounded control at the end of a table row, and the two-step
//! confirm that guards the destructive ones.
//!
//! # What this replaces
//!
//! `systems_tab.rs`'s kill affordance was a `div()` with a text colour and a hover fill:
//! *"bare accent text floating in the void… it reads as a stray word, not a destructive
//! action — and it is a destructive action rendered with less affordance than a
//! hyperlink."* [`ActionCell`] is the slot, and [`ConfirmButton`] is a real
//! [`crate::Button`] that escalates from a quiet bounded chip to a filled `danger` one.
//!
//! # Why the arm is a type and not a `bool`
//!
//! The two-step confirm sid already had **never fired**, reproducibly. The arm lived as
//! an `Option<Pid>` on the table delegate, and the delegate's own `set_processes` reset
//! it on arrival of every refresh — which the System tab runs on a 2-second timer. Arming
//! a row and confirming it was a race against that tick, and the tick won: the armed
//! state was never even on screen long enough to see.
//!
//! So the arm has two properties that a `bool` (or a bare `Option`) does not enforce, and
//! both are tested below:
//!
//! 1. **It survives a row-set refresh.** [`ConfirmArm::retain`] is the only thing that
//!    can drop an arm on a data change, and it drops it only when the row it belongs to
//!    is *gone*. A refresh that still contains the row leaves the arm standing.
//! 2. **It is bound to a key, not to a position.** A confirm can only ever fire on the
//!    exact key it was armed with, so a table that re-sorts or re-filters between the two
//!    clicks cannot redirect a `kill` onto a different process. Pressing a *different*
//!    key re-arms rather than firing.
//!
//! It also expires on its own after [`CONFIRM_WINDOW`], because an arm left standing
//! indefinitely turns the next click anywhere near it into a live trigger. Time is a
//! parameter ([`ConfirmArm::press`] takes `now`), so the expiry is tested with a
//! synthetic clock rather than a sleep.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ClickEvent, ElementId, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled, Window, prelude::FluentBuilder as _,
};

use crate::button::{Button, ButtonSize, ButtonVariant};
use crate::icon::Icon;
use crate::styled::h_flex;

/// A press handler, shared so the builder can move it into the wrapped `Button`'s own
/// `on_click` slot without a second boxing — same shape as `button.rs`'s.
type PressHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// How long an armed confirm stays live before it disarms itself.
///
/// Long enough to read the label and move the pointer a few pixels; short enough that an
/// arm cannot survive a distraction and turn a later stray click into a kill.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(6);

/// What a press on a two-step control did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// The control is now armed. Nothing has happened yet.
    Armed,
    /// The confirm landed — do the thing.
    Fire,
}

/// A two-step confirm, bound to the key of the row it belongs to.
///
/// `K` is whatever identifies a row *independently of its position*: a pid, a connection
/// id, a path. See the module docs for why that matters.
#[derive(Clone, Debug)]
pub struct ConfirmArm<K> {
    armed: Option<(K, Instant)>,
}

impl<K: Copy + PartialEq> ConfirmArm<K> {
    /// A disarmed control.
    pub fn new() -> Self {
        Self { armed: None }
    }

    /// Register a press on `key`.
    ///
    /// Returns [`Confirm::Fire`] only when this is the second press on the *same*
    /// unexpired key; every other press arms `key` instead — including a press on a
    /// different row while one is already armed, which silently moves the arm rather
    /// than firing on either.
    pub fn press(&mut self, key: K, now: Instant) -> Confirm {
        if self.is_armed(key, now) {
            self.armed = None;
            return Confirm::Fire;
        }
        self.armed = Some((key, now));
        Confirm::Armed
    }

    /// Whether `key` is the armed row and its window is still open.
    pub fn is_armed(&self, key: K, now: Instant) -> bool {
        self.armed_key(now) == Some(key)
    }

    /// The armed key, if one is armed and unexpired.
    pub fn armed_key(&self, now: Instant) -> Option<K> {
        self.armed
            .filter(|(_, at)| now.saturating_duration_since(*at) < CONFIRM_WINDOW)
            .map(|(key, _)| key)
    }

    /// Disarm unconditionally — after the action fires by another route, or when the
    /// view the control lives on is dismissed.
    pub fn disarm(&mut self) {
        self.armed = None;
    }

    /// Keep the arm only if its row still exists.
    ///
    /// **This is the whole of what a data refresh may do to an arm.** Call it with a
    /// predicate over the new row set after replacing the rows; a refresh that still
    /// contains the armed row must leave the arm alone, or the confirm becomes a race
    /// against the refresh timer and the timer wins.
    pub fn retain(&mut self, keep: impl Fn(K) -> bool) {
        if let Some((key, _)) = self.armed
            && !keep(key)
        {
            self.armed = None;
        }
    }
}

impl<K: Copy + PartialEq> Default for ConfirmArm<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// The trailing cell of a table row: its actions, right-anchored.
///
/// Anchoring is the point. The action a row owns belongs at a predictable edge of that
/// row, not floating at whatever x-offset a fixed-width column happened to leave it at.
///
/// ```ignore
/// ActionCell::new().child(ConfirmButton::new(id, "kill").armed(armed).on_press(..))
/// ```
#[derive(IntoElement)]
pub struct ActionCell {
    children: Vec<AnyElement>,
}

impl ActionCell {
    /// An empty cell.
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for ActionCell {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for ActionCell {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ActionCell {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        // `w_full` against the column the table sized, `justify_end` to the row's edge.
        // No min-width of its own: the column width is the table's decision, and fighting
        // it is what overflows a row (see `sid_ui::table`'s `TABLE_CHROME`).
        h_flex()
            .w_full()
            .min_w_0()
            .justify_end()
            .gap_1()
            .px_2()
            .children(self.children)
    }
}

/// A destructive action that has to be asked twice.
///
/// Quiet and bounded at rest (a `Ghost` chip — still a *button*, unlike the bare word it
/// replaces); filled `danger` once armed, with a label that says what the next click
/// does. The arming itself is [`ConfirmArm`]'s job — this type only renders the state it
/// is handed, so the same widget works for a table row, a list row or a form footer.
#[derive(IntoElement)]
pub struct ConfirmButton {
    id: ElementId,
    label: SharedString,
    armed_label: SharedString,
    armed: bool,
    size: ButtonSize,
    icon: Option<Icon>,
    tooltip: Option<SharedString>,
    on_press: Option<PressHandler>,
}

impl ConfirmButton {
    /// A `Sm` two-step button labelled `label`, confirming with "confirm".
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            armed_label: "confirm".into(),
            armed: false,
            size: ButtonSize::Sm,
            icon: None,
            tooltip: None,
            on_press: None,
        }
    }

    /// Whether the first click has landed — from [`ConfirmArm::is_armed`].
    pub fn armed(mut self, armed: bool) -> Self {
        self.armed = armed;
        self
    }

    /// Override the armed label.
    pub fn armed_label(mut self, label: impl Into<SharedString>) -> Self {
        self.armed_label = label.into();
        self
    }

    /// Set the size.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// A leading icon at rest. Dropped once armed — the armed state is a word, not a
    /// picture, because that is the click that does the damage.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A tooltip naming the consequence ("send SIGTERM").
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Called on **every** press — both the arming one and the confirming one. Route it
    /// straight into [`ConfirmArm::press`] and act on [`Confirm::Fire`].
    pub fn on_press(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_press = Some(Rc::new(handler));
        self
    }

    /// The variant for the current state: a quiet bounded chip, or a filled `danger` one.
    fn variant(&self) -> ButtonVariant {
        if self.armed {
            ButtonVariant::Danger
        } else {
            ButtonVariant::Ghost
        }
    }

    /// The label for the current state.
    fn shown_label(&self) -> SharedString {
        if self.armed {
            self.armed_label.clone()
        } else {
            self.label.clone()
        }
    }
}

impl RenderOnce for ConfirmButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let variant = self.variant();
        let label = self.shown_label();
        let icon = self.icon.filter(|_| !self.armed);
        Button::new(self.id, label)
            .variant(variant)
            .size(self.size)
            .when_some(icon, Button::icon)
            .when_some(self.tooltip, Button::tooltip)
            .when_some(self.on_press, |this, handler| {
                this.on_click(move |ev, window, cx| handler(ev, window, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed reference point, so every test's clock is explicit.
    fn t0() -> Instant {
        Instant::now()
    }

    fn after(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn a_first_press_only_arms() {
        let start = t0();
        let mut arm = ConfirmArm::new();
        assert_eq!(arm.press(7u32, start), Confirm::Armed);
        assert!(arm.is_armed(7, start));
    }

    #[test]
    fn a_second_press_on_the_same_row_fires() {
        let start = t0();
        let mut arm = ConfirmArm::new();
        arm.press(7u32, start);
        assert_eq!(arm.press(7, after(start, 1)), Confirm::Fire);
        // And the arm is spent: a third press starts over rather than firing again.
        assert_eq!(arm.press(7, after(start, 2)), Confirm::Armed);
    }

    #[test]
    fn an_arm_survives_a_refresh_that_keeps_its_row() {
        // THE regression this type exists for. The System tab re-probes every 2 seconds
        // and the old code cleared the arm on every arrival, so the confirm raced the
        // refresh timer and lost — reproducibly, 2 runs out of 2. A refresh that still
        // lists the armed row must not touch the arm.
        let start = t0();
        let mut arm = ConfirmArm::new();
        arm.press(7u32, start);

        let rows_after_refresh = [3u32, 7, 11];
        arm.retain(|pid| rows_after_refresh.contains(&pid));

        assert!(arm.is_armed(7, after(start, 2)), "the tick disarmed it");
        assert_eq!(arm.press(7, after(start, 2)), Confirm::Fire);
    }

    #[test]
    fn an_arm_survives_several_refreshes() {
        let start = t0();
        let mut arm = ConfirmArm::new();
        arm.press(7u32, start);
        for _ in 0..3 {
            arm.retain(|pid| pid == 7);
        }
        assert_eq!(arm.press(7, after(start, 4)), Confirm::Fire);
    }

    #[test]
    fn an_arm_is_dropped_when_its_row_disappears() {
        // The process exited on its own between the two clicks. The arm has nothing to
        // point at, and the *next* press must arm afresh rather than fire into the gap.
        let start = t0();
        let mut arm = ConfirmArm::new();
        arm.press(7u32, start);
        arm.retain(|pid| pid != 7);
        assert!(!arm.is_armed(7, start));
        assert_eq!(arm.press(7, after(start, 1)), Confirm::Armed);
    }

    #[test]
    fn a_confirm_can_never_land_on_a_row_it_was_not_armed_for() {
        // The safety property. A live table re-sorts underneath the pointer; if the arm
        // were positional, the second click would kill whatever slid into that row.
        let start = t0();
        let mut arm = ConfirmArm::new();
        arm.press(7u32, start);
        assert!(!arm.is_armed(9, start));
        assert_eq!(
            arm.press(9, after(start, 1)),
            Confirm::Armed,
            "a press on another row fired the first row's confirm"
        );
        // Arming row 9 disarmed row 7 — one arm at a time, and it is row 9's.
        assert!(!arm.is_armed(7, after(start, 1)));
        assert!(arm.is_armed(9, after(start, 1)));
    }

    #[test]
    fn an_arm_expires_on_its_own() {
        // An arm left standing turns a later stray click into a kill.
        let start = t0();
        let mut arm = ConfirmArm::new();
        arm.press(7u32, start);
        let expired = start + CONFIRM_WINDOW;
        assert!(!arm.is_armed(7, expired));
        assert_eq!(arm.press(7, expired), Confirm::Armed);
    }

    #[test]
    fn an_arm_is_live_right_up_to_the_deadline() {
        let start = t0();
        let mut arm = ConfirmArm::new();
        arm.press(7u32, start);
        let just_inside = start + CONFIRM_WINDOW - Duration::from_millis(1);
        assert!(arm.is_armed(7, just_inside));
        assert_eq!(arm.press(7, just_inside), Confirm::Fire);
    }

    #[test]
    fn disarming_clears_the_arm() {
        let start = t0();
        let mut arm = ConfirmArm::new();
        arm.press(7u32, start);
        arm.disarm();
        assert!(!arm.is_armed(7, start));
        assert_eq!(arm.armed_key(start), None);
    }

    #[test]
    fn nothing_is_armed_to_begin_with() {
        let arm: ConfirmArm<u32> = ConfirmArm::new();
        assert_eq!(arm.armed_key(t0()), None);
        assert!(!arm.is_armed(7, t0()));
    }

    #[test]
    fn retain_on_a_disarmed_control_is_a_no_op() {
        let mut arm: ConfirmArm<u32> = ConfirmArm::new();
        arm.retain(|_| false);
        assert_eq!(arm.armed_key(t0()), None);
    }

    #[test]
    fn the_button_escalates_from_bounded_to_filled() {
        // At rest it is still a *button* (Ghost carries a hairline — see `button.rs`),
        // not the bare coloured word it replaces; armed, it is the palette's danger fill.
        let idle = ConfirmButton::new("k", "kill");
        assert_eq!(idle.variant(), ButtonVariant::Ghost);
        assert_eq!(idle.shown_label().as_ref(), "kill");

        let armed = ConfirmButton::new("k", "kill").armed(true);
        assert_eq!(armed.variant(), ButtonVariant::Danger);
        assert_eq!(armed.shown_label().as_ref(), "confirm");
    }

    #[test]
    fn the_armed_label_says_what_the_next_click_does() {
        let armed = ConfirmButton::new("k", "kill")
            .armed_label("send SIGTERM")
            .armed(true);
        assert_eq!(armed.shown_label().as_ref(), "send SIGTERM");
        assert_ne!(armed.shown_label(), armed.label);
    }
}
