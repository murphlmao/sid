//! Enumerate built-in themes (`cosmos`, `void`, `dusk`, `cosmos-light`) plus
//! user-authored themes layered on top.
//!
//! The registry is an in-memory `BTreeMap` keyed by `Theme::name`. Iteration
//! order is therefore lexicographic by name, which makes the theme-picker
//! presentation order deterministic without an extra sort.
//!
//! # Examples
//!
//! ```
//! use sid_ui::theme_registry::ThemeRegistry;
//!
//! let r = ThemeRegistry::with_builtins();
//! assert!(r.get("cosmos").is_some());
//! assert!(r.get("void").is_some());
//! ```

use std::collections::BTreeMap;

use crate::theme::Theme;
use crate::themes::{cosmos, cosmos_light, dusk, void};

/// Registry of available themes. Built-in themes are seeded by
/// [`ThemeRegistry::with_builtins`]; user-authored themes are added via
/// [`ThemeRegistry::register`] (which overrides a built-in with the same name).
pub struct ThemeRegistry {
    by_name: BTreeMap<String, Theme>,
}

impl ThemeRegistry {
    /// Construct an empty registry. Use [`Self::register`] to seed it.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// let r = ThemeRegistry::empty();
    /// assert!(r.is_empty());
    /// ```
    pub fn empty() -> Self {
        Self {
            by_name: BTreeMap::new(),
        }
    }

    /// Construct a registry seeded with all four built-in themes
    /// (`cosmos`, `void`, `dusk`, `cosmos-light`).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// let r = ThemeRegistry::with_builtins();
    /// assert_eq!(r.len(), 4);
    /// ```
    pub fn with_builtins() -> Self {
        let mut r = Self::empty();
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            r.register(t);
        }
        r
    }

    /// Insert (or replace) the theme keyed by `t.name`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_ui::themes::cosmos;
    ///
    /// let mut r = ThemeRegistry::empty();
    /// r.register(cosmos());
    /// assert!(r.get("cosmos").is_some());
    /// ```
    pub fn register(&mut self, t: Theme) {
        self.by_name.insert(t.name.clone(), t);
    }

    /// Look up a theme by exact name. `None` if not registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// assert!(r.get("cosmos").is_some());
    /// assert!(r.get("missing").is_none());
    /// ```
    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.by_name.get(name)
    }

    /// Iterate themes in lexicographic name order.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let names: Vec<_> = r.iter().map(|t| t.name.clone()).collect();
    /// let mut sorted = names.clone();
    /// sorted.sort();
    /// assert_eq!(names, sorted);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = &Theme> {
        self.by_name.values()
    }

    /// Names of all registered themes, lexicographic order.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// assert!(r.names().contains(&"cosmos"));
    /// ```
    pub fn names(&self) -> Vec<&str> {
        self.by_name.keys().map(|s| s.as_str()).collect()
    }

    /// Number of registered themes.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    ///
    /// assert_eq!(ThemeRegistry::with_builtins().len(), 4);
    /// ```
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// `true` if no themes are registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    ///
    /// assert!(ThemeRegistry::empty().is_empty());
    /// assert!(!ThemeRegistry::with_builtins().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

impl Default for ThemeRegistry {
    /// Defaults to [`Self::with_builtins`].
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Color, GlyphSet, Theme};

    fn fake(name: &str, r: u8) -> Theme {
        Theme {
            name: name.into(),
            background: Color::rgb(r, 0, 0),
            surface: Color::rgb(0, 0, 0),
            foreground: Color::rgb(0, 0, 0),
            muted: Color::rgb(0, 0, 0),
            accent_primary: Color::rgb(0, 0, 0),
            accent_success: Color::rgb(0, 0, 0),
            accent_warning: Color::rgb(0, 0, 0),
            accent_error: Color::rgb(0, 0, 0),
            border: Color::rgb(0, 0, 0),
            glyphs: GlyphSet::default(),
        }
    }

    #[test]
    fn builtins_present() {
        let r = ThemeRegistry::with_builtins();
        for name in ["cosmos", "void", "dusk", "cosmos-light"] {
            assert!(r.get(name).is_some(), "missing builtin {name}");
        }
    }

    #[test]
    fn get_by_name_returns_theme() {
        let r = ThemeRegistry::with_builtins();
        assert_eq!(r.get("cosmos").unwrap().name, "cosmos");
    }

    #[test]
    fn get_unknown_returns_none() {
        let r = ThemeRegistry::with_builtins();
        assert!(r.get("nonexistent-theme").is_none());
    }

    #[test]
    fn user_themes_override_builtins() {
        let mut r = ThemeRegistry::with_builtins();
        r.register(fake("cosmos", 1));
        assert_eq!(r.get("cosmos").unwrap().background.r, 1);
    }

    #[test]
    fn empty_registry_has_no_themes() {
        let r = ThemeRegistry::empty();
        assert_eq!(r.len(), 0);
        assert!(r.is_empty());
    }

    #[test]
    fn iter_yields_themes_in_sorted_order() {
        let r = ThemeRegistry::with_builtins();
        let names: Vec<_> = r.iter().map(|t| t.name.clone()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn names_match_iter() {
        let r = ThemeRegistry::with_builtins();
        let from_iter: Vec<_> = r.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(r.names(), from_iter);
    }

    #[test]
    fn default_uses_builtins() {
        assert_eq!(ThemeRegistry::default().len(), 4);
    }

    #[test]
    fn register_twice_keeps_latest() {
        let mut r = ThemeRegistry::empty();
        r.register(fake("x", 1));
        r.register(fake("x", 2));
        assert_eq!(r.len(), 1);
        assert_eq!(r.get("x").unwrap().background.r, 2);
    }
}
