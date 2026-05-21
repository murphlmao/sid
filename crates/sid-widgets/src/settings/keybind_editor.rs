//! Keybind editor sub-view.
//!
//! Renders the registered actions in an in-memory list; pressing `Enter` on
//! a row puts the editor into capture mode (driven by the
//! [`sid_core::keybind_capture::CaptureState`] state machine). When a chord is
//! captured the view checks for conflicts: if the chord is already bound to a
//! different action, the view transitions to `ConfirmOverwrite` and the user
//! decides whether to proceed. On apply the in-memory [`KeybindMap`] is
//! mutated; persistence is the owning composer's job (Task 20).
//!
//! Re-binding the action `app.quit` is *allowed* — per the spec's "should warn
//! but allow" directive — but yields a non-empty
//! [`KeybindEditorView::dangerous_action_warnings`] for the UI to show.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_core::action::{ActionId, ActionRegistry};
use sid_core::event::KeyChord;
use sid_core::keybind::{KeyBinding, KeybindMap};
use sid_core::keybind_capture::{CaptureInput, CaptureState};
use sid_ui::Theme;

/// Action ids that warrant a "are you sure?" warning when rebound. Centralised
/// so the test and the rendering code agree.
const DANGEROUS_ACTIONS: &[&str] = &["app.quit"];

/// Keybind editor state. Operates on a local mutable copy of the
/// [`KeybindMap`]; the owning composer is responsible for persisting it.
///
/// # Examples
///
/// ```
/// use sid_core::action::{Action, ActionRegistry};
/// use sid_core::keybind::KeybindMap;
/// use sid_widgets::settings::keybind_editor::KeybindEditorView;
///
/// let mut reg = ActionRegistry::new();
/// reg.register(Action::new("app.quit", "Quit"));
/// let view = KeybindEditorView::new(&reg, KeybindMap::cosmos_default());
/// assert_eq!(view.focused_action().map(|a| a.as_str()), Some("app.quit"));
/// ```
pub struct KeybindEditorView {
    actions: Vec<ActionId>,
    /// Local mutable copy of the binding map; saved back to the store on apply.
    map: KeybindMap,
    focused: usize,
    capture: CaptureState,
    /// Warnings emitted by the last `on_chord_captured` call (consumed by the
    /// rendering code; cleared when capture transitions back to `Idle`).
    warnings: Vec<&'static str>,
}

impl KeybindEditorView {
    /// Build a new editor view from a registry snapshot and a binding map.
    ///
    /// The action list is taken from `registry.all()`; iteration order is the
    /// registry's id-sorted order.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let view = KeybindEditorView::new(&ActionRegistry::new(), KeybindMap::new());
    /// assert!(view.focused_action().is_none());
    /// ```
    pub fn new(registry: &ActionRegistry, map: KeybindMap) -> Self {
        let actions: Vec<_> = registry.all().map(|a| a.id.clone()).collect();
        Self {
            actions,
            map,
            focused: 0,
            capture: CaptureState::new(),
            warnings: Vec::new(),
        }
    }

    /// Action id at the current focus row, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    /// use sid_core::keybind::KeybindMap;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("a", "A"));
    /// let view = KeybindEditorView::new(&reg, KeybindMap::new());
    /// assert_eq!(view.focused_action().unwrap().as_str(), "a");
    /// ```
    pub fn focused_action(&self) -> Option<&ActionId> {
        self.actions.get(self.focused)
    }

    /// Lookup the chord currently bound to `action`, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::action::{Action, ActionId, ActionRegistry};
    /// use sid_core::event::KeyChord;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("app.quit", "Quit"));
    /// let view = KeybindEditorView::new(&reg, KeybindMap::cosmos_default());
    /// let quit = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    /// assert_eq!(view.binding_for(&ActionId::new("app.quit")), Some(quit));
    /// ```
    pub fn binding_for(&self, action: &ActionId) -> Option<KeyChord> {
        self.map.iter().find(|(_, a)| *a == action).map(|(c, _)| *c)
    }

    /// Move focus down (wraps).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    /// use sid_core::keybind::KeybindMap;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("a", "A"));
    /// reg.register(Action::new("b", "B"));
    /// let mut view = KeybindEditorView::new(&reg, KeybindMap::new());
    /// view.next();
    /// assert_eq!(view.focused_action().unwrap().as_str(), "b");
    /// ```
    pub fn next(&mut self) {
        if !self.actions.is_empty() {
            self.focused = (self.focused + 1) % self.actions.len();
        }
    }

