//! Workspace roots editor sub-view.
//!
//! Stores a list of absolute directory paths that the workspace discovery
//! walker should consider as scan origins. Persisted as a JSON-array under
//! the `workspace_roots` setting key (same setting Plan 2's discovery reads).
//!
//! # Examples
//!
//! ```
//! use std::path::PathBuf;
//! use sid_widgets::settings::workspace_roots::WorkspaceRootsView;
//!
//! let view = WorkspaceRootsView::new(vec![PathBuf::from("/tmp")]);
//! assert_eq!(view.roots().len(), 1);
//! ```

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_core::SidError;
use sid_store::settings_keys::WORKSPACE_ROOTS;
use sid_store::{SettingValue, Store};
use sid_ui::Theme;

/// Inline tilde-expansion (`~/foo` -> `$HOME/foo`). Anything else is returned
/// unchanged.
fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return format!("{}/{}", home.to_string_lossy(), rest);
    }
    s.to_string()
}

/// State for the workspace roots editor.
pub struct WorkspaceRootsView {
    roots: Vec<PathBuf>,
    focused: usize,
    /// Input buffer while adding a new root. `None` outside add mode.
    input: Option<String>,
    /// Most recent validation error to display.
    last_error: Option<String>,
}

impl WorkspaceRootsView {
    /// Construct from an existing list of roots.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_widgets::settings::workspace_roots::WorkspaceRootsView;
    ///
    /// let v = WorkspaceRootsView::new(vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    /// assert_eq!(v.roots().len(), 2);
    /// ```
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            focused: 0,
            input: None,
            last_error: None,
        }
    }

    /// Borrow the current root list.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Focused root index.
    pub fn focused_index(&self) -> usize {
        self.focused
    }

    /// Move focus down (wraps).
    pub fn next(&mut self) {
        if !self.roots.is_empty() {
            self.focused = (self.focused + 1) % self.roots.len();
        }
    }

    /// Move focus up (wraps).
    pub fn prev(&mut self) {
        if !self.roots.is_empty() {
            self.focused = if self.focused == 0 {
                self.roots.len() - 1
            } else {
                self.focused - 1
            };
        }
    }

    /// Begin add-mode (an empty text buffer).
    pub fn begin_add(&mut self) {
        self.input = Some(String::new());
        self.last_error = None;
    }

    /// `true` if the view is currently in add-mode.
    pub fn is_adding(&self) -> bool {
        self.input.is_some()
    }

    /// Borrow the current input buffer.
    pub fn input(&self) -> Option<&str> {
        self.input.as_deref()
    }

    /// Last validation error, if any.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Append `c` to the input buffer (no-op outside add-mode).
    pub fn type_char(&mut self, c: char) {
        if let Some(s) = self.input.as_mut() {
            s.push(c);
        }
    }

    /// Pop the last character from the input buffer (no-op outside add-mode).
    pub fn backspace(&mut self) {
        if let Some(s) = self.input.as_mut() {
            s.pop();
        }
    }

    /// Cancel add-mode and discard any pending input.
    pub fn cancel_add(&mut self) {
        self.input = None;
        self.last_error = None;
    }

    /// Try to commit the current input as a new root. On success the canonical
    /// path is appended to [`Self::roots`] and the input is cleared. On failure
    /// the input is preserved and the error is stored in [`Self::last_error`].
    pub fn commit_add(&mut self) -> Result<PathBuf, String> {
        let Some(raw) = self.input.clone() else {
            return Err("not in add mode".into());
        };
        if raw.contains('\0') {
            let err = "path contains NUL byte".to_string();
            self.last_error = Some(err.clone());
            return Err(err);
        }
        let p = PathBuf::from(expand_tilde(&raw));
        if !p.exists() {
            let err = format!("path does not exist: {}", p.display());
            self.last_error = Some(err.clone());
            return Err(err);
        }
        if !p.is_dir() {
            let err = format!("not a directory: {}", p.display());
            self.last_error = Some(err.clone());
            return Err(err);
        }
        let abs = std::fs::canonicalize(&p).map_err(|e| {
            let s = e.to_string();
            self.last_error = Some(s.clone());
            s
        })?;
        if self.roots.iter().any(|r| r == &abs) {
            let err = format!("already registered: {}", abs.display());
            self.last_error = Some(err.clone());
            return Err(err);
        }
        self.roots.push(abs.clone());
        self.input = None;
        self.last_error = None;
        Ok(abs)
    }

    /// Remove the focused root and return it, clamping focus to the new last
    /// index. Returns `None` if the list is empty.
    pub fn remove_focused(&mut self) -> Option<PathBuf> {
        if self.roots.is_empty() {
            return None;
        }
        let r = self.roots.remove(self.focused);
        if !self.roots.is_empty() && self.focused >= self.roots.len() {
            self.focused = self.roots.len() - 1;
        } else if self.roots.is_empty() {
            self.focused = 0;
        }
        Some(r)
    }

    /// Render the workspace roots editor into `area`.
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border.into()))
            .title(" Workspace roots ")
            .title_style(Style::default().fg(theme.foreground.into()));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let banner = self.input.is_some() || self.last_error.is_some();
        let list_h = if banner {
            inner.height.saturating_sub(1)
        } else {
            inner.height
        };
        let mut rows: Vec<Line> = Vec::with_capacity(self.roots.len());
        for (i, p) in self.roots.iter().enumerate() {
            let cursor = if i == self.focused { '>' } else { ' ' };
            let line = Line::from(format!("{cursor} {}", p.display()));
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
        let list_rect = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: list_h,
        };
        frame.render_widget(Paragraph::new(rows), list_rect);

        if banner {
            let banner_rect = Rect {
                x: inner.x,
                y: inner.y + list_h,
                width: inner.width,
                height: 1,
            };
            let (label, color) = if let Some(err) = &self.last_error {
                (format!(" ! {err}"), theme.accent_error)
            } else if let Some(buf) = &self.input {
                (format!(" + {buf}"), theme.accent_warning)
            } else {
                (String::new(), theme.muted)
            };
            frame.render_widget(
                Paragraph::new(label).style(Style::default().fg(color.into())),
                banner_rect,
            );
        }
    }

    /// Persist the current root list under the `workspace_roots` setting key.
    pub fn save(&self, store: &dyn Store) -> Result<(), SidError> {
        let json = serde_json::to_string(&self.roots)
            .map_err(|e| SidError::Storage(format!("workspace_roots encode: {e}")))?;
        store.put_setting(WORKSPACE_ROOTS, &SettingValue(json.into_bytes()))
    }

    /// Build a view from the persisted setting. Returns the default (`$HOME/vcs`
    /// if it exists, else empty) when the setting is absent.
    pub fn load(store: &dyn Store) -> Result<Self, SidError> {
        let roots = match store.get_setting(WORKSPACE_ROOTS)? {
            None => default_roots(),
            Some(v) => serde_json::from_slice::<Vec<PathBuf>>(&v.0)
                .map_err(|e| SidError::Storage(format!("workspace_roots decode: {e}")))?,
        };
        Ok(Self::new(roots))
    }
}

