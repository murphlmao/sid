//! Theme picker sub-view: a list of available themes with a focused index.
//!
//! The view distinguishes between the **focused** theme (the row the cursor is
//! parked on — drives the live-preview pane on the right) and the **applied**
//! theme (the one currently persisted to the `theme_name` setting). The two
//! diverge while the user is browsing and converge on `apply_focused`.
//!
//! # Panics
//!
//! [`ThemePickerView::new`] panics if the registry is empty. The registry is
//! always seeded with the four built-in themes by
//! [`ThemeRegistry::with_builtins`], so an empty registry indicates a
//! programmer error in wiring. This mirrors the Plan 1 `TabManager::new(vec![])`
//! convention.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_ui::theme::Theme;
use sid_ui::theme_registry::ThemeRegistry;

use crate::settings::live_preview::render_preview;

/// Outcome of dispatching a key event to the theme picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemePickerOutcome {
    /// Event was not consumed.
    None,
    /// Focus moved — the live preview pane should re-render.
    PreviewChanged,
    /// User pressed enter / "a" — the named theme should be persisted.
    Applied {
        /// Name of the theme that was applied.
        name: String,
    },
}

/// State for the theme picker sub-view.
///
/// # Examples
///
/// ```
/// use sid_ui::theme_registry::ThemeRegistry;
/// use sid_widgets::settings::theme_picker::ThemePickerView;
///
/// let registry = ThemeRegistry::with_builtins();
/// let view = ThemePickerView::new(&registry, "cosmos");
/// assert_eq!(view.focused().name, "cosmos");
/// assert_eq!(view.applied_name(), "cosmos");
/// ```
pub struct ThemePickerView {
    themes: Vec<Theme>,
    focused: usize,
    /// The name currently *applied* (persisted), which may differ from the
    /// focused one (live-preview vs persisted distinction).
    applied: String,
}

impl ThemePickerView {
    /// Construct a new view, focused on `applied_name` if it exists, otherwise
    /// on the first theme.
    ///
    /// # Panics
    ///
    /// Panics if the registry is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let v = ThemePickerView::new(&r, "void");
    /// assert_eq!(v.focused().name, "void");
    /// ```
    pub fn new(registry: &ThemeRegistry, applied_name: &str) -> Self {
        let themes: Vec<Theme> = registry.iter().cloned().collect();
        assert!(
            !themes.is_empty(),
            "ThemePickerView::new called with empty registry"
        );
        let focused = themes
            .iter()
            .position(|t| t.name == applied_name)
            .unwrap_or(0);
        Self {
            themes,
            focused,
            applied: applied_name.to_string(),
        }
    }

    /// Reference to the focused theme.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let v = ThemePickerView::new(&r, "cosmos");
    /// assert_eq!(v.focused().name, "cosmos");
    /// ```
    pub fn focused(&self) -> &Theme {
        &self.themes[self.focused]
    }

    /// Index of the focused theme within [`Self::themes`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let v = ThemePickerView::new(&r, "cosmos");
    /// assert!(v.focused_index() < r.len());
    /// ```
    pub fn focused_index(&self) -> usize {
        self.focused
    }

    /// Name of the persisted (applied) theme. May differ from the focused name
    /// while the user is browsing.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let v = ThemePickerView::new(&r, "void");
    /// assert_eq!(v.applied_name(), "void");
    /// ```
    pub fn applied_name(&self) -> &str {
        &self.applied
    }

    /// Move focus to the next theme, wrapping at the end.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let mut v = ThemePickerView::new(&r, "cosmos");
    /// let before = v.focused_index();
    /// v.next();
    /// assert_ne!(v.focused_index(), before);
    /// ```
    pub fn next(&mut self) {
        if self.themes.is_empty() {
            return;
        }
        self.focused = (self.focused + 1) % self.themes.len();
    }

    /// Move focus to the previous theme, wrapping at the start.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let mut v = ThemePickerView::new(&r, "cosmos");
    /// v.next();
    /// v.prev();
    /// assert_eq!(v.focused().name, "cosmos");
    /// ```
    pub fn prev(&mut self) {
        if self.themes.is_empty() {
            return;
        }
        self.focused = if self.focused == 0 {
            self.themes.len() - 1
        } else {
            self.focused - 1
        };
    }

