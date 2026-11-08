//! The depth ladder.
//!
//! # v2: soft shadows, amended from "borders + surface shifts only"
//!
//! v1's law was *"Borders + surface shifts only. No shadows."* On a near-black canvas
//! that meant a card was a fill 3% brighter than the window plus a 1px hairline — an
//! outline, not an object — and a screen of panels was a lattice of hairlines. In the
//! light palette it could not separate a card from the desk at all without making the
//! card *darker* than the canvas, which read as a recess. That is a large part of what
//! Murphy meant by clinical.
//!
//! Design system v2 (`docs/design/2026-11-07-friendly-ui-v2.md`) keeps the rungs and
//! gives the ones that sit *above* the canvas a soft, theme-aware shadow, drawn in the
//! palette's own [`Theme::shadow`] ink. The hairline stays (softened in the palettes) so
//! the dark themes still have an edge where a black shadow on a near-black canvas has
//! little to darken.
//!
//! | Rung | Fill | Border | Shadow | For |
//! |---|---|---|---|---|
//! | [`Elevation::Bg`] | `bg` | none | none | the canvas, and sidebars (which share it) |
//! | [`Elevation::Surface`] | `surface` | `border` | small | cards, panels, table headers |
//! | [`Elevation::Well`] | `well` | `border` | none | inputs, editors, terminals — a recess |
//! | [`Elevation::Floating`] | raised surface | `border` | medium | menus, popovers, toasts, tooltips |
//! | [`Elevation::Overlay`] | `surface` | `border` | large | modals, over the scrim |
//!
//! `Floating` is the one rung with no scrim under it, so it also takes a fill a step
//! brighter than `surface` on the dark palettes ([`crate::bridge::raised_surface`]); on
//! the light palette a card is already near-white, so a floating panel keeps the surface
//! fill and lets its shadow separate it.

use gpui::{BoxShadow, Hsla, px, rgb};

use crate::theme::{self, Theme};

/// A rung on the depth ladder. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// The window canvas. No fill shift, no border, no shadow.
    Bg,
    /// Raised: cards, sections, table headers, chrome bars.
    Surface,
    /// Recessed: inputs, editors, terminal wells.
    Well,
    /// Floating chrome with no scrim under it: menus, popovers, toasts, tooltips.
    Floating,
    /// Modals — `Surface`'s fill, sitting over the scrim, with the deepest shadow.
    Overlay,
}

/// Every rung, for exhaustive tests and the gallery.
pub const ALL_ELEVATIONS: &[Elevation] = &[
    Elevation::Bg,
    Elevation::Surface,
    Elevation::Well,
    Elevation::Floating,
    Elevation::Overlay,
];

/// One layer of a shadow, as authored: offset-y, blur, and alpha for the dark palettes
/// and for the light one. (No x offset: light comes from straight above.)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowLayer {
    /// Downward offset, px.
    pub y: f32,
    /// Blur radius, px.
    pub blur: f32,
    /// Ink alpha on a dark palette.
    pub dark_alpha: f32,
    /// Ink alpha on a light palette.
    pub light_alpha: f32,
}

const SMALL: &[ShadowLayer] = &[
    ShadowLayer {
        y: 1.,
        blur: 2.,
        dark_alpha: 0.30,
        light_alpha: 0.06,
    },
    ShadowLayer {
        y: 2.,
        blur: 8.,
        dark_alpha: 0.18,
        light_alpha: 0.05,
    },
];

const MEDIUM: &[ShadowLayer] = &[
    ShadowLayer {
        y: 2.,
        blur: 4.,
        dark_alpha: 0.30,
        light_alpha: 0.06,
    },
    ShadowLayer {
        y: 8.,
        blur: 24.,
        dark_alpha: 0.45,
        light_alpha: 0.12,
    },
];

const LARGE: &[ShadowLayer] = &[
    ShadowLayer {
        y: 4.,
        blur: 8.,
        dark_alpha: 0.30,
        light_alpha: 0.06,
    },
    ShadowLayer {
        y: 16.,
        blur: 48.,
        dark_alpha: 0.60,
        light_alpha: 0.20,
    },
];

impl Elevation {
    /// The background token for this rung.
    pub fn fill(self, theme: &Theme) -> u32 {
        match self {
            Elevation::Bg => theme.bg,
            Elevation::Surface | Elevation::Overlay => theme.surface,
            Elevation::Well => theme.well,
            Elevation::Floating => {
                if theme::is_light(theme) {
                    theme.surface
                } else {
                    crate::bridge::raised_surface(theme)
                }
            }
        }
    }