fn default_roots() -> Vec<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join("vcs");
        if p.exists() {
            return vec![p];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use sid_store::{OpenStore, RedbStore};
    use tempfile::tempdir;

    use super::*;

    fn store() -> (tempfile::TempDir, RedbStore) {
        let d = tempdir().unwrap();
        let s = RedbStore::open(&d.path().join("s.redb")).unwrap();
        (d, s)
    }

    #[test]
    fn new_starts_with_provided_list() {
        let v = WorkspaceRootsView::new(vec![PathBuf::from("/a")]);
        assert_eq!(v.roots(), &[PathBuf::from("/a")]);
    }

    #[test]
    fn begin_add_then_type_then_commit_appends() {
        let d = tempdir().unwrap();
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        for c in d.path().to_string_lossy().chars() {
            v.type_char(c);
        }
        let added = v.commit_add().unwrap();
        assert!(v.roots().iter().any(|r| r == &added));
        assert!(!v.is_adding());
    }

    #[test]
    fn add_nonexistent_path_returns_err_and_keeps_input() {
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        for c in "/no/such/dir/here".chars() {
            v.type_char(c);
        }
        assert!(v.commit_add().is_err());
        assert!(v.is_adding(), "input should be retained on failure");
        assert!(v.last_error().is_some());
    }

    #[test]
    fn add_file_path_returns_err() {
        let d = tempdir().unwrap();
        let file = d.path().join("a-file.txt");
        std::fs::write(&file, b"hi").unwrap();
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        for c in file.to_string_lossy().chars() {
            v.type_char(c);
        }
        assert!(v.commit_add().is_err());
    }

    #[test]
    fn add_duplicate_returns_err() {
        let d = tempdir().unwrap();
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        for c in d.path().to_string_lossy().chars() {
            v.type_char(c);
        }
        v.commit_add().unwrap();
        v.begin_add();
        for c in d.path().to_string_lossy().chars() {
            v.type_char(c);
        }
        assert!(v.commit_add().is_err());
    }

    #[test]
    fn add_path_with_nul_byte_returns_err_not_panic() {
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        v.type_char('/');
        v.type_char('\0');
        v.type_char('x');
        assert!(v.commit_add().is_err());
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        v.type_char('a');
        v.type_char('b');
        v.backspace();
        assert_eq!(v.input(), Some("a"));
    }

    #[test]
    fn cancel_add_clears_buffer_and_error() {
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        v.type_char('x');
        v.cancel_add();
        assert!(!v.is_adding());
        assert!(v.last_error().is_none());
    }

    #[test]
    fn remove_focused_shrinks_list() {
        let mut v = WorkspaceRootsView::new(vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        let removed = v.remove_focused().unwrap();
        assert_eq!(removed, PathBuf::from("/a"));
        assert_eq!(v.roots().len(), 1);
    }

    #[test]
    fn remove_focused_on_empty_returns_none() {
        let mut v = WorkspaceRootsView::new(vec![]);
        assert!(v.remove_focused().is_none());
    }

    #[test]
    fn remove_clamps_focused_to_last() {
        let mut v = WorkspaceRootsView::new(vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c"),
        ]);
        v.next();
        v.next(); // focused = 2
        v.remove_focused();
        assert_eq!(v.focused_index(), 1);
    }

    #[test]
    fn very_long_path_does_not_panic() {
        let big = "/tmp/".to_string() + &"x".repeat(4096);
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        for c in big.chars() {
            v.type_char(c);
        }
        let _ = v.commit_add(); // probably Err — path won't exist
    }

    #[test]
    fn tilde_path_is_expanded() {
        // Ensure $HOME points to a real temp dir so the expansion succeeds.
        let d = tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        // SAFETY: tests in this file run with --test-threads=1 to avoid env races.
        // (Note: clippy will whine about this — we accept it; the binary test
        // restores the original $HOME on exit.)
        unsafe {
            std::env::set_var("HOME", d.path());
        }
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        // ~/  refers to `d.path()` which exists.
        v.type_char('~');
        v.type_char('/');
        let res = v.commit_add();
        match prev_home {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        assert!(
            res.is_ok(),
            "tilde expansion should resolve to tempdir: {res:?}"
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let (_d, store) = store();
        let tmp = tempdir().unwrap();
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        for c in tmp.path().to_string_lossy().chars() {
            v.type_char(c);
        }
        v.commit_add().unwrap();
        v.save(&store).unwrap();

        let v2 = WorkspaceRootsView::load(&store).unwrap();
        assert_eq!(v2.roots(), v.roots());
    }

    #[test]
    fn load_without_setting_returns_default_or_empty() {
        let (_d, store) = store();
        // We don't know what $HOME is in the test environment; just assert no panic.
        let _ = WorkspaceRootsView::load(&store).unwrap();
    }

    #[test]
    fn load_with_malformed_json_returns_err() {
        let (_d, store) = store();
        store
            .put_setting(WORKSPACE_ROOTS, &SettingValue(b"not-json".to_vec()))
            .unwrap();
        assert!(WorkspaceRootsView::load(&store).is_err());
    }

    #[test]
    fn load_with_wrong_json_type_returns_err() {
        let (_d, store) = store();
        store
            .put_setting(WORKSPACE_ROOTS, &SettingValue(b"\"not-an-array\"".to_vec()))
            .unwrap();
        assert!(WorkspaceRootsView::load(&store).is_err());
    }

    #[test]
    fn save_thousand_paths_round_trips() {
        let (_d, store) = store();
        let roots: Vec<PathBuf> = (0..1000)
            .map(|i| PathBuf::from(format!("/p/{i}")))
            .collect();
        let v = WorkspaceRootsView::new(roots.clone());
        v.save(&store).unwrap();
        let v2 = WorkspaceRootsView::load(&store).unwrap();
        assert_eq!(v2.roots().len(), 1000);
        assert_eq!(v2.roots()[999], roots[999]);
    }

    #[test]
    fn symlink_pointing_to_dir_is_accepted_and_canonicalized() {
        let d = tempdir().unwrap();
        let real = d.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = d.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut v = WorkspaceRootsView::new(vec![]);
        v.begin_add();
        for c in link.to_string_lossy().chars() {
            v.type_char(c);
        }
        let added = v.commit_add().unwrap();
        // canonicalize should resolve the symlink.
        assert_eq!(added, std::fs::canonicalize(&link).unwrap());
    }
}
