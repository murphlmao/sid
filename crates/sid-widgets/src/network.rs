//! Network tab widget. Assembles the per-pane state types in this module's
//! submodules into a single top-level [`NetworkWidget`].
//!
//! The widget owns:
//!   - [`PortsTableState`]    — listening sockets pane
//!   - [`ProcessesTableState`]— processes pane
//!   - [`InterfacesSidebarState`] — left-rail interfaces list
//!   - [`FilterInputState`]   — `/` filter editor
//!   - [`KillConfirmModalState`] — kill confirmation overlay
//!   - a focus marker for the active pane
//!   - an optional broadcast receiver fed from a `SysProbe`
//!
//! Rendering goes through both abstractions sid uses:
//!
//!  - [`Widget::render`] writes a text summary into a [`RenderTarget`] so the
//!    Plan 1 wire path keeps working;
//!  - [`NetworkWidget::render_into_frame`] is a ratatui-aware draw used by
//!    insta snapshot tests and by the future direct-render plumbing.

pub mod filter_input;
pub mod interfaces_sidebar;
pub mod kill_modal;
pub mod ports_table;
pub mod processes_table;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use serde::{Deserialize, Serialize};
use sid_core::adapters::sys::Pid;
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, FooterHint, RenderTarget, Widget, WidgetId};
use sid_ui::Theme;
use sid_ui::themes::cosmos;

use crate::network::filter_input::{
    FilterInputState, FilterMode, match_interface, match_listening_port, match_process,
};
use crate::network::interfaces_sidebar::InterfacesSidebarState;
use crate::network::kill_modal::KillConfirmModalState;
use crate::network::ports_table::{PortsSortBy, PortsTableState, SortDir};
use crate::network::processes_table::{ProcessesSortBy, ProcessesTableState};

// Re-export the KillOutcome type so the binary's JobQueue wiring (Task 24)
// can feed completion results back into the widget without naming
// sid_core::sys_probe::kill_job directly.
pub use sid_core::sys_probe::kill_job::KillOutcome;

/// Toast level for a kill outcome. The widget produces these; the binary's
/// render code maps them to colours.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::{KillToast, ToastLevel};
/// let t = KillToast { level: ToastLevel::Success, message: "killed".into() };
/// assert_eq!(t.level, ToastLevel::Success);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToastLevel {
    /// Successful, expected outcome (e.g., process exited on SIGTERM).
    Success,
    /// Worked, but louder than expected (e.g., SIGKILL escalation).
    Warning,
    /// Did not work (e.g., permission denied).
    Error,
}

/// A toast queued by the widget after a kill action completes.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::{KillToast, ToastLevel};
/// let t = KillToast { level: ToastLevel::Error, message: "boom".into() };
/// assert_eq!(t.message, "boom");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KillToast {
    /// Severity of the toast.
    pub level: ToastLevel,
    /// Human-readable message.
    pub message: String,
}

/// Currently-focused pane. Cycled with Tab / Shift+Tab.
///
/// Also exposed as [`NetFocus`] for parity with the other widgets'
/// `<Widget>Focus` naming convention.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::Focus;
/// assert_ne!(Focus::Ports, Focus::Processes);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Focus {
    /// Listening ports table is focused.
    #[default]
    Ports,
    /// Processes table is focused.
    Processes,
    /// Interfaces sidebar is focused.
    Interfaces,
}

/// Strict pane-focus model alias matching the other widgets'
/// `<Widget>Focus` convention. See [`Focus`].
pub type NetFocus = Focus;

/// Persisted UI preferences. Captures sort + focus so a sid restart restores
/// the user's view layout. The actual data comes from the next probe tick.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PersistedPrefs {
    focus: Focus,
    ports_sort: Option<(PortsSortBy, SortDir)>,
    procs_sort: Option<(ProcessesSortBy, SortDir)>,
}

/// Versioned wire format for [`Widget::save_state`] / [`Widget::load_state`].
const PERSIST_VERSION: u8 = 1;

