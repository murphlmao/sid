//! Behavior toggles sub-view.
//!
//! The right pane is a vertical (label, value) list. Each [`Toggle`] carries a
//! canonical `key` (from `sid_store::settings_keys`) and a typed [`ToggleValue`]
//! — bool, choice, u64, or string. The user moves up/down with the focus
//! arrow keys and cycles the focused value with left/right.
//!
//! The view tracks a set of `dirty` keys so [`BehaviorTogglesView::flush_dirty`]
//! can issue only the necessary `put_*` calls against a [`sid_store::Store`].
//!
//! # Examples
//!
//! ```
//! use sid_widgets::settings::behavior_toggles::BehaviorTogglesView;
//!
//! let v = BehaviorTogglesView::defaults();
//! assert_eq!(v.toggles().len(), 6);
//! assert_eq!(v.focused_index(), 0);
//! ```

use std::collections::BTreeSet;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use sid_core::SidError;
use sid_store::{Store, TypedSettings};
use sid_ui::Theme;

/// Outcome of a single key event routed into the behavior toggles view.
///
/// Mirrors the [`crate::settings::theme_picker::ThemePickerOutcome`] shape
/// so the wire layer can dispatch with a uniform match. The caller is
/// expected to forward `Toggled` outcomes to the binary's settings
/// dispatch (which then calls the right `Store::put_*`).
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::behavior_toggles::{
///     BehaviorTogglesOutcome, ToggleValue,
/// };
/// let o = BehaviorTogglesOutcome::Toggled {
///     key: "auto_restore_session",
///     value: ToggleValue::Bool(true),
/// };
/// assert!(matches!(o, BehaviorTogglesOutcome::Toggled { .. }));
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BehaviorTogglesOutcome {
    /// No state change — caller should not emit.
    None,
    /// User cycled the focused value. The wire layer should put_* the
    /// new value at `key`.
    Toggled {
        /// Canonical setting key (see [`sid_store::settings_keys`]).
        key: &'static str,
        /// The new value as held by the view.
        value: ToggleValue,
    },
}

/// Typed value for a single [`Toggle`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToggleValue {
    /// A two-state switch.
    Bool(bool),
    /// A pick-one-of-N option.
    Choice {
        /// All available options.
        options: Vec<String>,
        /// Currently selected index (always `< options.len()`).
        selected: usize,
    },
    /// A bounded integer (clamps to `[min, max]`).
    U64 {
        /// Current value.
        value: u64,
        /// Minimum (inclusive).
        min: u64,
        /// Maximum (inclusive).
        max: u64,
        /// Step used by `cycle_focused_value`.
        step: u64,
    },
    /// A free-form string.
    String(String),
}

/// One row in the behavior toggles list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Toggle {
    /// Canonical setting key (see `sid_store::settings_keys`).
    pub key: &'static str,
    /// Human-readable label.
    pub label: &'static str,
    /// Current value.
    pub value: ToggleValue,
}

/// State for the behavior toggles sub-view.
pub struct BehaviorTogglesView {
    toggles: Vec<Toggle>,
    focused: usize,
    /// Set of keys modified since last `clear_dirty` / `flush_dirty`.
    dirty: BTreeSet<&'static str>,
}

