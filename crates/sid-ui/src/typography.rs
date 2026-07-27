//! The type scale — sid's font sizes, weights and families, decided once.
//!
//! # The problem this replaces
//!
//! Before this module the app had **222 hand-typed text-size calls** across 23 files
//! and no scale: 140 `text_xs()`, 75 `text_sm()`, a bare `text_size(px(14.))`, six
//! separate `const MONO` declarations holding *two different font names*, and four
//! weights chosen ad hoc (`BOLD` in five places, `MEDIUM` in five others). Worse, the
//! component crate's own `Badge`, `Card`, `List` row, `Toolbar` and table header set
//! **no size at all** — so the same badge rendered 12px inside a `text_xs` panel and
//! 16px at the root, because `gpui` text styles cascade. "Font sizes all over the
//! place" was a literal description of the source.
//!
//! # The scale
//!
//! Three sizes, two weights, one mono family. That is the whole vocabulary:
//!
//! | Role          | Size | Weight | Ink          | For                                     |
//! |---------------|------|--------|--------------|-----------------------------------------|
//! | [`Title`]     | 16px | Medium | `fg_strong`  | screen / panel / modal headings         |
//! | [`Body`]      | 14px | Normal | `fg`         | rows, values, fields, buttons, copy     |
//! | [`Label`]     | 12px | Medium | `muted`      | UPPERCASE section headers               |
//! | [`Meta`]      | 12px | Normal | `muted`      | hints, counts, timestamps, badge words  |
//! | [`Mono`]      | 14px | Normal | `fg`         | paths, addresses, data cells            |
//! | [`MonoMeta`]  | 12px | Normal | `muted`      | dim paths, ids, hashes in a row's tail  |
//!
//! Six roles, and every one of them is *absolute*: a role sets size **and** weight
//! **and** family **and** ink, so a `Meta` inside a `Title` is still 12px Normal. The
//! old helpers only set what they happened to care about, which is why the cascade
//! leaked. Colour stays overridable — `.text_meta(&t).text_color(rgb(t.danger))` is
//! the intended spelling for a dim *error* hint — but size, weight and family are the
//! role's to decide.
//!
//! # Why three sizes and not five
//!
//! sid is a dense instrument panel, not a document. A 12/14/16 ladder gives exactly
//! three legible steps at the distances involved; anything wider makes a tab strip
//! look like a landing page. Note that the top of the ladder is genuinely new — before
//! this module *no* heading was larger than body text, and the only 16px text in the
//! app was accidental (unsized elements inheriting `gpui`'s 16px default). Hierarchy
//! now comes from the scale rather than from bold-at-the-same-size.
//!
//! # Why two weights
//!
//! `BOLD` (700) at 12-16px on a dark panel blooms — it reads as a colour change, not
//! an emphasis, and sid already spends colour on meaning. `MEDIUM` (500) is the whole
//! emphasis budget, and it is spent on [`Title`] and [`Label`] only.
//!
//! # `px`, not `rems`
//!
//! `gpui`'s `text_xs()`/`text_sm()` are `rems(0.75)`/`rems(0.875)` against a 16px
//! `rem_size` sid never changes, so they were always 12px and 14px in disguise. Naming
//! the pixels removes a layer of indirection nothing was using, and matches how the
//! rest of the design system measures (`px_3`, `h(px(42.))`, the terminal's 14px cell).
//! `gpui` pixels are *logical* — the compositor scale factor is applied below this
//! layer — so a px scale is still HiDPI-correct.
//!
//! [`Title`]: TypeRole::Title
//! [`Body`]: TypeRole::Body
//! [`Label`]: TypeRole::Label
//! [`Meta`]: TypeRole::Meta
//! [`Mono`]: TypeRole::Mono
//! [`MonoMeta`]: TypeRole::MonoMeta

use gpui::{FontWeight, Pixels, Styled, px, rgb};

use crate::theme::Theme;

