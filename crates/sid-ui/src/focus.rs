//! Tab traversal that cannot leave an open modal.
//!
//! # The defect
//!
//! [`gpui::Window::focus_next`] walks the window's **flat** tab-stop list and wraps at
//! its ends, so Tab off a modal's last field lands on whatever the background painted
//! next — SSH home's quick-connect box, Database's result filter — with the scrim still
//! down and the field invisible under it. gpui's `tab_group` does not fix that: reading
//! `gpui-pre-0.3.4/src/tab_stop.rs`, a group is purely a **renumbering**. It gives its
//! children a deeper sort path so they order together; `next()` still runs straight out
//! the far side into the next path.
//!
//! # The correction, and why it lives here
//!
//! `gpui-component` already ships the missing half: `FocusTrapElement::focus_trap`
//! registers a container, and `Root`'s `Tab`/`TabPrev` handlers step focus and then keep
//! stepping while it sits outside the innermost registered trap. [`crate::Modal`]
//! registers itself as one, so every control that lets the keystroke reach the `Root`
//! action — every [`crate::Button`], every focusable row — is trapped for free.
//!
//! **One control does not let it reach the Root**: [`crate::TextInput`]. The library
//! binds `tab`/`shift-tab` to its own indent actions inside the input's key context and
//! then installs handlers for them only in *multi-line* mode, so a single-line field
//! swallows the keystroke; sid's wrapper takes those two actions back and turns them
//! into traversal (see `crate::input`). That is a second, private Tab path, and it was
//! the one that escaped — a modal is nothing but fields. Routing it through [`next`] /
//! [`prev`] is what makes the trap total.

use gpui::{App, Window};

/// Register an element as a focus trap: `element.focus_trap(id, &handle)`.
///
/// Re-exported so a hand-rolled overlay in the `sid` crate — the config editor is the one
/// that is not a [`crate::Modal`] — can trap focus without naming `gpui_component`
/// itself. Everything built on `Modal` gets this for free and should not touch it.
pub use gpui_component::FocusTrapElement;

/// The most stops a trap cycle steps through before giving up.
///
/// A lap of the window's tab ring is the real bound and [`cycle_done`]'s
/// `back_at_start` catches it; this is the backstop for the pathological case — a
/// registered trap whose container holds no tab stop at all — where "back where we
/// started" never becomes true because focus started outside the ring.
const MAX_CYCLE_STEPS: usize = 200;

/// Whether a trap cycle has finished stepping. Pure, so the three escape conditions are
/// testable without standing up a window.
///
/// It stops the moment focus is back inside the trap; failing that it gives up rather
/// than spinning, either because a full lap brought focus back where it started or
/// because it has spent its step budget. Giving up leaves focus wherever the last step
/// put it, which is the same place plain `focus_next` would have left it — a trap that
/// hangs the app is worse than a trap that leaks.
fn cycle_done(inside_trap: bool, back_at_start: bool, steps: usize) -> bool {
    inside_trap || back_at_start || steps >= MAX_CYCLE_STEPS
}

/// Tab: the next tab stop, without leaving the innermost focus trap.
pub fn next(window: &mut Window, cx: &mut App) {
    cycle(window, cx, Window::focus_next);
}

/// Shift-Tab: the previous tab stop, without leaving the innermost focus trap.
pub fn prev(window: &mut Window, cx: &mut App) {
    cycle(window, cx, Window::focus_prev);
}

/// Step focus with `step`, then keep stepping until it is back inside the active trap.
///
/// With no trap registered this is exactly `step` — the whole app outside a modal keeps
/// gpui's own traversal, unwrapped.
fn cycle(window: &mut Window, cx: &mut App, step: fn(&mut Window, &mut App)) {
    let Some(trap) = gpui_base::active_focus_trap(window, cx) else {
        step(window, cx);
        return;
    };
    let start = window.focused(cx);
    let mut steps = 0usize;
    loop {
        step(window, cx);
        steps += 1;
        if cycle_done(
            trap.contains_focused(window, cx),
            window.focused(cx) == start,
            steps,
        ) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cycle_stops_the_moment_focus_is_back_inside_the_trap() {
        // The common case: one step off the modal's last field lands on a background
        // stop, the second lands back on the modal's first. Nothing further runs.
        assert!(!cycle_done(false, false, 1));
        assert!(cycle_done(true, false, 2));
    }

    #[test]
    fn a_cycle_that_never_re_enters_gives_up_instead_of_spinning() {
        // Both backstops, because they cover different failures: a trap registered
        // around a container with no tab stop in it never satisfies `inside_trap`, and
        // if focus started outside the ring it never returns to `start` either.
        assert!(cycle_done(false, true, 3), "a full lap");
        assert!(cycle_done(false, false, MAX_CYCLE_STEPS), "the budget");
        assert!(cycle_done(false, false, MAX_CYCLE_STEPS + 1));
    }
}