impl BehaviorTogglesView {
    /// Build the canonical default toggle list.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::settings::behavior_toggles::BehaviorTogglesView;
    /// let v = BehaviorTogglesView::defaults();
    /// assert_eq!(v.toggles().len(), 6);
    /// ```
    pub fn defaults() -> Self {
        use sid_store::settings_keys::*;
        Self {
            toggles: vec![
                Toggle {
                    key: AUTO_RESTORE_SESSION,
                    label: "Auto-restore session",
                    value: ToggleValue::Choice {
                        options: vec!["yes".into(), "ask".into(), "no".into()],
                        selected: 1,
                    },
                },
                // AUTO_SCAN_WORKSPACES removed (deprecated — the store key is
                // retained for backward-compat but the toggle has been removed
                // from the UI).
                Toggle {
                    key: PERSIST_DEBOUNCE_MS,
                    label: "State persist debounce (ms)",
                    value: ToggleValue::U64 {
                        value: 250,
                        min: 50,
                        max: 5000,
                        step: 10,
                    },
                },
                Toggle {
                    key: HEARTBEAT_INTERVAL_SECS,
                    label: "Session heartbeat interval (s)",
                    value: ToggleValue::U64 {
                        value: 5,
                        min: 1,
                        max: 300,
                        step: 1,
                    },
                },
                Toggle {
                    key: DEFAULT_TAB,
                    label: "Default tab on launch",
                    value: ToggleValue::Choice {
                        options: vec![
                            "workspaces".into(),
                            "ssh".into(),
                            "database".into(),
                            "network".into(),
                            "system".into(),
                            "settings".into(),
                        ],
                        selected: 0,
                    },
                },
                Toggle {
                    key: USE_OS_KEYRING,
                    label: "Use OS keyring for secrets (requires restart)",
                    value: ToggleValue::Bool(false),
                },
                Toggle {
                    key: CONFIG_EDITOR,
                    label: "Config editor (System tab)",
                    value: ToggleValue::Choice {
                        // Order/strings mirror `sid_core::EditorChoice`:
                        // nano (default) / vim / vi run inline; terminal spawns
                        // the user's terminal emulator. `selected: 0` == nano.
                        options: vec!["nano".into(), "vim".into(), "vi".into(), "terminal".into()],
                        selected: 0,
                    },
                },
            ],
            focused: 0,
            dirty: BTreeSet::new(),
        }
    }

    /// Borrow the toggle list.
    pub fn toggles(&self) -> &[Toggle] {
        &self.toggles
    }

    /// Focused row index.
    pub fn focused_index(&self) -> usize {
        self.focused
    }

    /// Focused toggle, if any.
    pub fn focused(&self) -> Option<&Toggle> {
        self.toggles.get(self.focused)
    }

    /// Move focus down (wraps).
    pub fn next(&mut self) {
        if !self.toggles.is_empty() {
            self.focused = (self.focused + 1) % self.toggles.len();
        }
    }

    /// Move focus up (wraps).
    pub fn prev(&mut self) {
        if !self.toggles.is_empty() {
            self.focused = if self.focused == 0 {
                self.toggles.len() - 1
            } else {
                self.focused - 1
            };
        }
    }

