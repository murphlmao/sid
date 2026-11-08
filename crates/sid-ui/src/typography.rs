//! The type scale — sid's font sizes, weights and families, decided once.
//!
//! # The problem this replaced
//!
//! Before this module the app had **222 hand-typed text-size calls** across 23 files
//! and no scale: 140 `text_xs()`, 75 `text_sm()`, a bare `text_size(px(14.))`, six
//! separate `const MONO` declarations holding *two different font names*, and four
//! weights chosen ad hoc. The component crate's own `Badge`, `Card`, `List` row,
//! `Toolbar` and table header set **no size at all**, so the same badge rendered 12px
//! inside a `text_xs` panel and 16px at the root, because `gpui` text styles cascade.
//!
//! # v2: five rungs, three weights (2026-11-07)
//!
//! v1 was three sizes (12 / 14 / 16) and two weights, on the argument that sid is a dense
//! instrument panel. It worked, and it was most of why the app read as clinical: nothing
//! on any screen was bigger than 16px or heavier than 500, so no screen had a place for
//! the eye to land, and every section header was 12px UPPERCASE fine print. Design system
//! v2 (`docs/design/2026-11-07-friendly-ui-v2.md`) keeps the discipline — a role is still
//! the only way to size text — and widens the ladder:
//!
//! | Role          | Size | Weight   | Ink          | For                                         |
//! |---------------|------|----------|--------------|---------------------------------------------|
//! | [`Display`]   | 22px | Semibold | `fg_strong`  | the screen's page heading, one per screen   |
//! | [`Title`]     | 18px | Semibold | `fg_strong`  | modal titles, detail-pane headings          |
//! | [`Heading`]   | 15px | Semibold | `fg_strong`  | card / panel / group headers                |
//! | [`Body`]      | 15px | Normal   | `fg`         | rows, values, fields, copy                  |
//! | [`Control`]   | 15px | Medium   | `fg`         | button, tab and segment labels              |
//! | [`Label`]     | 13px | Medium   | `muted`      | field labels, small-control labels          |
//! | [`Meta`]      | 13px | Normal   | `muted`      | hints, counts, timestamps, badge words      |
//! | [`Mono`]      | 14px | Normal   | `fg`         | data cells, paths, hosts, ports             |
//! | [`Cell`]      | 14px | Normal   | `fg`         | an English value inside a data table        |
//! | [`MonoMeta`]  | 13px | Normal   | `muted`      | ids, hashes, dim paths                      |
//!
//! Every role is still *absolute*: it sets size **and** weight **and** family **and** ink,
//! so a `Meta` inside a `Title` is still 13px Normal. Colour stays overridable —
//! `.text_meta(&t).text_color(rgb(t.danger))` is the intended spelling for a dim *error*
//! hint — but size, weight and family are the role's to decide.
//!
//! ## Why these rungs
//!
//! Body and Meta move up one pixel each (14 → 15, 12 → 13): the "a little bigger" Murphy
//! asked for, everywhere text is actually read. The top grows a Display rung (22) and a
//! real Title (18) so a screen has a heading and a modal has a title, and a Heading at
//! body size but semibold so a card header can be *strong* without being *loud*. 14 is
//! the *table* rung: DejaVu Sans Mono is roughly a fifth wider per glyph than the UI
//! sans, so a 15px data cell would cost a results grid a column; tables stay
//! instrument-sized, which is also why "bigger" is this ladder and not a default zoom.
//! [`Cell`] is the proportional twin on that rung, so a table that mixes identifiers and
//! English (Workspaces' fleet view) still sits on one size.
//!
//! ## Why semibold is allowed now, and bold still is not
//!
//! v1 banned everything above Medium because `BOLD` (700) at 12–16px blooms on a dark
//! panel. That is still true, and 700 is still absent. Semibold (600) at 15–22px is a
//! different animal: the system UI face here (Noto Sans) ships a true SemiBold master, so
//! it renders as a weight rather than a smeared synthetic bold, and at heading sizes it
//! reads as hierarchy, not as a colour change.
//!
//! # Authored in px, emitted in rems
//!
//! The ladder is *stated* in pixels — [`TypeRole::size`] — and what reaches `gpui` is
//! [`TypeRole::length`], the same measurement in rems, so app zoom (`rem_size`, GitHub #4)
//! scales text together with the rem-based spacing around it. At 100% zoom the two are
//! identical.
//!
//! [`Display`]: TypeRole::Display
//! [`Title`]: TypeRole::Title
//! [`Heading`]: TypeRole::Heading
//! [`Body`]: TypeRole::Body
//! [`Control`]: TypeRole::Control
//! [`Label`]: TypeRole::Label
//! [`Meta`]: TypeRole::Meta
//! [`Mono`]: TypeRole::Mono
//! [`Cell`]: TypeRole::Cell
//! [`MonoMeta`]: TypeRole::MonoMeta

