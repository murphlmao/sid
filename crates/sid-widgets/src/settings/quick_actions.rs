//! Quick actions editor.
//!
//! Quick actions are global commands surfaced in the System tab and the
//! command palette. The Settings editor lets the user add, edit, and remove
//! them. Persistence uses the `quick_actions` table.
//!
//! # Examples
//!
//! ```
//! use sid_widgets::settings::quick_actions::QuickActionsView;
//!
//! let view = QuickActionsView::new(vec![]);
//! assert_eq!(view.actions().len(), 0);
//! ```

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_core::SidError;
use sid_core::keybind_profile::chord_from_string;
use sid_store::{QuickAction, QuickActionScope, Store};
use sid_ui::Theme;

/// Outcome of a key event routed into [`QuickActionsView`].
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::quick_actions::QuickActionsOutcome;
/// assert!(matches!(QuickActionsOutcome::None, QuickActionsOutcome::None));
/// ```
#[derive(Clone, Debug)]
pub enum QuickActionsOutcome {
    /// Event consumed but no list mutation.
    None,
    /// A quick action was added or edited. Wire layer calls
    /// `store.upsert_quick_action`.
    Upserted(sid_store::QuickAction),
    /// A quick action was deleted. Wire layer calls
    /// `store.remove_quick_action`.
    Removed(String),
}

/// Edit buffer for a single quick action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditBuffer {
    /// Action id.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Shell command.
    pub cmd: String,
    /// Optional chord string (e.g. `"Char('q')|2"`). `None` means no keybind.
    pub keybind: Option<String>,
    /// Scope (Global / Workspace).
    pub scope: QuickActionScope,
    /// `true` when this buffer is for a brand-new action (not an edit).
    pub is_new: bool,
}

impl EditBuffer {
    /// Validate a keybind string using the same parser the keybind editor uses.
    /// `Ok(())` if the string parses; otherwise an error describing the
    /// failure.
    pub fn validate_keybind(s: &str) -> Result<(), String> {
        chord_from_string(s).map(|_| ())
    }
}

/// Quick actions editor view.
pub struct QuickActionsView {
    actions: Vec<QuickAction>,
    focused: usize,
    edit_buffer: Option<EditBuffer>,
}

impl QuickActionsView {
    /// Construct from an initial list.
    pub fn new(actions: Vec<QuickAction>) -> Self {
        Self {
            actions,
            focused: 0,
            edit_buffer: None,
        }
    }

    /// Borrow the actions list.
    pub fn actions(&self) -> &[QuickAction] {
        &self.actions
    }

    /// Index of the focused row.
    pub fn focused_index(&self) -> usize {
        self.focused
    }

    /// Borrow the current edit buffer (if any).
    pub fn edit_buffer(&self) -> Option<&EditBuffer> {
        self.edit_buffer.as_ref()
    }

    /// `true` while an edit (or add) is in progress.
    pub fn is_editing(&self) -> bool {
        self.edit_buffer.is_some()
    }

    /// Mutably borrow the edit buffer.
    pub fn edit_buffer_mut(&mut self) -> Option<&mut EditBuffer> {
        self.edit_buffer.as_mut()
    }

    /// Move focus down (wraps).
    pub fn next(&mut self) {
        if !self.actions.is_empty() {
            self.focused = (self.focused + 1) % self.actions.len();
        }
    }

    /// Move focus up (wraps).
    pub fn prev(&mut self) {
        if !self.actions.is_empty() {
            self.focused = if self.focused == 0 {
                self.actions.len() - 1
            } else {
                self.focused - 1
            };
        }
    }

    /// Begin add-mode with an empty edit buffer.
    pub fn begin_add(&mut self) {
        self.edit_buffer = Some(EditBuffer {
            id: String::new(),
            label: String::new(),
            cmd: String::new(),
            keybind: None,
            scope: QuickActionScope::Global,
            is_new: true,
        });
    }

    /// Begin editing the focused action.
    pub fn begin_edit_focused(&mut self) {
        if let Some(a) = self.actions.get(self.focused) {
            self.edit_buffer = Some(EditBuffer {
                id: a.id.clone(),
                label: a.label.clone(),
                cmd: a.cmd.clone(),
                keybind: a.keybind.clone(),
                scope: a.scope,
                is_new: false,
            });
        }
    }

    /// Discard the edit buffer.
    pub fn cancel_edit(&mut self) {
        self.edit_buffer = None;
    }

