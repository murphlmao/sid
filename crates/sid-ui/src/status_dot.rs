//! The connection-state dot — a mark that says what it means.
//!
//! SSH home shipped a 6px circle, `●` when a session was live and `○` when it was not,
//! in `success` or `muted`, with no label, no tooltip and no legend anywhere on the
//! screen. The audit's verdict (`docs/design/2026-07-26-ui-overhaul-plan.md` §1.2):
//! *"Connection state is a 6px hollow circle with no legend. Nothing distinguishes
//! disconnected from unknown."*
//!
//! A status mark is only information if the reader can decode it, so this type carries
//! its own name three ways: **shape** (a filled disc is live, a hollow ring is not),
//! **colour** (a semantic token), and **words** (a tooltip on every dot, an optional
//! inline label, and [`StatusLegend`] for the screen that wants the whole vocabulary
//! spelled out once).
//!
//! [`ConnectionState::token`] and [`ConnectionState::solid`] are the decision; the render
//! path is glue over them.

use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::tooltip::Tooltip;

use crate::styled::{StyledExt as _, h_flex};
use crate::theme::{self, Theme};

/// The diameter of the mark itself. 8px, up from the 6px it replaces: at 6px a 1px ring
/// is more border than interior and the hollow/filled distinction stops reading.
const DOT: f32 = 8.;

/// The width of the slot a dot sits in, so a column of rows aligns whether or not each
/// row's dot is the same shape.
const SLOT: f32 = 12.;

/// What a saved connection is doing right now.
///
/// Four states, not two: the old dot could only say "live" or "not live", which folded
/// *never tried*, *dialling* and *failed* into one indistinguishable ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// No session open for this row. The resting state, and the common one.
    Offline,
    /// A session is dialling / authenticating.
    Connecting,
    /// A session is open.
    Live,
    /// The last attempt failed.
    Failed,
}

/// Every state, for the legend and for exhaustive tests.
pub const ALL_CONNECTION_STATES: &[ConnectionState] = &[
    ConnectionState::Offline,
    ConnectionState::Connecting,
    ConnectionState::Live,
    ConnectionState::Failed,
];

impl ConnectionState {
    /// The words. This is the accessible name of the mark — it is what the tooltip
    /// shows, what the legend prints, and what an inline label renders.
    ///
    /// Phrased from the reader's side ("not connected"), not the machine's ("offline"),
    /// because the row is about a host the reader is trying to reach.
    pub fn label(self) -> &'static str {
        match self {
            ConnectionState::Offline => "not connected",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Live => "connected",
            ConnectionState::Failed => "connection failed",
        }
    }

    /// The semantic token this state is drawn in.
    ///
    /// `Offline` takes `muted` rather than `faint`: it is the state most rows are in,
    /// and a resting state that is nearly invisible reads as a rendering fault. `faint`
    /// is for decoration, and a status mark is not decoration.
    pub fn token(self, theme: &Theme) -> u32 {
        match self {
            ConnectionState::Offline => theme.muted,
            ConnectionState::Connecting => theme.warning,
            ConnectionState::Live => theme.success,
            ConnectionState::Failed => theme.danger,
        }
    }

    /// Whether the mark is a filled disc (something is actually open, or was) or a
    /// hollow ring (nothing is).
    ///
    /// Shape carries the same distinction as colour on purpose: `void`'s palette is
    /// near-monochrome, so its `success` and `muted` are two greys — without the
    /// fill/ring difference, "connected" and "not connected" would be the same mark.
    pub fn solid(self) -> bool {
        !matches!(self, ConnectionState::Offline)
    }
}

/// A connection-state mark.
///
/// ```ignore
/// StatusDot::new(("ssh-row-dot", row_id), ConnectionState::Live)
/// StatusDot::new("legend-live", ConnectionState::Live).with_label()
/// ```
#[derive(IntoElement)]
pub struct StatusDot {
    id: ElementId,
    state: ConnectionState,
    with_label: bool,
}

impl StatusDot {
    /// A dot. The `id` is required because the tooltip is: gpui only hangs a tooltip on
    /// a stateful element, and a status mark with no name is the thing this type exists
    /// to delete.
    pub fn new(id: impl Into<ElementId>, state: ConnectionState) -> Self {
        Self {
            id: id.into(),
            state,
            with_label: false,
        }
    }

    /// Also print the state's word beside the mark. For legends and for wide layouts
    /// that have the room — in a dense row the tooltip carries it instead.
    pub fn with_label(mut self) -> Self {
        self.with_label = true;
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let colour = rgb(self.state.token(&theme));
        let label = self.state.label();
        h_flex()
            .id(self.id)
            .flex_none()
            .gap_1p5()
            .child(
                div().w(px(SLOT)).flex().justify_center().child(
                    div()
                        .w(px(DOT))
                        .h(px(DOT))
                        .rounded_full()
                        .border_1()
                        .border_color(colour)
                        .when(self.state.solid(), |this| this.bg(colour)),
                ),
            )
            .when(self.with_label, |this| {
                this.child(div().hint_text(&theme).child(label))
            })
            .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
    }
}