    /// Move focus up (wraps).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    /// use sid_core::keybind::KeybindMap;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("a", "A"));
    /// reg.register(Action::new("b", "B"));
    /// let mut view = KeybindEditorView::new(&reg, KeybindMap::new());
    /// view.prev();
    /// assert_eq!(view.focused_action().unwrap().as_str(), "b");
    /// ```
    pub fn prev(&mut self) {
        if !self.actions.is_empty() {
            self.focused = if self.focused == 0 {
                self.actions.len() - 1
            } else {
                self.focused - 1
            };
        }
    }

    /// Current capture state.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::keybind_capture::CaptureState;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let view = KeybindEditorView::new(&ActionRegistry::new(), KeybindMap::new());
    /// assert_eq!(view.capture_state(), &CaptureState::Idle);
    /// ```
    pub fn capture_state(&self) -> &CaptureState {
        &self.capture
    }

    /// Begin capture for the focused action. No-op if there is no focused
    /// action (empty registry).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::keybind_capture::CaptureState;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("a", "A"));
    /// let mut view = KeybindEditorView::new(&reg, KeybindMap::new());
    /// view.enter_capture();
    /// assert!(matches!(view.capture_state(), CaptureState::Waiting { .. }));
    /// ```
    pub fn enter_capture(&mut self) {
        self.warnings.clear();
        if let Some(a) = self.actions.get(self.focused).cloned() {
            self.capture = std::mem::take(&mut self.capture).step(CaptureInput::EnterCaptureFor(a));
        }
    }

    /// Cancel any in-progress capture. Returns to [`CaptureState::Idle`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::keybind_capture::CaptureState;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("a", "A"));
    /// let mut view = KeybindEditorView::new(&reg, KeybindMap::new());
    /// view.enter_capture();
    /// view.cancel_capture();
    /// assert_eq!(view.capture_state(), &CaptureState::Idle);
    /// ```
    pub fn cancel_capture(&mut self) {
        self.capture = std::mem::take(&mut self.capture).step(CaptureInput::Cancel);
        self.warnings.clear();
    }

    /// Borrow the in-memory binding map.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let view = KeybindEditorView::new(&ActionRegistry::new(), KeybindMap::new());
    /// assert_eq!(view.map().iter().count(), 0);
    /// ```
    pub fn map(&self) -> &KeybindMap {
        &self.map
    }

    /// Drive the capture machine with a freshly-captured chord. If the chord
    /// is unbound (or already maps to the same action), the new binding is
    /// applied immediately. Otherwise the editor transitions to
    /// [`CaptureState::ConfirmOverwrite`].
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::action::{Action, ActionRegistry};
    /// use sid_core::event::KeyChord;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_core::keybind_capture::CaptureState;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("a", "A"));
    /// let mut view = KeybindEditorView::new(&reg, KeybindMap::new());
    /// view.enter_capture();
    /// view.on_chord_captured(KeyChord::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    /// // Unbound chord, no conflict — applies and returns to Idle.
    /// assert_eq!(view.capture_state(), &CaptureState::Idle);
    /// ```
    pub fn on_chord_captured(&mut self, chord: KeyChord) {
        self.capture = std::mem::take(&mut self.capture).step(CaptureInput::ChordPressed(chord));
        let CaptureState::Captured { for_action, chord } = self.capture.clone() else {
            return;
        };
        // Snapshot any warnings *before* we mutate the map.
        self.update_warnings(&for_action, &chord);
        match self.map.lookup(&chord) {
            Some(existing) if existing != &for_action => {
                let conflicting = existing.clone();
                self.capture =
                    std::mem::take(&mut self.capture).step(CaptureInput::ConflictResolved {
                        conflicting_action: conflicting,
                    });
            }
            _ => {
                self.capture = std::mem::take(&mut self.capture).step(CaptureInput::NoConflict);
                self.apply_if_ready();
            }
        }
    }

    /// Confirm the overwrite (the captured chord becomes the new binding,
    /// displacing the conflicting action).
    pub fn confirm_overwrite_yes(&mut self) {
        self.capture = std::mem::take(&mut self.capture).step(CaptureInput::ConfirmYes);
        self.apply_if_ready();
    }

