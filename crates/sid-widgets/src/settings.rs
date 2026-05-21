//! Settings tab widget.
//!
//! Composes the six Plan-7 sub-views (theme picker, keybind editor, behavior
//! toggles, workspace roots, quick actions, DB path) plus the reset modal into
//! a single [`Widget`] with a left/right pane layout.
//!
//! The composer carries each sub-view as a [`SettingsCategory`] variant and
//! tracks which category is focused. `Tab` / `Shift+Tab` cycle the focused
//! category; the focused category receives all other key events.
//!
//! State is persisted via [`Widget::save_state`] / [`Widget::load_state`] —
//! only the focused-category index — which keeps the user on the same
//! category across launches when the surrounding state machinery restores
//! widget state.

pub mod behavior_toggles;
pub mod db_path;
pub mod keybind_editor;
pub mod live_preview;
pub mod quick_actions;
pub mod reset;
pub mod theme_picker;
pub mod workspace_roots;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use serde::{Deserialize, Serialize};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_ui::Theme;

use crate::settings::behavior_toggles::BehaviorTogglesView;
use crate::settings::db_path::DbPathView;
use crate::settings::keybind_editor::KeybindEditorView;
use crate::settings::quick_actions::QuickActionsView;
use crate::settings::reset::ResetView;
use crate::settings::theme_picker::{ThemePickerOutcome, ThemePickerView};
use crate::settings::workspace_roots::WorkspaceRootsView;
use crate::stub::ComingSoonBody;

/// One sub-view in the Settings tab.
pub enum SettingsCategory {
    /// Theme picker + live preview.
    Theme(ThemePickerView),
    /// Keybind editor with capture mode.
    Keybinds(KeybindEditorView),
    /// Behavior toggles (bool/choice/u64).
    Behavior(BehaviorTogglesView),
    /// Workspace roots editor.
    WorkspaceRoots(WorkspaceRootsView),
    /// Quick actions editor.
    QuickActions(QuickActionsView),
    /// DB path override.
    DbPath(DbPathView),
    /// Reset to defaults modal.
    Reset(ResetView),
}

impl SettingsCategory {
    /// Stable label used in the left-pane category list.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Theme(_) => "Theme",
            Self::Keybinds(_) => "Keybinds",
            Self::Behavior(_) => "Behavior",
            Self::WorkspaceRoots(_) => "Workspace roots",
            Self::QuickActions(_) => "Quick actions",
            Self::DbPath(_) => "DB path",
            Self::Reset(_) => "Reset to defaults",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SettingsState {
    focused_category: u8,
}

/// Tab widget for the Settings tab.
///
/// The widget can be built in two flavours:
///
/// * [`SettingsWidget::new`] — a zero-argument constructor used by the binary
///   wire path. Renders a "coming soon" body and no categories. Once Task 24
///   lands, the binary swaps this for the rich constructor.
/// * [`SettingsWidget::with_categories`] — accepts a populated list of
///   [`SettingsCategory`] variants. Used by Plan-7 tests and by the binary
///   once it is rewired.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::SettingsWidget;
///
/// let w = SettingsWidget::new();
/// assert_eq!(w.id().as_str(), "settings.root");
/// assert_eq!(w.title(), "Settings");
/// ```
pub struct SettingsWidget {
    id: WidgetId,
    body: Option<ComingSoonBody>,
    categories: Vec<SettingsCategory>,
    focused_category: usize,
}

impl SettingsWidget {
    /// Create a zero-categories `SettingsWidget` that renders the legacy
    /// "coming soon" body. Used by the current binary wire path.
    pub fn new() -> Self {
        Self {
            id: WidgetId::new("settings.root"),
            body: Some(ComingSoonBody::new(
                "Settings",
                "theme picker, keybind editor, behavior toggles — coming in Plan 7",
            )),
            categories: Vec::new(),
            focused_category: 0,
        }
    }

