//! `SystemWidget` — System tab state, focus management, and sub-panel models.
//!
//! Plan 6 splits the System tab into three sub-panels (pinned configs,
//! systemctl services, quick actions). This module exposes the pure-Rust
//! state structs the binary's draw layer renders against. The rendering
//! itself remains a "coming soon" body until the cosmos render harness
//! lands — adapter-pattern-clean wiring of `SystemctlClient` and
//! `TerminalSpawner` lives in the binary (`wire.rs`).

use std::collections::{HashMap, VecDeque};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table};
use sid_core::adapters::systemctl::{JournalEntry, SystemUnit, UnitBus, UnitState};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, FooterHint, RenderTarget, Widget, WidgetId};
use sid_store::{PinnedConfig, QuickAction, QuickActionScope};
use sid_ui::Theme;
use sid_ui::themes::cosmos;

use crate::stub::ComingSoonBody;

/// One of the three sub-panels in the System tab.
///
/// # Examples
///
/// ```
/// use sid_widgets::system::SystemPane;
/// assert_ne!(SystemPane::PinnedConfigs, SystemPane::Services);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SystemPane {
    PinnedConfigs,
    Services,
    QuickActions,
}

/// Pure-Rust state for the System tab.
///
/// # Examples
///
/// ```
/// use sid_widgets::system::{SystemPane, SystemState};
/// let s = SystemState::new();
/// assert_eq!(s.focused_pane(), SystemPane::PinnedConfigs);
/// ```
pub struct SystemState {
    focused: SystemPane,
    filters: HashMap<SystemPane, String>,
}

impl Default for SystemState {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemState {
    /// Create a fresh state focused on Pinned Configs with no filter.
    pub fn new() -> Self {
        Self {
            focused: SystemPane::PinnedConfigs,
            filters: HashMap::new(),
        }
    }

    /// Currently-focused sub-panel.
    pub fn focused_pane(&self) -> SystemPane {
        self.focused
    }

    /// Stable string label for the focused sub-panel.
    pub fn focused_pane_label(&self) -> &'static str {
        match self.focused {
            SystemPane::PinnedConfigs => "PinnedConfigs",
            SystemPane::Services => "Services",
            SystemPane::QuickActions => "QuickActions",
        }
    }

    /// Cycle focus forward (PinnedConfigs → Services → QuickActions → wrap).
    pub fn cycle_focus_forward(&mut self) {
        self.focused = match self.focused {
            SystemPane::PinnedConfigs => SystemPane::Services,
            SystemPane::Services => SystemPane::QuickActions,
            SystemPane::QuickActions => SystemPane::PinnedConfigs,
        };
    }

    /// Cycle focus backward.
    pub fn cycle_focus_backward(&mut self) {
        self.focused = match self.focused {
            SystemPane::PinnedConfigs => SystemPane::QuickActions,
            SystemPane::Services => SystemPane::PinnedConfigs,
            SystemPane::QuickActions => SystemPane::Services,
        };
    }

    /// Set the filter substring for the focused pane.
    pub fn set_filter(&mut self, s: String) {
        self.filters.insert(self.focused, s);
    }

    /// Clear the filter substring for the focused pane.
    pub fn clear_filter(&mut self) {
        self.filters.remove(&self.focused);
    }