// Make the sort enums serde-able. They are local to this crate so the
// derive is sound and the on-disk shape is part of the persist version.
mod sort_serde {
    use super::*;
    impl Serialize for PortsSortBy {
        fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
            (*self as u8).serialize(ser)
        }
    }
    impl<'de> Deserialize<'de> for PortsSortBy {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            Ok(match u8::deserialize(d)? {
                0 => PortsSortBy::Port,
                1 => PortsSortBy::Pid,
                2 => PortsSortBy::Command,
                3 => PortsSortBy::Protocol,
                _ => PortsSortBy::Port,
            })
        }
    }
    impl Serialize for SortDir {
        fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
            (*self as u8).serialize(ser)
        }
    }
    impl<'de> Deserialize<'de> for SortDir {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            Ok(match u8::deserialize(d)? {
                0 => SortDir::Asc,
                1 => SortDir::Desc,
                _ => SortDir::Asc,
            })
        }
    }
    impl Serialize for ProcessesSortBy {
        fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
            (*self as u8).serialize(ser)
        }
    }
    impl<'de> Deserialize<'de> for ProcessesSortBy {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            Ok(match u8::deserialize(d)? {
                0 => ProcessesSortBy::Pid,
                1 => ProcessesSortBy::Name,
                2 => ProcessesSortBy::Cpu,
                3 => ProcessesSortBy::Rss,
                4 => ProcessesSortBy::Started,
                _ => ProcessesSortBy::Pid,
            })
        }
    }
}

/// Network tab widget — assembles the five pane state types and renders a
/// three-column layout.
///
/// # Examples
///
/// ```
/// use sid_widgets::NetworkWidget;
/// let w = NetworkWidget::new();
/// assert!(w.is_focused_on_ports());
/// ```
pub struct NetworkWidget {
    id: WidgetId,
    ports: PortsTableState,
    procs: ProcessesTableState,
    ifs: InterfacesSidebarState,
    filter: FilterInputState,
    kill_modal: KillConfirmModalState,
    focus: Focus,
    /// Toasts queued by kill-job completions, waiting to be drained by the
    /// host's render code. Populated by [`Self::on_kill_outcome`]; consumed
    /// by [`Self::take_toast`].
    pending_toasts: std::collections::VecDeque<KillToast>,
}

impl NetworkWidget {
    /// Create a new `NetworkWidget` with empty data and default focus on
    /// the ports pane.
    pub fn new() -> Self {
        Self {
            id: WidgetId::new("network.root"),
            ports: PortsTableState::new(),
            procs: ProcessesTableState::new(),
            ifs: InterfacesSidebarState::new(),
            filter: FilterInputState::new(),
            kill_modal: KillConfirmModalState::new(),
            focus: Focus::default(),
            pending_toasts: std::collections::VecDeque::new(),
        }
    }

    /// Feed a `KillOutcome` completed by the JobQueue back into the widget.
    /// Produces a toast and pushes it onto `pending_toasts`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::sys::Pid;
    /// use sid_core::sys_probe::kill_job::KillOutcome;
    /// use sid_widgets::network::ToastLevel;
    /// use sid_widgets::NetworkWidget;
    ///
    /// let mut w = NetworkWidget::new();
    /// w.on_kill_outcome(KillOutcome::Killed(Pid::from_u32(42)));
    /// let t = w.take_toast().unwrap();
    /// assert_eq!(t.level, ToastLevel::Success);
    /// assert!(t.message.contains("42"));
    /// ```
    pub fn on_kill_outcome(&mut self, outcome: KillOutcome) {
        let toast = match outcome {
            KillOutcome::Killed(pid) => KillToast {
                level: ToastLevel::Success,
                message: format!("killed PID {}", pid.as_u32()),
            },
            KillOutcome::EscalatedToSigkill(pid) => KillToast {
                level: ToastLevel::Warning,
                message: format!("PID {} ignored SIGTERM; SIGKILL sent", pid.as_u32()),
            },
            KillOutcome::Failed(pid, msg) => KillToast {
                level: ToastLevel::Error,
                message: format!("kill PID {} failed: {msg}", pid.as_u32()),
            },
        };
        self.pending_toasts.push_back(toast);
    }

    /// Pop the oldest queued toast, if any.
    pub fn take_toast(&mut self) -> Option<KillToast> {
        self.pending_toasts.pop_front()
    }