use gpui::{FontWeight, Pixels, Rems, Styled, px, rgb};

use crate::scale::scaled;

use crate::theme::Theme;

/// The UI's monospace family: paths, addresses, ids, data cells.
///
/// One name, in one place — it replaced six `const MONO` declarations that had drifted
/// into two different fonts. Deliberately a boring, universally-present family: this
/// text is *read*, not admired.
///
/// The **terminal grid** is not this. `sid-term` paints a PTY at a font the user
/// configures (CaskaydiaCove Nerd Font Mono by default) with kitty cell geometry; it is
/// an instrument with its own metrics, not UI text.
pub const UI_MONO: &str = "DejaVu Sans Mono";

/// The five rungs, in pixels. Nothing else is a size.
const DISPLAY_PX: f32 = 22.;
const TITLE_PX: f32 = 18.;
const BODY_PX: f32 = 15.;
const MONO_PX: f32 = 14.;
const SMALL_PX: f32 = 13.;

/// What a role decides: everything about a run of text except the words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeSpec {
    /// Font size, in logical pixels **at 100% zoom** — the number the ladder is about.
    pub size: Pixels,
    /// The same font size in rems — the form that carries app zoom.
    pub length: Rems,
    /// Font weight — `NORMAL`, `MEDIUM` or `SEMIBOLD`, never heavier.
    pub weight: FontWeight,
    /// `Some(UI_MONO)` for the monospace roles, `None` for the proportional ones.
    pub family: Option<&'static str>,
    /// The role's default ink, read from the active palette.
    pub ink: u32,
}

/// A named place on sid's type scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeRole {
    /// The screen's page heading. One per screen.
    Display,
    /// A modal title or a detail pane's heading.
    Title,
    /// A card, panel or group header: body-sized, semibold, strong ink.
    Heading,
    /// Primary readable content: row labels, values, field text, copy.
    Body,
    /// The label inside a control — a button, a tab, a segment. Body-sized, medium, so a
    /// control's words read as something to press rather than as prose.
    Control,
    /// A field label or a small control's label. **Sentence case** — v2 retired the
    /// UPPERCASE section header this role used to carry.
    Label,
    /// Secondary orientation text: hints, counts, timestamps, footnotes, empty-state
    /// subtext, the words inside a badge.
    Meta,
    /// Monospace content: paths, addresses, hosts, ports, data cells.
    Mono,
    /// A proportional value inside a data table — "3 days ago", a commit subject — on the
    /// same rung as [`TypeRole::Mono`], because a table's columns differ by family and
    /// ink, never by size.
    Cell,
    /// Monospace content that is also secondary: a dim path, an opaque id, a hash.
    MonoMeta,
}