    /// Filter substring for the focused pane, if any.
    pub fn filter(&self) -> Option<&str> {
        self.filters.get(&self.focused).map(String::as_str)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pinned configs sub-panel
// ─────────────────────────────────────────────────────────────────────────────

/// Modal state for the pinned-configs sub-panel.
#[derive(Debug)]
pub enum PinnedConfigsModal {
    /// No modal showing.
    Closed,
    /// Add-pin modal with editable buffers.
    Add {
        path_buf: String,
        label_buf: String,
        opener_buf: String,
    },
    /// Edit-pin modal with the original record + editable buffers.
    Edit {
        original: PinnedConfig,
        path_buf: String,
        label_buf: String,
        opener_buf: String,
    },
    /// Confirm-delete modal.
    ConfirmDelete { target: PinnedConfig },
}

/// State for the Pinned Configs sub-panel.
///
/// # Examples
///
/// ```
/// use sid_widgets::system::PinnedConfigsState;
/// let s = PinnedConfigsState::new(vec![]);
/// assert!(s.selected().is_none());
/// ```
pub struct PinnedConfigsState {
    items: Vec<PinnedConfig>,
    selected_idx: usize,
    pub modal: PinnedConfigsModal,
}

impl PinnedConfigsState {
    /// Build the state with an initial list of pins.
    pub fn new(items: Vec<PinnedConfig>) -> Self {
        Self {
            items,
            selected_idx: 0,
            modal: PinnedConfigsModal::Closed,
        }
    }

    /// View the pin list.
    pub fn items(&self) -> &[PinnedConfig] {
        &self.items
    }

    /// Replace the pin list (e.g. after a store mutation).
    /// Clamps the selected index to the new bounds.
    pub fn replace_items(&mut self, items: Vec<PinnedConfig>) {
        self.items = items;
        if self.selected_idx >= self.items.len() {
            self.selected_idx = self.items.len().saturating_sub(1);
        }
    }

    /// Currently-selected pin (or `None` if the list is empty).
    pub fn selected(&self) -> Option<&PinnedConfig> {
        self.items.get(self.selected_idx)
    }

    /// Move the selection forward (wraps).
    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.items.len();
    }

    /// Move the selection backward (wraps).
    pub fn select_prev(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let n = self.items.len();
        self.selected_idx = (self.selected_idx + n - 1) % n;
    }

    /// Pins visible under `filter` (label or path substring match).
    pub fn visible<'a>(&'a self, filter: Option<&str>) -> Vec<&'a PinnedConfig> {
        match filter {
            None => self.items.iter().collect(),
            Some(needle) => self
                .items
                .iter()
                .filter(|p| p.label.contains(needle) || p.path.to_string_lossy().contains(needle))
                .collect(),
        }
    }

    /// Build a fresh `Add` modal.
    pub fn begin_add(&self) -> PinnedConfigsModal {
        PinnedConfigsModal::Add {
            path_buf: String::new(),
            label_buf: String::new(),
            opener_buf: String::new(),
        }
    }

    /// Build an `Edit` modal preloaded with the selected pin, or `None` if no
    /// pin is selected.
    pub fn begin_edit_selected(&self) -> Option<PinnedConfigsModal> {
        let sel = self.selected()?.clone();
        Some(PinnedConfigsModal::Edit {
            path_buf: sel.path.to_string_lossy().into_owned(),
            label_buf: sel.label.clone(),
            opener_buf: sel.opener_cmd.clone().unwrap_or_default(),
            original: sel,
        })
    }

    /// Build a `ConfirmDelete` modal preloaded with the selected pin.
    pub fn begin_confirm_delete(&self) -> Option<PinnedConfigsModal> {
        Some(PinnedConfigsModal::ConfirmDelete {
            target: self.selected()?.clone(),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Services sub-panel
// ─────────────────────────────────────────────────────────────────────────────

/// Per-unit action surfaced in the services popup menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServicesAction {
    Start,
    Stop,
    Restart,
    JournalTail,
}

/// State for the Services sub-panel.
///
/// # Examples
///
/// ```
/// use sid_widgets::system::ServicesState;
/// let s = ServicesState::new(vec![]);
/// assert!(s.units().is_empty());
/// ```
pub struct ServicesState {
    units: Vec<SystemUnit>,
    selected_idx: usize,
    menu_open: bool,
}

impl ServicesState {
    /// Build the state from a unit list.
    pub fn new(units: Vec<SystemUnit>) -> Self {
        Self {
            units,
            selected_idx: 0,
            menu_open: false,
        }
    }

    /// View the unit list.
    pub fn units(&self) -> &[SystemUnit] {
        &self.units
    }

    /// Replace the unit list; clamp selected index.
    pub fn replace_units(&mut self, units: Vec<SystemUnit>) {
        self.units = units;
        if self.selected_idx >= self.units.len() {
            self.selected_idx = self.units.len().saturating_sub(1);
        }
    }

    /// Currently-selected unit.
    pub fn selected(&self) -> Option<&SystemUnit> {
        self.units.get(self.selected_idx)
    }

    /// Move the selection forward (wraps).
    pub fn select_next(&mut self) {
        if self.units.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.units.len();
    }

    /// Move the selection backward (wraps).
    pub fn select_prev(&mut self) {
        if self.units.is_empty() {
            return;
        }
        let n = self.units.len();
        self.selected_idx = (self.selected_idx + n - 1) % n;
    }

    /// Units passing both filters.
    pub fn visible<'a>(
        &'a self,
        name_filter: Option<&str>,
        state_filter: Option<UnitState>,
    ) -> Vec<&'a SystemUnit> {
        self.units
            .iter()
            .filter(|u| name_filter.is_none_or(|n| u.name.contains(n)))
            .filter(|u| state_filter.is_none_or(|s| u.state == s))
            .collect()
    }

    /// Open the action popup.
    pub fn open_menu(&mut self) {
        self.menu_open = true;
    }

    /// Close the action popup.
    pub fn close_menu(&mut self) {
        self.menu_open = false;
    }

    /// Is the menu currently open?
    pub fn menu_open(&self) -> bool {
        self.menu_open
    }

    /// All possible action types surfaced by the menu.
    pub const fn menu_actions() -> &'static [ServicesAction] {
        &[
            ServicesAction::Start,
            ServicesAction::Stop,
            ServicesAction::Restart,
            ServicesAction::JournalTail,
        ]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Journal tail modal
// ─────────────────────────────────────────────────────────────────────────────

/// State for the journal-tail modal (per-unit log view).
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::UnitBus;
/// use sid_widgets::system::JournalTailState;
/// let s = JournalTailState::new("x.service".into(), UnitBus::User);
/// assert!(!s.is_following());
/// ```
pub struct JournalTailState {
    unit_name: String,
    bus: UnitBus,
    entries: VecDeque<JournalEntry>,
    follow: bool,
}

impl JournalTailState {
    /// Maximum entries retained in follow mode (oldest dropped).
    pub const MAX_ENTRIES: usize = 1000;

    /// Create a fresh modal state for the given unit.
    pub fn new(unit_name: String, bus: UnitBus) -> Self {
        Self {
            unit_name,
            bus,
            entries: VecDeque::with_capacity(Self::MAX_ENTRIES),
            follow: false,
        }
    }

    /// Unit name being tailed.
    pub fn unit_name(&self) -> &str {
        &self.unit_name
    }

    /// Bus of the unit being tailed.
    pub fn bus(&self) -> UnitBus {
        self.bus
    }

    /// Current entry buffer.
    pub fn entries(&self) -> &VecDeque<JournalEntry> {
        &self.entries
    }

    /// Replace entries from a one-shot tail call (clears the buffer first).
    pub fn set_entries(&mut self, v: Vec<JournalEntry>) {
        self.entries.clear();
        for e in v {
            self.entries.push_back(e);
            if self.entries.len() > Self::MAX_ENTRIES {
                self.entries.pop_front();
            }
        }
    }

    /// Append one entry from the follow stream, dropping the oldest if needed.
    pub fn push_followed(&mut self, e: JournalEntry) {
        self.entries.push_back(e);
        if self.entries.len() > Self::MAX_ENTRIES {
            self.entries.pop_front();
        }
    }

    /// Enter follow mode.
    pub fn start_follow(&mut self) {
        self.follow = true;
    }

    /// Leave follow mode.
    pub fn stop_follow(&mut self) {
        self.follow = false;
    }

    /// Is follow mode active?
    pub fn is_following(&self) -> bool {
        self.follow
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Quick actions sub-panel
// ─────────────────────────────────────────────────────────────────────────────

/// Modal state for the quick-actions sub-panel.
#[derive(Debug)]
pub enum QuickActionsModal {
    Closed,
    Add {
        label_buf: String,
        cmd_buf: String,
        keybind_buf: String,
    },
    Edit {
        original: QuickAction,
        label_buf: String,
        cmd_buf: String,
        keybind_buf: String,
    },
    ConfirmDelete {
        target: QuickAction,
    },
}

/// State for the Quick Actions sub-panel inside the System tab.
///
/// This is a separate view over the same `quick_actions` redb table that
/// Plan 7's Settings `QuickActionsView` operates on. The two views read/write
/// through the same [`sid_store::Store::list_quick_actions`] interface.
///
/// # Examples
///
/// ```
/// use sid_widgets::system::QuickActionsState;
/// let s = QuickActionsState::new(vec![]);
/// assert!(s.selected().is_none());
/// ```
pub struct QuickActionsState {
    items: Vec<QuickAction>,
    selected_idx: usize,
    pub modal: QuickActionsModal,
}

impl QuickActionsState {
    /// Build the state with an initial list.
    pub fn new(items: Vec<QuickAction>) -> Self {
        Self {
            items,
            selected_idx: 0,
            modal: QuickActionsModal::Closed,
        }
    }

    /// View the action list.
    pub fn items(&self) -> &[QuickAction] {
        &self.items
    }

    /// Replace the action list; clamp selected index.
    pub fn replace_items(&mut self, items: Vec<QuickAction>) {
        self.items = items;
        if self.selected_idx >= self.items.len() {
            self.selected_idx = self.items.len().saturating_sub(1);
        }
    }

    /// Currently-selected action.
    pub fn selected(&self) -> Option<&QuickAction> {
        self.items.get(self.selected_idx)
    }

    /// Move the selection forward (wraps).
    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.items.len();
    }

    /// Move the selection backward (wraps).
    pub fn select_prev(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let n = self.items.len();
        self.selected_idx = (self.selected_idx + n - 1) % n;
    }

    /// Actions visible under `filter` (label or cmd substring match).
    pub fn visible<'a>(&'a self, filter: Option<&str>) -> Vec<&'a QuickAction> {
        match filter {
            None => self.items.iter().collect(),
            Some(needle) => self
                .items
                .iter()
                .filter(|a| a.label.contains(needle) || a.cmd.contains(needle))
                .collect(),
        }
    }

    /// Build a fresh `Add` modal.
    pub fn begin_add(&self) -> QuickActionsModal {
        QuickActionsModal::Add {
            label_buf: String::new(),
            cmd_buf: String::new(),
            keybind_buf: String::new(),
        }
    }

    /// Build an `Edit` modal preloaded with the selected action, or `None`.
    pub fn begin_edit_selected(&self) -> Option<QuickActionsModal> {
        let sel = self.selected()?.clone();
        Some(QuickActionsModal::Edit {
            label_buf: sel.label.clone(),
            cmd_buf: sel.cmd.clone(),
            keybind_buf: sel.keybind.clone().unwrap_or_default(),
            original: sel,
        })
    }

    /// Build a `ConfirmDelete` modal preloaded with the selected action.
    pub fn begin_confirm_delete(&self) -> Option<QuickActionsModal> {
        Some(QuickActionsModal::ConfirmDelete {
            target: self.selected()?.clone(),
        })
    }
}

