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
    /// The terminal's base-16 ANSI palette (normal 0-7, bright 8-15), in the standard
    /// black/red/green/yellow/blue/magenta/cyan/white order. kitty renders shell art
    /// through the USER's configured scheme, not stock xterm RGBs — sid does the same
    /// through the active theme, so terminal colors follow the theme like everything
    /// else (terminal-fidelity F2). Indices 16-255 stay the universal xterm cube/ramp.
    pub ansi: [u32; 16],
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
        ansi: [
            0x241d14, 0xd04a4a, 0x9ab86a, 0xe8b04a, 0x7a90a8, 0xc088a0, 0xa8c0b0, 0xd8c8b0,
            0x605548, 0xe87040, 0xb8d890, 0xf0c878, 0x98b0c8, 0xd8a8c0, 0xc0d8c8, 0xfff8ea,
        ],
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

/// Install `name`'s palette as the process-wide active theme. Call once at startup
/// (from the persisted `Settings::theme`) and again whenever the settings screen
/// switches themes (followed by `cx.refresh_windows()` so every element re-reads it).
pub fn install(name: &str, cx: &mut App) {
    cx.set_global(by_name(name));
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
/// palette, `Dark` otherwise. `gpui-component` (the `Input`/`Table` widgets the SQL
/// editor and results grid borrow) layers its own theming system on top of gpui and
/// knows nothing about sid's tokens, so every window that mounts a
/// `gpui_component::Root` must call
/// `gpui_component::Theme::change(component_mode(theme::active(cx)), ..)` before
/// first paint — see `main.rs`'s startup window, `db_tab.rs`'s
/// relationships-diagram pop-out, and the settings screen's live switch
/// (`ui::settings_tab::AppState::set_theme`).
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
    fn cosmos_ansi_red_is_the_galaxy_accent() {
        assert_eq!(cosmos().ansi[1], cosmos().accent);
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
