//! `SystemWidget` — System tab state, focus management, and sub-panel models.
//!
//! Plan 6 splits the System tab into three sub-panels (pinned configs,
//! systemctl services, quick actions). This module exposes the pure-Rust
//! state structs the binary's draw layer renders against. The rendering
//! itself remains a "coming soon" body until the cosmos render harness
//! lands — adapter-pattern-clean wiring of `SystemctlClient` and
//! `TerminalSpawner` lives in the binary (`wire.rs`).

use std::collections::{HashMap, VecDeque};

use sid_core::adapters::systemctl::{JournalEntry, SystemUnit, UnitBus, UnitState};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_store::{PinnedConfig, QuickAction};

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
                .filter(|p| {
                    p.label.contains(needle) || p.path.to_string_lossy().contains(needle)
                })
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

    fn render(&self, target: &mut dyn RenderTarget) {
        self.body.render(target);
    }

    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            match (chord.code, chord.mods) {
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.state.cycle_focus_forward();
                    return EventOutcome::Consumed;
                }
                (KeyCode::BackTab, _) => {
                    self.state.cycle_focus_backward();
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
        }
        self.body.handle_event(ev, ctx)
    }
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
