//! DB path override sub-view.
//!
//! Displays the path sid is currently using for the redb database (read-only
//! info), plus an editable override that lives in `sid.toml`. Editing the
//! override writes the file *but does not* move the active database — the
//! change takes effect on the next launch.
//!
//! Tilde expansion is **not** performed at write time: the file stores the
//! literal string the user typed. The binary expands tildes at the next
//! launch.
//!
//! # Examples
//!
//! ```
//! use std::path::PathBuf;
//! use sid_widgets::settings::db_path::DbPathView;
//! use tempfile::tempdir;
//!
//! let d = tempdir().unwrap();
//! let view = DbPathView::open(
//!     PathBuf::from("/tmp/active.redb"),
//!     d.path().join("sid.toml"),
//! ).unwrap();
//! assert_eq!(view.active_path(), &PathBuf::from("/tmp/active.redb"));
//! assert!(view.override_path().is_none());
//! ```

use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_store::sid_toml::{SidToml, SidTomlError, read_sid_toml, write_sid_toml};
use sid_ui::Theme;

/// Returned by [`DbPathView::commit_edit`] to signal a successful write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestartNotice {
    /// Path to the `sid.toml` that was just rewritten.
    pub sid_toml_path: PathBuf,
}

/// Outcome of a key event routed to [`DbPathView`].
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::db_path::DbPathOutcome;
/// assert!(matches!(DbPathOutcome::None, DbPathOutcome::None));
/// ```
#[derive(Clone, Debug)]
pub enum DbPathOutcome {
    /// Event consumed; no persistent change.
    None,
    /// DB path override successfully written to `sid.toml`.
    Written(RestartNotice),
}

/// DB path override view state.
pub struct DbPathView {
    active_path: PathBuf,
    sid_toml_path: PathBuf,
    cfg: SidToml,
    input: Option<String>,
    last_error: Option<String>,
}

impl DbPathView {
    /// Open the view by reading the current `sid.toml` at `sid_toml_path`.
    /// If the file is absent, the view is initialised with no override.
    pub fn open(active_path: PathBuf, sid_toml_path: PathBuf) -> Result<Self, SidTomlError> {
        let cfg = read_sid_toml(&sid_toml_path)?;
        Ok(Self {
            active_path,
            sid_toml_path,
            cfg,
            input: None,
            last_error: None,
        })
    }

    /// The path sid is currently using (read-only — set at launch).
    pub fn active_path(&self) -> &Path {
        &self.active_path
    }

    /// The path in the current `sid.toml`, if any. May differ from
    /// [`Self::active_path`] when the user has edited the override but not yet
    /// restarted.
    pub fn override_path(&self) -> Option<&Path> {
        self.cfg.db_path_override.as_deref()
    }

    /// Path to the `sid.toml` file the view is reading/writing.
    pub fn sid_toml_path(&self) -> &Path {
        &self.sid_toml_path
    }

    /// `true` if an edit is in progress.
    pub fn is_editing(&self) -> bool {
        self.input.is_some()
    }

    /// Current input buffer, if any.
    pub fn input(&self) -> Option<&str> {
        self.input.as_deref()
    }

    /// Last write error, if any.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Enter edit-mode; the input buffer is pre-populated with the current
    /// override (empty string if none).
    pub fn begin_edit(&mut self) {
        let initial = self
            .cfg
            .db_path_override
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.input = Some(initial);
        self.last_error = None;
    }

    /// Append `c` to the input buffer (no-op outside edit mode).
    pub fn type_char(&mut self, c: char) {
        if let Some(s) = self.input.as_mut() {
            s.push(c);
        }
    }

    /// Pop the last character from the input buffer.
    pub fn backspace(&mut self) {
        if let Some(s) = self.input.as_mut() {
            s.pop();
        }
    }

    /// Discard the input buffer.
    pub fn cancel_edit(&mut self) {
        self.input = None;
        self.last_error = None;
    }

