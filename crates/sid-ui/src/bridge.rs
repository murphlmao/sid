//! The theme bridge — **the keystone**: sid's 13 semantic tokens projected onto
//! `gpui_component`'s ~100-field `ThemeColor`.
//!
//! sid borrows real widgets from `gpui-component` (the process/port/results `Table`s,
//! the SQL editor `Input`, every `PopupMenu`) and will borrow more. Those widgets read
//! their colours from `gpui_component`'s own `Theme` global, not from [`crate::theme`],
//! so before this module existed the only thing sid told the library about its palette
//! was a single bit — `ThemeMode::Light | Dark` — and every borrowed widget rendered in
//! stock shadcn gray, a foreign object inside cosmos chrome.
//!
//! [`theme_config`] is the whole mapping, as one pure function: a sid [`Theme`] in, a
//! `ThemeConfig` out. [`apply`] pushes it into a `gpui_component::Theme` (the library's
//! own `apply_config`, its intended extension point, plus the handful of fields that
//! entry point cannot reach). [`sync`] does that to the process-wide global and
//! refreshes the window — call it from every site that changes the sid theme.
//!
//! ## Mapping rules
//!
//! Three of the library's names are false friends, and getting them wrong is what
//! "washed-out gray" looks like:
//!
//! - `ThemeColor::accent` is **not** sid's `accent`. It is the hover/active *background*
//!   for menu and list items, so it maps to sid's `selection`. sid's `accent` (the
//!   "engage" colour) maps to `primary`.
//! - `ThemeColor::muted` is a *background* (skeletons, switch tracks) and maps to
//!   `surface`; only `muted_foreground` is sid's `muted` text token.
//! - `ThemeColor::selection` is the *text*-selection fill inside an input, not sid's
//!   row-selection token; it maps to `accent` (the library clamps its alpha, see below).
//!
//! Filled swatches (primary/danger/success/warning buttons and badges) get their label
//! colour from [`contrast_ink`] rather than a fixed token, because no single token is
//! readable on every palette's fills: cosmos-light's `fg_strong` is pure black, which is
//! unreadable on its dark-red accent, while dusk's amber accent needs dark ink and its
//! `fg_strong` is near-white. Hover/pressed states are derived the same way — one step
//! toward the palette's lightest ink for hover, one step toward its darkest for pressed —
//! so every state stays inside the palette.
//!
//! ## What is deliberately left at library defaults
//!
//! - **The `highlight` (tree-sitter) style set.** [`sync`] runs the library's own
//!   `Theme::change` first, which installs the bundled per-mode highlight theme, and
//!   [`theme_config`] leaves `highlight: None` so that survives. Re-colouring SQL syntax
//!   tokens is a separate design pass, not part of the chrome bridge.
//! - **Fonts.** The library's Linux mono default (`DejaVu Sans Mono`) is already the
//!   family `sid` names for monospace content, and its 16px/13px base sizes already
//!   match. Nothing to correct.
//! - **`list_active` / `table_active` / `selection` alpha.** `apply_config` clamps these
//!   to 0.2/0.2/0.3 on purpose (they are drawn *over* a row, not instead of it). They are
//!   mapped to `accent` precisely so the clamp yields a subtle accent wash on the active
//!   row instead of an invisible one.

use std::rc::Rc;

use gpui::{App, Hsla, SharedString, Window};
use gpui_component::{ThemeColor, ThemeConfig, ThemeConfigColors};

use crate::theme::{self, Theme};

/// The theme-agnostic modal scrim: black at 66% alpha.
///
/// The one documented raw-colour exemption in the design system (`.interface-design/
/// system.md`) — a scrim must darken *whatever* is behind it, so it cannot follow a
/// palette. Centralised here so no call site has to type a hex literal, and reused as
/// the library's `overlay` colour by [`theme_config`].
pub const SCRIM: u32 = 0x000000a8;

// ---- token arithmetic -------------------------------------------------------
//
// Tokens are `0xRRGGBB`; these helpers stay in that space so a derived colour is
// still expressible as a token and the "semantic tokens are the only colour source"
// rule holds for hover/pressed/label colours too.

/// Split a `0xRRGGBB` token into its channels.
#[inline]
const fn channels(color: u32) -> (u32, u32, u32) {
    ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff)
}

