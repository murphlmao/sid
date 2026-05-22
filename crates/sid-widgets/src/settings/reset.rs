//! Reset-to-defaults sub-view.
//!
//! Clears the canonical setting keys (theme name, keybind profile, behavior
//! toggles, ...) so subsequent reads fall back to the compiled-in defaults.
//! Does **not** wipe the `themes`, `keybinds`, `quick_actions`, or
//! `workspaces` tables — those carry user data, not configuration.
//!
//! # Examples
//!
//! ```
//! use sid_widgets::settings::reset::ResetView;
//! let view = ResetView::new();
//! assert!(!view.is_confirming());
//! ```

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_core::SidError;
use sid_store::{Store, settings_keys};
use sid_ui::Theme;

/// Setting keys cleared by [`ResetView::confirm`].
pub const FACTORY_KEYS: &[&str] = &[
    settings_keys::THEME_NAME,
    settings_keys::KEYBIND_PROFILE_NAME,
    settings_keys::WORKSPACE_ROOTS,
    settings_keys::PERSIST_DEBOUNCE_MS,
    settings_keys::HEARTBEAT_INTERVAL_SECS,
    settings_keys::AUTO_RESTORE_SESSION,
    settings_keys::AUTO_SCAN_WORKSPACES,
    settings_keys::DEFAULT_TAB,
    settings_keys::SETTINGS_FOCUSED_CATEGORY,
];

/// Reset modal state. The view is "armed" via [`Self::open_confirm`] and
/// committed via [`Self::confirm`].
pub struct ResetView {
    confirm_open: bool,
}

impl ResetView {
    /// Construct in non-confirming state.
    pub fn new() -> Self {
        Self {
            confirm_open: false,
        }
    }

    /// `true` if the confirm modal is open.
    pub fn is_confirming(&self) -> bool {
        self.confirm_open
    }

    /// Arm the confirm modal.
    pub fn open_confirm(&mut self) {
        self.confirm_open = true;
    }

    /// Dismiss the modal without writing.
    pub fn cancel(&mut self) {
        self.confirm_open = false;
    }

