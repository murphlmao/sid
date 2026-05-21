use serde::{Deserialize, Serialize};

/// A 24-bit RGB colour token used throughout the theme system.
///
/// # Examples
///
/// ```
/// use sid_ui::theme::Color;
///
/// let c = Color::rgb(0x0b, 0x0b, 0x14);
/// assert_eq!(c.r, 0x0b);
/// assert_eq!(c.g, 0x0b);
/// assert_eq!(c.b, 0x14);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Construct a colour from raw RGB components.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme::Color;
    ///
    /// let black = Color::rgb(0, 0, 0);
    /// assert_eq!(black.r, 0);
    /// assert_eq!(black.g, 0);
    /// assert_eq!(black.b, 0);
    ///
    /// let white = Color::rgb(255, 255, 255);
    /// assert_eq!(white.r, 255);
    /// assert_eq!(white.g, 255);
    /// assert_eq!(white.b, 255);
    /// ```
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Convert a [`Color`] into a ratatui [`ratatui::style::Color::Rgb`].
///
/// # Examples
///
/// ```
/// use sid_ui::theme::Color;
/// use ratatui::style::Color as RatColor;
///
/// let c = Color::rgb(0x12, 0x34, 0x56);
/// let rc: RatColor = c.into();
/// assert!(matches!(rc, RatColor::Rgb(0x12, 0x34, 0x56)));
/// ```
impl From<Color> for ratatui::style::Color {
    fn from(c: Color) -> Self {
        ratatui::style::Color::Rgb(c.r, c.g, c.b)
    }
}

/// Unicode glyphs used in the TUI. Kept configurable so themes can swap them.
///
/// # Examples
///
/// ```
/// use sid_ui::theme::GlyphSet;
///
/// let g = GlyphSet::default();
/// assert_eq!(g.star, '★');
/// assert_eq!(g.small_star, '✦');
/// assert_eq!(g.dot, '·');
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GlyphSet {
    /// Large filled star — used as the primary logo glyph.
    pub star: char,
    /// Small four-pointed star — used as a tab title prefix.
    pub small_star: char,
    /// Middle dot — used as a subtle list separator.
    pub dot: char,
}

impl Default for GlyphSet {
    fn default() -> Self {
        Self {
            star: '★',
            small_star: '✦',
            dot: '·',
        }
    }
}

/// A complete colour theme for the sid TUI.
///
/// Every colour-bearing widget reads colours from a `Theme` rather than
/// embedding literal hex values. This keeps all colour decisions in one place
/// and makes theme-switching a single swap at the app level.
///
/// # Field semantics
///
/// - `background` — the main terminal canvas (darkest layer).
/// - `surface` — floating panels, pop-overs, and card backgrounds.
/// - `foreground` — default text colour on either `background` or `surface`.
/// - `muted` — de-emphasised text: timestamps, secondary metadata.
/// - `accent_primary` — interactive highlights and the active-tab indicator.
/// - `accent_success` — positive status; green/teal tones.
/// - `accent_warning` — cautionary status; amber/yellow tones.
/// - `accent_error` — error and destructive-action colour.
/// - `border` — widget border lines; should sit between background and surface.
/// - `glyphs` — the set of Unicode characters used for decoration.
///
/// # Examples
///
/// ```
/// use sid_ui::theme::{Color, GlyphSet, Theme};
///
/// let t = Theme {
///     name: "my-theme".into(),
///     background: Color::rgb(0x0b, 0x0b, 0x14),
///     surface: Color::rgb(0x13, 0x13, 0x1f),
///     foreground: Color::rgb(0xe6, 0xe6, 0xf0),
///     muted: Color::rgb(0x4a, 0x4a, 0x60),
///     accent_primary: Color::rgb(0xd4, 0x41, 0x41),
///     accent_success: Color::rgb(0xa8, 0xd8, 0xe8),
///     accent_warning: Color::rgb(0xe8, 0xb0, 0x4a),
///     accent_error: Color::rgb(0xff, 0x55, 0x70),
///     border: Color::rgb(0x1f, 0x1f, 0x2e),
///     glyphs: GlyphSet::default(),
/// };
/// assert_eq!(t.name, "my-theme");
/// // background is darker than surface
/// assert!(t.background.r <= t.surface.r);
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Theme {
    /// Human-readable theme name (e.g. `"cosmos"`, `"void"`).
    pub name: String,
    /// Main terminal canvas.
    pub background: Color,
    /// Floating panel / card background.
    pub surface: Color,
    /// Default text colour.
    pub foreground: Color,
    /// De-emphasised text.
    pub muted: Color,
    /// Interactive highlight / active-tab indicator.
    pub accent_primary: Color,
    /// Positive status.
    pub accent_success: Color,
    /// Cautionary status.
    pub accent_warning: Color,
    /// Error / destructive-action colour.
    pub accent_error: Color,
    /// Widget border lines.
    pub border: Color,
    /// Decoration glyphs.
    pub glyphs: GlyphSet,
}