/// The UI's monospace family: paths, addresses, ids, data cells.
///
/// One name, in one place — it replaces six `const MONO` declarations that had drifted
/// into two different fonts. Deliberately a boring, universally-present family: this
/// text is *read*, not admired, and a missing font falls back to whatever the system
/// picks, which is how the app used to end up with two mono looks on one screen.
///
/// The **terminal grid** is not this. `sid-term` paints a PTY at a font the user
/// configures (CaskaydiaCove Nerd Font Mono by default, for the shell-prompt glyphs)
/// with kitty cell geometry; it is an instrument with its own metrics, not UI text.
pub const UI_MONO: &str = "DejaVu Sans Mono";

/// The three sizes on the ladder, in pixels. Nothing else is a size.
const TITLE_PX: f32 = 16.;
const BODY_PX: f32 = 14.;
const META_PX: f32 = 12.;

/// What a role decides: everything about a run of text except the words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeSpec {
    /// Font size, in logical pixels.
    pub size: Pixels,
    /// Font weight — only `NORMAL` or `MEDIUM` ever appear.
    pub weight: FontWeight,
    /// `Some(UI_MONO)` for the monospace roles, `None` for the proportional ones.
    pub family: Option<&'static str>,
    /// The role's default ink, read from the active palette. Overridable at the call
    /// site; the role only decides what it means to be *unremarkable*.
    pub ink: u32,
}

/// A named place on sid's type scale.
///
/// Call sites name the *role*, never a size — see the module docs for the ladder and
/// [`Typography`] for the helpers that apply one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeRole {
    /// A screen, panel or modal heading. The only step above body text.
    Title,
    /// Primary readable content: row labels, values, field text, button labels, copy.
    Body,
    /// An UPPERCASE section header. Uppercasing is the caller's (the string is theirs);
    /// the role supplies the size, the weight that keeps small caps legible, and the
    /// muted ink that keeps a header from competing with its own contents.
    Label,
    /// Secondary orientation text: hints, counts, timestamps, footnotes, empty-state
    /// subtext, the words inside a badge.
    Meta,
    /// Monospace content at body weight: paths, addresses, hosts, ports, data cells.
    Mono,
    /// Monospace content that is also secondary: a dim path, an opaque id, a hash.
    MonoMeta,
}

/// Every role, for the gallery's type specimen and for exhaustive tests.
pub const ALL_TYPE_ROLES: &[TypeRole] = &[
    TypeRole::Title,
    TypeRole::Body,
    TypeRole::Label,
    TypeRole::Meta,
    TypeRole::Mono,
    TypeRole::MonoMeta,
];

impl TypeRole {
    /// This role's font size. One of exactly three values.
    pub const fn size(self) -> Pixels {
        match self {
            TypeRole::Title => px(TITLE_PX),
            TypeRole::Body | TypeRole::Mono => px(BODY_PX),
            TypeRole::Label | TypeRole::Meta | TypeRole::MonoMeta => px(META_PX),
        }
    }

    /// This role's font weight. `MEDIUM` is the entire emphasis budget.
    pub const fn weight(self) -> FontWeight {
        match self {
            TypeRole::Title | TypeRole::Label => FontWeight::MEDIUM,
            TypeRole::Body | TypeRole::Meta | TypeRole::Mono | TypeRole::MonoMeta => {
                FontWeight::NORMAL
            }
        }
    }

