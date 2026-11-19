//! The sid theme system — ported from the POC (`sid-poc/crates/sid-ui/src/{theme,themes}.rs`).
//!
//! This is the **single source of colour** for the whole app: every other module reads
//! semantic tokens from here, and [`crate::bridge`] projects those same tokens onto
//! `gpui-component`'s 100-field `ThemeColor` so the borrowed library widgets render in
//! the active sid palette too.
//!
//! Every colour-bearing element reads semantic tokens from the active [`Theme`] (a gpui
//! [`Global`]) instead of embedding literal hex values; switching themes is one
//! [`install`] call + a window refresh. The four built-ins are the POC's palettes:
//! `cosmos` (the signature deep-space look), `void` (pure-black OLED monochrome),
//! `dusk` (warm amber), and `cosmos-light`.
//!
//! Tokens are `u32` `0xRRGGBB` values, passed straight to [`gpui::rgb`]. The POC's
//! `muted` doubled as dim-text-and-borders on a terminal grid; GPUI's small
//! anti-aliased type needs more contrast, so this port splits it into `muted`
//! (readable secondary text) and `faint` (decorative/disabled), and adds the
//! tokens a windowed UI has that a TUI doesn't: `well` (input/editor recesses),
//! `selection` (active row/tab fill), `fg_strong` (emphasized text).
//!
//! The active theme name persists as `sid_store::Settings::theme` (v4); resolution is
//! by name via [`by_name`], falling back to `cosmos` for unknown names (never an
//! error — a store written by a future sid with more themes must still open here).
//!
//! **Custom colours** (`Settings::theme_overrides`, v9) are a token-key -> `#rrggbb` map
//! laid over whichever base palette is active. [`install`] is the one choke point: it
//! always resolves `base palette + current overrides`, so startup, a palette switch and
//! `SID_THEME` all pick them up, and so does everything downstream of [`active`] (every
//! tab, the gpui-component [`crate::bridge`], the terminal's ANSI set). Overrides are
//! global, not per palette — they survive a palette switch. Resolution is lenient: an
//! unknown key or a malformed value is skipped, never an error.

use std::collections::BTreeMap;

use gpui::{App, Global};
use gpui_component::ThemeMode;

/// A complete semantic palette. Fields are `0xRRGGBB` for [`gpui::rgb`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Registry name (also what `Settings::theme` stores).
    pub name: &'static str,
    /// Window background.
    pub bg: u32,
    /// Raised panels: cards, modals, the tab strip, table headers.
    pub surface: u32,
    /// Recessed areas: text inputs, editors, terminal wells.
    pub well: u32,
    /// Hairline borders and separators.
    pub border: u32,
    /// Primary text.
    pub fg: u32,
    /// Emphasized text (active tab, headings).
    pub fg_strong: u32,
    /// Secondary/readable-dim text (subtitles, hints).
    pub muted: u32,
    /// Decorative/disabled — the faintest visible tone.
    pub faint: u32,
    /// The accent — selection highlights, primary buttons, the ✦ logo.
    pub accent: u32,
    /// Positive states: connected dots, OK results.
    pub success: u32,
    /// Cautionary states: connecting, degraded-secrets badge.
    pub warning: u32,
    /// Errors and destructive actions.
    pub danger: u32,
    /// Active row / selected item fill.
    pub selection: u32,
    /// The ink soft shadows are drawn in (`crate::elevation`). Pure black on the dark
    /// palettes — on a near-black canvas anything lighter reads as a glow, not a shadow —
    /// and a tinted indigo on the light one, so a card's shadow belongs to the palette
    /// instead of reading as a grey smudge. Alphas live in `elevation.rs`, never here.
    pub shadow: u32,
    /// The terminal's base-16 ANSI palette (normal 0-7, bright 8-15), in the standard
    /// black/red/green/yellow/blue/magenta/cyan/white order. kitty renders shell art
    /// through the USER's configured scheme, not stock xterm RGBs — sid does the same
    /// through the active theme, so terminal colors follow the theme like everything
    /// else (terminal-fidelity F2). Indices 16-255 stay the universal xterm cube/ramp.
    pub ansi: [u32; 16],
}

impl Global for Theme {}