/// Parse a quick-action command via [`shell_words::split`]. Errors out on
/// malformed quoting.
///
/// # Examples
///
/// ```
/// let v = sid_widgets::system::parse_quick_action_cmd("echo hi").unwrap();
/// assert_eq!(v, vec!["echo", "hi"]);
/// ```
pub fn parse_quick_action_cmd(cmd: &str) -> Result<Vec<String>, shell_words::ParseError> {
    shell_words::split(cmd)
}

// ─────────────────────────────────────────────────────────────────────────────
// SystemWidget (top-level)
// ─────────────────────────────────────────────────────────────────────────────

/// Top-level System tab widget.
///
/// In Plan 6's deliverable scope this wraps a coming-soon body for rendering
/// while exposing the full state machinery used by the binary's draw layer
/// once the render harness lands. The state types ([`SystemState`],
/// [`PinnedConfigsState`], [`ServicesState`], [`JournalTailState`],
/// [`QuickActionsState`]) are all pure-Rust and fully unit-tested.
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::SystemWidget;
///
/// let w = SystemWidget::new();
/// assert_eq!(w.id().as_str(), "system.root");
/// assert_eq!(w.title(), "System");
/// ```
pub struct SystemWidget {
    body: ComingSoonBody,
    id: WidgetId,
    state: SystemState,
    pinned: PinnedConfigsState,
    services: ServicesState,
    quick_actions: QuickActionsState,
    journal: JournalTailState,
}