    /// Create a widget pre-populated with `categories`. The first category is
    /// focused by default.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    /// use sid_widgets::settings::reset::ResetView;
    /// use sid_widgets::{SettingsCategory, SettingsWidget};
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let w = SettingsWidget::with_categories(vec![
    ///     SettingsCategory::Theme(ThemePickerView::new(&r, "cosmos")),
    ///     SettingsCategory::Reset(ResetView::new()),
    /// ]);
    /// assert_eq!(w.category_labels(), vec!["Theme", "Reset to defaults"]);
    /// assert_eq!(w.focused_category_index(), 0);
    /// ```
    pub fn with_categories(categories: Vec<SettingsCategory>) -> Self {
        Self {
            id: WidgetId::new("settings.root"),
            body: None,
            categories,
            focused_category: 0,
        }
    }

    /// Ordered list of category labels (`["Theme", "Keybinds", ...]`).
    pub fn category_labels(&self) -> Vec<&'static str> {
        self.categories.iter().map(|c| c.label()).collect()
    }

    /// Focused category index. Always `0` when no categories are present.
    pub fn focused_category_index(&self) -> usize {
        self.focused_category
    }

    /// Borrow the focused category, if any.
    pub fn focused_category(&self) -> Option<&SettingsCategory> {
        self.categories.get(self.focused_category)
    }

    /// Mutably borrow the focused category, if any.
    pub fn focused_category_mut(&mut self) -> Option<&mut SettingsCategory> {
        self.categories.get_mut(self.focused_category)
    }

    /// Advance focus by one (wraps).
    pub fn focus_next(&mut self) {
        if !self.categories.is_empty() {
            self.focused_category = (self.focused_category + 1) % self.categories.len();
        }
    }

    /// Reverse focus by one (wraps).
    pub fn focus_prev(&mut self) {
        if !self.categories.is_empty() {
            self.focused_category = if self.focused_category == 0 {
                self.categories.len() - 1
            } else {
                self.focused_category - 1
            };
        }
    }

    /// Set the focused category, returning `true` on success. No-op if `idx`
    /// is out of range.
    pub fn focus_set(&mut self, idx: usize) -> bool {
        if idx < self.categories.len() {
            self.focused_category = idx;
            true
        } else {
            false
        }
    }

    /// Render the Settings tab into `area` using `theme` as the chrome theme.
    ///
    /// Layout: a 25%/75% horizontal split. The left pane lists the category
    /// labels with the focused row highlighted; the right pane delegates to
    /// the focused sub-view's own `render_into_frame`. When the widget has no
    /// categories the area is rendered as an empty bordered block.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::Terminal;
    /// use ratatui::backend::TestBackend;
    /// use sid_ui::themes::cosmos;
    /// use sid_widgets::SettingsWidget;
    ///
    /// let w = SettingsWidget::with_categories(vec![]);
    /// let backend = TestBackend::new(80, 24);
    /// let mut term = Terminal::new(backend).unwrap();
    /// let theme = cosmos();
    /// term.draw(|f| w.render_into_frame(f, f.area(), &theme)).unwrap();
    /// ```
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        if self.categories.is_empty() {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border.into()))
                .title(" Settings ")
                .title_style(Style::default().fg(theme.foreground.into()));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            if inner.width > 0 && inner.height > 0 {
                let body = Paragraph::new(Line::from("(no categories)"))
                    .style(Style::default().fg(theme.muted.into()));
                frame.render_widget(body, inner);
            }
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
            .split(area);
        let left = chunks[0];
        let right = chunks[1];

        // Left pane: category list.
        let left_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border.into()))
            .title(" Categories ")
            .title_style(Style::default().fg(theme.foreground.into()));
        let left_inner = left_block.inner(left);
        frame.render_widget(left_block, left);
        if left_inner.width > 0 && left_inner.height > 0 {
            let mut rows: Vec<Line> = Vec::with_capacity(self.categories.len());
            for (i, cat) in self.categories.iter().enumerate() {
                let cursor = if i == self.focused_category { '>' } else { ' ' };
                let marker = if i == self.focused_category { '*' } else { 'o' };
                let line = Line::from(format!("{cursor} {marker} {}", cat.label()));
                let line = if i == self.focused_category {
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
            frame.render_widget(Paragraph::new(rows), left_inner);
        }

        // Right pane: focused sub-view.
        match self.focused_category() {
            Some(SettingsCategory::Theme(v)) => v.render_into_frame(frame, right, theme),
            Some(SettingsCategory::Keybinds(v)) => v.render_into_frame(frame, right, theme),
            Some(SettingsCategory::Behavior(v)) => v.render_into_frame(frame, right, theme),
            Some(SettingsCategory::WorkspaceRoots(v)) => v.render_into_frame(frame, right, theme),
            Some(SettingsCategory::QuickActions(v)) => v.render_into_frame(frame, right, theme),
            Some(SettingsCategory::DbPath(v)) => v.render_into_frame(frame, right, theme),
            Some(SettingsCategory::Reset(v)) => v.render_into_frame(frame, right, theme),
            None => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border.into()));
                frame.render_widget(block, right);
            }
        }
    }
}

