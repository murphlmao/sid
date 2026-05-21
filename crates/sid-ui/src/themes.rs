use crate::theme::{Color, GlyphSet, Theme};

/// The default sid theme — deep space, galaxy aesthetic with red accents.
///
/// # Palette
///
/// | Role            | Hex      |
/// |-----------------|----------|
/// | background      | `#0b0b14`|
/// | surface         | `#13131f`|
/// | foreground      | `#e6e6f0`|
/// | muted           | `#4a4a60`|
/// | accent_primary  | `#d44141`|
/// | accent_success  | `#a8d8e8`|
/// | accent_warning  | `#e8b04a`|
/// | accent_error    | `#ff5570`|
/// | border          | `#1f1f2e`|
///
/// # Examples
///
/// ```
/// use sid_ui::themes::cosmos;
///
/// let t = cosmos();
/// assert_eq!(t.name, "cosmos");
/// assert_eq!(t.background.r, 0x0b);
/// assert_eq!(t.accent_primary.r, 0xd4);
/// ```
pub fn cosmos() -> Theme {
    Theme {
        name: "cosmos".into(),
        background: Color::rgb(0x0b, 0x0b, 0x14),
        surface: Color::rgb(0x13, 0x13, 0x1f),
        foreground: Color::rgb(0xe6, 0xe6, 0xf0),
        muted: Color::rgb(0x4a, 0x4a, 0x60),
        accent_primary: Color::rgb(0xd4, 0x41, 0x41),
        accent_success: Color::rgb(0xa8, 0xd8, 0xe8),
        accent_warning: Color::rgb(0xe8, 0xb0, 0x4a),
        accent_error: Color::rgb(0xff, 0x55, 0x70),
        border: Color::rgb(0x1f, 0x1f, 0x2e),
        glyphs: GlyphSet::default(),
    }
}

/// Pure-black background, near-monochrome palette — maximum contrast, minimal colour.
///
/// Designed for displays and terminals where pure OLED black saves power or
/// where the user prefers stark monochrome aesthetics.
///
/// # Examples
///
/// ```
/// use sid_ui::themes::void;
///
/// let t = void();
/// assert_eq!(t.name, "void");
/// // Background is pure black
/// assert_eq!(t.background.r, 0x00);
/// assert_eq!(t.background.g, 0x00);
/// assert_eq!(t.background.b, 0x00);
/// ```
pub fn void() -> Theme {
    Theme {
        name: "void".into(),
        background: Color::rgb(0x00, 0x00, 0x00),
        surface: Color::rgb(0x0a, 0x0a, 0x0a),
        foreground: Color::rgb(0xee, 0xee, 0xee),
        muted: Color::rgb(0x55, 0x55, 0x55),
        accent_primary: Color::rgb(0xd4, 0x41, 0x41),
        accent_success: Color::rgb(0xc0, 0xc0, 0xc0),
        accent_warning: Color::rgb(0xe0, 0xa0, 0x40),
        accent_error: Color::rgb(0xff, 0x33, 0x33),
        border: Color::rgb(0x22, 0x22, 0x22),
        glyphs: GlyphSet::default(),
    }
}

/// Warmer dark theme with amber accents — twilight palette.
///
/// Evokes a sunset or dim-lit workspace; amber and orange tones replace the
/// cold blue-grays of cosmos.
///
/// # Examples
///
/// ```
/// use sid_ui::themes::dusk;
///
/// let t = dusk();
/// assert_eq!(t.name, "dusk");
/// // Background has a warm (red > blue) tint
/// assert!(t.background.r > t.background.b);
/// // Accent is amber/orange
/// assert!(t.accent_primary.r > t.accent_primary.g);
/// ```
pub fn dusk() -> Theme {
    Theme {
        name: "dusk".into(),
        background: Color::rgb(0x14, 0x10, 0x0c),
        surface: Color::rgb(0x1c, 0x18, 0x12),
        foreground: Color::rgb(0xf0, 0xe5, 0xd0),
        muted: Color::rgb(0x60, 0x55, 0x48),
        accent_primary: Color::rgb(0xe8, 0x70, 0x40),
        accent_success: Color::rgb(0xa8, 0xd8, 0x90),
        accent_warning: Color::rgb(0xe8, 0xb0, 0x4a),
        accent_error: Color::rgb(0xd0, 0x4a, 0x4a),
        border: Color::rgb(0x2a, 0x22, 0x1a),
        glyphs: GlyphSet::default(),
    }
}

/// Light variant of the cosmos palette — for well-lit environments.
///
/// Inverts the luminance relationship of cosmos: the background is now off-white
/// and accents are darkened for contrast on a light canvas.
///
/// # Examples
///
/// ```
/// use sid_ui::themes::cosmos_light;
///
/// let t = cosmos_light();
/// assert_eq!(t.name, "cosmos-light");
/// // Background is light (all channels > 128)
/// assert!(t.background.r > 128);
/// assert!(t.background.g > 128);
/// assert!(t.background.b > 128);
/// // Foreground is dark (all channels < 64)
/// assert!(t.foreground.r < 64);
/// assert!(t.foreground.g < 64);
/// assert!(t.foreground.b < 64);
/// ```
pub fn cosmos_light() -> Theme {
    Theme {
        name: "cosmos-light".into(),
        background: Color::rgb(0xf4, 0xf4, 0xf8),
        surface: Color::rgb(0xea, 0xea, 0xf2),
        foreground: Color::rgb(0x18, 0x18, 0x24),
        muted: Color::rgb(0x70, 0x70, 0x82),
        accent_primary: Color::rgb(0xb0, 0x30, 0x30),
        accent_success: Color::rgb(0x40, 0x80, 0x90),
        accent_warning: Color::rgb(0xb0, 0x80, 0x30),
        accent_error: Color::rgb(0xc0, 0x30, 0x40),
        border: Color::rgb(0xd0, 0xd0, 0xdc),
        glyphs: GlyphSet::default(),
    }
}