    /// Borrow the ports state. Tests use this to assert sort/select state
    /// after driving events.
    pub fn ports(&self) -> &PortsTableState {
        &self.ports
    }

    /// Borrow the processes state.
    pub fn processes(&self) -> &ProcessesTableState {
        &self.procs
    }

    /// Borrow the interfaces sidebar state.
    pub fn interfaces(&self) -> &InterfacesSidebarState {
        &self.ifs
    }

    /// Borrow the filter input state.
    pub fn filter(&self) -> &FilterInputState {
        &self.filter
    }

    /// Borrow the kill modal state.
    pub fn kill_modal(&self) -> &KillConfirmModalState {
        &self.kill_modal
    }

    /// Currently-focused pane.
    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// Currently-focused pane (parity with the other widgets'
    /// `focused_pane()` method).
    pub fn focused_pane(&self) -> NetFocus {
        self.focus
    }

    /// Stable string label for the focused pane.
    pub fn focused_pane_label(&self) -> &'static str {
        match self.focus {
            Focus::Ports => "Ports",
            Focus::Processes => "Processes",
            Focus::Interfaces => "Interfaces",
        }
    }

    /// True when focus is on the ports pane.
    pub fn is_focused_on_ports(&self) -> bool {
        self.focus == Focus::Ports
    }

    /// Replace the data displayed in all three panes from a fresh
    /// [`SysSnapshot`]. Selection is preserved by index (ports/procs) or by
    /// name (interfaces) according to each pane's `set_data` contract.
    pub fn apply_snapshot(&mut self, snap: sid_core::sys_probe::SysSnapshot) {
        self.ports.set_data(snap.listening_ports);
        self.procs.set_data(snap.processes);
        self.ifs.set_data(snap.interfaces);
    }

    /// Cycle focus forward (Tab).
    pub fn focus_next(&mut self) {
        self.focus = match self.focus {
            Focus::Ports => Focus::Processes,
            Focus::Processes => Focus::Interfaces,
            Focus::Interfaces => Focus::Ports,
        };
    }

    /// Cycle focus backward (Shift+Tab).
    pub fn focus_prev(&mut self) {
        self.focus = match self.focus {
            Focus::Ports => Focus::Interfaces,
            Focus::Processes => Focus::Ports,
            Focus::Interfaces => Focus::Processes,
        };
    }

    /// PID of the currently-selected row in the focused pane, if any.
    /// Used by the kill action to pick the target.
    pub fn focused_pid(&self) -> Option<Pid> {
        match self.focus {
            Focus::Ports => self.ports.selected_row().and_then(|r| r.pid),
            Focus::Processes => self.procs.selected_row().map(|r| r.pid),
            Focus::Interfaces => None,
        }
    }

    /// Cycle the sort column of the focused table (s key). Sidebar focus is
    /// a no-op since the interfaces pane is provider-sorted.
    pub fn cycle_sort(&mut self) {
        match self.focus {
            Focus::Ports => {
                let next = match self.ports.sort_by() {
                    None | Some(PortsSortBy::Protocol) => PortsSortBy::Port,
                    Some(PortsSortBy::Port) => PortsSortBy::Pid,
                    Some(PortsSortBy::Pid) => PortsSortBy::Command,
                    Some(PortsSortBy::Command) => PortsSortBy::Protocol,
                };
                let dir = self.ports.sort_dir();
                self.ports.set_sort(next, dir);
            }
            Focus::Processes => {
                let next = match self.procs.sort_by() {
                    None | Some(ProcessesSortBy::Started) => ProcessesSortBy::Pid,
                    Some(ProcessesSortBy::Pid) => ProcessesSortBy::Name,
                    Some(ProcessesSortBy::Name) => ProcessesSortBy::Cpu,
                    Some(ProcessesSortBy::Cpu) => ProcessesSortBy::Rss,
                    Some(ProcessesSortBy::Rss) => ProcessesSortBy::Started,
                };
                let dir = self.procs.sort_dir();
                self.procs.set_sort(next, dir);
            }
            Focus::Interfaces => {}
        }
    }

    /// Render the widget into a ratatui [`Frame`]. Used by the insta
    /// snapshot tests and by the future direct-frame plumbing.
    ///
    /// Layout:
    ///
    /// ```text
    /// ┌──────────────┬───────────────────────────────┐
    /// │ Interfaces   │  Listening ports              │
    /// │              ├───────────────────────────────┤
    /// │              │  Processes                    │
    /// └──────────────┴───────────────────────────────┘
    /// ```
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Min(0)])
            .split(area);
        let sidebar_rect = split[0];
        let right_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(45), Constraint::Min(0)])
            .split(split[1]);
        let ports_rect = right_split[0];
        let procs_rect = right_split[1];

        self.render_interfaces(frame, sidebar_rect, theme);
        self.render_ports(frame, ports_rect, theme);
        self.render_processes(frame, procs_rect, theme);

        // Filter banner (rendered below) consumes one row from the bottom.
        if self.filter.is_filtering() || self.filter.mode() == &FilterMode::Editing {
            let banner = Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(1),
                width: area.width,
                height: 1,
            };
            let label = format!(" / {}", self.filter.query());
            frame.render_widget(
                Paragraph::new(label).style(Style::default().fg(theme.accent_warning.into())),
                banner,
            );
        }

        // Kill modal overlay.
        if !self.kill_modal.is_closed() {
            self.render_kill_modal(frame, area, theme);
        }
    }

    fn render_interfaces(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let q = self.filter.query();
        let mut rows = Vec::with_capacity(self.ifs.rows().len());
        for (i, ifc) in self.ifs.rows().iter().enumerate() {
            if !q.is_empty() && !match_interface(q, ifc) {
                continue;
            }
            let glyph = if ifc.is_up { '*' } else { '_' };
            let marker = if i == self.ifs.selected_index() && self.focus == Focus::Interfaces {
                '>'
            } else {
                ' '
            };
            let label = format!("{marker} {glyph} {}", ifc.name);
            rows.push(Line::from(label));
        }
        let focused = self.focus == Focus::Interfaces;
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
            .title(" Interfaces ")
            .title_style(title_style);
        frame.render_widget(Paragraph::new(rows).block(block), rect);
    }

    fn render_ports(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let header = Row::new(["PORT", "PID", "PROTO", "COMMAND", "ADDR"]).style(
            Style::default()
                .fg(theme.muted.into())
                .add_modifier(Modifier::BOLD),
        );
        let q = self.filter.query();
        let body: Vec<Row> = self
            .ports
            .rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| q.is_empty() || match_listening_port(q, r))
            .map(|(i, r)| {
                let pid_s = r
                    .pid
                    .map(|p| p.as_u32().to_string())
                    .unwrap_or_else(|| "-".into());
                let proto = format!("{:?}", r.protocol).to_lowercase();
                let style = if i == self.ports.selected_index() && self.focus == Focus::Ports {
                    Style::default()
                        .fg(theme.background.into())
                        .bg(theme.accent_primary.into())
                } else {
                    Style::default().fg(theme.foreground.into())
                };
                Row::new(vec![
                    r.port.to_string(),
                    pid_s,
                    proto,
                    r.command.clone(),
                    r.local_addr.clone(),
                ])
                .style(style)
            })
            .collect();
        let focused = self.focus == Focus::Ports;
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
            .title(" Listening ports ")
            .title_style(title_style);
        let table = Table::new(
            body,
            [
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(6),
                Constraint::Min(10),
                Constraint::Length(18),
            ],
        )
        .header(header)
        .block(block);
        frame.render_widget(table, rect);
    }

    fn render_processes(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let header = Row::new(["PID", "NAME", "CPU%", "RSS", "USER"]).style(
            Style::default()
                .fg(theme.muted.into())
                .add_modifier(Modifier::BOLD),
        );
        let q = self.filter.query();
        let body: Vec<Row> = self
            .procs
            .rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| q.is_empty() || match_process(q, r))
            .map(|(i, r)| {
                let cpu = format!("{:.1}", r.cpu_pct);
                let rss = format_bytes(r.rss_bytes);
                let user = r.user.clone().unwrap_or_else(|| "-".into());
                let style = if i == self.procs.selected_index() && self.focus == Focus::Processes {
                    Style::default()
                        .fg(theme.background.into())
                        .bg(theme.accent_primary.into())
                } else {
                    Style::default().fg(theme.foreground.into())
                };
                Row::new(vec![
                    r.pid.as_u32().to_string(),
                    r.name.clone(),
                    cpu,
                    rss,
                    user,
                ])
                .style(style)
            })
            .collect();
        let focused = self.focus == Focus::Processes;
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
            .title(" Processes ")
            .title_style(title_style);
        let table = Table::new(
            body,
            [
                Constraint::Length(7),
                Constraint::Min(10),
                Constraint::Length(6),
                Constraint::Length(8),
                Constraint::Length(8),
            ],
        )
        .header(header)
        .block(block);
        frame.render_widget(table, rect);
    }

    fn render_kill_modal(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let pid = self
            .kill_modal
            .target_pid()
            .map(|p| p.as_u32())
            .unwrap_or(0);
        let line = if self.kill_modal.is_confirm_sigterm() {
            format!("Kill PID {pid}? Send SIGTERM (y/n)")
        } else if self.kill_modal.is_awaiting_term() {
            format!("SIGTERM sent to PID {pid}; waiting for exit...")
        } else if self.kill_modal.is_confirm_sigkill() {
            format!("PID {pid} did not exit. Send SIGKILL? (y/n)")
        } else if self.kill_modal.is_done() {
            format!("kill PID {pid}: {:?}", self.kill_modal.result())
        } else {
            String::new()
        };
        let w = area.width.min(60);
        let h = 3u16.min(area.height);
        let modal = Rect {
            x: area.x + area.width.saturating_sub(w) / 2,
            y: area.y + area.height.saturating_sub(h) / 2,
            width: w,
            height: h,
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent_primary.into()))
            .title(" kill ")
            .title_style(Style::default().fg(theme.foreground.into()));
        frame.render_widget(
            Paragraph::new(Line::from(Span::raw(line))).block(block),
            modal,
        );
    }
}