/// The POC's signature theme — deep space, galaxy aesthetic, violet accent (red is reserved for `danger`).
pub fn cosmos() -> Theme {
    Theme {
        name: "cosmos",
        bg: 0x0b0b14,
        surface: 0x181626,
        well: 0x080810,
        border: 0x29263d,
        fg: 0xe9e6f3,
        fg_strong: 0xffffff,
        muted: 0x9893ab,
        faint: 0x554f6c,
        accent: 0x7c6cf2,
        success: 0xa8d8e8,
        warning: 0xe8b04a,
        danger: 0xff5570,
        selection: 0x242138,
        shadow: 0x000000,
        ansi: [
            0x1a1a26, 0xd44141, 0x77c48a, 0xe8b04a, 0x6a8dd8, 0xb07ad0, 0xa8d8e8, 0xc8c8d4,
            0x4a4a60, 0xff5570, 0x99e0a8, 0xf0c878, 0x8fb0f0, 0xd0a0e8, 0xc8ecf8, 0xffffff,
        ],
    }
}

/// Pure-black, near-monochrome — OLED-friendly maximum contrast.
pub fn void() -> Theme {
    Theme {
        name: "void",
        bg: 0x000000,
        surface: 0x101010,
        well: 0x050505,
        border: 0x262626,
        fg: 0xeeeeee,
        fg_strong: 0xffffff,
        muted: 0x999999,
        faint: 0x555555,
        accent: 0x7c6cf2,
        success: 0xc0c0c0,
        warning: 0xe0a040,
        danger: 0xff3333,
        selection: 0x1c1c1c,
        shadow: 0x000000,
        ansi: [
            0x000000, 0xd44141, 0x8fbf8f, 0xd0b070, 0x8090c0, 0xb090c0, 0x90c0c0, 0xcccccc,
            0x555555, 0xff3333, 0xa8d8a8, 0xe8cc90, 0xa0b0e0, 0xd0b0e0, 0xb0e0e0, 0xffffff,
        ],
    }
}

/// Warm dark theme with amber accents — the twilight palette.
pub fn dusk() -> Theme {
    Theme {
        name: "dusk",
        bg: 0x14100c,
        surface: 0x211b14,
        well: 0x0e0b08,
        border: 0x32291e,
        fg: 0xf0e5d0,
        fg_strong: 0xfff8ea,
        muted: 0xa6977c,
        faint: 0x605548,
        accent: 0xe87040,
        success: 0xa8d890,
        warning: 0xe8b04a,
        // A rose-red, not the old brick #d04a4a: that one was 4.00:1 on this palette's
        // own `surface` (an error sentence you had to lean in to read) and sat close
        // enough to the amber `accent` that a destructive button and an engage button
        // were the same warm smudge at a glance. `ansi[1]` keeps the brick — the
        // terminal's red is the user's shell colour, not a UI status ink.
        danger: 0xe05070,
        selection: 0x2b231a,
        shadow: 0x000000,
        ansi: [
            0x241d14, 0xd04a4a, 0x9ab86a, 0xe8b04a, 0x7a90a8, 0xc088a0, 0xa8c0b0, 0xd8c8b0,
            0x605548, 0xe87040, 0xb8d890, 0xf0c878, 0x98b0c8, 0xd8a8c0, 0xc0d8c8, 0xfff8ea,
        ],
    }
}