    /// Jump focus to `idx`. Returns `false` (no-op) if `idx` is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let mut v = ThemePickerView::new(&r, "cosmos");
    /// assert!(v.jump(2));
    /// assert!(!v.jump(99));
    /// ```
    pub fn jump(&mut self, idx: usize) -> bool {
        if idx >= self.themes.len() {
            return false;
        }
        self.focused = idx;
        true
    }

    /// Apply the focused theme: copy its name into the applied slot and return
    /// the new applied name.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let mut v = ThemePickerView::new(&r, "cosmos");
    /// v.next();
    /// let name = v.apply_focused().to_string();
    /// assert_eq!(v.applied_name(), name);
    /// ```
    pub fn apply_focused(&mut self) -> &str {
        self.applied = self.focused().name.clone();
        &self.applied
    }

    /// Slice of all known themes, in registry order (lexicographic by name).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let v = ThemePickerView::new(&r, "cosmos");
    /// assert_eq!(v.themes().len(), 4);
    /// ```
    pub fn themes(&self) -> &[Theme] {
        &self.themes
    }

    /// Dispatch a `sid-core` event. Returns an outcome describing what changed.
    ///
    /// Recognised keys: Up/k = previous, Down/j = next, Enter = apply, Home /
    /// End = jump to ends.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::{Event, KeyChord};
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::{ThemePickerOutcome, ThemePickerView};
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let mut v = ThemePickerView::new(&r, "cosmos");
    /// let ev = Event::Key(KeyChord::new(KeyCode::Down, KeyModifiers::empty()));
    /// assert_eq!(v.handle_event(&ev), ThemePickerOutcome::PreviewChanged);
    /// ```
    pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> ThemePickerOutcome {
        use crossterm::event::KeyCode;
        use sid_core::event::Event;
        match ev {
            Event::Key(k) => match k.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.next();
                    ThemePickerOutcome::PreviewChanged
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.prev();
                    ThemePickerOutcome::PreviewChanged
                }
                KeyCode::Home => {
                    self.focused = 0;
                    ThemePickerOutcome::PreviewChanged
                }
                KeyCode::End => {
                    self.focused = self.themes.len().saturating_sub(1);
                    ThemePickerOutcome::PreviewChanged
                }
                KeyCode::Enter => ThemePickerOutcome::Applied {
                    name: self.apply_focused().to_string(),
                },
                _ => ThemePickerOutcome::None,
            },
            _ => ThemePickerOutcome::None,
        }
    }

    /// Render the theme picker into `area` using `theme` as the chrome theme.
    ///
    /// Layout: a vertical split — the upper portion shows the list of theme
    /// names (focused row marked with `>`, applied row marked with `*`); the
    /// lower portion embeds the [`render_preview`] block for the focused theme.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::Terminal;
    /// use ratatui::backend::TestBackend;
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_ui::themes::cosmos;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let v = ThemePickerView::new(&r, "cosmos");
    /// let backend = TestBackend::new(40, 16);
    /// let mut term = Terminal::new(backend).unwrap();
    /// let theme = cosmos();
    /// term.draw(|f| v.render_into_frame(f, f.area(), &theme)).unwrap();
    /// ```
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border.into()))
            .title(" Theme ")
            .title_style(Style::default().fg(theme.foreground.into()));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Vertical split: list above, live-preview below.
        let preview_height = inner.height.saturating_sub(1).min(12);
        let list_height = inner.height.saturating_sub(preview_height);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(list_height),
                Constraint::Length(preview_height),
            ])
            .split(inner);

        // List rows.
        let mut rows: Vec<Line> = Vec::with_capacity(self.themes.len());
        for (i, t) in self.themes.iter().enumerate() {
            let cursor = if i == self.focused { '>' } else { ' ' };
            let marker = if t.name == self.applied { '*' } else { 'o' };
            let line = Line::from(format!("{cursor} {marker} {}", t.name));
            let line = if i == self.focused {
                line.style(
                    Style::default()
                        .fg(theme.accent_primary.into())
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                line.style(Style::default().fg(theme.foreground.into()))
            };
            rows.push(line);
        }
        frame.render_widget(Paragraph::new(rows), chunks[0]);

        // Live preview using the focused theme (not the chrome theme).
        if preview_height >= 2 && chunks[1].width > 0 {
            let preview = render_preview(self.focused(), chunks[1].width, preview_height);
            let lines: Vec<Line> = preview.lines().map(|l| Line::from(l.to_string())).collect();
            frame.render_widget(Paragraph::new(lines), chunks[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};
    use proptest::prelude::*;
    use sid_core::event::{Event, KeyChord};
    use sid_ui::theme::{Color, GlyphSet, Theme};
    use sid_ui::theme_registry::ThemeRegistry;

    use super::*;

    fn fake(name: &str) -> Theme {
        Theme {
            name: name.into(),
            background: Color::rgb(0, 0, 0),
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

    fn small() -> ThemeRegistry {
        let mut r = ThemeRegistry::empty();
        r.register(fake("a"));
        r.register(fake("b"));
        r.register(fake("c"));
        r
    }

    #[test]
    fn new_with_known_applied_starts_focused_on_applied() {
        let r = small();
        let v = ThemePickerView::new(&r, "b");
        assert_eq!(v.focused().name, "b");
    }

    #[test]
    fn new_with_unknown_applied_starts_at_zero() {
        let r = small();
        let v = ThemePickerView::new(&r, "unknown");
        assert_eq!(v.focused_index(), 0);
        assert_eq!(v.applied_name(), "unknown");
    }

    #[test]
    fn next_wraps() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "c");
        v.next();
        assert_eq!(v.focused().name, "a");
    }

    #[test]
    fn prev_wraps() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "a");
        v.prev();
        assert_eq!(v.focused().name, "c");
    }

    #[test]
    fn jump_in_bounds_succeeds() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "a");
        assert!(v.jump(2));
        assert_eq!(v.focused().name, "c");
    }

    #[test]
    fn jump_out_of_bounds_returns_false() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "a");
        assert!(!v.jump(99));
        assert_eq!(v.focused().name, "a");
    }

    #[test]
    fn apply_focused_updates_applied() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "a");
        v.next();
        let name = v.apply_focused().to_string();
        assert_eq!(name, "b");
        assert_eq!(v.applied_name(), "b");
    }

    #[test]
    #[should_panic(expected = "empty registry")]
    fn new_with_empty_registry_panics() {
        let r = ThemeRegistry::empty();
        let _ = ThemePickerView::new(&r, "anything");
    }

    #[test]
    fn handle_event_down_arrow_advances() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "a");
        let ev = Event::Key(KeyChord::new(KeyCode::Down, KeyModifiers::empty()));
        assert_eq!(v.handle_event(&ev), ThemePickerOutcome::PreviewChanged);
        assert_eq!(v.focused().name, "b");
    }

    #[test]
    fn handle_event_j_advances() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "a");
        let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::empty()));
        v.handle_event(&ev);
        assert_eq!(v.focused().name, "b");
    }

    #[test]
    fn handle_event_enter_applies() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "a");
        v.next();
        let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::empty()));
        match v.handle_event(&ev) {
            ThemePickerOutcome::Applied { name } => assert_eq!(name, "b"),
            o => panic!("expected Applied, got {o:?}"),
        }
        assert_eq!(v.applied_name(), "b");
    }

    #[test]
    fn handle_event_unknown_returns_none() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "a");
        let ev = Event::Key(KeyChord::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert_eq!(v.handle_event(&ev), ThemePickerOutcome::None);
    }

    #[test]
    fn home_and_end_jump_to_edges() {
        let r = small();
        let mut v = ThemePickerView::new(&r, "b");
        v.handle_event(&Event::Key(KeyChord::new(
            KeyCode::End,
            KeyModifiers::empty(),
        )));
        assert_eq!(v.focused_index(), 2);
        v.handle_event(&Event::Key(KeyChord::new(
            KeyCode::Home,
            KeyModifiers::empty(),
        )));
        assert_eq!(v.focused_index(), 0);
    }

    proptest! {
        #[test]
        fn prop_next_then_prev_is_identity(start in 0usize..3, steps in 1usize..16) {
            let r = small();
            let names = ["a", "b", "c"];
            let mut v = ThemePickerView::new(&r, names[start]);
            for _ in 0..steps {
                v.next();
                v.prev();
            }
            prop_assert_eq!(v.focused().name.clone(), names[start]);
        }

        #[test]
        fn prop_n_nexts_then_n_prevs_returns_to_start(start in 0usize..3, n in 0usize..32) {
            let r = small();
            let names = ["a", "b", "c"];
            let mut v = ThemePickerView::new(&r, names[start]);
            for _ in 0..n { v.next(); }
            for _ in 0..n { v.prev(); }
            prop_assert_eq!(v.focused().name.clone(), names[start]);
        }
    }
}