    /// Route a key event into the view, returning what happened.
    ///
    /// Up/Down move focus only. Left/Right cycle the focused value and
    /// return [`BehaviorTogglesOutcome::Toggled`] so the wire layer can
    /// dispatch to the right `Store::put_*`. Other keys are no-ops.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::{Event, KeyChord};
    /// use sid_widgets::settings::behavior_toggles::{
    ///     BehaviorTogglesOutcome, BehaviorTogglesView,
    /// };
    ///
    /// let mut v = BehaviorTogglesView::defaults();
    /// let ev = Event::Key(KeyChord::new(KeyCode::Right, KeyModifiers::NONE));
    /// assert!(matches!(v.handle_event(&ev), BehaviorTogglesOutcome::Toggled { .. }));
    /// ```
    pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> BehaviorTogglesOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        let sid_core::event::Event::Key(chord) = ev else {
            return BehaviorTogglesOutcome::None;
        };
        match (chord.code, chord.mods) {
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                self.next();
                BehaviorTogglesOutcome::None
            }
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                self.prev();
                BehaviorTogglesOutcome::None
            }
            (KeyCode::Char('l') | KeyCode::Right, KeyModifiers::NONE) => {
                self.cycle_focused_value(1);
                if let Some(t) = self.focused() {
                    BehaviorTogglesOutcome::Toggled {
                        key: t.key,
                        value: t.value.clone(),
                    }
                } else {
                    BehaviorTogglesOutcome::None
                }
            }
            (KeyCode::Char('h') | KeyCode::Left, KeyModifiers::NONE) => {
                self.cycle_focused_value(-1);
                if let Some(t) = self.focused() {
                    BehaviorTogglesOutcome::Toggled {
                        key: t.key,
                        value: t.value.clone(),
                    }
                } else {
                    BehaviorTogglesOutcome::None
                }
            }
            _ => BehaviorTogglesOutcome::None,
        }
    }

    /// Cycle the focused toggle's value. `dir` is `+1` to advance and `-1`
    /// to reverse. Other values are treated as `+1`.
    ///
    /// - `Bool` flips.
    /// - `Choice` advances (wraps).
    /// - `U64` adds `dir * step`, clamped to `[min, max]`. If `max == min`,
    ///   no change.
    /// - `String` is left untouched (string toggles are edited via the
    ///   input path, not cycling).
    pub fn cycle_focused_value(&mut self, dir: i32) {
        let step_dir: i64 = if dir < 0 { -1 } else { 1 };
        let Some(t) = self.toggles.get_mut(self.focused) else {
            return;
        };
        let key = t.key;
        let changed = match &mut t.value {
            ToggleValue::Bool(b) => {
                *b = !*b;
                true
            }
            ToggleValue::Choice { options, selected } => {
                if options.is_empty() {
                    false
                } else {
                    let len = options.len() as i64;
                    let new = ((*selected as i64) + step_dir).rem_euclid(len);
                    *selected = new as usize;
                    true
                }
            }
            ToggleValue::U64 {
                value,
                min,
                max,
                step,
            } => {
                if *max == *min {
                    false
                } else {
                    let delta = (*step as i64) * step_dir;
                    let raw = (*value as i64).saturating_add(delta);
                    let clamped = raw.clamp(*min as i64, *max as i64);
                    let new = clamped as u64;
                    let did_change = new != *value;
                    *value = new;
                    did_change
                }
            }
            ToggleValue::String(_) => false,
        };
        if changed {
            self.dirty.insert(key);
        }
    }

    /// Iterate over dirty keys.
    pub fn dirty_keys(&self) -> impl Iterator<Item = &&'static str> {
        self.dirty.iter()
    }

    /// Clear the dirty set without writing.
    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
    }

    /// Load every toggle from `store`. Unknown / missing keys leave the value
    /// at its current default. Invalid stored bytes propagate as
    /// `SidError::Storage`.
    pub fn load_from_store(&mut self, store: &dyn Store) -> Result<(), SidError> {
        for t in self.toggles.iter_mut() {
            match &mut t.value {
                ToggleValue::Bool(b) => {
                    if let Some(v) = store.get_bool(t.key)? {
                        *b = v;
                    }
                }
                ToggleValue::U64 {
                    value, min, max, ..
                } => {
                    if let Some(v) = store.get_u64(t.key)? {
                        // Clamp to validity on load; the stored value might
                        // have been set when the bounds were different.
                        *value = v.clamp(*min, *max);
                    }
                }
                ToggleValue::Choice { options, selected } => {
                    if let Some(s) = store.get_string(t.key)?
                        && let Some(idx) = options.iter().position(|o| o == &s)
                    {
                        *selected = idx;
                    }
                }
                ToggleValue::String(s) => {
                    if let Some(v) = store.get_string(t.key)? {
                        *s = v;
                    }
                }
            }
        }
        Ok(())
    }

    /// Render the toggles list into `area`.
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
            .title(" Behavior ")
            .title_style(title_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let mut rows: Vec<Line> = Vec::with_capacity(self.toggles.len());
        for (i, t) in self.toggles.iter().enumerate() {
            let cursor = if i == self.focused { '>' } else { ' ' };
            let value = match &t.value {
                ToggleValue::Bool(b) => {
                    if *b {
                        "on".to_string()
                    } else {
                        "off".to_string()
                    }
                }
                ToggleValue::Choice { options, selected } => options
                    .get(*selected)
                    .cloned()
                    .unwrap_or_else(|| "?".into()),
                ToggleValue::U64 { value, .. } => value.to_string(),
                ToggleValue::String(s) => s.clone(),
            };
            let line = Line::from(format!("{cursor} {:<36} {}", t.label, value));
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
        frame.render_widget(Paragraph::new(rows), inner);
    }

    /// Write every dirty toggle to `store`. Returns the number of keys
    /// written. Dirty set is cleared on success.
    pub fn flush_dirty(&mut self, store: &dyn Store) -> Result<usize, SidError> {
        let dirty: Vec<&'static str> = self.dirty.iter().copied().collect();
        let mut wrote = 0;
        for key in dirty {
            let t = self
                .toggles
                .iter()
                .find(|t| t.key == key)
                .expect("dirty key must exist in toggles");
            match &t.value {
                ToggleValue::Bool(b) => store.put_bool(key, *b)?,
                ToggleValue::U64 { value, .. } => store.put_u64(key, *value)?,
                ToggleValue::Choice { options, selected } => {
                    store.put_string(key, &options[*selected])?
                }
                ToggleValue::String(s) => store.put_string(key, s)?,
            }
            wrote += 1;
        }
        self.dirty.clear();
        Ok(wrote)
    }
}

