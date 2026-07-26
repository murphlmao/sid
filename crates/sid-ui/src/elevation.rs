//! The depth ladder.
//!
//! `.interface-design/system.md`: *"Borders + surface shifts only. No shadows."* That
//! rule is right but under-specified — followed literally it degrades into "everything
//! is `bg` with hairlines", which is exactly what the System tab's unframed meter bars
//! became. Naming the rungs makes the ladder a choice a call site has to make instead
//! of a default it falls into.
//!
//! Four rungs, in reading order:
//!
//! | Rung | Fill | Border | For |
//! |---|---|---|---|
//! | [`Elevation::Bg`] | `bg` | none | the canvas, and sidebars (which share it) |
//! | [`Elevation::Surface`] | `surface` | `border` | cards, sections, chrome, table headers |
//! | [`Elevation::Well`] | `well` | `border` | inputs, editors, terminals — a recess |
//! | [`Elevation::Overlay`] | `surface` | `border` | modals and popovers, over the scrim |
//!
//! `Overlay` shares `Surface`'s fill on purpose: what raises a modal is the scrim
//! underneath it ([`crate::bridge::SCRIM`]), not a brighter panel. The rung exists so
//! modal code says what it means, and so a future palette can separate the two without
//! touching a call site.

use crate::theme::Theme;

/// A rung on the depth ladder. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// The window canvas. No fill shift, no border.
    Bg,
    /// Raised: cards, sections, table headers, chrome bars.
    Surface,
    /// Recessed: inputs, editors, terminal wells.
    Well,
    /// Modals and popovers — `Surface`'s fill, sitting over the scrim.
    Overlay,
}

impl Elevation {
    /// The background token for this rung.
    pub fn fill(self, theme: &Theme) -> u32 {
        match self {
            Elevation::Bg => theme.bg,
            Elevation::Surface | Elevation::Overlay => theme.surface,
            Elevation::Well => theme.well,
        }
    }

    /// The hairline border token for this rung — `None` for the canvas, which is not a
    /// bounded region and must not draw one.
    pub fn border(self, theme: &Theme) -> Option<u32> {
        match self {
            Elevation::Bg => None,
            Elevation::Surface | Elevation::Well | Elevation::Overlay => Some(theme.border),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{cosmos, cosmos_light, dusk, void};

    #[test]
    fn the_canvas_rung_never_draws_a_border() {
        assert_eq!(Elevation::Bg.border(&cosmos()), None);
        for rung in [Elevation::Surface, Elevation::Well, Elevation::Overlay] {
            assert_eq!(rung.border(&cosmos()), Some(cosmos().border));
        }
    }

    #[test]
    fn every_palette_separates_the_rungs() {
        // A ladder whose rungs share a fill is not a ladder — this is what makes
        // "raised" and "recessed" legible without shadows.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
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
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            assert_eq!(
                Elevation::Overlay.fill(&t),
                Elevation::Surface.fill(&t),
                "{}",
                t.name
            );
        }
    }
}