/// Perceived brightness of a token, `0.0..=1.0`, using the sRGB luma weights
/// (0.2126 R + 0.7152 G + 0.0722 B). Not gamma-corrected — this only has to sort
/// "needs light ink" from "needs dark ink", and luma does that correctly for the
/// amber/red/cyan fills sid actually uses, where a plain channel average does not.
pub fn brightness(color: u32) -> f32 {
    let (r, g, b) = channels(color);
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

/// WCAG 2.1 relative luminance of a `0xRRGGBB` token, `0.0..=1.0`.
///
/// Unlike [`brightness`] (a cheap sort key), this is gamma-corrected — the curve a
/// reader's eye actually follows, which compresses hard near black. That compression is
/// exactly why a fixed-percentage mix reads as a clear step on a light palette and as
/// almost nothing on a near-black one: the same RGB delta is a much smaller share of
/// the perceptual range down there. [`raised_surface`]'s minimum-step floor is stated in
/// this space rather than in raw RGB for that reason.
fn relative_luminance(color: u32) -> f32 {
    let (r, g, b) = channels(color);
    let channel = |v: u32| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// A token as hue, saturation and lightness, each `0.0..=1.0`, through gpui's own
/// conversion — the same one the renderer uses, so a colour read here is the colour
/// that lands on screen.
///
/// Hue and saturation are what make a palette's `surface` *that palette's* surface;
/// lightness is the only one of the three [`raised_surface`] is allowed to move.
fn hsl(color: u32) -> (f32, f32, f32) {
    let c: Hsla = gpui::rgb(color).into();
    (c.h, c.s, c.l)
}

/// Blend `factor` of `toward` into `color` (0.0 = unchanged, 1.0 = `toward`).
pub fn mix(color: u32, toward: u32, factor: f32) -> u32 {
    let f = factor.clamp(0.0, 1.0);
    let (r1, g1, b1) = channels(color);
    let (r2, g2, b2) = channels(toward);
    let ch = |a: u32, b: u32| {
        (a as f32 + (b as f32 - a as f32) * f)
            .round()
            .clamp(0.0, 255.0) as u32
    };
    (ch(r1, r2) << 16) | (ch(g1, g2) << 8) | ch(b1, b2)
}

/// The palette's lightest ink — white-ish in every built-in. Dark palettes have it as
/// `fg_strong`; a light palette's `fg_strong` is black, so there it is `bg`, the
/// lightest surface such a palette has.
///
/// It used to be `well`, back when a light `well` was pure white. It no longer is —
/// `well` is the bottom rung of the depth ladder in every palette now, so on a light
/// one it is the *darkest* surface, the opposite of what this asks for.
pub fn light_ink(t: &Theme) -> u32 {
    if theme::is_light(t) {
        t.bg
    } else {
        t.fg_strong
    }
}

/// The palette's darkest ink — black-ish in every built-in. Mirror of [`light_ink`]:
/// a light palette's `fg_strong` *is* the darkest ink; a dark palette's `bg` is.
pub fn dark_ink(t: &Theme) -> u32 {
    if theme::is_light(t) {
        t.fg_strong
    } else {
        t.bg
    }
}

/// A label colour that stays readable on top of `fill`.
///
/// Replaces the "near-black label" hack that filled badges hand-rolled: derived from
/// the fill's own brightness, not from the tone's name, without a raw hex literal. The
/// dark palettes' pale amber `warning` gets dark ink and their dark-red `accent` gets
/// light ink — and cosmos-light, whose `warning` has to be a deep amber to be readable
/// as text, gets a light label on that same badge.
pub fn contrast_ink(t: &Theme, fill: u32) -> u32 {
    if brightness(fill) > 0.5 {
        dark_ink(t)
    } else {
        light_ink(t)
    }
}

/// `color` one step toward the palette's lightest ink — the house hover state for a
/// filled control.
pub fn hover_of(t: &Theme, color: u32) -> u32 {
    mix(color, light_ink(t), 0.12)
}

/// `color` one step toward the palette's darkest ink — the house pressed/active state
/// for a filled control.
pub fn pressed_of(t: &Theme, color: u32) -> u32 {
    mix(color, dark_ink(t), 0.14)
}

/// A step above `surface` for chrome with no scrim to separate it from the panel it
/// floats over — a `PopupMenu`/popover, a tooltip.
///
/// `Elevation::Overlay` (sid-ui's own modal/card depth ladder) can share `surface`'s
/// fill for a modal because the scrim underneath does the separating; a popover has no
/// scrim, so on a `surface`-backed panel only the hairline told them apart.
///
/// **Only the lightness moves.** The raise walks `surface`'s HSL L toward `fg`'s —
/// the reading ink, which sits at the opposite end of the palette's brightness from
/// `surface` in every built-in — so the same branchless formula still lightens the three
/// dark palettes and darkens cosmos-light, but the popover comes out as the *same
/// material lit brighter* rather than as a different one. Mixing the whole colour toward
/// `fg` dragged the saturation out with it: far enough up to clear [`MIN_LUMA_STEP`],
/// cosmos's navy `surface` (hsl 240, s 0.24) landed on `#4a4a55` — grey, s 0.07 — and
/// dusk's warm brown went the same way. Pinned by
/// `a_popover_keeps_its_surfaces_hue`, which was RED on exactly that.
///
/// The floor itself is unchanged and still does the real work: a flat percentage is
/// almost nothing near black (the 7% mix moved ~0.007 of relative luma on void), so the
/// step is stated in relative luminance and walked until it is real. cosmos-light's
/// needs 3% of the way to `fg`, void's 27%, cosmos's 32%.
pub fn raised_surface(t: &Theme) -> u32 {
    let base = relative_luminance(t.surface);
    let (_, _, from) = hsl(t.surface);
    let (_, _, to) = hsl(t.fg);
    // 1, 2, 3, ..., 100 percent of the way from `surface`'s lightness to `fg`'s. The
    // walk is monotonic — one endpoint is the surface and the other is the ink whose
    // contrast hunted it (see the module docs' contrast law) — so each step moves
    // further from `base` than the last, and the loop always terminates.
    (1..=100)
        .map(|pct: i32| with_lightness(t.surface, from + (to - from) * pct as f32 / 100.0))
        .find(|&candidate| (relative_luminance(candidate) - base).abs() >= MIN_LUMA_STEP)
        .unwrap_or(t.fg)
}

/// `color` at a different lightness, its hue and saturation untouched.
fn with_lightness(color: u32, l: f32) -> u32 {
    let (h, s, _) = hsl(color);
    let rgba = gpui::Rgba::from(Hsla {
        h,
        s,
        l: l.clamp(0.0, 1.0),
        a: 1.0,
    });
    let channel = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u32;
    (channel(rgba.r) << 16) | (channel(rgba.g) << 8) | channel(rgba.b)
}

/// The smallest relative-luminance step [`raised_surface`] must clear above `surface`.
///
/// WCAG relative luminance compresses hard near black, so a flat 7% RGB mix that reads
/// as a clear step on cosmos-light (surface near white) can move as little as ~0.007 of
/// relative luma on a near-black surface (void's `0x0a0a0a`) — the popover then sits a
/// few RGB values from the selected row (`0x161616`) it is supposed to separate from.
/// 0.06 is small enough that no palette needs a light/dark branch to clear it, and large
/// enough that a near-black surface's popover reads as a distinct panel.
const MIN_LUMA_STEP: f32 = 0.06;

/// `0xRRGGBB` as the `#RRGGBB` string the library's config parser accepts.
fn hex(color: u32) -> Option<SharedString> {
    Some(format!("#{:06x}", color & 0xff_ffff).into())
}

/// `0xRRGGBBAA` as the `#RRGGBBAA` string the library's config parser accepts.
fn hex_alpha(color: u32) -> Option<SharedString> {
    Some(format!("#{color:08x}").into())
}

// ---- the mapping ------------------------------------------------------------

/// The `gpui-component` theme configuration for a sid palette. Pure — no globals, no
/// window — so the mapping is unit-testable on its own.
pub fn theme_config(t: &Theme) -> ThemeConfig {
    ThemeConfig {
        is_default: false,
        name: format!("sid-{}", t.name).into(),
        mode: theme::component_mode(t),
        // Fonts: library defaults already match sid (see module docs).
        font_size: None,
        font_family: None,
        mono_font_family: None,
        mono_font_size: None,
        // `rounded_md` — the corner spec every sid row/chip/card already uses.
        radius: Some(6),
        radius_lg: Some(8),
        // Design law: depth is borders + surface shifts. No shadows, anywhere.
        shadow: Some(false),
        colors: config_colors(t),
        // Left at the library's per-mode default — see module docs.
        highlight: None,
    }
}

/// The colour block of [`theme_config`].
///
/// `ThemeConfigColors` keeps its 12 `base.*` fields private, which makes struct-literal
/// construction impossible from outside the library (E0451) — even with a functional
/// update. Field assignment on a `default()` is the only way in; [`apply`] sets the
/// unreachable `base.*` block afterwards, straight onto the resulting `ThemeColor`.
fn config_colors(t: &Theme) -> ThemeConfigColors {
    let mut colors = ThemeConfigColors::default();
    map_colors(&mut colors, t);
    colors
}

#[rustfmt::skip]
fn map_colors(colors: &mut ThemeConfigColors, t: &Theme) {
    // Derived states, named once so the assignments below read as decisions.
    let ink_on_accent = contrast_ink(t, t.accent);
    let ink_on_danger = contrast_ink(t, t.danger);
    let ink_on_success = contrast_ink(t, t.success);
    let ink_on_warning = contrast_ink(t, t.warning);
    // sid has no "info" token; the palette's own ANSI blue is its blue.
    let info = t.ansi[4];
    let ink_on_info = contrast_ink(t, info);

    // -- canvas ---------------------------------------------------------
    colors.background = hex(t.bg);
    colors.foreground = hex(t.fg);
    colors.border = hex(t.border);
    // `muted` is a BACKGROUND in this library (skeletons, switch tracks).
    colors.muted = hex(t.surface);
    colors.muted_foreground = hex(t.muted);
    // The theme-agnostic scrim behind dialogs/sheets.
    colors.overlay = hex_alpha(SCRIM);
    colors.window_border = hex(t.border);
    colors.tiles = hex(t.bg);

    // -- primary = sid's "engage" accent ---------------------------------
    colors.primary = hex(t.accent);
    colors.primary_foreground = hex(ink_on_accent);
    colors.primary_hover = hex(hover_of(t, t.accent));
    colors.primary_active = hex(pressed_of(t, t.accent));

    // -- secondary = a raised surface, the default control chrome --------
    colors.secondary = hex(t.surface);
    colors.secondary_foreground = hex(t.fg);
    colors.secondary_hover = hex(t.selection);
    colors.secondary_active = hex(pressed_of(t, t.surface));

    // -- status fills ---------------------------------------------------
    colors.danger = hex(t.danger);
    colors.danger_foreground = hex(ink_on_danger);
    colors.danger_hover = hex(hover_of(t, t.danger));
    colors.danger_active = hex(pressed_of(t, t.danger));
    colors.success = hex(t.success);
    colors.success_foreground = hex(ink_on_success);
    colors.success_hover = hex(hover_of(t, t.success));
    colors.success_active = hex(pressed_of(t, t.success));
    colors.warning = hex(t.warning);
    colors.warning_foreground = hex(ink_on_warning);
    colors.warning_hover = hex(hover_of(t, t.warning));
    colors.warning_active = hex(pressed_of(t, t.warning));
    colors.info = hex(info);
    colors.info_foreground = hex(ink_on_info);
    colors.info_hover = hex(hover_of(t, info));
    colors.info_active = hex(pressed_of(t, info));
    colors.chart_bullish = hex(t.success);
    colors.chart_bearish = hex(t.danger);

    // -- interaction ----------------------------------------------------
    // NOT sid's accent: this is the menu/list-item hover background.
    colors.accent = hex(t.selection);
    colors.accent_foreground = hex(t.fg);
    colors.ring = hex(t.accent);
    colors.caret = hex(t.accent);
    // Input text selection. Alpha-clamped to 0.3 upstream — intended.
    colors.selection = hex(t.accent);
    colors.input = hex(t.border);
    colors.link = hex(t.accent);
    colors.link_hover = hex(hover_of(t, t.accent));
    colors.link_active = hex(pressed_of(t, t.accent));
    colors.drag_border = hex(t.accent);
    colors.drop_target = hex(t.selection);

    // -- lists ----------------------------------------------------------
    colors.list = hex(t.bg);
    colors.list_head = hex(t.surface);
    // No zebra striping in this design system: even rows match odd rows.
    colors.list_even = hex(t.bg);
    colors.list_hover = hex(t.selection);
    colors.list_active = hex(t.accent);
    colors.list_active_border = hex(t.accent);

    // -- tables (the System/Network/results grids) ----------------------
    colors.table = hex(t.bg);
    // The fix for the washed-out gray process-table header.
    colors.table_head = hex(t.surface);
    colors.table_head_foreground = hex(t.muted);
    colors.table_row_border = hex(t.border);
    colors.table_even = hex(t.bg);
    colors.table_hover = hex(t.selection);
    colors.table_active = hex(t.accent);
    colors.table_active_border = hex(t.accent);

    // -- popovers / menus ----------------------------------------------
    // Raised one step above `surface`, not equal to it: a `PopupMenu`/popover floats
    // over a `surface` panel with no scrim between them, unlike a modal. `Tooltip`
    // reads this same field (gpui-component 0.6.1 has no separate `tooltip` colour).
    colors.popover = hex(raised_surface(t));
    colors.popover_foreground = hex(t.fg);

    // -- chrome ---------------------------------------------------------
    colors.title_bar = hex(t.surface);
    colors.title_bar_border = hex(t.border);
    colors.tab = hex(t.bg);
    colors.tab_foreground = hex(t.muted);
    colors.tab_active = hex(t.surface);
    colors.tab_active_foreground = hex(t.fg_strong);
    colors.tab_bar = hex(t.bg);
    colors.tab_bar_segmented = hex(t.surface);
    // Design law: sidebars share `bg` with the canvas.
    colors.sidebar = hex(t.bg);
    colors.sidebar_foreground = hex(t.fg);
    colors.sidebar_border = hex(t.border);
    colors.sidebar_accent = hex(t.selection);
    colors.sidebar_accent_foreground = hex(t.fg_strong);
    colors.sidebar_primary = hex(t.accent);
    colors.sidebar_primary_foreground = hex(ink_on_accent);

    // -- containers -----------------------------------------------------
    colors.group_box = hex(t.surface);
    colors.group_box_foreground = hex(t.fg);
    colors.group_box_title_foreground = hex(t.muted);
    colors.accordion = hex(t.surface);
    colors.description_list_label = hex(t.surface);
    colors.description_list_label_foreground = hex(t.muted);

    // -- indicators -----------------------------------------------------
    colors.progress_bar = hex(t.accent);
    colors.slider_bar = hex(t.accent);
    colors.slider_thumb = hex(light_ink(t));
    colors.switch = hex(t.faint);
    colors.switch_thumb = hex(light_ink(t));
    colors.skeleton = hex(t.surface);
    colors.scrollbar = hex(t.bg);
    colors.scrollbar_thumb = hex(t.faint);
    colors.scrollbar_thumb_hover = hex(t.muted);

    // -- charts: the palette's own ANSI hues, so plots follow the theme --
    colors.chart_1 = hex(t.ansi[4]);
    colors.chart_2 = hex(t.ansi[6]);
    colors.chart_3 = hex(t.ansi[2]);
    colors.chart_4 = hex(t.ansi[3]);
    colors.chart_5 = hex(t.ansi[5]);

    // The 12 `base.*` slots are private in this struct and unreachable from here;
    // `apply` writes them straight onto the resulting `ThemeColor` instead.
}

/// Project a sid palette onto `out`.
///
/// Two steps: the library's own `apply_config` (which also fills in the derived
/// fields [`theme_config`] leaves unset), then the 12 `base.*` colours, which
/// `ThemeConfigColors` keeps private and so cannot travel through a config. They come
/// from the palette's ANSI set, which is exactly what they are: the theme's red,
/// green, blue, yellow, magenta and cyan.
pub fn apply(t: &Theme, out: &mut gpui_component::Theme) {
    out.apply_config(&Rc::new(theme_config(t)));
    base_colors(t, &mut out.colors);
}

/// The `base.*` colour block, straight from the palette's ANSI 1-6 (normal) and
/// 9-14 (bright) slots.
fn base_colors(t: &Theme, out: &mut ThemeColor) {
    let c = |i: usize| -> gpui::Hsla { gpui::rgb(t.ansi[i]).into() };
    out.red = c(1);
    out.red_light = c(9);
    out.green = c(2);
    out.green_light = c(10);
    out.yellow = c(3);
    out.yellow_light = c(11);
    out.blue = c(4);
    out.blue_light = c(12);
    out.magenta = c(5);
    out.magenta_light = c(13);
    out.cyan = c(6);
    out.cyan_light = c(14);
}

/// Sync `gpui-component`'s theme global with the active sid palette, and refresh
/// `window` so widgets already on screen re-read it.
///
/// Call this everywhere the sid theme is installed or changed: `main.rs`'s startup
/// window, the settings screen's live switch, and any second window that mounts its own
/// `gpui_component::Root`. Without it, every borrowed widget renders in stock shadcn.
pub fn sync(window: Option<&mut Window>, cx: &mut App) {
    let t = theme::active(cx).clone();
    // `Theme::change` creates the global if this is the first call, sets the mode, and
    // installs the bundled highlight theme for that mode (which our config keeps).
    gpui_component::Theme::change(theme::component_mode(&t), None, cx);
    apply(&t, gpui_component::Theme::global_mut(cx));
    if let Some(window) = window {
        window.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{cosmos, cosmos_light, dusk, void};
    use gpui::rgb;
    use gpui_component::ThemeMode;

    /// Build a component theme from a sid palette the same way [`sync`] does, minus
    /// the globals — the whole bridge, testable without an `App`.
    fn bridged(t: &Theme) -> gpui_component::Theme {
        let mut out = gpui_component::Theme::default();
        apply(t, &mut out);
        out
    }

    #[test]
    fn every_palette_round_trips_its_canvas_tokens() {
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let c = bridged(&t);
            assert_eq!(c.background, rgb(t.bg).into(), "{}: background", t.name);
            assert_eq!(c.foreground, rgb(t.fg).into(), "{}: foreground", t.name);
            assert_eq!(c.border, rgb(t.border).into(), "{}: border", t.name);
            assert_eq!(
                c.muted_foreground,
                rgb(t.muted).into(),
                "{}: muted_foreground",
                t.name
            );
            assert_eq!(c.primary, rgb(t.accent).into(), "{}: primary", t.name);
            assert_eq!(c.danger, rgb(t.danger).into(), "{}: danger", t.name);
            assert_eq!(c.success, rgb(t.success).into(), "{}: success", t.name);
            assert_eq!(c.warning, rgb(t.warning).into(), "{}: warning", t.name);
        }
    }

    #[test]
    fn every_palette_reskins_the_table_and_menu_surfaces() {
        // The commit's visible promise: table headers, popup menus and hover fills
        // come out of the sid palette instead of stock shadcn gray.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let c = bridged(&t);
            assert_eq!(c.table_head, rgb(t.surface).into(), "{}: head", t.name);
            assert_eq!(
                c.table_head_foreground,
                rgb(t.muted).into(),
                "{}: head fg",
                t.name
            );
            assert_eq!(c.table, rgb(t.bg).into(), "{}: table bg", t.name);
            assert_eq!(c.table_hover, rgb(t.selection).into(), "{}: hover", t.name);
            // Raised one step above `surface`, not equal to it — see `raised_surface`.
            assert_eq!(
                c.popover,
                rgb(raised_surface(&t)).into(),
                "{}: popover",
                t.name
            );
            assert_eq!(c.ring, rgb(t.accent).into(), "{}: ring", t.name);
            assert_eq!(c.caret, rgb(t.accent).into(), "{}: caret", t.name);
            // The library's `accent` is the list/menu hover fill, NOT sid's accent.
            assert_eq!(c.accent, rgb(t.selection).into(), "{}: accent", t.name);
        }
    }

    #[test]
    fn base_colors_follow_the_palettes_ansi_set() {
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let c = bridged(&t);
            assert_eq!(c.red, rgb(t.ansi[1]).into(), "{}: red", t.name);
            assert_eq!(c.green, rgb(t.ansi[2]).into(), "{}: green", t.name);
            assert_eq!(c.blue, rgb(t.ansi[4]).into(), "{}: blue", t.name);
            assert_eq!(
                c.cyan_light,
                rgb(t.ansi[14]).into(),
                "{}: cyan_light",
                t.name
            );
            assert_eq!(c.chart_1, rgb(t.ansi[4]).into(), "{}: chart_1", t.name);
        }
    }

    #[test]
    fn mode_and_shadow_follow_the_palette() {
        assert_eq!(bridged(&cosmos()).mode, ThemeMode::Dark);
        assert_eq!(bridged(&cosmos_light()).mode, ThemeMode::Light);
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            // Design law: borders + surface shifts, never shadows.
            assert!(!bridged(&t).shadow, "{}: shadow off", t.name);
            assert_eq!(bridged(&t).radius, gpui::px(6.), "{}: radius", t.name);
        }
    }

    #[test]
    fn the_config_is_named_after_the_palette() {
        // The library stores the config per mode and re-applies it on any later
        // `Theme::change`; the name is how a human tells cosmos from stock in a dump.
        assert_eq!(theme_config(&cosmos()).name.as_ref(), "sid-cosmos");
        assert_eq!(
            theme_config(&cosmos_light()).name.as_ref(),
            "sid-cosmos-light"
        );
    }

    #[test]
    fn filled_swatch_labels_stay_readable_in_every_palette() {
        // The reason `contrast_ink` exists: no single token works everywhere.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            for (label, fill) in [
                ("accent", t.accent),
                ("danger", t.danger),
                ("success", t.success),
                ("warning", t.warning),
            ] {
                let ink = contrast_ink(&t, fill);
                let delta = (brightness(ink) - brightness(fill)).abs();
                assert!(
                    delta > 0.3,
                    "{}: {label} label contrast too low ({delta:.2})",
                    t.name
                );
            }
        }
    }

    #[test]
    fn the_ink_follows_the_fill_not_the_tone_name() {
        // Pins the cases that motivated the helper: a *pale* amber fill needs a
        // near-black label (what `app.rs`'s badges hard-coded), a dark-red accent needs
        // white — including on cosmos-light, whose `fg_strong` is pure black.
        for t in [cosmos(), void(), dusk()] {
            assert_eq!(contrast_ink(&t, t.warning), dark_ink(&t), "{}", t.name);
        }
        for t in [cosmos(), void(), cosmos_light()] {
            assert_eq!(contrast_ink(&t, t.accent), light_ink(&t), "{}", t.name);
        }
        // dusk's accent is a bright amber-orange: dark ink, like a warning.
        assert_eq!(contrast_ink(&dusk(), dusk().accent), dark_ink(&dusk()));
        // And the mirror image — the whole reason the ink is derived from the fill
        // rather than the tone's name: cosmos-light's `warning` is a DEEP amber (a pale
        // one cannot be read as text on an off-white canvas), so the same helper hands
        // that badge a light label, like the rest of that palette's filled swatches.
        let light = cosmos_light();
        assert_eq!(contrast_ink(&light, light.warning), light_ink(&light));
    }

    #[test]
    fn light_and_dark_ink_are_actually_light_and_dark() {
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            assert!(brightness(light_ink(&t)) > 0.9, "{}: light ink", t.name);
            assert!(brightness(dark_ink(&t)) < 0.1, "{}: dark ink", t.name);
        }
    }

    #[test]
    fn hover_brightens_and_pressed_darkens() {
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let b = brightness(t.accent);
            assert!(brightness(hover_of(&t, t.accent)) > b, "{}: hover", t.name);
            assert!(
                brightness(pressed_of(&t, t.accent)) < b,
                "{}: pressed",
                t.name
            );
        }
    }

    /// WCAG 2.1 contrast ratio between two `0xRRGGBB` tokens, `1.0..=21.0`.
    fn contrast(a: u32, b: u32) -> f32 {
        let (x, y) = (relative_luminance(a), relative_luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn raised_surface_differs_from_surface_and_stays_readable() {
        // The bug: a `PopupMenu`/popover is `popover = surface` floating over a
        // `surface` panel, so only the hairline told them apart in every palette.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let popover = raised_surface(&t);
            assert_ne!(
                popover, t.surface,
                "{}: popover must differ from surface",
                t.name
            );
            let c = contrast(popover, t.fg);
            assert!(
                c >= 4.5,
                "{}: popover_foreground (fg) on the raised popover is {c:.2}:1",
                t.name
            );
        }
    }

    #[test]
    fn raised_surface_lightens_dark_palettes_and_darkens_cosmos_light() {
        // One formula, no light/dark branch: mixing toward `fg` (the reading ink, which
        // sits at the opposite brightness extreme from `surface` in every built-in)
        // lightens a dark palette's popover and darkens cosmos-light's automatically.
        for t in [cosmos(), void(), dusk()] {
            assert!(
                brightness(raised_surface(&t)) > brightness(t.surface),
                "{}: dark palette's popover should lighten",
                t.name
            );
        }
        let light = cosmos_light();
        assert!(
            brightness(raised_surface(&light)) < brightness(light.surface),
            "cosmos-light's popover should darken"
        );
    }

    #[test]
    fn a_popover_clears_a_minimum_step_above_surface_in_every_palette() {
        // void's `surface` (0x0a0a0a) and the `selection` fill under the row a popover
        // opens on top of (0x161616) are four RGB values apart. `raised_surface`'s 7%
        // mix toward `fg` moves only ~0.007 of relative luma there — WCAG relative
        // luminance compresses hard near black, so the same percentage that reads as a
        // clear step on cosmos-light is almost nothing this close to 0. Enforce a floor
        // in that space so every palette gets a visible step, not just the ones where a
        // flat percentage happened to be enough.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let delta =
                (relative_luminance(raised_surface(&t)) - relative_luminance(t.surface)).abs();
            assert!(
                delta >= MIN_LUMA_STEP,
                "{}: popover cleared only {delta:.4} relative luma above surface (floor {MIN_LUMA_STEP})",
                t.name
            );
        }
    }

    #[test]
    fn a_popover_keeps_its_surfaces_hue() {
        // The defect: raising the popover by mixing `surface` toward `fg` walks the
        // colour toward a near-neutral ink, so on cosmos the navy panel (hsl 240, s
        // 0.24) came out grey (#4a4a55, s 0.07) — a different *material* floating over
        // the panel, not the same one lit brighter. Hue angle alone does not catch it
        // (mixing two colours of the same hue keeps that hue); what collapses is the
        // saturation, so both are pinned here.
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            let (sh, ss, _) = hsl(t.surface);
            let (ph, ps, _) = hsl(raised_surface(&t));
            let drift = ((sh - ph).abs() * 360.0).min((1.0 - (sh - ph).abs()) * 360.0);
            assert!(
                drift <= 5.0,
                "{}: popover hue is {drift:.1} degrees off surface's",
                t.name
            );
            assert!(
                ps >= ss * 0.8,
                "{}: popover saturation collapsed from {ss:.3} to {ps:.3}",
                t.name
            );
        }
    }

    #[test]
    fn mix_interpolates_per_channel() {
        assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(mix(0x000000, 0xff0000, 0.5), 0x800000);
        // Out-of-range factors clamp rather than wrap.
        assert_eq!(mix(0x102030, 0xffffff, -1.0), 0x102030);
        assert_eq!(mix(0x102030, 0xffffff, 2.0), 0xffffff);
    }

    #[test]
    fn brightness_orders_the_palette_correctly() {
        let t = cosmos();
        assert!(brightness(t.bg) < brightness(t.surface));
        assert!(brightness(t.surface) < brightness(t.fg));
        assert!(brightness(t.fg) < brightness(t.fg_strong));
        // Luma, not a channel average: amber reads far brighter than dark red even
        // though their channel sums are close.
        assert!(brightness(t.warning) > brightness(t.accent) + 0.3);
    }

    #[test]
    fn the_scrim_is_translucent_black() {
        let c = bridged(&cosmos());
        assert_eq!(c.overlay, gpui::rgba(SCRIM).into());
        assert!(
            c.overlay.a > 0.0 && c.overlay.a < 1.0,
            "scrim is translucent"
        );
    }
}