/// Light variant of cosmos — off-white canvas, darkened accents.
///
/// A hue that reads as "amber" or "cyan" against a near-black canvas is a *pale* one,
/// and pale ink on an off-white canvas is a smear: this palette used to carry cosmos's
/// tones over at roughly their original lightness, so `warning`, `success` and `muted`
/// all sat under the 4.5:1 floor as text (2.9, 3.7 and 4.1 against `surface`). The
/// status hues are now the *dark* member of their family — the same amber and teal,
/// deep enough to read as a sentence, as a 1px outline badge and as a status dot on
/// any of the three light surfaces. That flips which ink a filled swatch wants, and
/// [`crate::bridge::contrast_ink`] flips with it: the solid warning badge takes a light
/// label here, like every other tone in this palette. The fill follows the token, so no
/// call site has to know.
pub fn cosmos_light() -> Theme {
    Theme {
        name: "cosmos-light",
        bg: 0xf1f0f6,
        // v2: paper on the desk. v1's surface sat *below* the canvas, so every card in
        // the light theme read as a grey recess; a near-white card over an off-white
        // canvas is the convention every light UI uses, and the soft shadow
        // (`Theme::shadow`, `crate::elevation`) now does the separating a darker fill
        // used to.
        surface: 0xfdfcff,
        // Below `bg`, and well below `surface`: a recess is cut *into* the panel it sits
        // in. Pure white was once the brightest token in the palette, so every input,
        // editor and terminal well read as the one thing raised off the card.
        well: 0xebeaf2,
        border: 0xdcdae6,
        fg: 0x181824,
        fg_strong: 0x000000,
        muted: 0x5c5c6e,
        faint: 0xa0a0b0,
        // Violet, not red: red means "this destroys something" (`danger`) and nothing else.
        accent: 0x5b46d6,
        success: 0x246e7c,
        warning: 0x8a5f1a,
        // A deep crimson, a hue and a lightness away from the red `accent` — the same
        // separation cosmos draws between its #d44141 and #ff5570. The old #c03040 was
        // 1.13:1 from `accent` and the same hue family: "engage" and "destroy" were one
        // colour. `ansi[9]` keeps #c03040 as the terminal's bright red.
        danger: 0x8c1550,
        selection: 0xe7e5f0,
        shadow: 0x1d1a33,
        ansi: [
            0x181824, 0xb03030, 0x3a7a4a, 0xa07020, 0x3a5aa8, 0x8040a0, 0x2a7a8a, 0x8a8a98,
            0x707082, 0xc03040, 0x4a9a5a, 0xb08030, 0x5a7ac8, 0xa060c0, 0x408090, 0x4a4a58,
        ],
    }
}

/// Every built-in, in the order the settings screen's picker lists them.
pub const THEME_NAMES: &[&str] = &["cosmos", "void", "dusk", "cosmos-light"];

/// Resolve a persisted theme name. Unknown names fall back to [`cosmos`] — a store
/// written by a future sid (or a hand-edited value) must never fail to open.
pub fn by_name(name: &str) -> Theme {
    match name {
        "void" => void(),
        "dusk" => dusk(),
        "cosmos-light" => cosmos_light(),
        _ => cosmos(),
    }
}

// ---- custom colours -----------------------------------------------------------

/// One user-customisable colour token: its stable storage key (the field name, what
/// `Settings::theme_overrides` is keyed by), its label, the group the editor files it
/// under, and the field it writes.
pub struct ColorToken {
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    slot: fn(&mut Theme) -> &mut u32,
}

impl ColorToken {
    /// This token's value in `t`.
    pub fn get(&self, t: &Theme) -> u32 {
        *(self.slot)(&mut t.clone())
    }
}

/// The editor's groups, in display order.
pub const COLOR_GROUPS: [&str; 3] = ["Surfaces", "Text", "Accent and status"];

/// Every customisable token, grouped as the editor lists them. `shadow` is left out on
/// purpose (it is an ink for alpha-blended shadows, not a colour anyone sees as such), and
/// so is the terminal's `ansi` set.
pub const COLOR_TOKENS: [ColorToken; 13] = {
    const fn token(
        key: &'static str,
        label: &'static str,
        group: &'static str,
        slot: fn(&mut Theme) -> &mut u32,
    ) -> ColorToken {
        ColorToken {
            key,
            label,
            group,
            slot,
        }
    }
    let [surfaces, text, status] = COLOR_GROUPS;
    [
        token("bg", "Background", surfaces, |t| &mut t.bg),
        token("surface", "Panels", surfaces, |t| &mut t.surface),
        token("well", "Inputs", surfaces, |t| &mut t.well),
        token("border", "Borders", surfaces, |t| &mut t.border),
        token("selection", "Selection", surfaces, |t| &mut t.selection),
        token("fg", "Text", text, |t| &mut t.fg),
        token("fg_strong", "Headings", text, |t| &mut t.fg_strong),
        token("muted", "Secondary text", text, |t| &mut t.muted),
        token("faint", "Faint text", text, |t| &mut t.faint),
        token("accent", "Accent", status, |t| &mut t.accent),
        token("success", "Success", status, |t| &mut t.success),
        token("warning", "Warning", status, |t| &mut t.warning),
        token("danger", "Danger", status, |t| &mut t.danger),
    ]
};

