//! The measurement tokens — spacing, radius, control heights and row density, decided once.
//!
//! [`crate::typography`] did for text sizes what this module does for everything else
//! with a length: before it, "how far apart are two cards" or "how round is a modal" was
//! whatever `gpui` shorthand a call site typed (`gap_2`, `rounded_lg`), and v1's answers
//! — 8px gutters, 6px corners, 24px controls — were a large part of why sid read as a
//! clinical instrument panel. Design system v2 (`docs/design/2026-11-07-friendly-ui-v2.md`)
//! moves every one of them, and names them so they can be moved again in one place.
//!
//! Every value here is authored in design pixels at 100% zoom and handed to `gpui` through
//! [`crate::scale::scaled`], so app zoom still moves all of it together with the type.

use gpui::{Pixels, Rems, px};

use crate::scale::scaled;

/// A rung on the 4px spacing grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Space {
    /// 4px — hairline gaps, a badge's inner padding.
    X1,
    /// 8px — between adjacent controls, icon to label.
    X2,
    /// 12px — between rows/fields in a card, a row's horizontal padding.
    X3,
    /// 16px — a card's padding, the gap between cards.
    X4,
    /// 20px — a tab's content inset.
    X5,
    /// 24px — between major regions.
    X6,
    /// 32px — around an empty state.
    X8,
}

/// Every rung, ascending, for tests and the gallery.
pub const ALL_SPACES: &[Space] = &[
    Space::X1,
    Space::X2,
    Space::X3,
    Space::X4,
    Space::X5,
    Space::X6,
    Space::X8,
];

impl Space {
    /// The rung in design pixels at 100% zoom.
    pub const fn px(self) -> Pixels {
        px(match self {
            Space::X1 => 4.,
            Space::X2 => 8.,
            Space::X3 => 12.,
            Space::X4 => 16.,
            Space::X5 => 20.,
            Space::X6 => 24.,
            Space::X8 => 32.,
        })
    }

    /// The rung as a zoomable length — what a style call wants.
    pub fn len(self) -> Rems {
        scaled(f32::from(self.px()))
    }
}

/// A tab's content inset from the window edge. v1: 12.
pub const PAGE_GUTTER: Space = Space::X5;
/// Between cards and panels on one screen. v1: 8–12.
pub const SECTION_GAP: Space = Space::X4;
/// Inside a card. v1: 12.
pub const CARD_PADDING: Space = Space::X4;
/// Between rows or fields stacked inside a card. v1: 8.
pub const STACK_GAP: Space = Space::X3;
/// Between adjacent controls (a button pair, a header's actions).
pub const CONTROL_GAP: Space = Space::X2;
/// Between an icon and its label inside a control. v1: 6.
pub const ICON_GAP: Space = Space::X2;

/// A corner radius on the v2 scale.
///
/// Softer than v1 on purpose: 6px on a 24px control and 8px on a 460px modal read as
/// square, and square plus hairline is most of what "clinical" meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Radius {
    /// 4px — a checkbox's box, a keycap.
    Xs,
    /// 6px — badges, tooltips, chips inside a row.
    Sm,
    /// 8px — buttons, fields, rows, segmented controls, menu items.
    Md,
    /// 12px — cards, panels, toasts, popovers, menus.
    Lg,
    /// 16px — modals.
    Xl,
    /// A pill — switches, count pills, status pills.
    Full,
}

impl Radius {
    /// The radius in design pixels. `Full` is a radius no control is tall enough to
    /// exceed, which is how `gpui`'s own `rounded_full` spells a pill.
    pub const fn px(self) -> Pixels {
        px(match self {
            Radius::Xs => 4.,
            Radius::Sm => 6.,
            Radius::Md => 8.,
            Radius::Lg => 12.,
            Radius::Xl => 16.,
            Radius::Full => 9999.,
        })
    }

    /// The radius as a zoomable length.
    pub fn len(self) -> Rems {
        scaled(f32::from(self.px()))
    }
}

/// The smallest box anything clickable is allowed to be. v1's small control was 24px —
/// most of the app's targets — which is a precision task with a trackpad.
pub const MIN_HIT: Pixels = px(32.);

/// How tightly a list packs its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Density {
    /// 44px — the default list row: hosts, connections, workspaces, settings rows.
    #[default]
    Comfortable,
    /// 36px — sidebars and lists that sit beside a bigger surface.
    Compact,
    /// 32px — trees and other nested navigation; the hit-target floor.
    Dense,
}

impl Density {
    /// The minimum row height at this density, in design pixels.
    pub const fn row_height(self) -> Pixels {
        px(match self {
            Density::Comfortable => 44.,
            Density::Compact => 36.,
            Density::Dense => 32.,
        })
    }

    /// The row height as a zoomable length.
    pub fn row_len(self) -> Rems {
        scaled(f32::from(self.row_height()))
    }
}

/// Apply a radius from the scale to any element.
pub trait Rounded: gpui::Styled + Sized {
    /// Round all four corners to `radius`.
    fn corners(self, radius: Radius) -> Self {
        self.rounded(radius.len())
    }
}

impl<T: gpui::Styled + Sized> Rounded for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spacing_grid_is_ascending_multiples_of_four() {
        let mut last = 0.;
        for &s in ALL_SPACES {
            let v = f32::from(s.px());
            assert!(v > last, "{s:?} out of order");
            assert_eq!(v % 4., 0., "{s:?} is off the 4px grid");
            last = v;
        }
    }

    #[test]
    fn the_radius_scale_is_ascending_and_controls_are_softer_than_v1() {
        let ladder = [
            Radius::Xs,
            Radius::Sm,
            Radius::Md,
            Radius::Lg,
            Radius::Xl,
            Radius::Full,
        ];
        for pair in ladder.windows(2) {
            assert!(
                pair[0].px() < pair[1].px(),
                "{:?} !< {:?}",
                pair[0],
                pair[1]
            );
        }
        // v1's control corner was 6px and its modal 8px; v2 is the reason this file exists.
        assert!(Radius::Md.px() > px(6.));
        assert!(Radius::Xl.px() > px(8.));
    }

    #[test]
    fn no_density_goes_below_the_hit_target() {
        for d in [Density::Comfortable, Density::Compact, Density::Dense] {
            assert!(d.row_height() >= MIN_HIT, "{d:?}");
        }
        assert!(Density::Comfortable.row_height() > Density::Compact.row_height());
        assert!(Density::Compact.row_height() > Density::Dense.row_height());
    }

    #[test]
    fn the_semantic_gaps_are_grid_rungs() {
        assert_eq!(PAGE_GUTTER.px(), px(20.));
        assert_eq!(SECTION_GAP.px(), px(16.));
        assert_eq!(CARD_PADDING.px(), px(16.));
        assert_eq!(STACK_GAP.px(), px(12.));
        assert_eq!(CONTROL_GAP.px(), px(8.));
        assert_eq!(ICON_GAP.px(), px(8.));
    }

    #[test]
    fn lengths_are_the_authored_pixels_at_100_percent() {
        let rem = crate::scale::UiScale::DEFAULT.rem_size();
        assert_eq!(Space::X4.len().to_pixels(rem), px(16.));
        assert_eq!(Radius::Lg.len().to_pixels(rem), px(12.));
        assert_eq!(Density::Comfortable.row_len().to_pixels(rem), px(44.));
    }
}
