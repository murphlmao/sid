//! Motion — entrances that settle instead of snapping, and a reduced-motion switch.
//!
//! v1 left motion "deliberately not on the list" (`docs/design/2026-09-09-ui-cohesion-loop.md`).
//! The cost turned out to be real: a modal, a toast or a menu that exists in one frame and
//! not the previous reads as abrupt, and abrupt is half of what "clinical" meant. Design
//! system v2 adds the smallest motion vocabulary that fixes it:
//!
//! - **Three durations** — [`FAST`] 120ms, [`BASE`] 160ms, [`SLOW`] 200ms — all on one
//!   ease-out cubic curve ([`ease_out`]): quick to start, soft to land.
//! - **Two entrances** — [`Enter::Fade`] (opacity) and [`Enter::Rise`] (opacity plus an
//!   8px upward settle). The settle is a *relative offset*, so it never moves layout:
//!   neighbours do not jump while something arrives.
//! - **Entrances only.** `gpui` drops an element the frame it leaves the tree; animating
//!   an exit would mean retaining it in state for 160ms, which buys nothing a user would
//!   miss.
//!
//! # Reduced motion
//!
//! `gpui` carries an app-wide flag, `App::reduce_motion`, and its own `with_animation`
//! already honours it. This module is the one place sid sets it — from the
//! `SID_REDUCED_MOTION` environment variable ([`init`]) and from the Settings switch
//! ([`set_reduced`]) — and [`enter`] goes further than `gpui` does: under reduced motion it
//! does not build an animation element at all, so the element renders in its final state
//! exactly as if this module did not exist.

use std::time::Duration;

use gpui::{Animation, AnimationExt as _, AnyElement, App, ElementId, IntoElement, Styled, px};

/// Small reveals: tooltips, the switch thumb.
pub const FAST: Duration = Duration::from_millis(120);
/// Toasts, popovers, tab content.
pub const BASE: Duration = Duration::from_millis(160);
/// Modals.
pub const SLOW: Duration = Duration::from_millis(200);

/// How far a [`Enter::Rise`] travels, in design pixels.
pub const RISE_PX: f32 = 8.;

/// The environment variable that turns motion off at startup.
pub const REDUCED_MOTION_ENV: &str = "SID_REDUCED_MOTION";

/// The house easing: cubic ease-out. Fast start, soft landing.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}

/// How an element arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enter {
    /// Opacity 0 → 1.
    Fade,
    /// Opacity 0 → 1 while settling [`RISE_PX`] upward into place.
    Rise,
}

impl Enter {
    /// The (opacity, downward offset in px) for eased progress `t` in `0..=1`. Pure, so
    /// the curve is testable without a window.
    pub fn frame(self, t: f32) -> (f32, f32) {
        let t = t.clamp(0., 1.);
        match self {
            Enter::Fade => (t, 0.),
            Enter::Rise => (t, RISE_PX * (1. - t)),
        }
    }
}

/// Whether motion is currently reduced.
pub fn reduced(cx: &App) -> bool {
    cx.reduce_motion()
}

/// Turn reduced motion on or off for the whole app — the Settings switch's setter.
pub fn set_reduced(reduced: bool, cx: &mut App) {
    cx.set_reduce_motion(reduced);
}

/// Whether an environment value asks for reduced motion: anything set and non-empty
/// except `0` / `false` / `no` / `off`.
pub fn env_requests_reduced(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") => false,
        Some(v) => !matches!(
            v.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
    }
}

/// Apply `SID_REDUCED_MOTION` at startup. Only ever turns motion *off* — calling it
/// again later (it runs from `theme::install`, which re-runs on every theme switch)
/// cannot undo a Settings choice.
pub fn init(cx: &mut App) {
    if env_requests_reduced(std::env::var(REDUCED_MOTION_ENV).ok().as_deref()) {
        cx.set_reduce_motion(true);
    }
}

/// Animate `element` in with `kind` over `duration`, keyed by `id`.
///
/// `id` must be stable for as long as the element stays mounted (the animation's clock
/// lives in element state under it) and should change when the *content* is new — a
/// toast keyed by its message animates again for a new message, not on every repaint.
///
/// Under reduced motion this returns the element untouched.
pub fn enter<E>(
    id: impl Into<ElementId>,
    kind: Enter,
    duration: Duration,
    cx: &App,
    element: E,
) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    if reduced(cx) {
        return element.into_any_element();
    }
    element
        .relative()
        .with_animation(
            id,
            Animation::new(duration).with_easing(ease_out),
            move |el, t| {
                let (opacity, dy) = kind.frame(t);
                el.opacity(opacity).top(px(dy))
            },
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_durations_are_short_and_ordered() {
        assert!(FAST < BASE && BASE < SLOW);
        // Anything past ~200ms reads as waiting rather than as responsiveness.
        assert!(SLOW <= Duration::from_millis(200));
        assert!(FAST >= Duration::from_millis(100));
    }

    #[test]
    fn ease_out_starts_fast_and_lands_exactly() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        // Ease-out: past the halfway mark in the first third of the time.
        assert!(ease_out(1. / 3.) > 0.5);
        // Monotonic.
        let mut last = 0.;
        for i in 1..=20 {
            let v = ease_out(i as f32 / 20.);
            assert!(v >= last);
            last = v;
        }
        // Clamped either side.
        assert_eq!(ease_out(-1.), 0.);
        assert_eq!(ease_out(2.), 1.);
    }

    #[test]
    fn every_entrance_ends_fully_visible_and_in_place() {
        for kind in [Enter::Fade, Enter::Rise] {
            assert_eq!(kind.frame(1.), (1., 0.), "{kind:?}");
            assert_eq!(kind.frame(0.).0, 0., "{kind:?} starts invisible");
        }
        assert_eq!(
            Enter::Rise.frame(0.).1,
            RISE_PX,
            "rise starts below its place"
        );
        assert_eq!(Enter::Fade.frame(0.).1, 0., "fade never moves");
    }

    #[test]
    fn the_environment_switch_reads_like_a_boolean() {
        assert!(!env_requests_reduced(None));
        assert!(!env_requests_reduced(Some("")));
        assert!(!env_requests_reduced(Some("0")));
        assert!(!env_requests_reduced(Some("false")));
        assert!(!env_requests_reduced(Some("OFF")));
        assert!(env_requests_reduced(Some("1")));
        assert!(env_requests_reduced(Some("yes")));
        assert!(env_requests_reduced(Some(" true ")));
    }
}