impl Default for NetworkWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for NetworkWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }

    fn title(&self) -> &str {
        "Network"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("K", "kill"),
            FooterHint::new("/", "filter"),
            FooterHint::new("R", "refresh"),
            FooterHint::new("Tab", "pane"),
        ]
    }

    fn render(&self, _target: &mut dyn RenderTarget) {
        // Text-mode rendering goes through the binary's draw fn for now.
        // The ratatui-aware `render_into_frame` is the canonical path; the
        // wire.rs body keeps using its own summary text in the meantime.
    }

    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        let Event::Key(chord) = ev else {
            return EventOutcome::Bubble;
        };

        // Filter editing has priority — every keystroke goes into the query.
        if self.filter.mode() == &FilterMode::Editing {
            match chord.code {
                KeyCode::Esc => {
                    self.filter.cancel();
                    return EventOutcome::Consumed;
                }
                KeyCode::Enter => {
                    self.filter.submit();
                    return EventOutcome::Consumed;
                }
                KeyCode::Backspace => {
                    self.filter.pop_char();
                    return EventOutcome::Consumed;
                }
                KeyCode::Char(c) => {
                    self.filter.push_char(c);
                    return EventOutcome::Consumed;
                }
                _ => return EventOutcome::Bubble,
            }
        }

        // Kill modal also has priority over normal navigation.
        if !self.kill_modal.is_closed() {
            match chord.code {
                KeyCode::Esc => {
                    self.kill_modal.close();
                    return EventOutcome::Consumed;
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.kill_modal.confirm();
                    return EventOutcome::Consumed;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.kill_modal.decline();
                    return EventOutcome::Consumed;
                }
                KeyCode::Enter if self.kill_modal.is_done() => {
                    self.kill_modal.acknowledge();
                    return EventOutcome::Consumed;
                }
                _ => return EventOutcome::Bubble,
            }
        }

        // Tab / Shift+Tab cycle the focused pane FIRST.
        match chord.code {
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
        // Alt+<key> is reserved for future cross-pane actions.
        if chord.mods.contains(KeyModifiers::ALT) {
            // TODO: cross-pane actions on Alt+<key>
            return EventOutcome::Bubble;
        }
        match chord.code {
            KeyCode::Char('/') => {
                self.filter.enter_filter();
                EventOutcome::Consumed
            }
            KeyCode::Char('s') => {
                self.cycle_sort();
                EventOutcome::Consumed
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.selection_next();
                EventOutcome::Consumed
            }
            // Capital `K` (Shift+k) opens the kill modal; lowercase `k` is
            // always vim-style "up" navigation. This was previously
            // overloaded — lowercase k on Ports/Processes opened the modal,
            // which contradicted the j/k navigation convention and surprised
            // users. Capital K is now the only kill chord on Network, and
            // the footer hint `[ K: kill ]` matches that.
            KeyCode::Char('K') if matches!(self.focus, Focus::Ports | Focus::Processes) => {
                if let Some(pid) = self.focused_pid() {
                    self.kill_modal.open(pid);
                }
                EventOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selection_prev();
                EventOutcome::Consumed
            }
            _ => EventOutcome::Bubble,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let prefs = PersistedPrefs {
            focus: self.focus,
            ports_sort: self.ports.sort_by().map(|s| (s, self.ports.sort_dir())),
            procs_sort: self.procs.sort_by().map(|s| (s, self.procs.sort_dir())),
        };
        let body = postcard::to_allocvec(&prefs).unwrap_or_default();
        let mut out = Vec::with_capacity(body.len() + 1);
        out.push(PERSIST_VERSION);
        out.extend_from_slice(&body);
        out
    }

    fn load_state(&mut self, bytes: &[u8]) {
        let Some((&version, body)) = bytes.split_first() else {
            return;
        };
        if version != PERSIST_VERSION {
            return;
        }
        let Ok(prefs) = postcard::from_bytes::<PersistedPrefs>(body) else {
            return;
        };
        self.focus = prefs.focus;
        if let Some((by, dir)) = prefs.ports_sort {
            self.ports.set_sort(by, dir);
        }
        if let Some((by, dir)) = prefs.procs_sort {
            self.procs.set_sort(by, dir);
        }
    }
}