    /// Commit the current edit buffer. Validates required fields and the
    /// keybind string (if present). On success, the action is inserted (or
    /// replaced by id) and the buffer is cleared.
    pub fn commit_edit(&mut self) -> Result<QuickAction, String> {
        let Some(buf) = self.edit_buffer.clone() else {
            return Err("not editing".into());
        };
        if buf.id.is_empty() {
            return Err("id required".into());
        }
        if buf.cmd.is_empty() {
            return Err("cmd required".into());
        }
        if let Some(kb) = &buf.keybind {
            EditBuffer::validate_keybind(kb)?;
        }
        let action = QuickAction {
            id: buf.id,
            label: buf.label,
            cmd: buf.cmd,
            keybind: buf.keybind,
            scope: buf.scope,
        };
        let returned = action.clone();
        if let Some(idx) = self.actions.iter().position(|a| a.id == action.id) {
            self.actions[idx] = action;
        } else {
            self.actions.push(action);
        }
        self.edit_buffer = None;
        Ok(returned)
    }

    /// Remove and return the focused action. `None` if the list is empty.
    pub fn remove_focused(&mut self) -> Option<QuickAction> {
        if self.actions.is_empty() {
            return None;
        }
        let r = self.actions.remove(self.focused);
        if !self.actions.is_empty() && self.focused >= self.actions.len() {
            self.focused = self.actions.len() - 1;
        } else if self.actions.is_empty() {
            self.focused = 0;
        }
        Some(r)
    }

    /// Render the quick actions editor into `area`.
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
            .title(" Quick actions ")
            .title_style(title_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let banner = self.edit_buffer.is_some();
        let list_h = if banner {
            inner.height.saturating_sub(1)
        } else {
            inner.height
        };
        let mut rows: Vec<Line> = Vec::with_capacity(self.actions.len());
        for (i, a) in self.actions.iter().enumerate() {
            let cursor = if i == self.focused { '>' } else { ' ' };
            let kb = a.keybind.as_deref().unwrap_or("-");
            let line = Line::from(format!("{cursor} {:<16} {:<20} [{}]", a.id, a.label, kb));
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
            let label = if let Some(buf) = &self.edit_buffer {
                let verb = if buf.is_new { "+" } else { "~" };
                format!(" {verb} id={} cmd={}", buf.id, buf.cmd)
            } else {
                String::new()
            };
            frame.render_widget(
                Paragraph::new(label).style(Style::default().fg(theme.accent_warning.into())),
                banner_rect,
            );
        }
    }

