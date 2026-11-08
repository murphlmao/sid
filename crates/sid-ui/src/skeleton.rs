//! Skeletons — the shape of content that is on its way.
//!
//! sid's loading states were a muted sentence ("probing…", "no system overview yet") in
//! the spot the content would eventually fill, which reads as *empty* rather than as
//! *arriving*, and then the layout jumps when the data lands. A skeleton holds the
//! content's shape — bars where lines will be, a block where a chart will be — in the
//! `selection` tone, pulsing gently so it reads as alive. Under reduced motion it holds
//! still.
//!
//! Three shapes: [`Skeleton::line`] (one text line, optionally a fraction of the width),
//! [`Skeleton::block`] (a fixed-height panel), and [`Skeleton::rows`] (a list
//! placeholder at the 44px row height, each with a leading dot, a title bar and a short
//! meta bar).

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, IntoElement, ParentElement as _,
    RenderOnce, Styled as _, Window, div, relative, rgb,
};

use crate::metrics::{Density, Radius, Rounded as _};
use crate::motion;
use crate::scale::scaled;
use crate::styled::{h_flex, v_flex};
use crate::theme;

/// One pulse, there and back.
pub const PULSE: Duration = Duration::from_millis(1400);

/// The faintest the pulse gets. It never fades to nothing — a skeleton that blinks out
/// reads as a rendering fault.
pub const PULSE_FLOOR: f32 = 0.45;

/// The pulse's opacity at progress `t` in `0..=1`: floor → 1 → floor, eased each way.
pub fn pulse_opacity(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    let half = if t < 0.5 { t * 2. } else { (1. - t) * 2. };
    // Quadratic ease-in-out over the half-wave.
    let wave = if half < 0.5 {
        2. * half * half
    } else {
        1. - (-2. * half + 2.).powi(2) / 2.
    };
    PULSE_FLOOR + (1. - PULSE_FLOOR) * wave
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// One text line; `Some(f)` is a fraction of the available width.
    Line(Option<f32>),
    /// A fixed-height block, design px.
    Block(f32),
    /// `n` list rows.
    Rows(usize),
}

/// A loading placeholder. See the module docs.
#[derive(IntoElement)]
pub struct Skeleton {
    id: ElementId,
    shape: Shape,
}

impl Skeleton {
    /// One text line, full width.
    pub fn line(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            shape: Shape::Line(None),
        }
    }

    /// One text line at `fraction` (0..=1) of the available width.
    pub fn line_fraction(id: impl Into<ElementId>, fraction: f32) -> Self {
        Self {
            id: id.into(),
            shape: Shape::Line(Some(fraction.clamp(0.05, 1.))),
        }
    }

    /// A block `height` design px tall — a chart, a preview, a card body.
    pub fn block(id: impl Into<ElementId>, height: f32) -> Self {
        Self {
            id: id.into(),
            shape: Shape::Block(height),
        }
    }

    /// `n` placeholder list rows at the comfortable row height.
    pub fn rows(id: impl Into<ElementId>, n: usize) -> Self {
        Self {
            id: id.into(),
            shape: Shape::Rows(n),
        }
    }
}

/// A rounded bar in the skeleton tone.
fn bar(fill: u32) -> gpui::Div {
    div().h(scaled(12.)).corners(Radius::Sm).bg(rgb(fill))
}

/// The width fractions the placeholder rows cycle through, so a list of skeleton rows
/// does not read as a ruled grid.
const ROW_WIDTHS: [f32; 4] = [0.42, 0.58, 0.36, 0.5];

impl RenderOnce for Skeleton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        // One step above `selection`: a skeleton sits on cards, and the bare row-fill
        // token pulsing down to 45% was nearly invisible on the dark palettes.
        let fill = crate::bridge::hover_of(&theme, theme.selection);
        let body: AnyElement = match self.shape {
            Shape::Line(fraction) => bar(fill)
                .w(relative(fraction.unwrap_or(1.)))
                .into_any_element(),
            Shape::Block(height) => div()
                .w_full()
                .h(scaled(height))
                .corners(Radius::Lg)
                .bg(rgb(fill))
                .into_any_element(),
            Shape::Rows(n) => v_flex()
                .w_full()
                .children((0..n).map(|ix| {
                    h_flex()
                        .w_full()
                        .h(Density::Comfortable.row_len())
                        .px_3()
                        .gap_3()
                        .child(
                            div()
                                .flex_none()
                                .size(scaled(10.))
                                .rounded_full()
                                .bg(rgb(fill)),
                        )
                        .child(bar(fill).w(relative(ROW_WIDTHS[ix % ROW_WIDTHS.len()])))
                        .child(div().flex_1())
                        .child(bar(fill).flex_none().w(scaled(56.)))
                }))
                .into_any_element(),
        };
        let holder = div().w_full().child(body);
        if motion::reduced(cx) {
            holder.opacity(0.75).into_any_element()
        } else {
            holder
                .with_animation(self.id, Animation::new(PULSE).repeat(), |el, t| {
                    el.opacity(pulse_opacity(t))
                })
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pulse_breathes_between_its_floor_and_full() {
        assert!((pulse_opacity(0.) - PULSE_FLOOR).abs() < 1e-6);
        assert!((pulse_opacity(1.) - PULSE_FLOOR).abs() < 1e-6);
        assert!((pulse_opacity(0.5) - 1.).abs() < 1e-6);
        for i in 0..=20 {
            let o = pulse_opacity(i as f32 / 20.);
            assert!((PULSE_FLOOR..=1.).contains(&o), "{o}");
        }
    }

    #[test]
    fn a_line_fraction_is_clamped_to_something_visible() {
        assert_eq!(
            Skeleton::line_fraction("l", 3.).shape,
            Shape::Line(Some(1.))
        );
        assert_eq!(
            Skeleton::line_fraction("l", 0.).shape,
            Shape::Line(Some(0.05))
        );
        assert_eq!(Skeleton::rows("r", 3).shape, Shape::Rows(3));
    }
}