impl NetworkWidget {
    fn selection_next(&mut self) {
        match self.focus {
            Focus::Ports => self.ports.select_next(),
            Focus::Processes => self.procs.select_next(),
            Focus::Interfaces => self.ifs.select_next(),
        }
    }
    fn selection_prev(&mut self) {
        match self.focus {
            Focus::Ports => self.ports.select_prev(),
            Focus::Processes => self.procs.select_prev(),
            Focus::Interfaces => self.ifs.select_prev(),
        }
    }
}

/// Format a byte count as a short human string (KB / MB / GB).
fn format_bytes(b: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = KB * 1_024;
    const GB: u64 = MB * 1_024;
    if b >= GB {
        format!("{:.1}G", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1}M", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1}K", b as f64 / KB as f64)
    } else {
        format!("{b}B")
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
/// use sid_widgets::network::render_to_string;
/// use sid_widgets::NetworkWidget;
/// let w = NetworkWidget::new();
/// let s = render_to_string(&w, 80, 24);
/// assert!(s.contains("Interfaces"));
/// ```
pub fn render_to_string(widget: &NetworkWidget, width: u16, height: u16) -> String {
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

    use super::NetworkWidget;

    #[test]
    fn id_and_title_correct() {
        let w = NetworkWidget::new();
        assert_eq!(w.id().as_str(), "network.root");
        assert_eq!(w.title(), "Network");
    }

    #[test]
    fn save_state_round_trips_focus() {
        let mut w = NetworkWidget::new();
        w.focus_next(); // Processes
        let bytes = w.save_state();
        let mut w2 = NetworkWidget::new();
        w2.load_state(&bytes);
        assert_eq!(w2.focus(), w.focus());
    }

    #[test]
    fn load_state_bad_version_is_noop() {
        let mut w = NetworkWidget::new();
        let initial = w.focus();
        // Version byte 0xFF unknown.
        w.load_state(&[0xFF, 0x00, 0x01, 0x02]);
        assert_eq!(w.focus(), initial);
    }

    #[test]
    fn load_state_empty_is_noop() {
        let mut w = NetworkWidget::new();
        let initial = w.focus();
        w.load_state(&[]);
        assert_eq!(w.focus(), initial);
    }
}