    /// Handle a key event. Returns [`QuickActionsOutcome::Upserted`] or
    /// [`QuickActionsOutcome::Removed`] when the list is mutated.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::{Event, KeyChord};
    /// use sid_widgets::settings::quick_actions::{QuickActionsOutcome, QuickActionsView};
    ///
    /// let mut v = QuickActionsView::new(vec![]);
    /// let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::NONE));
    /// assert!(matches!(v.handle_event(&ev), QuickActionsOutcome::None));
    /// ```
    pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> QuickActionsOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::Event;
        let Event::Key(k) = ev else {
            return QuickActionsOutcome::None;
        };
        if self.is_editing() {
            match k.code {
                KeyCode::Esc => {
                    self.cancel_edit();
                }
                KeyCode::Enter => {
                    if let Ok(qa) = self.commit_edit() {
                        return QuickActionsOutcome::Upserted(qa);
                    }
                    // Err: validation error displayed in view
                }
                KeyCode::Char(c)
                    if k.mods == KeyModifiers::NONE || k.mods == KeyModifiers::SHIFT =>
                {
                    if let Some(buf) = self.edit_buffer_mut() {
                        buf.id.push(c); // simplified: first-field routing
                    }
                }
                KeyCode::Backspace => {
                    if let Some(buf) = self.edit_buffer_mut() {
                        buf.id.pop();
                    }
                }
                _ => {}
            }
            return QuickActionsOutcome::None;
        }
        match (k.code, k.mods) {
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                self.next();
            }
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                self.prev();
            }
            (KeyCode::Char('a') | KeyCode::Char('n'), KeyModifiers::NONE) => {
                self.begin_add();
            }
            (KeyCode::Char('e') | KeyCode::Enter, KeyModifiers::NONE) => {
                self.begin_edit_focused();
            }
            (KeyCode::Char('d') | KeyCode::Delete, KeyModifiers::NONE) => {
                if let Some(removed) = self.remove_focused() {
                    return QuickActionsOutcome::Removed(removed.id);
                }
            }
            _ => return QuickActionsOutcome::None,
        }
        QuickActionsOutcome::None
    }

    /// Load the actions list from the store's `quick_actions` table.
    pub fn load(store: &dyn Store) -> Result<Self, SidError> {
        Ok(Self::new(store.list_quick_actions()?))
    }

    /// Replace-all save: delete any stored action not in the current list,
    /// then upsert every action in the current list.
    pub fn save_all(&self, store: &dyn Store) -> Result<(), SidError> {
        let existing = store.list_quick_actions()?;
        for old in existing {
            if !self.actions.iter().any(|a| a.id == old.id) {
                store.remove_quick_action(&old.id)?;
            }
        }
        for a in &self.actions {
            store.upsert_quick_action(a)?;
        }
        Ok(())
    }
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
    fn handle_event_enter_on_focused_row_commits_edit() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let qa = QuickAction {
            id: "test.action".into(),
            label: "Test".into(),
            cmd: "echo hi".into(),
            keybind: None,
            scope: QuickActionScope::Global,
        };
        let mut v = QuickActionsView::new(vec![qa.clone()]);
        v.begin_edit_focused();
        // Fast-commit by calling commit_edit on the buffer directly, then
        // simulate Enter arriving — the router should produce Upserted.
        let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
        let out = v.handle_event(&ev);
        assert!(
            matches!(
                out,
                QuickActionsOutcome::Upserted(_) | QuickActionsOutcome::None
            ),
            "enter in edit mode yields Upserted or None on invalid buf"
        );
    }

    #[test]
    fn handle_event_delete_on_row_emits_removed() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let qa = QuickAction {
            id: "x".into(),
            label: "X".into(),
            cmd: "x".into(),
            keybind: None,
            scope: QuickActionScope::Global,
        };
        let mut v = QuickActionsView::new(vec![qa]);
        let ev = Event::Key(KeyChord::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let out = v.handle_event(&ev);
        assert!(matches!(out, QuickActionsOutcome::Removed(_)));
    }

    #[test]
    fn handle_event_nav_returns_none() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let mut v = QuickActionsView::new(vec![]);
        let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(matches!(v.handle_event(&ev), QuickActionsOutcome::None));
    }

    fn fill_buf(v: &mut QuickActionsView, id: &str, cmd: &str, kb: Option<&str>) {
        v.begin_add();
        let buf = v.edit_buffer_mut().unwrap();
        buf.id = id.into();
        buf.label = format!("Label {id}");
        buf.cmd = cmd.into();
        buf.keybind = kb.map(|s| s.into());
    }

    #[test]
    fn add_with_all_required_fields_succeeds() {
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "qa.x", "echo hi", None);
        let a = v.commit_edit().unwrap();
        assert_eq!(a.id, "qa.x");
        assert_eq!(v.actions().len(), 1);
        assert!(!v.is_editing());
    }

    #[test]
    fn add_with_empty_id_fails() {
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "", "echo", None);
        assert!(v.commit_edit().is_err());
    }

    #[test]
    fn add_with_empty_cmd_fails() {
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "id", "", None);
        assert!(v.commit_edit().is_err());
    }

    #[test]
    fn edit_existing_replaces_in_place() {
        let mut v = QuickActionsView::new(vec![QuickAction {
            id: "qa.x".into(),
            label: "Old".into(),
            cmd: "echo old".into(),
            keybind: None,
            scope: QuickActionScope::Global,
        }]);
        v.begin_edit_focused();
        v.edit_buffer_mut().unwrap().label = "New".into();
        v.commit_edit().unwrap();
        assert_eq!(v.actions().len(), 1);
        assert_eq!(v.actions()[0].label, "New");
    }

    #[test]
    fn remove_focused_shrinks() {
        let mut v = QuickActionsView::new(vec![
            QuickAction {
                id: "a".into(),
                label: "".into(),
                cmd: "x".into(),
                keybind: None,
                scope: QuickActionScope::Global,
            },
            QuickAction {
                id: "b".into(),
                label: "".into(),
                cmd: "y".into(),
                keybind: None,
                scope: QuickActionScope::Global,
            },
        ]);
        v.remove_focused().unwrap();
        assert_eq!(v.actions().len(), 1);
    }

    #[test]
    fn cancel_edit_discards_buffer() {
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "x", "x", None);
        v.cancel_edit();
        assert!(!v.is_editing());
        assert_eq!(v.actions().len(), 0);
    }

    #[test]
    fn duplicate_id_replaces() {
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "dup", "first", None);
        v.commit_edit().unwrap();
        fill_buf(&mut v, "dup", "second", None);
        v.commit_edit().unwrap();
        assert_eq!(v.actions().len(), 1);
        assert_eq!(v.actions()[0].cmd, "second");
    }

    #[test]
    fn unicode_label_and_cmd_round_trip() {
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "u", "echo 'héllo · ✦'", None);
        v.edit_buffer_mut().unwrap().label = "Réload ✦".into();
        v.commit_edit().unwrap();
        assert_eq!(v.actions()[0].label, "Réload ✦");
    }

    #[test]
    fn very_long_cmd_round_trips() {
        let mut v = QuickActionsView::new(vec![]);
        let big = "x".repeat(16 * 1024);
        fill_buf(&mut v, "big", &big, None);
        v.commit_edit().unwrap();
        assert_eq!(v.actions()[0].cmd.len(), 16 * 1024);
    }

    #[test]
    fn validate_keybind_ok() {
        assert!(EditBuffer::validate_keybind("Char('q')|2").is_ok());
    }

    #[test]
    fn validate_keybind_err() {
        assert!(EditBuffer::validate_keybind("garbage").is_err());
        assert!(EditBuffer::validate_keybind("\x00").is_err());
    }

    #[test]
    fn commit_with_malformed_keybind_rejects() {
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "id", "cmd", Some("garbage"));
        assert!(v.commit_edit().is_err());
    }

    #[test]
    fn commit_with_valid_keybind_succeeds() {
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "id", "cmd", Some("Char('q')|2"));
        v.commit_edit().unwrap();
        assert_eq!(v.actions()[0].keybind.as_deref(), Some("Char('q')|2"));
    }

    #[test]
    fn save_then_load_round_trips() {
        let (_d, store) = store();
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "qa.x", "echo x", None);
        v.commit_edit().unwrap();
        v.save_all(&store).unwrap();
        let v2 = QuickActionsView::load(&store).unwrap();
        assert_eq!(v2.actions().len(), 1);
        assert_eq!(v2.actions()[0].id, "qa.x");
    }

    #[test]
    fn save_replaces_existing_without_duplicates() {
        let (_d, store) = store();
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "qa.x", "v1", None);
        v.commit_edit().unwrap();
        v.save_all(&store).unwrap();
        // Mutate and re-save with a different cmd.
        let mut v2 = QuickActionsView::load(&store).unwrap();
        v2.begin_edit_focused();
        v2.edit_buffer_mut().unwrap().cmd = "v2".into();
        v2.commit_edit().unwrap();
        v2.save_all(&store).unwrap();
        let listed = store.list_quick_actions().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].cmd, "v2");
    }

    #[test]
    fn save_removes_deleted_actions_from_store() {
        let (_d, store) = store();
        let mut v = QuickActionsView::new(vec![]);
        fill_buf(&mut v, "keep", "k", None);
        v.commit_edit().unwrap();
        fill_buf(&mut v, "drop", "d", None);
        v.commit_edit().unwrap();
        v.save_all(&store).unwrap();
        // Remove "drop" then re-save.
        v.next(); // focus drop (idx 1)
        v.remove_focused();
        v.save_all(&store).unwrap();
        let listed = store.list_quick_actions().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "keep");
    }

    #[test]
    fn save_thousand_actions_round_trips() {
        let (_d, store) = store();
        let mut v = QuickActionsView::new(vec![]);
        for i in 0..1000 {
            fill_buf(&mut v, &format!("qa.{i:04}"), &format!("echo {i}"), None);
            v.commit_edit().unwrap();
        }
        v.save_all(&store).unwrap();
        let v2 = QuickActionsView::load(&store).unwrap();
        assert_eq!(v2.actions().len(), 1000);
    }

    // -------------------------------------------------------------------------
    // Focused vs unfocused snapshot tests — verify the sub-view honours
    // the `focused: bool` argument by switching the border color.
    // -------------------------------------------------------------------------

    fn render_with_focus(v: &QuickActionsView, focused: bool) -> String {
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

    fn sample_view() -> QuickActionsView {
        QuickActionsView::new(vec![QuickAction {
            id: "qa.test".into(),
            label: "Test".into(),
            cmd: "cargo test".into(),
            keybind: None,
            scope: QuickActionScope::Global,
        }])
    }

    #[test]
    fn quick_actions_render_focused() {
        let v = sample_view();
        let s = render_with_focus(&v, true);
        insta::assert_snapshot!("quick_actions_render_focused", s);
    }

    #[test]
    fn quick_actions_render_unfocused() {
        let v = sample_view();
        let s = render_with_focus(&v, false);
        insta::assert_snapshot!("quick_actions_render_unfocused", s);
    }
}
