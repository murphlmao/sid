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

pub mod animation;
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
use sid_core::widget::{EventOutcome, FooterHint, RenderTarget, Widget, WidgetId};
use sid_ui::Theme;

use crate::settings::animation::AnimationView;
use crate::settings::behavior_toggles::BehaviorTogglesView;
use crate::settings::db_path::DbPathView;
use crate::settings::keybind_editor::KeybindEditorView;
use crate::settings::quick_actions::QuickActionsView;
use crate::settings::reset::ResetView;
use crate::settings::theme_picker::{ThemePickerOutcome, ThemePickerView};
use crate::settings::workspace_roots::WorkspaceRootsView;
use crate::stub::ComingSoonBody;

/// Encode a [`BehaviorTogglesOutcome::Toggled`] payload as a
/// query-string-style key/value blob for [`emit_action_with_payload`].
/// The wire layer's settings-outcome dispatch parses this back.
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::behavior_toggles::ToggleValue;
/// use sid_widgets::settings::encode_behavior_payload;
///
/// let s = encode_behavior_payload("auto_restore_session", &ToggleValue::Bool(true));
/// assert_eq!(s, "key=auto_restore_session&kind=bool&value=true");
/// ```
pub fn encode_behavior_payload(
    key: &str,
    value: &crate::settings::behavior_toggles::ToggleValue,
) -> String {
    use crate::settings::behavior_toggles::ToggleValue;
    match value {
        ToggleValue::Bool(b) => format!("key={key}&kind=bool&value={b}"),
        ToggleValue::Choice { options, selected } => {
            let picked = options.get(*selected).cloned().unwrap_or_default();
            format!("key={key}&kind=choice&value={picked}")
        }
        ToggleValue::U64 { value, .. } => format!("key={key}&kind=u64&value={value}"),
        ToggleValue::String(s) => format!("key={key}&kind=string&value={s}"),
    }
}

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
    /// Cosmic background animation tuner (density, FPS, supernova rate, glyphs).
    Animation(AnimationView),
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
            Self::Animation(_) => "Animation",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SettingsState {
    focused_category: u8,
}

/// Which pane in the Settings tab currently owns keyboard input.
///
/// `Tab` (and `Shift+Tab` for symmetry) toggles between the two. The accent
/// border is rendered on the focused pane; the other uses the muted color.
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::SettingsFocus;
/// assert_eq!(SettingsFocus::default(), SettingsFocus::Categories);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsFocus {
    /// The left-hand category list.
    #[default]
    Categories,
    /// The right-hand active sub-view.
    SubView,
}