/// `#rrggbb` or `rrggbb`, case-insensitive, surrounding whitespace ignored. Anything
/// else (short `#rgb`, alpha, a sign, a name) is `None`.
pub fn parse_hex(value: &str) -> Option<u32> {
    let value = value.trim();
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// A token as the `#rrggbb` string overrides are stored in.
pub fn to_hex(color: u32) -> String {
    format!("#{:06x}", color & 0xff_ffff)
}

/// `base` with every valid override applied. Unknown keys and malformed values are
/// skipped, so an empty (or all-garbage) map returns `base` unchanged.
pub fn with_overrides(mut base: Theme, overrides: &BTreeMap<String, String>) -> Theme {
    for token in &COLOR_TOKENS {
        if let Some(color) = overrides.get(token.key).and_then(|v| parse_hex(v)) {
            *(token.slot)(&mut base) = color;
        }
    }
    base
}

/// The overrides that would actually apply, normalised to lowercase `#rrggbb` — what the
/// editor treats as "this token is customised".
pub fn valid_overrides(overrides: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    COLOR_TOKENS
        .iter()
        .filter_map(|token| {
            let color = overrides.get(token.key).and_then(|v| parse_hex(v))?;
            Some((token.key.to_string(), to_hex(color)))
        })
        .collect()
}

/// Whether a text token is barely readable on the panels it sits on — the quiet
/// warning the editor shows on the row. Only the reading inks are checked (`faint` is
/// meant to recede); 3:1 is "very low", well under the 4.5:1 the built-ins clear.
pub fn low_contrast(t: &Theme, key: &str) -> bool {
    let ink = match key {
        "fg" => t.fg,
        "fg_strong" => t.fg_strong,
        "muted" => t.muted,
        _ => return false,
    };
    [t.bg, t.surface]
        .into_iter()
        .any(|bed| crate::bridge::contrast(ink, bed) < 3.0)
}

/// The process-wide override map ([`valid_overrides`] of whatever was set).
struct Overrides(BTreeMap<String, String>);

impl Global for Overrides {}

static NO_OVERRIDES: BTreeMap<String, String> = BTreeMap::new();

/// The active overrides, already validated and normalised.
pub fn overrides(cx: &App) -> &BTreeMap<String, String> {
    cx.try_global::<Overrides>().map_or(&NO_OVERRIDES, |o| &o.0)
}

/// Replace the overrides and, if a palette is already installed, re-install it with
/// them. Callers then [`crate::bridge::sync`] and refresh, exactly as after [`install`].
/// At startup call this *before* `install`.
pub fn set_overrides(overrides: &BTreeMap<String, String>, cx: &mut App) {
    let base = cx.try_global::<Theme>().map(|t| t.name);
    cx.set_global(Overrides(valid_overrides(overrides)));
    if let Some(base) = base {
        install(base, cx);
    }
}

/// The active base palette without any overrides — what a per-token reset returns to,
/// and the guaranteed-legible ink for the editor's own "Reset all".
pub fn base(cx: &App) -> Theme {
    by_name(active(cx).name)
}

/// Install `name`'s palette, with the current [`overrides`] laid over it, as the
/// process-wide active theme. Call once at startup (from the persisted
/// `Settings::theme`) and again whenever the settings screen switches themes (followed
/// by `cx.refresh_windows()` so every element re-reads it).
pub fn install(name: &str, cx: &mut App) {
    let theme = with_overrides(by_name(name), overrides(cx));
    cx.set_global(theme);
    // The one startup hook sid-ui owns, so `SID_REDUCED_MOTION` works without the app
    // shell having to know about it. Idempotent and one-way (it can only switch motion
    // off), so re-running on every theme switch cannot undo a Settings choice.
    crate::motion::init(cx);
}

/// The active theme. Elements call this at render time — never cache the result
/// across frames, or a theme switch won't take until some unrelated re-render.
pub fn active(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

/// Whether `theme` reads as a light palette — background luminance over the
/// midpoint. Only `cosmos-light` currently qualifies, but this is computed from
/// the palette's own colors (not name-matched) so a future light built-in needs
/// no second place updated.
pub fn is_light(theme: &Theme) -> bool {
    (theme.bg >> 16 & 0xff) > 128
}

/// The `gpui-component` `ThemeMode` matching `theme` — `Light` for a light sid
/// palette, `Dark` otherwise. `gpui-component` (the `Input`/`Table`/`PopupMenu`
/// widgets the SQL editor, results grid and context menus borrow) layers its own
/// theming system on top of gpui and knows nothing about sid's tokens.
///
/// This is only the *mode bit*; the palette itself is carried by
/// [`crate::bridge::theme_config`]. Callers want [`crate::bridge::sync`], which does
/// both — every window that mounts a `gpui_component::Root` must call it before first
/// paint (see `main.rs`'s startup window, `db_tab.rs`'s relationships-diagram pop-out,
/// and the settings screen's live switch, `ui::settings_tab::AppState::set_theme`).
pub fn component_mode(theme: &Theme) -> ThemeMode {
    if is_light(theme) {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_name_resolves_to_itself() {
        for &name in THEME_NAMES {
            assert_eq!(by_name(name).name, name);
        }
    }

    #[test]
    fn unknown_name_falls_back_to_cosmos() {
        assert_eq!(by_name("definitely-not-a-theme").name, "cosmos");
        assert_eq!(by_name("").name, "cosmos");
    }

    #[test]
    fn cosmos_matches_the_poc_palette() {
        let t = cosmos();
        assert_eq!(t.bg, 0x0b0b14);
        // v2 lifted the surface a step and tilted the neutrals toward violet; the
        // canvas is the POC's and stays. The accent moved red -> violet (red is
        // reserved for destructive actions).
        assert_eq!(t.surface, 0x181626);
        assert_eq!(t.border, 0x29263d);
    }

    #[test]
    fn light_theme_inverts_luminance() {
        let t = cosmos_light();
        // Background light, foreground dark — the POC's own doctest assertion.
        assert!(t.bg >> 16 > 128, "light background");
        assert!(t.fg >> 16 < 64, "dark foreground");
    }

    #[test]
    fn every_theme_ships_a_usable_ansi_palette() {
        // The terminal renders ls/prompt/art colors through this — each theme needs
        // 16 entries where the canonical hues are actually distinguishable (red vs
        // green vs blue) and bright-white differs from normal-white's slot.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let red = t.ansi[1];
            let green = t.ansi[2];
            let blue = t.ansi[4];
            assert!(
                (red >> 16 & 0xff) > (red & 0xff),
                "{}: ansi red leans red",
                t.name
            );
            assert!(
                (green >> 8 & 0xff) > (green >> 16 & 0xff),
                "{}: ansi green leans green",
                t.name
            );
            assert!(
                (blue & 0xff) > (blue >> 16 & 0xff),
                "{}: ansi blue leans blue",
                t.name
            );
            assert_ne!(t.ansi[7], t.ansi[15], "{}: white vs bright white", t.name);
        }
    }

    #[test]
    fn accent_is_violet_never_red_in_every_palette() {
        // Red is spent on destruction only; an accent whose red channel dominates
        // would put "engage" and "destroy" in the same family again. (dusk's amber
        // accent is orange, red-dominant by construction but a hue away from danger;
        // it is excluded.)
        for t in [cosmos(), void(), cosmos_light()] {
            let (r, b) = ((t.accent >> 16) & 0xff, t.accent & 0xff);
            assert!(b > r, "{}: accent leans blue/violet", t.name);
        }
    }

    #[test]
    fn dark_themes_keep_text_readable_against_bg() {
        // Cheap sanity: fg and muted must be far lighter than bg on the dark themes
        // (the POC's TUI-era `muted` was too dark for GPUI type — this guards the
        // split-token fix from regressing).
        for t in [cosmos(), void(), dusk()] {
            let lum = |c: u32| ((c >> 16 & 0xff) + (c >> 8 & 0xff) + (c & 0xff)) / 3;
            assert!(lum(t.fg) > lum(t.bg) + 120, "{}: fg readable", t.name);
            assert!(lum(t.muted) > lum(t.bg) + 80, "{}: muted readable", t.name);
        }
    }

    /// WCAG 2.1 contrast ratio between two `0xRRGGBB` tokens, `1.0..=21.0`.
    fn contrast(a: u32, b: u32) -> f32 {
        let luminance = |c: u32| {
            let channel = |shift: u32| {
                let v = ((c >> shift) & 0xff) as f32 / 255.;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
        };
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// A cheap perceptual distance between two `0xRRGGBB` tokens — the "redmean"
    /// approximation of CIE ΔE. A contrast ratio alone cannot answer "are these two
    /// tokens telling the user different things": two reds at the same lightness are
    /// 1.0:1 apart and still one colour, which is exactly cosmos-light's `accent` vs
    /// `danger`. This counts hue and lightness together.
    fn separation(a: u32, b: u32) -> f32 {
        let ch = |c: u32, shift: u32| ((c >> shift) & 0xff) as f32;
        let (r1, g1, b1) = (ch(a, 16), ch(a, 8), ch(a, 0));
        let (r2, g2, b2) = (ch(b, 16), ch(b, 8), ch(b, 0));
        let mean_r = (r1 + r2) / 2.;
        ((2. + mean_r / 256.) * (r1 - r2).powi(2)
            + 4. * (g1 - g2).powi(2)
            + (2. + (255. - mean_r) / 256.) * (b1 - b2).powi(2))
        .sqrt()
    }

    /// The floor two tokens that mean different things have to clear. Picked from the
    /// palettes that already read correctly: cosmos separates its red `accent` from its
    /// pink `danger` at 108, void at 81, dusk at 96 — cosmos-light's two reds sat at 36.
    const TONES_APART: f32 = 70.;

    #[test]
    fn accent_and_danger_are_distinguishable_in_every_palette() {
        // "Engage" and "this destroys something" are the two tones a user must never
        // have to read a label to tell apart, and they sit side by side on every form
        // footer. cosmos-light had them 1.13:1 and one hue apart: two reds.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let apart = separation(t.accent, t.danger);
            assert!(
                apart >= TONES_APART,
                "{}: accent and danger are {apart:.0} apart",
                t.name
            );
        }
    }

    #[test]
    fn well_is_recessed_relative_to_bg_in_every_palette() {
        // `well` is the bottom rung of the depth ladder (`.interface-design/system.md`):
        // inputs, editors and terminal wells are *cut into* the panel around them.
        // cosmos-light's was pure white — brighter than both `bg` and `surface`, so
        // every recess in the light palette read as raised.
        use crate::bridge::brightness;
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            assert!(
                brightness(t.well) < brightness(t.surface),
                "{}: well sits below the surface it is cut into",
                t.name
            );
            // ...and below the canvas too, wherever the canvas has room beneath it.
            // void's `bg` is pure black, the floor of the whole colour space, so its
            // well can only nudge *up* — the one palette this half cannot ask about.
            if t.bg != 0x000000 {
                assert!(
                    brightness(t.well) <= brightness(t.bg),
                    "{}: well sits below the canvas",
                    t.name
                );
            }
        }
    }

    #[test]
    fn every_palettes_status_inks_clear_aa_on_every_surface_they_land_on() {
        // The light pass's guard, widened. On a dark canvas the status hues are
        // *pale*, so they clear AA by construction and only `bg` was ever worth
        // checking; on an off-white canvas the same hues have to be dark, and an ink
        // checked against `bg` alone still smears on a card. So this sweeps every text
        // bed — and, since dusk's `danger` turned out to be a 4.0:1 smear on its own
        // surface, every palette.
        //
        // `accent` is swept on the light palettes only: on a dark built-in it is a
        // fill-first token that sits at 4.1:1 as text, which is a separate
        // (pre-existing) decision, not something this floor should silently re-open.
        //
        // `faint` is exempt everywhere — it is the decorative/disabled tone, the one
        // thing meant to recede (~2.2:1 in every built-in, dark ones included).
        // `selection` is exempt as a bed: it is a row fill, and a row's ink is `fg`.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let mut inks = vec![
                ("success", t.success),
                ("warning", t.warning),
                ("danger", t.danger),
            ];
            if is_light(&t) {
                inks.extend([
                    ("fg", t.fg),
                    ("fg_strong", t.fg_strong),
                    ("muted", t.muted),
                    ("accent", t.accent),
                ]);
            }
            for (ink_name, ink) in inks {
                for (bed_name, bed) in [("bg", t.bg), ("surface", t.surface), ("well", t.well)] {
                    let ratio = contrast(ink, bed);
                    assert!(
                        ratio >= 4.5,
                        "{}: {ink_name} on {bed_name} is {ratio:.2}:1",
                        t.name
                    );
                }
            }
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn an_empty_override_map_is_the_base_palette_exactly() {
        for &name in THEME_NAMES {
            assert_eq!(
                with_overrides(by_name(name), &BTreeMap::new()),
                by_name(name)
            );
        }
    }

    #[test]
    fn a_valid_override_is_applied_and_nothing_else_moves() {
        let got = with_overrides(cosmos(), &map(&[("accent", "#112233")]));
        assert_eq!(got.accent, 0x112233);
        assert_eq!(
            Theme {
                accent: cosmos().accent,
                ..got
            },
            cosmos()
        );
    }

    #[test]
    fn unknown_keys_and_malformed_values_are_ignored() {
        let got = with_overrides(
            cosmos(),
            &map(&[
                ("shadow", "#123456"),
                ("ansi", "#123456"),
                ("nope", "#123456"),
                ("bg", "#12345"),
                ("fg", "#fff"),
                ("muted", "#12345g"),
                ("accent", "+12345"),
                ("danger", "##123456"),
                ("warning", "#1234567"),
            ]),
        );
        assert_eq!(got, cosmos());
        assert!(valid_overrides(&map(&[("nope", "#123456"), ("bg", "x")])).is_empty());
    }

    #[test]
    fn hex_parsing_is_lenient_about_case_hash_and_whitespace_only() {
        for ok in ["#7c6cf2", "7c6cf2", "#7C6CF2", " 7c6cF2 "] {
            assert_eq!(parse_hex(ok), Some(0x7c6cf2), "{ok:?}");
        }
        for bad in ["", "#", "#7c6cf", "#7c6cf2ff", "#fff", "violet", "7c 6cf2"] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
        assert_eq!(to_hex(0x0a0b0c), "#0a0b0c");
    }

    #[test]
    fn overrides_are_normalised_and_survive_a_palette_switch() {
        let overrides = valid_overrides(&map(&[("accent", " 11AA33 ")]));
        assert_eq!(overrides, map(&[("accent", "#11aa33")]));
        for &name in THEME_NAMES {
            assert_eq!(with_overrides(by_name(name), &overrides).accent, 0x11aa33);
        }
    }

    #[test]
    fn every_token_key_is_unique_and_reads_its_own_field() {
        let mut keys: Vec<_> = COLOR_TOKENS.iter().map(|t| t.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), COLOR_TOKENS.len());
        // A distinct value per token: each one lands in its own field, none shares a slot.
        let mut t = cosmos();
        for (ix, token) in COLOR_TOKENS.iter().enumerate() {
            *(token.slot)(&mut t) = ix as u32;
        }
        for (ix, token) in COLOR_TOKENS.iter().enumerate() {
            assert_eq!(token.get(&t), ix as u32, "{}", token.key);
        }
        assert!(COLOR_TOKENS.iter().all(|t| COLOR_GROUPS.contains(&t.group)));
    }

    #[test]
    fn low_contrast_flags_unreadable_text_and_no_built_in() {
        for &name in THEME_NAMES {
            for token in &COLOR_TOKENS {
                assert!(
                    !low_contrast(&by_name(name), token.key),
                    "{name}: {}",
                    token.key
                );
            }
        }
        let silly = with_overrides(cosmos(), &map(&[("fg", "#0b0b14")]));
        assert!(low_contrast(&silly, "fg"));
        assert!(
            !low_contrast(&silly, "accent"),
            "only reading inks are checked"
        );
    }

    #[test]
    fn is_light_flags_only_the_light_palette() {
        assert!(is_light(&cosmos_light()));
        assert!(!is_light(&cosmos()));
        assert!(!is_light(&void()));
        assert!(!is_light(&dusk()));
    }

    #[test]
    fn component_mode_follows_is_light() {
        assert_eq!(component_mode(&cosmos_light()), ThemeMode::Light);
        assert_eq!(component_mode(&cosmos()), ThemeMode::Dark);
        assert_eq!(component_mode(&void()), ThemeMode::Dark);
        assert_eq!(component_mode(&dusk()), ThemeMode::Dark);
    }
}