impl Default for BehaviorTogglesView {
    fn default() -> Self {
        Self::defaults()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use sid_store::{OpenStore, RedbStore, SettingValue, settings_keys};
    use tempfile::tempdir;

    use super::*;

    fn store() -> (tempfile::TempDir, RedbStore) {
        let d = tempdir().unwrap();
        let s = RedbStore::open(&d.path().join("s.redb")).unwrap();
        (d, s)
    }

    #[test]
    fn defaults_has_six_toggles() {
        let v = BehaviorTogglesView::defaults();
        assert_eq!(v.toggles().len(), 6);
    }

    #[test]
    fn behavior_toggles_includes_config_editor_choice() {
        use sid_store::settings_keys::CONFIG_EDITOR;
        let v = BehaviorTogglesView::defaults();
        let t = v
            .toggles()
            .iter()
            .find(|t| t.key == CONFIG_EDITOR)
            .expect("config_editor toggle present");
        match &t.value {
            ToggleValue::Choice { options, selected } => {
                assert_eq!(options, &["nano", "vim", "vi", "terminal"]);
                // Default is nano (index 0), matching EditorChoice::default().
                assert_eq!(*selected, 0);
            }
            other => panic!("config_editor should be a Choice, got {other:?}"),
        }
    }

    #[test]
    fn behavior_toggles_includes_use_os_keyring() {
        use sid_store::settings_keys::USE_OS_KEYRING;
        let v = BehaviorTogglesView::defaults();
        assert!(v.toggles().iter().any(|t| t.key == USE_OS_KEYRING));
    }

    #[test]
    fn cycle_focused_bool_flips_and_marks_dirty() {
        let mut v = BehaviorTogglesView::defaults();
        // Index 4 is `use_os_keyring: Bool(false)` after removal of
        // `auto_scan_workspaces`.
        for _ in 0..4 {
            v.next();
        }
        v.cycle_focused_value(1);
        match &v.focused().unwrap().value {
            ToggleValue::Bool(b) => assert!(*b, "use_os_keyring should now be true"),
            other => panic!("expected Bool, got {other:?}"),
        }
        assert!(v.dirty_keys().any(|k| *k == settings_keys::USE_OS_KEYRING));
    }

    #[test]
    fn cycle_focused_choice_wraps() {
        let mut v = BehaviorTogglesView::defaults();
        // Index 0: auto_restore_session, options ["yes","ask","no"], selected = 1.
        v.cycle_focused_value(1); // ask -> no
        v.cycle_focused_value(1); // no -> yes (wrap)
        match &v.focused().unwrap().value {
            ToggleValue::Choice { selected, .. } => assert_eq!(*selected, 0),
            other => panic!("expected Choice, got {other:?}"),
        }
    }

    #[test]
    fn cycle_focused_choice_reverse_wraps_negative() {
        let mut v = BehaviorTogglesView::defaults();
        // Index 0: selected = 1; reverse cycle once -> 0; twice -> 2 (wrap).
        v.cycle_focused_value(-1);
        v.cycle_focused_value(-1);
        match &v.focused().unwrap().value {
            ToggleValue::Choice { selected, .. } => assert_eq!(*selected, 2),
            other => panic!("expected Choice, got {other:?}"),
        }
    }

    #[test]
    fn cycle_focused_u64_increments_clamped() {
        let mut v = BehaviorTogglesView::defaults();
        // Index 1: persist_debounce_ms = 250, max = 5000, step = 10
        // (was index 2 before auto_scan_workspaces removal).
        v.next();
        for _ in 0..1000 {
            v.cycle_focused_value(1);
        }
        match &v.focused().unwrap().value {
            ToggleValue::U64 { value, .. } => assert_eq!(*value, 5000),
            other => panic!("expected U64, got {other:?}"),
        }
    }

    #[test]
    fn cycle_focused_u64_decrements_clamped_at_min() {
        let mut v = BehaviorTogglesView::defaults();
        v.next();
        for _ in 0..1000 {
            v.cycle_focused_value(-1);
        }
        match &v.focused().unwrap().value {
            ToggleValue::U64 { value, min, .. } => assert_eq!(*value, *min),
            other => panic!("expected U64, got {other:?}"),
        }
    }

    #[test]
    fn cycle_u64_with_equal_min_max_is_noop() {
        let mut v = BehaviorTogglesView::defaults();
        // Replace persist_debounce_ms with min == max.
        v.next();
        if let Some(t) = v.toggles.get_mut(v.focused) {
            t.value = ToggleValue::U64 {
                value: 100,
                min: 100,
                max: 100,
                step: 10,
            };
        }
        v.cycle_focused_value(1);
        v.cycle_focused_value(-1);
        match &v.focused().unwrap().value {
            ToggleValue::U64 { value, .. } => assert_eq!(*value, 100),
            other => panic!("expected U64, got {other:?}"),
        }
        assert!(v.dirty_keys().count() == 0);
    }

    #[test]
    fn dirty_keys_reflects_only_modified() {
        let mut v = BehaviorTogglesView::defaults();
        v.cycle_focused_value(1); // index 0 — choice
        let dirty: Vec<_> = v.dirty_keys().copied().collect();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0], settings_keys::AUTO_RESTORE_SESSION);
    }

    #[test]
    fn clear_dirty_resets() {
        let mut v = BehaviorTogglesView::defaults();
        v.cycle_focused_value(1);
        v.clear_dirty();
        assert_eq!(v.dirty_keys().count(), 0);
    }

    #[test]
    fn next_prev_keep_focused_in_bounds() {
        let mut v = BehaviorTogglesView::defaults();
        for _ in 0..1000 {
            v.next();
        }
        assert!(v.focused_index() < v.toggles().len());
        for _ in 0..1000 {
            v.prev();
        }
        assert!(v.focused_index() < v.toggles().len());
    }

    #[test]
    fn flush_dirty_round_trips_to_store() {
        let (_d, store) = store();
        let mut v = BehaviorTogglesView::defaults();
        // Cycle persist_debounce_ms (index 1, U64, step=10, default=250).
        v.next();
        v.cycle_focused_value(1); // 250 + 10 = 260
        let wrote = v.flush_dirty(&store).unwrap();
        assert_eq!(wrote, 1);
        assert_eq!(v.dirty_keys().count(), 0);

        let mut v2 = BehaviorTogglesView::defaults();
        v2.load_from_store(&store).unwrap();
        // index 1 should now be 260.
        match &v2.toggles()[1].value {
            ToggleValue::U64 { value, .. } => assert_eq!(*value, 260),
            other => panic!("expected U64, got {other:?}"),
        }
    }

    #[test]
    fn flush_is_idempotent() {
        let (_d, store) = store();
        let mut v = BehaviorTogglesView::defaults();
        v.cycle_focused_value(1);
        v.flush_dirty(&store).unwrap();
        let second = v.flush_dirty(&store).unwrap();
        assert_eq!(second, 0);
    }

    #[test]
    fn load_with_unknown_choice_keeps_default() {
        let (_d, store) = store();
        store
            .put_setting(
                settings_keys::AUTO_RESTORE_SESSION,
                &SettingValue(b"banana".to_vec()),
            )
            .unwrap();
        let mut v = BehaviorTogglesView::defaults();
        v.load_from_store(&store).unwrap();
        // Still on the default selected=1 ("ask").
        match &v.toggles()[0].value {
            ToggleValue::Choice { selected, .. } => assert_eq!(*selected, 1),
            other => panic!("expected Choice, got {other:?}"),
        }
    }

    #[test]
    fn load_clamps_oob_u64() {
        let (_d, store) = store();
        store
            .put_u64(settings_keys::PERSIST_DEBOUNCE_MS, u64::MAX)
            .unwrap();
        let mut v = BehaviorTogglesView::defaults();
        v.load_from_store(&store).unwrap();
        // persist_debounce_ms is now at index 1 (auto_scan_workspaces removed).
        match &v.toggles()[1].value {
            ToggleValue::U64 { value, max, .. } => {
                assert_eq!(*value, *max, "expected clamped to max");
            }
            other => panic!("expected U64, got {other:?}"),
        }
    }

    #[test]
    fn load_invalid_bool_returns_err() {
        let (_d, store) = store();
        // auto_scan_workspaces is no longer in the toggle list; target
        // use_os_keyring (still a Bool toggle) instead.
        store
            .put_setting(
                settings_keys::USE_OS_KEYRING,
                &SettingValue(b"maybe".to_vec()),
            )
            .unwrap();
        let mut v = BehaviorTogglesView::defaults();
        assert!(v.load_from_store(&store).is_err());
    }

    proptest! {
        #[test]
        fn prop_focused_index_in_bounds(steps in 0usize..256) {
            let mut v = BehaviorTogglesView::defaults();
            for i in 0..steps {
                if i % 2 == 0 { v.next() } else { v.prev() }
                prop_assert!(v.focused_index() < v.toggles().len());
            }
        }

        #[test]
        fn prop_choice_selected_in_bounds(steps in 0usize..1024, dir in any::<i8>()) {
            let mut v = BehaviorTogglesView::defaults();
            // index 0 is a choice; stay focused there.
            let dir_i32 = if dir < 0 { -1 } else { 1 };
            for _ in 0..steps {
                v.cycle_focused_value(dir_i32);
                if let ToggleValue::Choice { options, selected } = &v.toggles()[0].value {
                    prop_assert!(*selected < options.len());
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // Focused vs unfocused snapshot tests — verify the sub-view honours
    // the `focused: bool` argument by switching the border color.
    // -------------------------------------------------------------------------

    fn render_with_focus(v: &BehaviorTogglesView, focused: bool) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        use sid_ui::themes::cosmos;
        let backend = TestBackend::new(60, 12);
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
    fn behavior_toggles_render_focused() {
        let v = BehaviorTogglesView::defaults();
        let s = render_with_focus(&v, true);
        insta::assert_snapshot!("behavior_toggles_render_focused", s);
    }

    #[test]
    fn behavior_toggles_render_unfocused() {
        let v = BehaviorTogglesView::defaults();
        let s = render_with_focus(&v, false);
        insta::assert_snapshot!("behavior_toggles_render_unfocused", s);
    }
}