impl SettingsFocus {
    /// Toggle to the other focus.
    pub fn toggle(self) -> Self {
        match self {
            SettingsFocus::Categories => SettingsFocus::SubView,
            SettingsFocus::SubView => SettingsFocus::Categories,
        }
    }
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
/// A pending settings outcome the widget signalled. The wire layer drains
/// these via [`SettingsWidget::take_pending_outcomes`] each event-loop pass
/// and dispatches them to the right [`sid_store::Store`] put_* method.
///
/// Carried out-of-band (in addition to the [`WidgetCtx::emit_action_with_payload`]
/// emit) so the binary can act without having to listen on the action
/// channel that `App::handle_event` owns.
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::PendingSettingsOutcome;
/// use sid_widgets::settings::behavior_toggles::ToggleValue;
///
/// let o = PendingSettingsOutcome::BehaviorToggled {
///     key: "auto_restore_session",
///     value: ToggleValue::Bool(true),
/// };
/// assert!(matches!(o, PendingSettingsOutcome::BehaviorToggled { .. }));
/// ```
#[derive(Clone, Debug)]
pub enum PendingSettingsOutcome {
    /// User cycled a Behavior toggle; wire should `put_*` the value.
    BehaviorToggled {
        /// Canonical setting key (see [`sid_store::settings_keys`]).
        key: &'static str,
        /// New value as held by the view.
        value: crate::settings::behavior_toggles::ToggleValue,
    },
    /// User added or removed a workspace root; wire should persist via
    /// `Store::put_setting(WORKSPACE_ROOTS, …)`.
    WorkspaceRootsChanged(Vec<std::path::PathBuf>),
    /// User added/edited a quick action; wire should call `upsert_quick_action`.
    QuickActionUpserted(sid_store::QuickAction),
    /// User deleted a quick action; wire should call `remove_quick_action`.
    QuickActionRemoved(String),
    /// User successfully rebound a key; wire should call
    /// `keybind_load::save_keybind_profile`.
    KeybindApplied {
        profile_name: String,
        map_snapshot: sid_core::keybind::KeybindMap,
    },
    /// DB path override written to `sid.toml`; wire emits a "restart required" toast.
    DbPathOverrideWritten(crate::settings::db_path::RestartNotice),
    /// Factory reset confirmed; wire calls `ResetView::confirm(&store)`.
    FactoryResetConfirmed,
    /// User applied a theme from the theme picker. Wire should call
    /// `put_string(THEME_NAME, name)`.
    ThemeApplied {
        /// Name of the selected theme.
        name: String,
    },
}

pub struct SettingsWidget {
    id: WidgetId,
    body: Option<ComingSoonBody>,
    categories: Vec<SettingsCategory>,
    focused_category: usize,
    focused_pane: SettingsFocus,
    /// Outcomes that fired since the last drain. The binary's wire layer
    /// drains via [`Self::take_pending_outcomes`] each event-loop pass.
    pending_outcomes: Vec<PendingSettingsOutcome>,
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
            focused_pane: SettingsFocus::default(),
            pending_outcomes: Vec::new(),
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
            focused_pane: SettingsFocus::default(),
            pending_outcomes: Vec::new(),
        }
    }

    /// Currently-focused pane (Categories or SubView).
    pub fn focused_pane(&self) -> SettingsFocus {
        self.focused_pane
    }

    /// Drain the pending outcomes queue. Returns every outcome that
    /// fired since the last call. The wire layer calls this after every
    /// `app.handle_event` pass and dispatches each to the right
    /// `Store::put_*`.
    pub fn take_pending_outcomes(&mut self) -> Vec<PendingSettingsOutcome> {
        std::mem::take(&mut self.pending_outcomes)
    }

    /// Stable string label for the focused pane.
    pub fn focused_pane_label(&self) -> &'static str {
        match self.focused_pane {
            SettingsFocus::Categories => "Categories",
            SettingsFocus::SubView => "SubView",
        }
    }

    /// Toggle the focused pane (2-way flip — same as Tab/Shift+Tab).
    pub fn toggle_focused_pane(&mut self) {
        self.focused_pane = self.focused_pane.toggle();
    }

    /// Focus the pane that contains the given coordinate. No-op when the
    /// coordinate falls outside `area`.
    ///
    /// Layout mirrors [`Self::render_into_frame`]: a 25/75 horizontal split.
    /// Columns left of the 25% boundary focus [`SettingsFocus::Categories`];
    /// everything else focuses [`SettingsFocus::SubView`]. No-op when the
    /// widget has no categories (the entire area renders as an empty block).
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::Rect;
    /// use sid_widgets::settings::SettingsFocus;
    /// use sid_widgets::SettingsWidget;
    /// use sid_ui::theme_registry::ThemeRegistry;
    /// use sid_widgets::settings::theme_picker::ThemePickerView;
    /// use sid_widgets::settings::reset::ResetView;
    /// use sid_widgets::SettingsCategory;
    ///
    /// let r = ThemeRegistry::with_builtins();
    /// let mut w = SettingsWidget::with_categories(vec![
    ///     SettingsCategory::Theme(ThemePickerView::new(&r, "cosmos")),
    ///     SettingsCategory::Reset(ResetView::new()),
    /// ]);
    /// let area = Rect { x: 0, y: 0, width: 100, height: 24 };
    /// w.focus_at(area, 80, 5);
    /// assert_eq!(w.focused_pane(), SettingsFocus::SubView);
    /// w.focus_at(area, 5, 5);
    /// assert_eq!(w.focused_pane(), SettingsFocus::Categories);
    /// ```
    pub fn focus_at(&mut self, area: Rect, col: u16, row: u16) {
        if self.categories.is_empty() {
            return;
        }
        if area.width == 0 || area.height == 0 {
            return;
        }
        if col < area.x || col >= area.x.saturating_add(area.width) {
            return;
        }
        if row < area.y || row >= area.y.saturating_add(area.height) {
            return;
        }
        let split_col = area.x.saturating_add(area.width.saturating_mul(25) / 100);
        self.focused_pane = if col < split_col {
            SettingsFocus::Categories
        } else {
            SettingsFocus::SubView
        };
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
    /// the focused sub-view's own `render_into_frame`, passing `subview_focused`
    /// so the sub-view paints its own accent-or-muted border. When the widget
    /// has no categories the area is rendered as an empty bordered block.
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
                .border_style(Style::default().fg(theme.muted.into()))
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

        // Left pane: category list. Accent border on the focused pane.
        let categories_focused = self.focused_pane == SettingsFocus::Categories;
        let left_border = if categories_focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut left_title_style = Style::default().fg(theme.foreground.into());
        if categories_focused {
            left_title_style = left_title_style.add_modifier(Modifier::BOLD);
        }
        let left_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(left_border.into()))
            .title(" Categories ")
            .title_style(left_title_style);
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

        // Right pane: focused sub-view. The sub-view owns its border and
        // consults `subview_focused` to pick accent vs muted color and the
        // bold title modifier.
        let subview_focused = self.focused_pane == SettingsFocus::SubView;
        match self.focused_category() {
            Some(SettingsCategory::Theme(v)) => {
                v.render_into_frame(frame, right, theme, subview_focused)
            }
            Some(SettingsCategory::Keybinds(v)) => {
                v.render_into_frame(frame, right, theme, subview_focused)
            }
            Some(SettingsCategory::Behavior(v)) => {
                v.render_into_frame(frame, right, theme, subview_focused)
            }
            Some(SettingsCategory::WorkspaceRoots(v)) => {
                v.render_into_frame(frame, right, theme, subview_focused)
            }
            Some(SettingsCategory::QuickActions(v)) => {
                v.render_into_frame(frame, right, theme, subview_focused)
            }
            Some(SettingsCategory::DbPath(v)) => {
                v.render_into_frame(frame, right, theme, subview_focused)
            }
            Some(SettingsCategory::Reset(v)) => {
                v.render_into_frame(frame, right, theme, subview_focused)
            }
            Some(SettingsCategory::Animation(v)) => {
                v.render_into_frame(frame, right, theme, subview_focused)
            }
            None => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.muted.into()));
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