    /// Render the DB path editor into `area`.
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
            .title(" DB path ")
            .title_style(title_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let active = format!("active : {}", self.active_path.display());
        let over = match self.override_path() {
            Some(p) => format!("override: {}", p.display()),
            None => "override: (none)".to_string(),
        };
        let edit = match &self.input {
            Some(s) => format!("edit   : {s}_"),
            None => "edit   : (Enter to edit)".to_string(),
        };
        let err = self
            .last_error
            .as_deref()
            .map(|e| format!("error  : {e}"))
            .unwrap_or_default();
        let lines: Vec<Line> = vec![
            Line::from(active).style(Style::default().fg(theme.foreground.into())),
            Line::from(over).style(Style::default().fg(theme.foreground.into())),
            Line::from(edit).style(Style::default().fg(theme.accent_warning.into())),
            Line::from(err).style(Style::default().fg(theme.accent_error.into())),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Route a key event to the editor state machine.
    ///
    /// - Outside edit mode: `Enter` → begin_edit.
    /// - Inside edit mode: `Esc` → cancel; `Backspace` → pop char;
    ///   printable → push char; `Enter` → commit.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::{Event, KeyChord};
    /// use sid_widgets::settings::db_path::{DbPathOutcome, DbPathView};
    /// use tempfile::tempdir;
    /// use std::path::PathBuf;
    ///
    /// let d = tempdir().unwrap();
    /// let mut v = DbPathView::open(PathBuf::from("/x.redb"), d.path().join("sid.toml")).unwrap();
    /// let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::NONE));
    /// assert!(matches!(v.handle_event(&ev), DbPathOutcome::None));
    /// ```
    pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> DbPathOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::Event;
        let Event::Key(k) = ev else {
            return DbPathOutcome::None;
        };
        if self.is_editing() {
            match k.code {
                KeyCode::Esc => {
                    self.cancel_edit();
                }
                KeyCode::Backspace => {
                    self.backspace();
                }
                KeyCode::Char(c)
                    if k.mods == KeyModifiers::NONE || k.mods == KeyModifiers::SHIFT =>
                {
                    self.type_char(c);
                }
                KeyCode::Enter => {
                    if let Ok(notice) = self.commit_edit() {
                        return DbPathOutcome::Written(notice);
                    }
                    // Err: last_error already set
                }
                _ => {}
            }
            return DbPathOutcome::None;
        }
        match k.code {
            KeyCode::Enter => {
                self.begin_edit();
            }
            _ => return DbPathOutcome::None,
        }
        DbPathOutcome::None
    }

    /// Commit the current input: write `sid.toml` and return a
    /// [`RestartNotice`]. Whitespace-only input is treated as empty (clears
    /// the override).
    pub fn commit_edit(&mut self) -> Result<RestartNotice, String> {
        let Some(raw) = self.input.take() else {
            return Err("not editing".into());
        };
        let trimmed = raw.trim();
        let new = if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        };
        self.cfg.db_path_override = new;
        match write_sid_toml(&self.sid_toml_path, &self.cfg) {
            Ok(()) => Ok(RestartNotice {
                sid_toml_path: self.sid_toml_path.clone(),
            }),
            Err(e) => {
                let s = e.to_string();
                self.last_error = Some(s.clone());
                Err(s)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn paths() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempdir().unwrap();
        let active = d.path().join("active.redb");
        let toml = d.path().join("sid.toml");
        (d, active, toml)
    }

    #[test]
    fn open_with_missing_toml_has_no_override() {
        let (_d, active, toml) = paths();
        let v = DbPathView::open(active.clone(), toml).unwrap();
        assert_eq!(v.active_path(), &active);
        assert!(v.override_path().is_none());
    }

    #[test]
    fn begin_edit_populates_input_from_current_override() {
        let (_d, active, toml) = paths();
        // Pre-write a sid.toml with an override.
        std::fs::write(&toml, "db_path_override = \"/x/y\"\n").unwrap();
        let mut v = DbPathView::open(active, toml).unwrap();
        v.begin_edit();
        assert_eq!(v.input(), Some("/x/y"));
    }

    #[test]
    fn commit_empty_clears_override_and_writes() {
        let (_d, active, toml) = paths();
        std::fs::write(&toml, "db_path_override = \"/x/y\"\n").unwrap();
        let mut v = DbPathView::open(active, toml.clone()).unwrap();
        v.begin_edit();
        // Clear everything by backspacing.
        while !v.input().unwrap().is_empty() {
            v.backspace();
        }
        let notice = v.commit_edit().unwrap();
        assert_eq!(notice.sid_toml_path, toml);
        assert!(v.override_path().is_none());
        // The file is updated to have no override.
        let again = DbPathView::open(PathBuf::from("/x"), toml).unwrap();
        assert!(again.override_path().is_none());
    }

    #[test]
    fn commit_non_empty_sets_override() {
        let (_d, active, toml) = paths();
        let mut v = DbPathView::open(active, toml.clone()).unwrap();
        v.begin_edit();
        for c in "/custom/sid.redb".chars() {
            v.type_char(c);
        }
        v.commit_edit().unwrap();
        assert_eq!(
            v.override_path()
                .and_then(|p| p.to_str())
                .map(|s| s.to_string()),
            Some("/custom/sid.redb".into())
        );
    }

    #[test]
    fn cancel_edit_discards_input() {
        let (_d, active, toml) = paths();
        let mut v = DbPathView::open(active, toml).unwrap();
        v.begin_edit();
        v.type_char('x');
        v.cancel_edit();
        assert!(!v.is_editing());
        assert!(v.input().is_none());
    }

    #[test]
    fn whitespace_only_input_clears_override() {
        let (_d, active, toml) = paths();
        std::fs::write(&toml, "db_path_override = \"/keep-me\"\n").unwrap();
        let mut v = DbPathView::open(active, toml).unwrap();
        v.begin_edit();
        // Replace the buffer with "   ".
        while !v.input().unwrap().is_empty() {
            v.backspace();
        }
        v.type_char(' ');
        v.type_char(' ');
        v.type_char(' ');
        v.commit_edit().unwrap();
        assert!(v.override_path().is_none());
    }

    #[test]
    fn tilde_input_is_stored_verbatim_not_expanded() {
        let (_d, active, toml) = paths();
        let mut v = DbPathView::open(active, toml.clone()).unwrap();
        v.begin_edit();
        for c in "~/data/sid.redb".chars() {
            v.type_char(c);
        }
        v.commit_edit().unwrap();
        let again = DbPathView::open(PathBuf::from("/x"), toml).unwrap();
        assert_eq!(
            again.override_path().and_then(|p| p.to_str()),
            Some("~/data/sid.redb"),
        );
    }

    #[test]
    fn commit_in_readonly_dir_returns_err_and_stashes_message() {
        let d = tempdir().unwrap();
        let active = d.path().join("active.redb");
        let parent = d.path().join("ro");
        std::fs::create_dir(&parent).unwrap();
        // chmod 0o500 — no write permission.
        let mut perms = std::fs::metadata(&parent).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o500);
        }
        std::fs::set_permissions(&parent, perms).unwrap();
        let toml = parent.join("sid.toml");
        let mut v = DbPathView::open(active, toml).unwrap();
        v.begin_edit();
        for c in "/q".chars() {
            v.type_char(c);
        }
        let res = v.commit_edit();
        // Restore writable perms so tempdir cleanup works.
        let mut perms = std::fs::metadata(&parent).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&parent, perms).unwrap();
        assert!(res.is_err(), "expected write to fail in read-only dir");
        assert!(v.last_error().is_some());
    }

    #[test]
    fn commit_without_begin_edit_returns_err() {
        let (_d, active, toml) = paths();
        let mut v = DbPathView::open(active, toml).unwrap();
        assert!(v.commit_edit().is_err());
    }

    #[test]
    fn handle_event_enter_begins_edit_when_idle() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let (_d, active, toml) = paths();
        let mut v = DbPathView::open(active, toml).unwrap();
        let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
        let out = v.handle_event(&ev);
        assert!(v.is_editing());
        assert!(matches!(out, DbPathOutcome::None));
    }

    #[test]
    fn handle_event_enter_in_edit_mode_commits_and_emits_outcome() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let (_d, active, toml) = paths();
        let mut v = DbPathView::open(active, toml).unwrap();
        v.begin_edit();
        // Empty commit → clears override.
        let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
        let out = v.handle_event(&ev);
        assert!(matches!(out, DbPathOutcome::Written(_)));
        assert!(!v.is_editing());
    }

    #[test]
    fn handle_event_esc_cancels_edit() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let (_d, active, toml) = paths();
        let mut v = DbPathView::open(active, toml).unwrap();
        v.begin_edit();
        let ev = Event::Key(KeyChord::new(KeyCode::Esc, KeyModifiers::NONE));
        let _ = v.handle_event(&ev);
        assert!(!v.is_editing());
    }

    // -------------------------------------------------------------------------
    // Focused vs unfocused snapshot tests — verify the sub-view honours
    // the `focused: bool` argument by switching the border color.
    // -------------------------------------------------------------------------

    fn render_with_focus(v: &DbPathView, focused: bool) -> String {
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

    fn sample_view() -> (tempfile::TempDir, DbPathView) {
        let d = tempdir().unwrap();
        let toml = d.path().join("sid.toml");
        let active = PathBuf::from("/var/lib/sid/sid.redb");
        let v = DbPathView::open(active, toml).unwrap();
        (d, v)
    }

    #[test]
    fn db_path_render_focused() {
        let (_d, v) = sample_view();
        let s = render_with_focus(&v, true);
        insta::assert_snapshot!("db_path_render_focused", s);
    }

    #[test]
    fn db_path_render_unfocused() {
        let (_d, v) = sample_view();
        let s = render_with_focus(&v, false);
        insta::assert_snapshot!("db_path_render_unfocused", s);
    }
}