/// Every role, for the gallery's type specimen and for exhaustive tests.
pub const ALL_TYPE_ROLES: &[TypeRole] = &[
    TypeRole::Display,
    TypeRole::Title,
    TypeRole::Heading,
    TypeRole::Body,
    TypeRole::Control,
    TypeRole::Label,
    TypeRole::Meta,
    TypeRole::Mono,
    TypeRole::Cell,
    TypeRole::MonoMeta,
];

impl TypeRole {
    /// This role's font size in logical pixels at 100% zoom. One of exactly five values.
    pub const fn size(self) -> Pixels {
        match self {
            TypeRole::Display => px(DISPLAY_PX),
            TypeRole::Title => px(TITLE_PX),
            TypeRole::Heading | TypeRole::Body | TypeRole::Control => px(BODY_PX),
            TypeRole::Mono | TypeRole::Cell => px(MONO_PX),
            TypeRole::Label | TypeRole::Meta | TypeRole::MonoMeta => px(SMALL_PX),
        }
    }

    /// This role's font size as a zoomable length — [`TypeRole::size`] in rems.
    pub fn length(self) -> Rems {
        scaled(f32::from(self.size()))
    }

    /// This role's font weight.
    pub const fn weight(self) -> FontWeight {
        match self {
            TypeRole::Display | TypeRole::Title | TypeRole::Heading => FontWeight::SEMIBOLD,
            TypeRole::Control | TypeRole::Label => FontWeight::MEDIUM,
            TypeRole::Body
            | TypeRole::Meta
            | TypeRole::Mono
            | TypeRole::Cell
            | TypeRole::MonoMeta => FontWeight::NORMAL,
        }
    }

    /// The family this role overrides to, if any.
    pub const fn family(self) -> Option<&'static str> {
        match self {
            TypeRole::Mono | TypeRole::MonoMeta => Some(UI_MONO),
            _ => None,
        }
    }

    /// Whether this role reads as *secondary* — the dim half of the ladder.
    pub const fn is_secondary(self) -> bool {
        matches!(self, TypeRole::Label | TypeRole::Meta | TypeRole::MonoMeta)
    }

    /// Whether this role is a heading — the strong-ink, semibold top of the ladder.
    pub const fn is_heading(self) -> bool {
        matches!(
            self,
            TypeRole::Display | TypeRole::Title | TypeRole::Heading
        )
    }

    /// This role's default ink in `theme`.
    pub const fn ink(self, theme: &Theme) -> u32 {
        match self {
            TypeRole::Display | TypeRole::Title | TypeRole::Heading => theme.fg_strong,
            TypeRole::Body | TypeRole::Control | TypeRole::Mono | TypeRole::Cell => theme.fg,
            TypeRole::Label | TypeRole::Meta | TypeRole::MonoMeta => theme.muted,
        }
    }

    /// The whole decision, as one value — the pure function the renderer applies.
    pub fn spec(self, theme: &Theme) -> TypeSpec {
        TypeSpec {
            size: self.size(),
            length: self.length(),
            weight: self.weight(),
            family: self.family(),
            ink: self.ink(theme),
        }
    }

    /// The role's name, for the gallery specimen and for diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            TypeRole::Display => "display",
            TypeRole::Title => "title",
            TypeRole::Heading => "heading",
            TypeRole::Body => "body",
            TypeRole::Control => "control",
            TypeRole::Label => "label",
            TypeRole::Meta => "meta",
            TypeRole::Mono => "mono",
            TypeRole::Cell => "cell",
            TypeRole::MonoMeta => "mono-meta",
        }
    }
}

/// Apply a [`TypeRole`] to any `gpui` element.
///
/// These are the only sanctioned way to size text in sid — `tests/hygiene.rs` fails the
/// build on a raw `text_xs()`/`text_sm()`/`text_size(..)`/`font_weight(..)` outside this
/// module.
pub trait Typography: Styled + Sized {
    /// Apply `role`'s complete spec: size, weight, family, ink.
    fn text_role(self, role: TypeRole, theme: &Theme) -> Self {
        let spec = role.spec(theme);
        let sized = self.text_size(spec.length).font_weight(spec.weight);
        let familied = match spec.family {
            Some(family) => sized.font_family(family),
            None => sized,
        };
        familied.text_color(rgb(spec.ink))
    }