/// The whole state vocabulary, spelled out once per screen.
///
/// The design system asks for orientation, not decoration: a reader who has never seen
/// sid's dots should be able to learn all four in one glance, from the same screen the
/// dots are on, without hovering anything.
#[derive(IntoElement)]
pub struct StatusLegend {
    id: &'static str,
}

impl StatusLegend {
    /// A legend. `id` prefixes each entry's element id, so two legends on one screen do
    /// not collide.
    pub fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl RenderOnce for StatusLegend {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let id = self.id;
        h_flex()
            .flex_wrap()
            .gap_3()
            .hint_text(&theme)
            .children(ALL_CONNECTION_STATES.iter().map(move |&state| {
                let entry = SharedString::from(format!("{id}-{}", state.label()));
                StatusDot::new(entry, state).with_label()
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::brightness;
    use crate::theme::{cosmos, cosmos_light, dusk, void};
    use std::collections::HashSet;

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    #[test]
    fn every_state_has_a_distinct_readable_name() {
        // The dot's accessible name. An empty or duplicated one puts the reader back
        // where the unlabelled circle left them.
        let labels: HashSet<&str> = ALL_CONNECTION_STATES.iter().map(|s| s.label()).collect();
        assert_eq!(labels.len(), ALL_CONNECTION_STATES.len(), "duplicate label");
        for state in ALL_CONNECTION_STATES {
            assert!(!state.label().is_empty(), "{state:?}: no label");
        }
    }

    #[test]
    fn disconnected_and_unknown_are_no_longer_the_same_mark() {
        // The audit's exact complaint: "Nothing distinguishes disconnected from
        // unknown." Four states, four colours, in every palette.
        for t in palettes() {
            let tokens: HashSet<u32> = ALL_CONNECTION_STATES.iter().map(|s| s.token(&t)).collect();
            assert_eq!(
                tokens.len(),
                ALL_CONNECTION_STATES.len(),
                "{}: two states share a colour",
                t.name
            );
        }
    }

    #[test]
    fn shape_carries_the_distinction_colour_alone_cannot() {
        // void's `success` (0xc0c0c0) and `muted` (0x999999) are two greys a hairline
        // apart. Without fill-vs-ring, "connected" and "not connected" would look the
        // same on that palette — so the shape rule is load-bearing, not decorative.
        assert!(!ConnectionState::Offline.solid(), "offline is a ring");
        for state in ALL_CONNECTION_STATES
            .iter()
            .filter(|s| **s != ConnectionState::Offline)
        {
            assert!(state.solid(), "{state:?}: should be a filled disc");
        }
        let t = void();
        let live = brightness(ConnectionState::Live.token(&t));
        let offline = brightness(ConnectionState::Offline.token(&t));
        assert!(
            (live - offline).abs() < 0.2,
            "this test's premise died: void's live/offline greys diverged ({live:.2} vs \
             {offline:.2}) — the shape rule may be re-examined"
        );
    }

    #[test]
    fn every_mark_separates_from_the_canvas() {
        // A status mark that dissolves into the background is not a status mark. The
        // resting state is the risky one: it is the quietest, and it is most rows.
        for t in palettes() {
            for state in ALL_CONNECTION_STATES {
                let delta = (brightness(state.token(&t)) - brightness(t.bg)).abs();
                assert!(
                    delta > 0.2,
                    "{}/{}: mark contrast {delta:.2} is too low",
                    t.name,
                    state.label()
                );
            }
        }
    }

    #[test]
    fn the_resting_state_is_muted_not_faint() {
        // `faint` is the decorative/disabled tone. Most rows are `Offline`, so drawing
        // that state in `faint` would render the whole column as decoration.
        for t in palettes() {
            assert_eq!(
                ConnectionState::Offline.token(&t),
                t.muted,
                "{}: offline is muted",
                t.name
            );
            assert_ne!(
                ConnectionState::Offline.token(&t),
                t.faint,
                "{}: offline must not be faint",
                t.name
            );
        }
    }

    #[test]
    fn live_and_failed_use_their_semantic_tokens() {
        for t in palettes() {
            assert_eq!(ConnectionState::Live.token(&t), t.success, "{}", t.name);
            assert_eq!(ConnectionState::Failed.token(&t), t.danger, "{}", t.name);
            assert_eq!(
                ConnectionState::Connecting.token(&t),
                t.warning,
                "{}",
                t.name
            );
        }
    }

    #[test]
    fn a_status_mark_never_spends_the_accent() {
        // `.interface-design/system.md`: accent means "engage". A row's state is
        // orientation — it is not asking to be clicked.
        for t in palettes() {
            for state in ALL_CONNECTION_STATES {
                assert_ne!(
                    state.token(&t),
                    t.accent,
                    "{}/{}: status is not an accent",
                    t.name,
                    state.label()
                );
            }
        }
    }
}