impl SystemWidget {
    /// Create a new `SystemWidget`.
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "System",
                "pinned configs, systemctl, custom quick-actions — state surface ready in Plan 6",
            ),
            id: WidgetId::new("system.root"),
            state: SystemState::new(),
            pinned: PinnedConfigsState::new(Vec::new()),
            services: ServicesState::new(Vec::new()),
            quick_actions: QuickActionsState::new(Vec::new()),
            journal: JournalTailState::new(String::new(), UnitBus::User),
        }
    }

    /// Borrow the focus/filter state.
    pub fn state(&self) -> &SystemState {
        &self.state
    }

    /// Mutably borrow the focus/filter state.
    pub fn state_mut(&mut self) -> &mut SystemState {
        &mut self.state
    }

    /// Borrow the pinned-configs sub-panel state.
    pub fn pinned_configs(&self) -> &PinnedConfigsState {
        &self.pinned
    }

    /// Mutably borrow the pinned-configs sub-panel state.
    pub fn pinned_configs_mut(&mut self) -> &mut PinnedConfigsState {
        &mut self.pinned
    }

    /// Borrow the services sub-panel state.
    pub fn services(&self) -> &ServicesState {
        &self.services
    }

    /// Mutably borrow the services sub-panel state.
    pub fn services_mut(&mut self) -> &mut ServicesState {
        &mut self.services
    }

    /// Borrow the quick-actions sub-panel state.
    pub fn quick_actions(&self) -> &QuickActionsState {
        &self.quick_actions
    }

    /// Mutably borrow the quick-actions sub-panel state.
    pub fn quick_actions_mut(&mut self) -> &mut QuickActionsState {
        &mut self.quick_actions
    }

    /// Borrow the journal-tail modal state. Plan 6 surfaces it through the
    /// services pane menu; the render path treats it as an overlay-on-services
    /// rather than a full pane.
    pub fn journal(&self) -> &JournalTailState {
        &self.journal
    }

    /// Mutably borrow the journal-tail modal state.
    pub fn journal_mut(&mut self) -> &mut JournalTailState {
        &mut self.journal
    }

    /// Replace the journal tail state wholesale. Used by the binary wiring
    /// when a user opens journal-tail for a specific unit, and by tests.
    pub fn set_journal(&mut self, j: JournalTailState) {
        self.journal = j;
    }

    /// Render the widget into a ratatui [`Frame`]. Used by the insta
    /// snapshot tests and by the future direct-frame plumbing.
    ///
    /// Layout: a one-row pane strip at the top, then the focused pane's body
    /// in the remainder, then a one-row filter / keybind hint at the bottom.
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);
        let strip_rect = split[0];
        let body_rect = split[1];
        let bar_rect = split[2];

        self.render_pane_strip(frame, strip_rect, theme);
        match self.state.focused_pane() {
            SystemPane::PinnedConfigs => self.render_pinned_configs(frame, body_rect, theme),
            SystemPane::Services => self.render_services(frame, body_rect, theme),
            SystemPane::QuickActions => self.render_quick_actions(frame, body_rect, theme),
        }
        self.render_filter_bar(frame, bar_rect, theme);

        // Journal tail overlay (when unit name is non-empty) — surfaced from
        // the services pane menu in real use; tests can populate via
        // `set_journal`.
        if !self.journal.unit_name().is_empty() {
            self.render_journal(frame, body_rect, theme);
        }
    }

    fn render_pane_strip(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let focused = self.state.focused_pane();
        let mut spans: Vec<Span<'_>> = Vec::new();
        for (i, (pane, label)) in [
            (SystemPane::PinnedConfigs, "Pinned configs"),
            (SystemPane::Services, "Services"),
            (SystemPane::QuickActions, "Quick actions"),
        ]
        .into_iter()
        .enumerate()
        {
            if i > 0 {
                spans.push(Span::styled(" · ", Style::default().fg(theme.muted.into())));
            }
            let glyph = if pane == focused { "● " } else { "○ " };
            let style = if pane == focused {
                Style::default()
                    .fg(theme.accent_primary.into())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted.into())
            };
            spans.push(Span::styled(format!("{glyph}{label}"), style));
        }
        spans.push(Span::styled(
            "   (Tab cycles)",
            Style::default().fg(theme.muted.into()),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
    }

    fn render_pinned_configs(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let filter = self.state.filter();
        let mut lines: Vec<Line<'_>> = Vec::new();
        let items = self.pinned.items();
        let selected = items.iter().position(|p| Some(p) == self.pinned.selected());
        for (i, item) in items.iter().enumerate() {
            if let Some(needle) = filter {
                if !item.label.contains(needle) && !item.path.to_string_lossy().contains(needle) {
                    continue;
                }
            }
            let marker = if Some(i) == selected { "> " } else { "  " };
            let path = item.path.to_string_lossy();
            let label = if item.label.is_empty() {
                format!("{marker}{path}")
            } else {
                format!("{marker}{path}  [{}]", item.label)
            };
            let style = if Some(i) == selected {
                Style::default()
                    .fg(theme.background.into())
                    .bg(theme.accent_primary.into())
            } else {
                Style::default().fg(theme.foreground.into())
            };
            lines.push(Line::from(Span::styled(label, style)));
        }
        if items.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no pinned configs)",
                Style::default().fg(theme.muted.into()),
            )));
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent_primary.into()))
            .title(" Pinned configs ")
            .title_style(
                Style::default()
                    .fg(theme.foreground.into())
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_services(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let filter = self.state.filter();
        let header = Row::new(["UNIT", "STATE", "SUB"]).style(
            Style::default()
                .fg(theme.muted.into())
                .add_modifier(Modifier::BOLD),
        );
        let units = self.services.units();
        let selected = units
            .iter()
            .position(|u| Some(u) == self.services.selected());
        let mut body: Vec<Row<'_>> = Vec::new();
        for (i, u) in units.iter().enumerate() {
            if let Some(needle) = filter {
                if !u.name.contains(needle) {
                    continue;
                }
            }
            let state_label = format!("{:?}", u.state).to_lowercase();
            let state_color = match u.state {
                UnitState::Active => theme.accent_success,
                UnitState::Failed => theme.accent_error,
                UnitState::Activating | UnitState::Reloading => theme.accent_warning,
                _ => theme.muted,
            };
            let row_style = if Some(i) == selected {
                Style::default()
                    .fg(theme.background.into())
                    .bg(theme.accent_primary.into())
            } else {
                Style::default().fg(theme.foreground.into())
            };
            let row = Row::new(vec![
                Span::raw(u.name.clone()),
                Span::styled(state_label, Style::default().fg(state_color.into())),
                Span::raw(u.sub_state.clone()),
            ])
            .style(row_style);
            body.push(row);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent_primary.into()))
            .title(" Services ")
            .title_style(
                Style::default()
                    .fg(theme.foreground.into())
                    .add_modifier(Modifier::BOLD),
            );
        let table = Table::new(
            body,
            [
                Constraint::Min(20),
                Constraint::Length(12),
                Constraint::Length(12),
            ],
        )
        .header(header)
        .block(block);
        frame.render_widget(table, rect);
    }

    fn render_journal(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        // Newest at bottom; show last `rect.height - 2` lines so the border
        // fits.
        let visible = rect.height.saturating_sub(2) as usize;
        let entries = self.journal.entries();
        let start = entries.len().saturating_sub(visible);
        let mut lines: Vec<Line<'_>> = Vec::with_capacity(visible);
        for entry in entries.iter().skip(start) {
            let stamp = format!("{:>10}", entry.timestamp_secs);
            let line = Line::from(vec![
                Span::styled(stamp, Style::default().fg(theme.muted.into())),
                Span::raw("  "),
                Span::styled(
                    entry.message.clone(),
                    Style::default().fg(theme.foreground.into()),
                ),
            ]);
            lines.push(line);
        }
        let follow_tag = if self.journal.is_following() {
            "on"
        } else {
            "off"
        };
        let title = format!(
            " Journal: {} (following: {follow_tag}) ",
            self.journal.unit_name()
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent_primary.into()))
            .title(title)
            .title_style(Style::default().fg(theme.foreground.into()));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_quick_actions(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let filter = self.state.filter();
        let header = Row::new(["LABEL", "COMMAND", "SCOPE", "KEY"]).style(
            Style::default()
                .fg(theme.muted.into())
                .add_modifier(Modifier::BOLD),
        );
        let items = self.quick_actions.items();
        let selected = items
            .iter()
            .position(|a| Some(a) == self.quick_actions.selected());
        let mut body: Vec<Row<'_>> = Vec::new();
        for (i, a) in items.iter().enumerate() {
            if let Some(needle) = filter {
                if !a.label.contains(needle) && !a.cmd.contains(needle) {
                    continue;
                }
            }
            let scope = match a.scope {
                QuickActionScope::Global => "global",
                QuickActionScope::Workspace => "workspace",
            };
            let key = a.keybind.clone().unwrap_or_else(|| "-".into());
            let row_style = if Some(i) == selected {
                Style::default()
                    .fg(theme.background.into())
                    .bg(theme.accent_primary.into())
            } else {
                Style::default().fg(theme.foreground.into())
            };
            body.push(
                Row::new(vec![a.label.clone(), a.cmd.clone(), scope.into(), key]).style(row_style),
            );
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent_primary.into()))
            .title(" Quick actions ")
            .title_style(
                Style::default()
                    .fg(theme.foreground.into())
                    .add_modifier(Modifier::BOLD),
            );
        let table = Table::new(
            body,
            [
                Constraint::Length(18),
                Constraint::Min(16),
                Constraint::Length(10),
                Constraint::Length(12),
            ],
        )
        .header(header)
        .block(block);
        frame.render_widget(table, rect);
    }

    fn render_filter_bar(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let line = match self.state.filter() {
            Some(q) if !q.is_empty() => Line::from(vec![
                Span::styled("/ ", Style::default().fg(theme.accent_warning.into())),
                Span::styled(q.to_string(), Style::default().fg(theme.foreground.into())),
            ]),
            _ => Line::from(Span::styled(
                "Tab: switch pane · /: filter · Enter: action",
                Style::default().fg(theme.muted.into()),
            )),
        };
        frame.render_widget(Paragraph::new(line), rect);
    }
}