    /// [`TypeRole::Display`] — the screen's page heading.
    fn text_display(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Display, theme)
    }

    /// [`TypeRole::Title`] — a modal title or detail-pane heading.
    fn text_title(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Title, theme)
    }

    /// [`TypeRole::Heading`] — a card, panel or group header.
    fn text_heading(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Heading, theme)
    }

    /// [`TypeRole::Body`] — primary readable content.
    fn text_body(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Body, theme)
    }

    /// [`TypeRole::Control`] — the label inside a button, tab or segment.
    fn text_control(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Control, theme)
    }

    /// [`TypeRole::Label`] — a field or small-control label, sentence case.
    fn text_label(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Label, theme)
    }

    /// [`TypeRole::Meta`] — secondary orientation text.
    fn text_meta(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Meta, theme)
    }

    /// [`TypeRole::Mono`] — monospace content.
    fn text_mono(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Mono, theme)
    }

    /// [`TypeRole::Cell`] — a proportional value inside a data table.
    fn text_cell(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Cell, theme)
    }

    /// [`TypeRole::MonoMeta`] — monospace content that is also secondary.
    fn text_mono_meta(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::MonoMeta, theme)
    }
}

impl<T: Styled + Sized> Typography for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::brightness;
    use crate::theme::{cosmos, cosmos_light, dusk, void};
    use gpui::{Div, Hsla, StyleRefinement, div};

    fn palettes() -> [Theme; 4] {
        [cosmos(), void(), dusk(), cosmos_light()]
    }

    /// Read back a `Div`'s refined style — enough to assert a helper set what it
    /// claims, without a renderer.
    fn style_of(mut d: Div) -> StyleRefinement {
        d.style().clone()
    }

    #[test]
    fn the_whole_ladder_is_five_rungs() {
        // v2's ladder. A sixth size is a design conversation, not a call-site fix.
        let mut sizes: Vec<f32> = ALL_TYPE_ROLES.iter().map(|r| f32::from(r.size())).collect();
        sizes.sort_by(f32::total_cmp);
        sizes.dedup();
        assert_eq!(
            sizes,
            vec![13., 14., 15., 18., 22.],
            "the scale is 13/14/15/18/22"
        );
    }

    #[test]
    fn v2_is_bigger_where_text_is_read() {
        // The point of v2: body and meta each gained a pixel over v1's 14/12, and a
        // heading now outranks 16px.
        assert!(TypeRole::Body.size() > px(14.));
        assert!(TypeRole::Meta.size() > px(12.));
        assert!(TypeRole::Title.size() > px(16.));
    }

    #[test]
    fn the_ladder_is_ordered_display_over_title_over_body_over_meta() {
        assert!(TypeRole::Display.size() > TypeRole::Title.size());
        assert!(TypeRole::Title.size() > TypeRole::Body.size());
        assert!(TypeRole::Body.size() > TypeRole::Meta.size());
        // Heading and Control are body-sized; they differ by weight, not size.
        assert_eq!(TypeRole::Heading.size(), TypeRole::Body.size());
        assert_eq!(TypeRole::Control.size(), TypeRole::Body.size());
        // Mono sits one rung under body (wider glyphs), and its dim twin on meta's rung.
        assert!(TypeRole::Mono.size() < TypeRole::Body.size());
        assert!(TypeRole::Mono.size() > TypeRole::Meta.size());
        assert_eq!(TypeRole::MonoMeta.size(), TypeRole::Meta.size());
        assert_eq!(TypeRole::Label.size(), TypeRole::Meta.size());
        // A table's English column sits on the mono rung, not on body's.
        assert_eq!(TypeRole::Cell.size(), TypeRole::Mono.size());
    }

    #[test]
    fn nothing_ships_heavier_than_semibold() {
        for &role in ALL_TYPE_ROLES {
            let w = role.weight();
            assert!(
                w == FontWeight::NORMAL || w == FontWeight::MEDIUM || w == FontWeight::SEMIBOLD,
                "{}: {w:?} — 700 still blooms on dark at these sizes",
                role.name()
            );
        }
        // Semibold is exactly the heading half of the ladder.
        for &role in ALL_TYPE_ROLES {
            assert_eq!(
                role.weight() == FontWeight::SEMIBOLD,
                role.is_heading(),
                "{}",
                role.name()
            );
        }
        assert_eq!(TypeRole::Control.weight(), FontWeight::MEDIUM);
        assert_eq!(TypeRole::Label.weight(), FontWeight::MEDIUM);
        assert_eq!(TypeRole::Body.weight(), FontWeight::NORMAL);
    }

    #[test]
    fn exactly_the_mono_roles_carry_the_mono_family() {
        for &role in ALL_TYPE_ROLES {
            let is_mono = matches!(role, TypeRole::Mono | TypeRole::MonoMeta);
            assert_eq!(role.family().is_some(), is_mono, "{}", role.name());
        }
        assert_eq!(TypeRole::Mono.family(), Some(UI_MONO));
        assert_eq!(TypeRole::MonoMeta.family(), Some(UI_MONO));
    }

    #[test]
    fn secondary_roles_are_actually_dimmer_in_every_palette() {
        for t in palettes() {
            let bg = brightness(t.bg);
            let body_gap = (brightness(TypeRole::Body.ink(&t)) - bg).abs();
            for &role in ALL_TYPE_ROLES.iter().filter(|r| r.is_secondary()) {
                let gap = (brightness(role.ink(&t)) - bg).abs();
                assert!(
                    gap < body_gap,
                    "{}/{}: secondary ink must recede (gap {gap} vs body {body_gap})",
                    t.name,
                    role.name()
                );
            }
            for &role in ALL_TYPE_ROLES.iter().filter(|r| r.is_heading()) {
                let gap = (brightness(role.ink(&t)) - bg).abs();
                assert!(
                    gap >= body_gap,
                    "{}/{}: a heading may not be dimmer",
                    t.name,
                    role.name()
                );
            }
        }
    }

    #[test]
    fn is_secondary_agrees_with_the_ink_it_picks() {
        for t in palettes() {
            for &role in ALL_TYPE_ROLES {
                assert_eq!(
                    role.is_secondary(),
                    role.ink(&t) == t.muted,
                    "{}/{}",
                    t.name,
                    role.name()
                );
            }
        }
    }

    #[test]
    fn every_roles_ink_is_a_token_of_the_palette_it_was_given() {
        for t in palettes() {
            for &role in ALL_TYPE_ROLES {
                let ink = role.ink(&t);
                assert!(
                    [t.fg, t.fg_strong, t.muted].contains(&ink),
                    "{}/{}: {ink:#08x} is not one of this palette's text tokens",
                    t.name,
                    role.name()
                );
            }
        }
    }

    #[test]
    fn spec_is_the_four_parts_together() {
        let t = cosmos();
        assert_eq!(
            TypeRole::MonoMeta.spec(&t),
            TypeSpec {
                size: px(13.),
                length: crate::scale::scaled(13.),
                weight: FontWeight::NORMAL,
                family: Some(UI_MONO),
                ink: t.muted,
            }
        );
    }

    #[test]
    fn applying_a_role_sets_size_weight_family_and_ink() {
        let t = cosmos();
        let text = style_of(div().text_mono_meta(&t)).text.clone();
        assert_eq!(text.font_size, Some(TypeRole::MonoMeta.length().into()));
        assert_eq!(text.font_weight, Some(FontWeight::NORMAL));
        assert_eq!(text.font_family.as_deref(), Some(UI_MONO));
        assert_eq!(text.color, Some(Hsla::from(rgb(t.muted))));
    }

    #[test]
    fn a_proportional_role_leaves_the_family_alone() {
        let t = cosmos();
        let text = style_of(div().text_display(&t)).text.clone();
        assert_eq!(text.font_family, None, "only the mono roles set a family");
        assert_eq!(text.font_size, Some(TypeRole::Display.length().into()));
        assert_eq!(text.font_weight, Some(FontWeight::SEMIBOLD));
        assert_eq!(text.color, Some(Hsla::from(rgb(t.fg_strong))));
    }

    #[test]
    fn a_role_pins_weight_even_when_it_is_the_default() {
        // The cascade bug this module fixed: a hint inside a heading must not inherit the
        // heading's weight. A role is absolute.
        let t = cosmos();
        let text = style_of(div().text_meta(&t)).text.clone();
        assert_eq!(text.font_weight, Some(FontWeight::NORMAL));
    }

    #[test]
    fn each_helper_applies_its_own_role() {
        let t = cosmos();
        let by_helper: Vec<(TypeRole, StyleRefinement)> = vec![
            (TypeRole::Display, style_of(div().text_display(&t))),
            (TypeRole::Title, style_of(div().text_title(&t))),
            (TypeRole::Heading, style_of(div().text_heading(&t))),
            (TypeRole::Body, style_of(div().text_body(&t))),
            (TypeRole::Control, style_of(div().text_control(&t))),
            (TypeRole::Label, style_of(div().text_label(&t))),
            (TypeRole::Meta, style_of(div().text_meta(&t))),
            (TypeRole::Mono, style_of(div().text_mono(&t))),
            (TypeRole::Cell, style_of(div().text_cell(&t))),
            (TypeRole::MonoMeta, style_of(div().text_mono_meta(&t))),
        ];
        assert_eq!(by_helper.len(), ALL_TYPE_ROLES.len(), "one helper per role");
        for (role, style) in by_helper {
            let text = style.text.clone();
            let name = role.name();
            assert_eq!(text.font_size, Some(role.length().into()), "{name} size");
            assert_eq!(text.font_weight, Some(role.weight()), "{name} weight");
            assert_eq!(
                text.color,
                Some(Hsla::from(rgb(role.ink(&t)))),
                "{name} ink"
            );
        }
    }

    #[test]
    fn a_roles_emitted_length_is_its_authored_pixel_size_when_unzoomed() {
        for &role in ALL_TYPE_ROLES {
            assert_eq!(
                role.length()
                    .to_pixels(crate::scale::UiScale::DEFAULT.rem_size()),
                role.size(),
                "{}",
                role.name()
            );
        }
    }

    #[test]
    fn the_whole_ladder_moves_with_the_rem_size() {
        let zoomed = crate::scale::UiScale::from_percent(200).rem_size();
        assert_eq!(TypeRole::Display.length().to_pixels(zoomed), px(44.));
        assert_eq!(TypeRole::Body.length().to_pixels(zoomed), px(30.));
        assert_eq!(TypeRole::Meta.length().to_pixels(zoomed), px(26.));
    }

    #[test]
    fn applying_a_role_emits_the_zoomable_length_not_the_authored_pixel() {
        let t = cosmos();
        let text = style_of(div().text_body(&t)).text.clone();
        assert_eq!(
            text.font_size,
            Some(gpui::AbsoluteLength::Rems(TypeRole::Body.length())),
        );
    }

    #[test]
    fn role_names_are_unique_so_the_specimen_cannot_double_up() {
        let mut names: Vec<&str> = ALL_TYPE_ROLES.iter().map(|r| r.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
        assert_eq!(
            total, 10,
            "ten roles — adding an eleventh is a design decision"
        );
    }
}