    /// Render the reset confirm stub into `area`.
    ///
    /// `focused` controls the outer border color (accent vs muted) and the
    /// title-bar bold modifier so the Settings composer can signal which pane
    /// currently owns keyboard input.
    pub fn render_into_frame(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        focused: bool,
    ) {
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Reset to defaults ")
            .title_style(title_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let lines: Vec<Line> = if self.confirm_open {
            vec![
                Line::from("Reset every setting key to its compiled-in default?")
                    .style(Style::default().fg(theme.foreground.into())),
                Line::from("(y)es   (n)o").style(
                    Style::default()
                        .fg(theme.accent_warning.into())
                        .add_modifier(Modifier::BOLD),
                ),
            ]
        } else {
            vec![
                Line::from("Press Enter to open the reset confirm modal.")
                    .style(Style::default().fg(theme.foreground.into())),
                Line::from("This clears settings only — themes, keybinds, quick actions and workspaces survive.")
                    .style(Style::default().fg(theme.muted.into())),
            ]
        };
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Execute the reset if the confirm modal is open. Returns the number of
    /// keys that were actually cleared. If the modal is closed, returns
    /// `Ok(0)` without touching the store.
    pub fn confirm(&mut self, store: &dyn Store) -> Result<usize, SidError> {
        if !self.confirm_open {
            return Ok(0);
        }
        self.confirm_open = false;
        let mut cleared = 0;
        for k in FACTORY_KEYS {
            if store.delete_setting(k)? {
                cleared += 1;
            }
        }
        Ok(cleared)
    }
}

impl Default for ResetView {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use sid_store::{
        KeybindEntry, KeybindProfile, OpenStore, QuickAction, QuickActionScope, RedbStore,
        SettingValue, ThemeGlyphs, ThemePalette, ThemeSpec, Workspace, WorkspaceKind, now_epoch,
    };
    use tempfile::tempdir;

    use super::*;

    fn store() -> (tempfile::TempDir, RedbStore) {
        let d = tempdir().unwrap();
        let s = RedbStore::open(&d.path().join("s.redb")).unwrap();
        (d, s)
    }

    #[test]
    fn new_starts_not_confirming() {
        assert!(!ResetView::new().is_confirming());
    }

    #[test]
    fn open_confirm_sets_state() {
        let mut v = ResetView::new();
        v.open_confirm();
        assert!(v.is_confirming());
    }

    #[test]
    fn cancel_dismisses() {
        let mut v = ResetView::new();
        v.open_confirm();
        v.cancel();
        assert!(!v.is_confirming());
    }

    #[test]
    fn confirm_without_open_is_zero_noop() {
        let (_d, store) = store();
        let mut v = ResetView::new();
        assert_eq!(v.confirm(&store).unwrap(), 0);
    }

    #[test]
    fn confirm_clears_factory_keys() {
        let (_d, store) = store();
        for k in FACTORY_KEYS {
            store.put_setting(k, &SettingValue(b"v".to_vec())).unwrap();
        }
        let mut v = ResetView::new();
        v.open_confirm();
        let cleared = v.confirm(&store).unwrap();
        assert_eq!(cleared, FACTORY_KEYS.len());
        for k in FACTORY_KEYS {
            assert!(store.get_setting(k).unwrap().is_none(), "{k} still present");
        }
        assert!(!v.is_confirming());
    }

    #[test]
    fn confirm_is_idempotent() {
        let (_d, store) = store();
        for k in FACTORY_KEYS {
            store.put_setting(k, &SettingValue(b"v".to_vec())).unwrap();
        }
        let mut v = ResetView::new();
        v.open_confirm();
        v.confirm(&store).unwrap();
        v.open_confirm();
        assert_eq!(v.confirm(&store).unwrap(), 0);
    }

    #[test]
    fn confirm_does_not_touch_other_settings() {
        let (_d, store) = store();
        store
            .put_setting("custom.user.key", &SettingValue(b"keep-me".to_vec()))
            .unwrap();
        store
            .put_setting(settings_keys::THEME_NAME, &SettingValue(b"cosmos".to_vec()))
            .unwrap();
        let mut v = ResetView::new();
        v.open_confirm();
        v.confirm(&store).unwrap();
        // Custom key survives.
        assert_eq!(
            store.get_setting("custom.user.key").unwrap().unwrap().0,
            b"keep-me".to_vec()
        );
        // Factory key is gone.
        assert!(
            store
                .get_setting(settings_keys::THEME_NAME)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn confirm_does_not_touch_other_tables() {
        let (_d, store) = store();
        // Seed each non-settings table with at least one record.
        store
            .upsert_theme(&ThemeSpec {
                name: "t".into(),
                palette: ThemePalette {
                    background: 0,
                    surface: 0,
                    foreground: 0,
                    muted: 0,
                    accent_primary: 0,
                    accent_success: 0,
                    accent_warning: 0,
                    accent_error: 0,
                    border: 0,
                },
                glyphs: ThemeGlyphs {
                    star: '*',
                    small_star: '.',
                    dot: '.',
                },
            })
            .unwrap();
        store
            .upsert_keybind_profile(&KeybindProfile {
                name: "p".into(),
                bindings: vec![KeybindEntry {
                    chord: "Char('q')|0".into(),
                    action: "app.quit".into(),
                }],
            })
            .unwrap();
        store
            .upsert_quick_action(&QuickAction {
                id: "q".into(),
                label: "Q".into(),
                cmd: "echo".into(),
                keybind: None,
                scope: QuickActionScope::Global,
            })
            .unwrap();
        store
            .upsert_workspace(&Workspace {
                path: "/tmp/x".into(),
                name: "x".into(),
                kind: WorkspaceKind::Repo,
                manifest_hash: 0,
                last_seen: now_epoch(),
                parent: None,
            })
            .unwrap();
        // Factory reset.
        let mut v = ResetView::new();
        v.open_confirm();
        v.confirm(&store).unwrap();
        // Themes / keybinds / quick_actions / workspaces all survive.
        assert_eq!(store.list_themes().unwrap().len(), 1);
        assert_eq!(store.list_keybind_profiles().unwrap().len(), 1);
        assert_eq!(store.list_quick_actions().unwrap().len(), 1);
        assert_eq!(store.list_workspaces().unwrap().len(), 1);
    }

    // -------------------------------------------------------------------------
    // Focused vs unfocused snapshot tests — verify the sub-view honours
    // the `focused: bool` argument by switching the border color.
    // -------------------------------------------------------------------------

    fn render_with_focus(v: &ResetView, focused: bool) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use sid_ui::themes::cosmos;
        let backend = TestBackend::new(60, 8);
        let mut term = Terminal::new(backend).unwrap();
        let theme = cosmos();
        term.draw(|f| v.render_into_frame(f, f.area(), &theme, focused))
            .unwrap();
        let buf = term.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            s.push('\n');
        }
        let tl = buf.cell((0, 0)).unwrap();
        s.push_str(&format!(
            "border_top_left: fg={:?} modifier={:?}\n",
            tl.fg, tl.modifier
        ));
        let title_cell = buf.cell((2, 0)).unwrap();
        s.push_str(&format!(
            "title_first_char: symbol={:?} fg={:?} modifier={:?}\n",
            title_cell.symbol(),
            title_cell.fg,
            title_cell.modifier
        ));
        s
    }

    #[test]
    fn reset_render_focused() {
        let v = ResetView::new();
        let s = render_with_focus(&v, true);
        insta::assert_snapshot!("reset_render_focused", s);
    }

    #[test]
    fn reset_render_unfocused() {
        let v = ResetView::new();
        let s = render_with_focus(&v, false);
        insta::assert_snapshot!("reset_render_unfocused", s);
    }
}