    /// Decline the overwrite (return to capture waiting state for another
    /// chord).
    pub fn confirm_overwrite_no(&mut self) {
        self.capture = std::mem::take(&mut self.capture).step(CaptureInput::ConfirmNo);
    }

    /// Warning strings emitted by the most recent chord capture. Cleared on
    /// `cancel_capture` / `enter_capture`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionRegistry;
    /// use sid_core::keybind::KeybindMap;
    /// use sid_widgets::settings::keybind_editor::KeybindEditorView;
    ///
    /// let view = KeybindEditorView::new(&ActionRegistry::new(), KeybindMap::new());
    /// assert!(view.dangerous_action_warnings().is_empty());
    /// ```
    pub fn dangerous_action_warnings(&self) -> &[&'static str] {
        &self.warnings
    }

    /// Render the keybind editor into `area`.
    ///
    /// Each row is `action.id   chord-or-(unbound)`. The focused row is
    /// highlighted; a capture-mode banner is rendered at the bottom while
    /// capture is active.
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let title = if matches!(self.capture, CaptureState::Idle) {
            " Keybinds "
        } else {
            " Keybinds (capturing — Esc to cancel) "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent_primary.into()))
            .title(title)
            .title_style(
                Style::default()
                    .fg(theme.foreground.into())
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Reserve one row at the bottom for warnings / capture banner if needed.
        let banner = !matches!(self.capture, CaptureState::Idle) || !self.warnings.is_empty();
        let list_h = if banner {
            inner.height.saturating_sub(1)
        } else {
            inner.height
        };

        let mut rows: Vec<Line> = Vec::with_capacity(self.actions.len());
        for (i, a) in self.actions.iter().enumerate() {
            let cursor = if i == self.focused { '>' } else { ' ' };
            let chord = self
                .binding_for(a)
                .map(|c| format!("{c:?}"))
                .unwrap_or_else(|| "(unbound)".into());
            let line = Line::from(format!("{cursor} {:<24} {}", a.as_str(), chord));
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
            let label = match &self.capture {
                CaptureState::Waiting { for_action } => {
                    format!(" capturing for {}…", for_action.as_str())
                }
                CaptureState::ConfirmOverwrite { .. } => {
                    " overwrite existing binding? (y/n) ".into()
                }
                _ => {
                    if let Some(w) = self.warnings.first() {
                        format!(" {w}")
                    } else {
                        String::new()
                    }
                }
            };
            frame.render_widget(
                Paragraph::new(label).style(Style::default().fg(theme.accent_warning.into())),
                banner_rect,
            );
        }
    }

    fn update_warnings(&mut self, for_action: &ActionId, chord: &KeyChord) {
        self.warnings.clear();
        // Rebinding *to* a dangerous action's existing chord -> warn.
        if DANGEROUS_ACTIONS.iter().any(|d| for_action.as_str() == *d) {
            self.warnings
                .push("warning: rebinding a dangerous action (e.g. app.quit)");
        }
        // Displacing a dangerous action's chord -> warn.
        if let Some(existing) = self.map.lookup(chord)
            && existing != for_action
            && DANGEROUS_ACTIONS.iter().any(|d| existing.as_str() == *d)
        {
            self.warnings
                .push("warning: overriding a dangerous chord (was bound to app.quit)");
        }
    }

    fn apply_if_ready(&mut self) {
        if let CaptureState::Apply { for_action, chord } = self.capture.clone() {
            self.map.bind(KeyBinding {
                chord,
                action: for_action,
            });
            self.capture = std::mem::take(&mut self.capture).step(CaptureInput::Reset);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::action::{Action, ActionId, ActionRegistry};
    use sid_core::event::KeyChord;
    use sid_core::keybind::{KeyBinding, KeybindMap};
    use sid_core::keybind_capture::CaptureState;

    use super::*;

    fn small_registry() -> ActionRegistry {
        let mut r = ActionRegistry::new();
        r.register(Action::new("a", "Alpha"));
        r.register(Action::new("b", "Bravo"));
        r.register(Action::new("c", "Charlie"));
        r
    }

    #[test]
    fn empty_registry_focused_action_is_none() {
        let view = KeybindEditorView::new(&ActionRegistry::new(), KeybindMap::new());
        assert!(view.focused_action().is_none());
    }

    #[test]
    fn focused_starts_at_zero() {
        let view = KeybindEditorView::new(&small_registry(), KeybindMap::new());
        assert_eq!(view.focused_action().unwrap().as_str(), "a");
    }

    #[test]
    fn next_cycles() {
        let mut view = KeybindEditorView::new(&small_registry(), KeybindMap::new());
        view.next();
        view.next();
        view.next();
        assert_eq!(view.focused_action().unwrap().as_str(), "a");
    }

    #[test]
    fn prev_cycles() {
        let mut view = KeybindEditorView::new(&small_registry(), KeybindMap::new());
        view.prev();
        assert_eq!(view.focused_action().unwrap().as_str(), "c");
    }

    #[test]
    fn enter_capture_from_idle_transitions_to_waiting() {
        let mut view = KeybindEditorView::new(&small_registry(), KeybindMap::new());
        view.enter_capture();
        assert!(matches!(view.capture_state(), CaptureState::Waiting { .. }));
    }

    #[test]
    fn enter_capture_with_empty_registry_is_noop() {
        let mut view = KeybindEditorView::new(&ActionRegistry::new(), KeybindMap::new());
        view.enter_capture();
        assert_eq!(view.capture_state(), &CaptureState::Idle);
    }

    #[test]
    fn cancel_capture_returns_to_idle() {
        let mut view = KeybindEditorView::new(&small_registry(), KeybindMap::new());
        view.enter_capture();
        view.cancel_capture();
        assert_eq!(view.capture_state(), &CaptureState::Idle);
    }

    #[test]
    fn binding_for_quit_returns_ctrl_q() {
        let mut reg = ActionRegistry::new();
        reg.register(Action::new("app.quit", "Quit"));
        let view = KeybindEditorView::new(&reg, KeybindMap::cosmos_default());
        let chord = view.binding_for(&ActionId::new("app.quit")).unwrap();
        assert_eq!(chord.code, KeyCode::Char('q'));
        assert_eq!(chord.mods, KeyModifiers::CONTROL);
    }

    #[test]
    fn binding_for_unbound_returns_none() {
        let view = KeybindEditorView::new(&small_registry(), KeybindMap::new());
        assert!(view.binding_for(&ActionId::new("a")).is_none());
    }

    #[test]
    fn capture_unbound_chord_applies_immediately() {
        let mut view = KeybindEditorView::new(&small_registry(), KeybindMap::new());
        view.enter_capture();
        view.on_chord_captured(KeyChord::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(view.capture_state(), &CaptureState::Idle);
        assert_eq!(
            view.binding_for(&ActionId::new("a")).map(|c| c.code),
            Some(KeyCode::Char('x'))
        );
    }

    #[test]
    fn capture_conflicting_chord_enters_confirm_overwrite() {
        let mut map = KeybindMap::new();
        map.bind(KeyBinding {
            chord: KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            action: ActionId::new("b"),
        });
        let mut view = KeybindEditorView::new(&small_registry(), map);
        view.enter_capture();
        view.on_chord_captured(KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(matches!(
            view.capture_state(),
            CaptureState::ConfirmOverwrite { .. }
        ));
    }

    #[test]
    fn confirm_overwrite_yes_displaces_old_binding() {
        let mut map = KeybindMap::new();
        let chord = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        map.bind(KeyBinding {
            chord,
            action: ActionId::new("b"),
        });
        let mut view = KeybindEditorView::new(&small_registry(), map);
        view.enter_capture(); // focused = "a"
        view.on_chord_captured(chord);
        view.confirm_overwrite_yes();
        assert_eq!(view.capture_state(), &CaptureState::Idle);
        assert_eq!(view.binding_for(&ActionId::new("a")), Some(chord));
    }

    #[test]
    fn confirm_overwrite_no_returns_to_waiting() {
        let mut map = KeybindMap::new();
        let chord = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        map.bind(KeyBinding {
            chord,
            action: ActionId::new("b"),
        });
        let mut view = KeybindEditorView::new(&small_registry(), map);
        view.enter_capture();
        view.on_chord_captured(chord);
        view.confirm_overwrite_no();
        assert!(matches!(view.capture_state(), CaptureState::Waiting { .. }));
    }

    #[test]
    fn capture_same_chord_for_same_action_is_idempotent() {
        let chord = KeyChord::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
        let mut map = KeybindMap::new();
        map.bind(KeyBinding {
            chord,
            action: ActionId::new("a"),
        });
        let mut view = KeybindEditorView::new(&small_registry(), map);
        view.enter_capture(); // focused = "a"
        view.on_chord_captured(chord);
        assert_eq!(view.capture_state(), &CaptureState::Idle);
        // Map still has exactly one binding for action "a".
        assert_eq!(view.binding_for(&ActionId::new("a")), Some(chord));
        assert_eq!(view.map().iter().count(), 1);
    }

    #[test]
    fn rebind_quit_emits_dangerous_warning_and_still_applies() {
        let mut reg = ActionRegistry::new();
        reg.register(Action::new("app.quit", "Quit"));
        let mut view = KeybindEditorView::new(&reg, KeybindMap::cosmos_default());
        view.enter_capture();
        view.on_chord_captured(KeyChord::new(KeyCode::Char('Q'), KeyModifiers::CONTROL));
        // app.quit is dangerous so warning fired (was *being* rebound).
        // After Apply, capture has reset to Idle but warnings persist until
        // cancel/enter.
        assert!(!view.dangerous_action_warnings().is_empty());
        let new_chord = view.binding_for(&ActionId::new("app.quit")).unwrap();
        assert_eq!(new_chord.code, KeyCode::Char('Q'));
    }

    #[test]
    fn overriding_a_dangerous_chord_emits_warning_and_confirms() {
        let mut reg = ActionRegistry::new();
        reg.register(Action::new("app.quit", "Quit"));
        reg.register(Action::new("custom", "Custom"));
        let mut view = KeybindEditorView::new(&reg, KeybindMap::cosmos_default());
        // Focus "custom" then attempt to grab Ctrl+Q (already bound to app.quit).
        view.next();
        view.enter_capture();
        view.on_chord_captured(KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(matches!(
            view.capture_state(),
            CaptureState::ConfirmOverwrite { .. }
        ));
        assert!(!view.dangerous_action_warnings().is_empty());
        view.confirm_overwrite_yes();
        assert_eq!(view.capture_state(), &CaptureState::Idle);
        assert_eq!(
            view.binding_for(&ActionId::new("custom")).unwrap().code,
            KeyCode::Char('q')
        );
        // app.quit no longer holds Ctrl+Q.
        assert_eq!(view.binding_for(&ActionId::new("app.quit")), None);
    }

    #[test]
    fn empty_warnings_after_cancel() {
        let mut reg = ActionRegistry::new();
        reg.register(Action::new("app.quit", "Quit"));
        let mut view = KeybindEditorView::new(&reg, KeybindMap::cosmos_default());
        view.enter_capture();
        view.on_chord_captured(KeyChord::new(KeyCode::Char('Q'), KeyModifiers::CONTROL));
        view.cancel_capture();
        assert!(view.dangerous_action_warnings().is_empty());
    }

    #[test]
    fn large_registry_next_cycles_cleanly() {
        let mut reg = ActionRegistry::new();
        for i in 0..1000 {
            reg.register(Action::new(format!("act.{i:04}"), format!("Action {i}")));
        }
        let mut view = KeybindEditorView::new(&reg, KeybindMap::new());
        for _ in 0..1000 {
            view.next();
        }
        assert_eq!(view.focused_action().unwrap().as_str(), "act.0000");
    }

    #[test]
    fn binding_for_finds_an_existing_chord_when_action_has_multiple() {
        // KeybindMap allows multiple chords → same action. `binding_for`
        // returns *some* chord that maps to the action; here we check that
        // after adding a second chord for action "a", at least one of the
        // two chords is still discoverable.
        let chord_a = KeyChord::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let mut map = KeybindMap::new();
        map.bind(KeyBinding {
            chord: chord_a,
            action: ActionId::new("a"),
        });
        let mut view = KeybindEditorView::new(&small_registry(), map);
        view.enter_capture();
        let chord_b = KeyChord::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        view.on_chord_captured(chord_b);
        let found = view.binding_for(&ActionId::new("a")).unwrap();
        assert!(
            found == chord_a || found == chord_b,
            "binding_for returned unexpected chord {found:?}"
        );
        // Both chord→action mappings exist in the map.
        let lookup_a = view.map().lookup(&chord_a).map(|a| a.as_str());
        let lookup_b = view.map().lookup(&chord_b).map(|a| a.as_str());
        assert_eq!(lookup_a, Some("a"));
        assert_eq!(lookup_b, Some("a"));
    }
}