    /// The hairline border token for this rung — `None` for the canvas, which is not a
    /// bounded region and must not draw one.
    pub fn border(self, theme: &Theme) -> Option<u32> {
        match self {
            Elevation::Bg => None,
            _ => Some(theme.border),
        }
    }

    /// The authored shadow layers for this rung. Empty for the canvas and the recess.
    pub fn shadow_layers(self) -> &'static [ShadowLayer] {
        match self {
            Elevation::Bg | Elevation::Well => &[],
            Elevation::Surface => SMALL,
            Elevation::Floating => MEDIUM,
            Elevation::Overlay => LARGE,
        }
    }

    /// The shadow, resolved against `theme`: [`Theme::shadow`] ink at the palette's alpha.
    pub fn shadow(self, theme: &Theme) -> Vec<BoxShadow> {
        let light = theme::is_light(theme);
        self.shadow_layers()
            .iter()
            .map(|layer| {
                let alpha = if light {
                    layer.light_alpha
                } else {
                    layer.dark_alpha
                };
                BoxShadow::new(px(0.), px(layer.y), shadow_ink(theme, alpha))
                    .blur_radius(px(layer.blur))
            })
            .collect()
    }
}

/// `theme.shadow` at `alpha` — the only way a shadow colour is spelled.
pub fn shadow_ink(theme: &Theme, alpha: f32) -> Hsla {
    let mut ink: Hsla = rgb(theme.shadow).into();
    ink.a = alpha.clamp(0., 1.);
    ink
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{cosmos, cosmos_light, dusk, void};

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    #[test]
    fn the_canvas_rung_never_draws_a_border() {
        assert_eq!(Elevation::Bg.border(&cosmos()), None);
        for &rung in &ALL_ELEVATIONS[1..] {
            assert_eq!(rung.border(&cosmos()), Some(cosmos().border), "{rung:?}");
        }
    }

    #[test]
    fn every_palette_separates_the_rungs() {
        for t in palettes() {
            let bg = Elevation::Bg.fill(&t);
            let surface = Elevation::Surface.fill(&t);
            let well = Elevation::Well.fill(&t);
            assert_ne!(bg, surface, "{}: bg vs surface", t.name);
            assert_ne!(bg, well, "{}: bg vs well", t.name);
            assert_ne!(surface, well, "{}: surface vs well", t.name);
        }
    }

    #[test]
    fn overlay_shares_the_surface_fill() {
        for t in palettes() {
            assert_eq!(
                Elevation::Overlay.fill(&t),
                Elevation::Surface.fill(&t),
                "{}",
                t.name
            );
        }
    }

    #[test]
    fn a_floating_panel_stands_off_a_card_on_dark_palettes() {
        // No scrim under a menu: on dark, the fill has to do some of the separating.
        for t in [cosmos(), void(), dusk()] {
            assert_ne!(
                Elevation::Floating.fill(&t),
                Elevation::Surface.fill(&t),
                "{}",
                t.name
            );
        }
    }

    #[test]
    fn the_canvas_and_the_recess_cast_no_shadow() {
        for t in palettes() {
            assert!(Elevation::Bg.shadow(&t).is_empty());
            assert!(Elevation::Well.shadow(&t).is_empty());
        }
    }

    #[test]
    fn higher_rungs_cast_deeper_shadows() {
        // The ladder in one property: the deepest layer's blur and offset grow with the
        // rung, so a modal visibly floats above a menu, which floats above a card.
        let deepest = |rung: Elevation| {
            rung.shadow_layers()
                .iter()
                .map(|l| (l.blur, l.y))
                .fold((0f32, 0f32), |a, b| (a.0.max(b.0), a.1.max(b.1)))
        };
        let (s, f, o) = (
            deepest(Elevation::Surface),
            deepest(Elevation::Floating),
            deepest(Elevation::Overlay),
        );
        assert!(s.0 < f.0 && f.0 < o.0, "blur {s:?} {f:?} {o:?}");
        assert!(s.1 < f.1 && f.1 < o.1, "offset {s:?} {f:?} {o:?}");
    }

    #[test]
    fn shadows_are_drawn_in_the_palettes_own_ink_and_lighter_on_light() {
        for t in palettes() {
            for shadow in Elevation::Floating.shadow(&t) {
                let mut opaque = shadow.color;
                opaque.a = 1.;
                assert_eq!(opaque, Hsla::from(rgb(t.shadow)), "{}", t.name);
            }
        }
        // A light palette needs far less ink for the same lift.
        for layer in ALL_ELEVATIONS.iter().flat_map(|r| r.shadow_layers()) {
            assert!(layer.light_alpha < layer.dark_alpha);
            assert!(layer.dark_alpha <= 0.6, "a shadow is never a black slab");
        }
    }
}