impl Default for SystemWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for SystemWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }

    fn title(&self) -> &str {
        self.body.title()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("N", "new"),
            FooterHint::new("E", "edit"),
            FooterHint::new("D", "remove"),
            FooterHint::new("Enter", "open"),
            FooterHint::new("Tab", "pane"),
        ]
    }

    fn render(&self, target: &mut dyn RenderTarget) {
        self.body.render(target);
    }

    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            // Tab / Shift+Tab cycle the focused pane FIRST.
            match (chord.code, chord.mods) {
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.state.cycle_focus_forward();
                    return EventOutcome::Consumed;
                }
                (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => {
                    self.state.cycle_focus_backward();
                    return EventOutcome::Consumed;
                }
                (KeyCode::BackTab, _) => {
                    self.state.cycle_focus_backward();
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
            // Alt+<key> is reserved for future cross-pane actions.
            if chord.mods.contains(KeyModifiers::ALT) {
                // TODO: cross-pane actions on Alt+<key>
                return EventOutcome::Bubble;
            }
            // j/k routes to the focused sub-panel via the binary's wire
            // layer (which owns the per-pane mutators). The widget itself
            // bubbles navigation; the focused-pane enum guarantees the
            // wire layer routes input to the same sub-panel that has the
            // accent border.
        }
        self.body.handle_event(ev, ctx)
    }
}