/// Render the widget into a fixed test buffer and emit BOTH the symbol grid
/// AND a parallel style-marker grid that captures fg color + bold per cell.
/// Used by composer-level snapshots that need to demonstrate the focused-vs-
/// unfocused border-and-title difference (which [`render_to_string`] doesn't
/// see because it only writes `cell.symbol()`).
///
/// Style markers are single characters chosen for readability in snapshot
/// diffs:
/// - ` ` — default / background
/// - `m` — muted (theme.muted; unfocused borders, subtle text)
/// - `a` — accent_primary (theme.accent_primary; focused borders)
/// - `f` — foreground (theme.foreground; ordinary text)
/// - `o` — other tracked color (everything else with a non-default fg)
/// - `B` — bold modifier overlaid on the above (uppercase indicates bold)
///
/// The output is two blocks separated by a blank line and a `STYLES:` header.
///
/// # Examples
///
/// ```
/// use sid_widgets::SettingsWidget;
/// use sid_widgets::settings::render_to_string_with_styles;
///
/// let w = SettingsWidget::with_categories(vec![]);
/// let s = render_to_string_with_styles(&w, 40, 6);
/// assert!(s.contains("STYLES:"));
/// ```
pub fn render_to_string_with_styles(widget: &SettingsWidget, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};
    use sid_ui::themes::cosmos;
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    term.draw(|f| widget.render_into_frame(f, f.area(), &theme))
        .unwrap();
    let buf = term.backend().buffer();

    // Symbol grid first (unchanged from render_to_string).
    let mut sym = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            sym.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        sym.push('\n');
    }

    // Style grid, one char per cell.
    let muted: Color = theme.muted.into();
    let accent: Color = theme.accent_primary.into();
    let fg: Color = theme.foreground.into();
    let mut styles = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = match buf.cell((x, y)) {
                Some(c) => c,
                None => {
                    styles.push(' ');
                    continue;
                }
            };
            let bold = cell.modifier.contains(Modifier::BOLD);
            let base = match cell.fg {
                c if c == muted => 'm',
                c if c == accent => 'a',
                c if c == fg => 'f',
                Color::Reset => ' ',
                _ => 'o',
            };
            // Bold uppercase, plain lowercase, blank stays blank.
            styles.push(if bold && base != ' ' {
                base.to_ascii_uppercase()
            } else {
                base
            });
        }
        styles.push('\n');
    }

    format!("{sym}\nSTYLES:\n{styles}")
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

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("Tab", "next category"),
            FooterHint::new("Enter", "apply"),
            FooterHint::new("N", "import"),
            FooterHint::new("?", "help"),
        ]
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
            // Tab / Shift+Tab / BackTab all TOGGLE the focused pane
            // (Categories ↔ SubView). This is the user's specific
            // complaint: previously Tab moved between category rows;
            // now Tab flips which pane receives j/k.
            match k.code {
                KeyCode::Tab | KeyCode::BackTab => {
                    self.toggle_focused_pane();
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
            // Alt+<key> is reserved for future cross-pane actions.
            if k.mods.contains(KeyModifiers::ALT) {
                // TODO: cross-pane actions on Alt+<key>
                return EventOutcome::Bubble;
            }
            // Pane-gated routing.
            match self.focused_pane {
                SettingsFocus::Categories => match (k.code, k.mods) {
                    (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                        self.focus_next();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                        self.focus_prev();
                        return EventOutcome::Consumed;
                    }
                    _ => {}
                },
                SettingsFocus::SubView => {
                    // Route to the focused category's local handler.
                    if let Some(cat) = self.focused_category_mut() {
                        match cat {
                            SettingsCategory::Theme(v) => match v.handle_event(ev) {
                                ThemePickerOutcome::None => {}
                                ThemePickerOutcome::PreviewChanged => {
                                    return EventOutcome::Consumed;
                                }
                                ThemePickerOutcome::Applied { name } => {
                                    ctx.emit_action_with_payload(
                                        "settings.outcome.theme_applied",
                                        &name,
                                    );
                                    self.pending_outcomes
                                        .push(PendingSettingsOutcome::ThemeApplied { name });
                                    return EventOutcome::Consumed;
                                }
                            },
                            SettingsCategory::Animation(v) => {
                                // The AnimationView owns its own event
                                // routing (j/k, h/l, arrows, Space/Enter,
                                // and `S` to flush via its embedded store).
                                match v.handle_event(ev, ctx) {
                                    EventOutcome::Consumed => {
                                        return EventOutcome::Consumed;
                                    }
                                    EventOutcome::Bubble => {}
                                }
                            }
                            SettingsCategory::Behavior(v) => {
                                use crate::settings::behavior_toggles::BehaviorTogglesOutcome;
                                match v.handle_event(ev) {
                                    BehaviorTogglesOutcome::None => {}
                                    BehaviorTogglesOutcome::Toggled { key, value } => {
                                        // Two-channel signal:
                                        // 1) emit_action_with_payload — for any
                                        //    listener / telemetry on the action
                                        //    bus.
                                        // 2) pending_outcomes — for the binary
                                        //    wire layer to drain and dispatch
                                        //    to Store::put_*.
                                        let payload = encode_behavior_payload(key, &value);
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.behavior_toggle",
                                            &payload,
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::BehaviorToggled { key, value },
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
                            SettingsCategory::WorkspaceRoots(v) => {
                                use crate::settings::workspace_roots::WorkspaceRootsOutcome;
                                match v.handle_event(ev) {
                                    WorkspaceRootsOutcome::None => {}
                                    WorkspaceRootsOutcome::RootsChanged(roots) => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.workspace_roots",
                                            roots
                                                .iter()
                                                .map(|p| p.display().to_string())
                                                .collect::<Vec<_>>()
                                                .join(":"),
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::WorkspaceRootsChanged(roots),
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
                            SettingsCategory::QuickActions(v) => {
                                use crate::settings::quick_actions::QuickActionsOutcome;
                                match v.handle_event(ev) {
                                    QuickActionsOutcome::None => {}
                                    QuickActionsOutcome::Upserted(qa) => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.quick_action_upserted",
                                            &qa.id,
                                        );
                                        self.pending_outcomes
                                            .push(PendingSettingsOutcome::QuickActionUpserted(qa));
                                        return EventOutcome::Consumed;
                                    }
                                    QuickActionsOutcome::Removed(id) => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.quick_action_removed",
                                            &id,
                                        );
                                        self.pending_outcomes
                                            .push(PendingSettingsOutcome::QuickActionRemoved(id));
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
                            SettingsCategory::Keybinds(v) => {
                                use crate::settings::keybind_editor::KeybindEditorOutcome;
                                match v.handle_event(ev) {
                                    KeybindEditorOutcome::None => {}
                                    KeybindEditorOutcome::Applied {
                                        action,
                                        chord,
                                        profile_name,
                                        map_snapshot,
                                    } => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.keybind_applied",
                                            format!("{}={:?}", action.as_str(), chord),
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::KeybindApplied {
                                                profile_name,
                                                map_snapshot,
                                            },
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
                            SettingsCategory::DbPath(v) => {
                                use crate::settings::db_path::DbPathOutcome;
                                match v.handle_event(ev) {
                                    DbPathOutcome::None => {}
                                    DbPathOutcome::Written(notice) => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.db_path_written",
                                            notice.sid_toml_path.display().to_string(),
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::DbPathOverrideWritten(notice),
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
                            SettingsCategory::Reset(v) => {
                                use crate::settings::reset::ResetOutcome;
                                match v.handle_event(ev) {
                                    ResetOutcome::None => {}
                                    ResetOutcome::Confirmed => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.factory_reset",
                                            "",
                                        );
                                        self.pending_outcomes
                                            .push(PendingSettingsOutcome::FactoryResetConfirmed);
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
                        }
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
    fn tab_event_flips_focused_pane() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = small_widget();
        assert_eq!(w.focused_pane(), SettingsFocus::Categories);
        assert_eq!(w.focused_category_index(), 0);
        let ev = Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::empty()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let outcome = w.handle_event(&ev, &mut ctx);
        assert_eq!(outcome, EventOutcome::Consumed);
        // Tab flips the focused pane — does NOT move category index.
        assert_eq!(w.focused_pane(), SettingsFocus::SubView);
        assert_eq!(w.focused_category_index(), 0);
    }

    #[test]
    fn shift_tab_event_also_flips_focused_pane() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = small_widget();
        let ev = Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::SHIFT));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let outcome = w.handle_event(&ev, &mut ctx);
        assert_eq!(outcome, EventOutcome::Consumed);
        assert_eq!(w.focused_pane(), SettingsFocus::SubView);
        assert_eq!(w.focused_category_index(), 0);
    }

    #[test]
    fn back_tab_event_also_flips_focused_pane() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = small_widget();
        let ev = Event::Key(KeyChord::new(KeyCode::BackTab, KeyModifiers::empty()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        w.handle_event(&ev, &mut ctx);
        assert_eq!(w.focused_pane(), SettingsFocus::SubView);
    }

    #[test]
    fn arrow_event_routes_to_focused_theme_picker_after_tab() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = small_widget();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        // Tab to SubView so the theme picker receives keys.
        let tab = Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::empty()));
        w.handle_event(&tab, &mut ctx);
        assert_eq!(w.focused_pane(), SettingsFocus::SubView);
        let down = Event::Key(KeyChord::new(KeyCode::Down, KeyModifiers::empty()));
        let outcome = w.handle_event(&down, &mut ctx);
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

    // ---------------------------------------------------------------------
    // Strict pane-focus model tests
    // ---------------------------------------------------------------------

    fn key(code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) -> Event {
        use sid_core::event::KeyChord;
        Event::Key(KeyChord::new(code, mods))
    }

    fn ctx() -> WidgetCtx {
        let (tx, _rx) = std::sync::mpsc::channel();
        WidgetCtx::new(tx)
    }

    #[test]
    fn default_focus_is_categories() {
        let w = small_widget();
        assert_eq!(w.focused_pane(), SettingsFocus::Categories);
        assert_eq!(w.focused_pane_label(), "Categories");
    }

    #[test]
    fn settings_tab_flips_focus_pane() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = small_widget();
        let mut c = ctx();
        assert_eq!(w.focused_pane(), SettingsFocus::Categories);
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), SettingsFocus::SubView);
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), SettingsFocus::Categories);
    }

    #[test]
    fn settings_shift_tab_cycles_focus_backward() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = small_widget();
        let mut c = ctx();
        w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
        assert_eq!(w.focused_pane(), SettingsFocus::SubView);
        w.handle_event(&key(KeyCode::BackTab, KeyModifiers::SHIFT), &mut c);
        assert_eq!(w.focused_pane(), SettingsFocus::Categories);
    }

    #[test]
    fn settings_j_in_categories_changes_category() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = small_widget();
        let mut c = ctx();
        assert_eq!(w.focused_pane(), SettingsFocus::Categories);
        assert_eq!(w.focused_category_index(), 0);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_category_index(), 1);
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_category_index(), 2);
    }

    #[test]
    fn settings_j_in_subview_does_not_change_category() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = small_widget();
        let mut c = ctx();
        // Tab to SubView.
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane(), SettingsFocus::SubView);
        let before = w.focused_category_index();
        w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::NONE), &mut c);
        // Focused category index unchanged.
        assert_eq!(w.focused_category_index(), before);
    }

    #[test]
    fn settings_border_follows_focus() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = small_widget();
        let mut c = ctx();
        assert_eq!(w.focused_pane_label(), "Categories");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "SubView");
        w.handle_event(&key(KeyCode::Tab, KeyModifiers::NONE), &mut c);
        assert_eq!(w.focused_pane_label(), "Categories");
    }

    #[test]
    fn settings_alt_keys_bubble_and_do_not_change_pane() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = small_widget();
        let mut c = ctx();
        let pane_before = w.focused_pane();
        let cat_before = w.focused_category_index();
        let outcome = w.handle_event(&key(KeyCode::Char('j'), KeyModifiers::ALT), &mut c);
        assert_eq!(outcome, EventOutcome::Bubble);
        assert_eq!(w.focused_pane(), pane_before);
        assert_eq!(w.focused_category_index(), cat_before);
    }

    #[test]
    fn theme_applied_pushes_pending_outcome() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        use sid_ui::theme_registry::ThemeRegistry;
        let r = ThemeRegistry::with_builtins();
        let mut w = SettingsWidget::with_categories(vec![SettingsCategory::Theme(
            crate::settings::theme_picker::ThemePickerView::new(&r, "cosmos"),
        )]);
        // Tab to SubView so Theme category receives keys.
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        w.handle_event(
            &Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::NONE)),
            &mut ctx,
        );
        // Enter applies the focused theme.
        w.handle_event(
            &Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE)),
            &mut ctx,
        );
        let outcomes = w.take_pending_outcomes();
        assert!(
            outcomes
                .iter()
                .any(|o| matches!(o, PendingSettingsOutcome::ThemeApplied { .. })),
            "ThemeApplied outcome expected"
        );
    }
}