/// Render the [`SettingsWidget`] into a fresh `(width, height)` test buffer
/// using the cosmos theme. Mirrors `network::render_to_string`.
///
/// # Examples
///
/// ```
/// use sid_widgets::SettingsWidget;
/// use sid_widgets::settings::render_to_string;
///
/// let w = SettingsWidget::with_categories(vec![]);
/// let s = render_to_string(&w, 80, 12);
/// assert!(s.contains("Settings"));
/// ```
pub fn render_to_string(widget: &SettingsWidget, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use sid_ui::themes::cosmos;
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    term.draw(|f| widget.render_into_frame(f, f.area(), &theme))
        .unwrap();
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        s.push('\n');
    }
    s
}

impl Default for SettingsWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for SettingsWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }

    fn title(&self) -> &str {
        "Settings"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn render(&self, target: &mut dyn RenderTarget) {
        // RenderTarget is intentionally minimal in Plan 1; the actual draw
        // happens via ratatui-aware paths in the binary. Keep this method as
        // a no-op (matching the other widgets) so the Widget trait is honoured.
        let _ = target;
    }

    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        if let Some(b) = &mut self.body {
            return b.handle_event(ev, ctx);
        }
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(k) = ev {
            match k.code {
                KeyCode::Tab if k.mods.contains(KeyModifiers::SHIFT) => {
                    self.focus_prev();
                    return EventOutcome::Consumed;
                }
                KeyCode::Tab => {
                    self.focus_next();
                    return EventOutcome::Consumed;
                }
                KeyCode::BackTab => {
                    self.focus_prev();
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
            // Route to the focused category's local handler.
            if let Some(cat) = self.focused_category_mut() {
                match cat {
                    SettingsCategory::Theme(v) => match v.handle_event(ev) {
                        ThemePickerOutcome::None => {}
                        _ => return EventOutcome::Consumed,
                    },
                    SettingsCategory::Keybinds(_)
                    | SettingsCategory::Behavior(_)
                    | SettingsCategory::WorkspaceRoots(_)
                    | SettingsCategory::QuickActions(_)
                    | SettingsCategory::DbPath(_)
                    | SettingsCategory::Reset(_) => {
                        // Per-category event routing is implemented by the
                        // binary wire path (which owns the relevant Store /
                        // ActionRegistry handles). Sub-views expose typed
                        // mutators; routing crossterm events to them is the
                        // binary's job, not the composer's. Returning
                        // `EventOutcome::Bubble` keeps the dispatch loop
                        // free to fall through to higher-level handlers.
                    }
                }
            }
        }
        EventOutcome::Bubble
    }

    fn save_state(&self) -> Vec<u8> {
        if self.categories.is_empty() {
            return Vec::new();
        }
        let state = SettingsState {
            focused_category: self.focused_category as u8,
        };
        postcard::to_allocvec(&state).unwrap_or_default()
    }

    fn load_state(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Ok(s) = postcard::from_bytes::<SettingsState>(bytes)
            && (s.focused_category as usize) < self.categories.len()
        {
            self.focused_category = s.focused_category as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use sid_core::widget::Widget;
    use sid_ui::theme_registry::ThemeRegistry;

    use super::*;

    fn small_widget() -> SettingsWidget {
        let r = ThemeRegistry::with_builtins();
        SettingsWidget::with_categories(vec![
            SettingsCategory::Theme(ThemePickerView::new(&r, "cosmos")),
            SettingsCategory::Behavior(BehaviorTogglesView::defaults()),
            SettingsCategory::Reset(ResetView::new()),
        ])
    }

    #[test]
    fn legacy_id_and_title_correct() {
        let w = SettingsWidget::new();
        assert_eq!(w.id().as_str(), "settings.root");
        assert_eq!(w.title(), "Settings");
    }

    #[test]
    fn legacy_save_state_is_empty() {
        let w = SettingsWidget::new();
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn legacy_load_state_is_noop() {
        let mut w = SettingsWidget::new();
        w.load_state(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00]);
        assert_eq!(w.id().as_str(), "settings.root");
    }

    #[test]
    fn with_categories_returns_labels() {
        let w = small_widget();
        assert_eq!(
            w.category_labels(),
            vec!["Theme", "Behavior", "Reset to defaults"]
        );
        assert_eq!(w.focused_category_index(), 0);
    }

    #[test]
    fn focus_next_cycles() {
        let mut w = small_widget();
        w.focus_next();
        assert_eq!(w.focused_category_index(), 1);
        w.focus_next();
        w.focus_next();
        assert_eq!(w.focused_category_index(), 0);
    }

    #[test]
    fn focus_prev_cycles_from_zero_to_last() {
        let mut w = small_widget();
        w.focus_prev();
        assert_eq!(w.focused_category_index(), 2);
    }

    #[test]
    fn focus_set_in_bounds_succeeds() {
        let mut w = small_widget();
        assert!(w.focus_set(2));
        assert_eq!(w.focused_category_index(), 2);
    }

    #[test]
    fn focus_set_out_of_bounds_is_noop() {
        let mut w = small_widget();
        assert!(!w.focus_set(99));
        assert_eq!(w.focused_category_index(), 0);
    }

    #[test]
    fn save_load_state_round_trips() {
        let mut w = small_widget();
        w.focus_next();
        w.focus_next(); // idx = 2
        let bytes = w.save_state();
        let mut w2 = small_widget();
        w2.load_state(&bytes);
        assert_eq!(w2.focused_category_index(), 2);
    }

    #[test]
    fn load_state_with_oob_index_is_clamped_to_zero() {
        let mut w = small_widget();
        // Manually craft a SettingsState with an out-of-range value.
        let bytes = postcard::to_allocvec(&SettingsState {
            focused_category: 250,
        })
        .unwrap();
        w.load_state(&bytes);
        assert_eq!(w.focused_category_index(), 0);
    }

    #[test]
    fn garbage_load_state_is_ignored() {
        let mut w = small_widget();
        w.focus_next();
        let before = w.focused_category_index();
        w.load_state(&[0xFF, 0xFE, 0xFD]);
        assert_eq!(w.focused_category_index(), before);
    }

    #[test]
    fn empty_widget_save_state_is_empty() {
        let w = SettingsWidget::with_categories(vec![]);
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn tab_event_cycles_forward() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = small_widget();
        let ev = Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::empty()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let outcome = w.handle_event(&ev, &mut ctx);
        assert_eq!(outcome, EventOutcome::Consumed);
        assert_eq!(w.focused_category_index(), 1);
    }

    #[test]
    fn shift_tab_event_cycles_backward() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = small_widget();
        let ev = Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::SHIFT));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let outcome = w.handle_event(&ev, &mut ctx);
        assert_eq!(outcome, EventOutcome::Consumed);
        assert_eq!(w.focused_category_index(), 2);
    }

    #[test]
    fn back_tab_event_cycles_backward() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = small_widget();
        let ev = Event::Key(KeyChord::new(KeyCode::BackTab, KeyModifiers::empty()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        w.handle_event(&ev, &mut ctx);
        assert_eq!(w.focused_category_index(), 2);
    }

    #[test]
    fn arrow_event_routes_to_focused_theme_picker() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = small_widget();
        let ev = Event::Key(KeyChord::new(KeyCode::Down, KeyModifiers::empty()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let outcome = w.handle_event(&ev, &mut ctx);
        // Theme picker consumed it (PreviewChanged).
        assert_eq!(outcome, EventOutcome::Consumed);
        if let Some(SettingsCategory::Theme(v)) = w.focused_category() {
            assert!(v.focused_index() > 0);
        } else {
            panic!("expected Theme category");
        }
    }

    proptest! {
        #[test]
        fn prop_focus_index_in_bounds(steps in 0usize..256) {
            let mut w = small_widget();
            for i in 0..steps {
                if i % 2 == 0 { w.focus_next() } else { w.focus_prev() }
                prop_assert!(w.focused_category_index() < w.categories.len());
            }
        }

        #[test]
        fn prop_save_load_round_trip(idx in 0u8..3) {
            let mut w = small_widget();
            w.focused_category = idx as usize;
            let bytes = w.save_state();
            let mut w2 = small_widget();
            w2.load_state(&bytes);
            prop_assert_eq!(w2.focused_category_index(), idx as usize);
        }
    }
}