    /// The family this role overrides to, if any.
    pub const fn family(self) -> Option<&'static str> {
        match self {
            TypeRole::Mono | TypeRole::MonoMeta => Some(UI_MONO),
            TypeRole::Title | TypeRole::Body | TypeRole::Label | TypeRole::Meta => None,
        }
    }

    /// Whether this role reads as *secondary* — the dim half of the ladder.
    pub const fn is_secondary(self) -> bool {
        matches!(self, TypeRole::Label | TypeRole::Meta | TypeRole::MonoMeta)
    }

    /// This role's default ink in `theme`.
    pub const fn ink(self, theme: &Theme) -> u32 {
        match self {
            TypeRole::Title => theme.fg_strong,
            TypeRole::Body | TypeRole::Mono => theme.fg,
            TypeRole::Label | TypeRole::Meta | TypeRole::MonoMeta => theme.muted,
        }
    }

    /// The whole decision, as one value — the pure function the renderer applies.
    pub const fn spec(self, theme: &Theme) -> TypeSpec {
        TypeSpec {
            size: self.size(),
            weight: self.weight(),
            family: self.family(),
            ink: self.ink(theme),
        }
    }

    /// The role's name, for the gallery specimen and for diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            TypeRole::Title => "title",
            TypeRole::Body => "body",
            TypeRole::Label => "label",
            TypeRole::Meta => "meta",
            TypeRole::Mono => "mono",
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
        let sized = self.text_size(spec.size).font_weight(spec.weight);
        let familied = match spec.family {
            Some(family) => sized.font_family(family),
            None => sized,
        };
        familied.text_color(rgb(spec.ink))
    }

    /// [`TypeRole::Title`] — a screen, panel or modal heading.
    fn text_title(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Title, theme)
    }

    /// [`TypeRole::Body`] — primary readable content.
    fn text_body(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Body, theme)
    }

    /// [`TypeRole::Label`] — an UPPERCASE section header (the caller uppercases).
    fn text_label(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Label, theme)
    }

    /// [`TypeRole::Meta`] — secondary orientation text.
    fn text_meta(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Meta, theme)
    }

    /// [`TypeRole::Mono`] — monospace content at body weight.
    fn text_mono(self, theme: &Theme) -> Self {
        self.text_role(TypeRole::Mono, theme)
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
    /// claims, without a renderer. Same trick as `styled.rs`'s tests.
    fn style_of(mut d: Div) -> StyleRefinement {
        d.style().clone()
    }

    #[test]
    fn the_whole_ladder_is_three_sizes() {
        // The point of the module. If a seventh role arrives wanting an 18px step, this
        // test is the conversation about whether sid is still an instrument panel.
        let mut sizes: Vec<f32> = ALL_TYPE_ROLES.iter().map(|r| f32::from(r.size())).collect();
        sizes.sort_by(f32::total_cmp);
        sizes.dedup();
        assert_eq!(sizes, vec![12., 14., 16.], "the scale is 12 / 14 / 16");
    }

    #[test]
    fn the_ladder_is_ordered_title_over_body_over_meta() {
        assert!(
            TypeRole::Title.size() > TypeRole::Body.size(),
            "a heading must outrank its contents by size, not only by weight"
        );
        assert!(TypeRole::Body.size() > TypeRole::Meta.size());
        // The mono roles ride the same two rungs as their proportional twins, so a
        // monospace address in a row lines up with the words beside it.
        assert_eq!(TypeRole::Mono.size(), TypeRole::Body.size());
        assert_eq!(TypeRole::MonoMeta.size(), TypeRole::Meta.size());
        assert_eq!(TypeRole::Label.size(), TypeRole::Meta.size());
    }

    #[test]
    fn only_two_weights_ship() {
        for &role in ALL_TYPE_ROLES {
            let w = role.weight();
            assert!(
                w == FontWeight::NORMAL || w == FontWeight::MEDIUM,
                "{}: {w:?} — the scale spends MEDIUM and nothing heavier",
                role.name()
            );
        }
        assert_eq!(TypeRole::Title.weight(), FontWeight::MEDIUM);
        assert_eq!(TypeRole::Label.weight(), FontWeight::MEDIUM);
        assert_eq!(TypeRole::Body.weight(), FontWeight::NORMAL);
    }

    #[test]
    fn exactly_the_mono_roles_carry_the_mono_family() {
        for &role in ALL_TYPE_ROLES {
            let is_mono = matches!(role, TypeRole::Mono | TypeRole::MonoMeta);
            assert_eq!(
                role.family().is_some(),
                is_mono,
                "{} family: {:?}",
                role.name(),
                role.family()
            );
        }
        // One family name, not six. The regression this pins: `const MONO` was declared
        // in six modules and two of them disagreed on the font.
        assert_eq!(TypeRole::Mono.family(), Some(UI_MONO));
        assert_eq!(TypeRole::MonoMeta.family(), Some(UI_MONO));
    }

    #[test]
    fn secondary_roles_are_actually_dimmer_in_every_palette() {
        // "Meta" has to be a visual fact, not just a name. `muted` sits between `fg`
        // and the background in all four palettes, so the comparison is directional:
        // dim means "closer to the background than body text is".
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
            // ...and the title has to advance, or the top of the ladder is decoration.
            let title_gap = (brightness(TypeRole::Title.ink(&t)) - bg).abs();
            assert!(
                title_gap >= body_gap,
                "{}: a heading may not be dimmer than its body",
                t.name
            );
        }
    }

    #[test]
    fn is_secondary_agrees_with_the_ink_it_picks() {
        // The classifier and the palette lookup are two statements of one fact; if they
        // ever disagree, `is_secondary` is lying to the gallery and to future roles.
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
                size: px(12.),
                weight: FontWeight::NORMAL,
                family: Some(UI_MONO),
                ink: t.muted,
            }
        );
    }

    #[test]
    fn applying_a_role_sets_size_weight_family_and_ink() {
        let t = cosmos();
        let s = style_of(div().text_mono_meta(&t));
        let text = s.text.clone().unwrap_or_default();
        assert_eq!(text.font_size, Some(px(12.).into()));
        assert_eq!(text.font_weight, Some(FontWeight::NORMAL));
        assert_eq!(text.font_family.as_deref().map(|f| &**f), Some(UI_MONO));
        assert_eq!(text.color, Some(Hsla::from(rgb(t.muted))));
    }

    #[test]
    fn a_proportional_role_leaves_the_family_alone() {
        let t = cosmos();
        let text = style_of(div().text_title(&t))
            .text
            .clone()
            .unwrap_or_default();
        assert_eq!(text.font_family, None, "only the mono roles set a family");
        assert_eq!(text.font_size, Some(px(16.).into()));
        assert_eq!(text.font_weight, Some(FontWeight::MEDIUM));
        assert_eq!(text.color, Some(Hsla::from(rgb(t.fg_strong))));
    }

    #[test]
    fn a_role_pins_weight_even_when_it_is_the_default() {
        // The cascade bug this fixes: `text_xs()` set a size and nothing else, so a hint
        // inside a bold header inherited the bold. A role is absolute.
        let t = cosmos();
        let text = style_of(div().text_meta(&t))
            .text
            .clone()
            .unwrap_or_default();
        assert_eq!(
            text.font_weight,
            Some(FontWeight::NORMAL),
            "Normal is stated, not assumed"
        );
    }

    #[test]
    fn each_helper_applies_its_own_role() {
        let t = cosmos();
        let by_helper: Vec<(&str, StyleRefinement)> = vec![
            (TypeRole::Title.name(), style_of(div().text_title(&t))),
            (TypeRole::Body.name(), style_of(div().text_body(&t))),
            (TypeRole::Label.name(), style_of(div().text_label(&t))),
            (TypeRole::Meta.name(), style_of(div().text_meta(&t))),
            (TypeRole::Mono.name(), style_of(div().text_mono(&t))),
            (
                TypeRole::MonoMeta.name(),
                style_of(div().text_mono_meta(&t)),
            ),
        ];
        for (name, style) in by_helper {
            let role = ALL_TYPE_ROLES
                .iter()
                .find(|r| r.name() == name)
                .expect("named role");
            let text = style.text.clone().unwrap_or_default();
            assert_eq!(text.font_size, Some(role.size().into()), "{name} size");
            assert_eq!(text.font_weight, Some(role.weight()), "{name} weight");
            assert_eq!(
                text.color,
                Some(Hsla::from(rgb(role.ink(&t)))),
                "{name} ink"
            );
        }
    }

    #[test]
    fn role_names_are_unique_so_the_specimen_cannot_double_up() {
        let mut names: Vec<&str> = ALL_TYPE_ROLES.iter().map(|r| r.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
        assert_eq!(
            total, 6,
            "six roles — adding a seventh is a design decision"
        );
    }
}
