//! The sid theme system — ported from the POC (`sid-poc/crates/sid-ui/src/{theme,themes}.rs`).
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

use gpui::{App, Global};

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
}

impl Global for Theme {}

/// The POC's signature theme — deep space, galaxy aesthetic, red accent.
pub fn cosmos() -> Theme {
    Theme {
        name: "cosmos",
        bg: 0x0b0b14,
        surface: 0x13131f,
        well: 0x080810,
        border: 0x1f1f2e,
        fg: 0xe6e6f0,
        fg_strong: 0xffffff,
        muted: 0x8a8a9a,
        faint: 0x4a4a60,
        accent: 0xd44141,
        success: 0xa8d8e8,
        warning: 0xe8b04a,
        danger: 0xff5570,
        selection: 0x1c1c2c,
    }
}

/// Pure-black, near-monochrome — OLED-friendly maximum contrast.
pub fn void() -> Theme {
    Theme {
        name: "void",
        bg: 0x000000,
        surface: 0x0a0a0a,
        well: 0x050505,
        border: 0x222222,
        fg: 0xeeeeee,
        fg_strong: 0xffffff,
        muted: 0x999999,
        faint: 0x555555,
        accent: 0xd44141,
        success: 0xc0c0c0,
        warning: 0xe0a040,
        danger: 0xff3333,
        selection: 0x161616,
    }
}

/// Warm dark theme with amber accents — the twilight palette.
pub fn dusk() -> Theme {
    Theme {
        name: "dusk",
        bg: 0x14100c,
        surface: 0x1c1812,
        well: 0x0e0b08,
        border: 0x2a221a,
        fg: 0xf0e5d0,
        fg_strong: 0xfff8ea,
        muted: 0x9a8c74,
        faint: 0x605548,
        accent: 0xe87040,
        success: 0xa8d890,
        warning: 0xe8b04a,
        danger: 0xd04a4a,
        selection: 0x241d14,
    }
}

/// Light variant of cosmos — off-white canvas, darkened accents.
pub fn cosmos_light() -> Theme {
    Theme {
        name: "cosmos-light",
        bg: 0xf4f4f8,
        surface: 0xeaeaf2,
        well: 0xffffff,
        border: 0xd0d0dc,
        fg: 0x181824,
        fg_strong: 0x000000,
        muted: 0x707082,
        faint: 0xa0a0b0,
        accent: 0xb03030,
        success: 0x408090,
        warning: 0xb08030,
        danger: 0xc03040,
        selection: 0xdedee8,
    }
}

/// Every built-in, in the order the settings screen's picker lists them.
// dead_code: consumed by the settings screen + theme sweep landing right after this
// module — remove the allow with the first consumer.
#[allow(dead_code)]
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

/// Install `name`'s palette as the process-wide active theme. Call once at startup
/// (from the persisted `Settings::theme`) and again whenever the settings screen
/// switches themes (followed by `cx.refresh_windows()` so every element re-reads it).
pub fn install(name: &str, cx: &mut App) {
    cx.set_global(by_name(name));
}

/// The active theme. Elements call this at render time — never cache the result
/// across frames, or a theme switch won't take until some unrelated re-render.
// dead_code: see THEME_NAMES above.
#[allow(dead_code)]
pub fn active(cx: &App) -> &Theme {
    cx.global::<Theme>()
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
        assert_eq!(t.accent, 0xd44141);
        assert_eq!(t.surface, 0x13131f);
        assert_eq!(t.border, 0x1f1f2e);
    }

    #[test]
    fn light_theme_inverts_luminance() {
        let t = cosmos_light();
        // Background light, foreground dark — the POC's own doctest assertion.
        assert!(t.bg >> 16 > 128, "light background");
        assert!(t.fg >> 16 < 64, "dark foreground");
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
}
