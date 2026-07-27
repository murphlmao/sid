//! The origin chip — which layer a record was read from.
//!
//! sid's store is layered and **attributive, never overriding** (CLAUDE.md rule 2): a
//! read is the union of the global store and the focused workspace's committed
//! `.sid/config.toml`, each row tagged with where it came from. That tag has to be on
//! screen, because two rows can share an alias and mean different machines.
//!
//! What it must *not* be is loud. `.interface-design/system.md`: *"Orientation badges
//! (origin, counts) are `faint`/`muted`. One accent, used sparingly."* The SSH row shipped
//! this as bare text in `faint` for global and `success` — a *hue* — for workspace, which
//! spends a status colour on metadata and leaves the reader deciding whether a green word
//! means the row is healthy. Here the two origins are told apart by **weight**, inside the
//! one neutral tone: global is the quiet, common case; a workspace origin is the notable
//! one and gets the heavier chip.
//!
//! [`ScopeChip::label`] and [`ScopeChip::fill`] are the decision; the render path is glue.
//! The type deliberately does not know `sid_store::Scope` — `sid-ui` depends on no domain
//! crate, so the caller maps its own origin onto [`ScopeOrigin`].

use gpui::{App, IntoElement, RenderOnce, SharedString, Window};

use crate::badge::{Badge, BadgeFill, BadgeTone};

/// Which layer a record lives in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeOrigin {
    /// The always-loaded global store.
    Global,
    /// A workspace's committed config, by that workspace's display name.
    Workspace(SharedString),
}

/// The suffix marking a row whose alias also exists in the other layer. The store keeps
/// both losslessly; this says so, rather than silently showing one of them.
const DUPLICATE_SUFFIX: &str = " · dup";

/// An origin chip.
///
/// ```ignore
/// ScopeChip::global()
/// ScopeChip::workspace("acme-api").duplicate(true)   // "acme-api · dup"
/// ```
#[derive(Clone, Debug, IntoElement)]
pub struct ScopeChip {
    origin: ScopeOrigin,
    duplicate: bool,
}

impl ScopeChip {
    /// A chip for `origin`.
    pub fn new(origin: ScopeOrigin) -> Self {
        Self {
            origin,
            duplicate: false,
        }
    }

    /// The global store's chip.
    pub fn global() -> Self {
        Self::new(ScopeOrigin::Global)
    }

    /// A workspace's chip, by display name.
    pub fn workspace(name: impl Into<SharedString>) -> Self {
        Self::new(ScopeOrigin::Workspace(name.into()))
    }

    /// Mark this row as shadowing (or being shadowed by) a same-alias row in the other
    /// layer.
    pub fn duplicate(mut self, duplicate: bool) -> Self {
        self.duplicate = duplicate;
        self
    }

    /// The chip's word.
    pub fn label(&self) -> String {
        let base = match &self.origin {
            ScopeOrigin::Global => "global",
            ScopeOrigin::Workspace(name) => name.as_ref(),
        };
        match self.duplicate {
            true => format!("{base}{DUPLICATE_SUFFIX}"),
            false => base.to_string(),
        }
    }

    /// The tone. Always neutral — this is the design law the old `success`-coloured
    /// workspace badge broke, pinned as a constant so a call site cannot pass another.
    pub const TONE: BadgeTone = BadgeTone::Neutral;

    /// How heavy the chip is.
    ///
    /// Weight, not hue, is what separates the two origins: `global` is the unremarkable
    /// default and stays a soft chip the eye skips; a workspace origin is the fact worth
    /// noticing and gets the solid one.
    pub fn fill(&self) -> BadgeFill {
        match self.origin {
            ScopeOrigin::Global => BadgeFill::Soft,
            ScopeOrigin::Workspace(_) => BadgeFill::Solid,
        }
    }
}

impl RenderOnce for ScopeChip {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let fill = self.fill();
        Badge::new(self.label()).tone(Self::TONE).fill(fill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, cosmos, cosmos_light, dusk, void};

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    #[test]
    fn a_global_row_says_global_and_a_workspace_row_says_its_name() {
        assert_eq!(ScopeChip::global().label(), "global");
        assert_eq!(ScopeChip::workspace("acme-api").label(), "acme-api");
    }

    #[test]
    fn a_shadowed_row_is_marked_rather_than_hidden() {
        // The store is lossless by rule; the chip has to admit when two layers hold the
        // same alias instead of quietly showing one of them.
        assert_eq!(ScopeChip::global().duplicate(true).label(), "global · dup");
        assert_eq!(
            ScopeChip::workspace("acme-api").duplicate(true).label(),
            "acme-api · dup"
        );
        assert_eq!(
            ScopeChip::workspace("acme-api").duplicate(false).label(),
            "acme-api"
        );
    }

    #[test]
    fn the_two_origins_are_told_apart_by_weight_not_hue() {
        // The regression this pins: the SSH row used `success` — a status colour — for
        // a workspace origin, so metadata borrowed the vocabulary of health.
        assert_ne!(
            ScopeChip::global().fill(),
            ScopeChip::workspace("w").fill(),
            "the two origins must be distinguishable"
        );
        assert_eq!(ScopeChip::global().fill(), BadgeFill::Soft);
        assert_eq!(ScopeChip::workspace("w").fill(), BadgeFill::Solid);
    }

    #[test]
    fn a_duplicate_marker_does_not_change_the_chip_weight() {
        // `· dup` is an annotation on the origin, not a fifth origin — the chip must
        // not start shouting because two layers agree on an alias.
        for chip in [ScopeChip::global(), ScopeChip::workspace("w")] {
            assert_eq!(chip.clone().duplicate(true).fill(), chip.fill());
        }
    }

    #[test]
    fn an_origin_chip_never_spends_the_accent() {
        // `.interface-design/system.md`: accent means "engage". An origin chip is
        // orientation and must read as furniture in every palette.
        assert_eq!(ScopeChip::TONE, BadgeTone::Neutral);
        for t in palettes() {
            for chip in [ScopeChip::global(), ScopeChip::workspace("acme-api")] {
                let paint = ScopeChip::TONE.paint(chip.fill(), &t);
                assert_ne!(paint.ink, t.accent, "{}: chip ink is the accent", t.name);
                assert_ne!(paint.ink, t.success, "{}: chip ink is a status hue", t.name);
                assert_ne!(paint.ink, t.danger, "{}: chip ink is a status hue", t.name);
            }
        }
    }

    #[test]
    fn both_weights_stay_bounded_and_visible() {
        // A chip is a box the eye can skip; one that dissolves into the canvas is just
        // differently-coloured text (the state this replaces).
        for t in palettes() {
            for chip in [ScopeChip::global(), ScopeChip::workspace("acme-api")] {
                let paint = ScopeChip::TONE.paint(chip.fill(), &t);
                assert_ne!(paint.border, t.bg, "{}: invisible hairline", t.name);
                assert_ne!(paint.fill, Some(t.bg), "{}: invisible fill", t.name);
            }
        }
    }
}