// ---------------------------------------------------------------------------
// Convenience: render the widget into a fresh ratatui `Buffer` for tests.
// ---------------------------------------------------------------------------

/// Render the widget into a fresh test buffer of `(width, height)` using
/// the cosmos theme.
///
/// Pulled out as a free helper so doc tests and integration tests can both
/// use it without spinning up a Terminal.
///
/// # Examples
///
/// ```
/// use sid_widgets::SystemWidget;
/// use sid_widgets::system::render_to_string;
/// let w = SystemWidget::new();
/// let s = render_to_string(&w, 80, 12);
/// assert!(s.contains("Pinned configs"));
/// ```
pub fn render_to_string(widget: &SystemWidget, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
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

#[cfg(test)]
mod tests {
    use sid_core::widget::Widget;

    use super::*;

    #[test]
    fn id_and_title_correct() {
        let w = SystemWidget::new();
        assert_eq!(w.id().as_str(), "system.root");
        assert_eq!(w.title(), "System");
    }

    #[test]
    fn save_state_is_empty() {
        let w = SystemWidget::new();
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn load_state_is_noop() {
        let mut w = SystemWidget::new();
        w.load_state(&[0x01, 0x02, 0x03, 0x04, 0x05]);
        assert_eq!(w.id().as_str(), "system.root");
    }

    #[test]
    fn state_initial_focus_is_pinned_configs() {
        let w = SystemWidget::new();
        assert_eq!(w.state().focused_pane(), SystemPane::PinnedConfigs);
    }
}
