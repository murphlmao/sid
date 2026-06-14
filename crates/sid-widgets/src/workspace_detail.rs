//! Workspace detail tab — multi-repo dashboard.
//!
//! Opened when the user presses Enter on a workspace in the Workspaces
//! overview. Renders every git repo discovered one level deep under the
//! workspace path as a row in a six-column table; the highlighted row's
//! sub-pane drill-in is a placeholder in v1 (the full Branches / Status /
//! Log / Diff / Commit / Actions refactor is a follow-up).

use std::{path::PathBuf, sync::Arc};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use sid_core::{
    adapters::git::GitProvider,
    context::WidgetCtx,
    event::Event,
    widget::{EventOutcome, FooterHint, RenderTarget, Widget, WidgetId},
};
use sid_store::Workspace;
use sid_ui::Theme;

use crate::{
    list_cursor::{CursorTarget, ListCursor},
    split_view::{SplitFocus, SplitView},
    workspace_detail_state::{DetailOp, DetailView, RepoDetail, RepoGit, SatelliteRow},
};

/// CI status badge for a sub-repo. v1 always reports `Unknown` — wiring a
/// real `gh run list` fetcher is tracked in the future-features backlog.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail::CiStatus;
/// assert_eq!(CiStatus::Unknown.glyph(), "-");
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CiStatus {
    /// CI run is in progress.
    Pending,
    /// Most recent run succeeded.
    Pass,
    /// Most recent run failed.
    Fail,
    /// No information (v1 default; populated by the future CI adapter).
    Unknown,
}

impl CiStatus {
    /// Single-character glyph used in the CI column.
    pub fn glyph(self) -> &'static str {
        match self {
            CiStatus::Pending => "*",
            CiStatus::Pass => "✓",
            CiStatus::Fail => "x",
            CiStatus::Unknown => "-",
        }
    }
}

/// Per-sub-repo summary rendered as one row in the dashboard table.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_widgets::workspace_detail::{CiStatus, RepoSummary};
///
/// let r = RepoSummary {
///     path: PathBuf::from("/vcs/x/y"),
///     name: "y".into(),
///     branch: "main".into(),
///     ahead: 0,
///     behind: 0,
///     dirty: 0,
///     last_commit_age_secs: 60,
///     ci_status: CiStatus::Unknown,
/// };
/// assert_eq!(r.name, "y");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoSummary {
    /// Absolute path to the sub-repo's `.git` parent directory.
    pub path: PathBuf,
    /// Display name (typically the directory's basename).
    pub name: String,
    /// Current branch name. Empty or `"?"` if not yet determined.
    pub branch: String,
    /// Commits ahead of upstream.
    pub ahead: u32,
    /// Commits behind upstream.
    pub behind: u32,
    /// Number of files with uncommitted changes.
    pub dirty: u32,
    /// Seconds since the most recent commit.
    pub last_commit_age_secs: u64,
    /// CI status badge.
    pub ci_status: CiStatus,
}

/// Tab widget for the Workspaces detail view (UX-v2 rework).
///
/// Owns the umbrella workspace, the row list (umbrella + satellites), a list
/// cursor, the right-pane drill-in `SplitView`, an inner list cursor for the
/// active pane list, the loaded `RepoDetail`, and a diff scroll offset. Git
/// data is loaded off-thread by the binary and pushed in via the `apply_*`
/// setters; this type never names `git2`.
pub struct WorkspaceDetailWidget {
    id: WidgetId,
    workspace: Workspace,
    rows: Vec<SatelliteRow>,
    list: ListCursor,
    split: SplitView<DetailView>,
    /// Cursor over the active pane list (commits or branches).
    pane_list: ListCursor,
    /// Loaded detail for the currently-selected row.
    detail: RepoDetail,
    /// Scroll offset within the diff view.
    diff_scroll: usize,
    #[allow(dead_code)] // The binary opens providers itself; kept for symmetry.
    git_factory: Option<Arc<dyn GitProvider>>,
    /// True until the satellite scan lands.
    scanning: bool,
}

impl WorkspaceDetailWidget {
    /// Construct with the umbrella workspace. The list seeds with the single
    /// umbrella row (`is_umbrella = true`); satellites arrive via
    /// [`Self::apply_satellites`]. The right pane starts on the ops menu with
    /// list focus.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_core::workspace_metadata::WorkspaceKind;
    /// use sid_store::Workspace;
    /// use sid_widgets::workspace_detail::WorkspaceDetailWidget;
    ///
    /// let ws = Workspace {
    ///     path: PathBuf::from("/stack"),
    ///     name: "gen4-stack".into(),
    ///     kind: WorkspaceKind::Umbrella,
    ///     manifest_hash: 0,
    ///     last_seen: 0,
    ///     parent: None,
    /// };
    /// let w = WorkspaceDetailWidget::new(ws, None);
    /// assert!(w.is_scanning());
    /// assert_eq!(w.rows().len(), 1);
    /// ```
    pub fn new(workspace: Workspace, git_factory: Option<Arc<dyn GitProvider>>) -> Self {
        let tab_id = format!("workspace_detail:{}", workspace.path.display());
        let umbrella_row = SatelliteRow {
            name: workspace.name.clone(),
            path: workspace.path.clone(),
            is_umbrella: true,
            git: RepoGit::loading(),
        };
        Self {
            id: WidgetId::new(tab_id),
            workspace,
            rows: vec![umbrella_row],
            list: ListCursor::new(1, false, 0),
            split: SplitView::default(),
            pane_list: ListCursor::new(0, false, 0),
            detail: RepoDetail::default(),
            diff_scroll: 0,
            git_factory,
            scanning: true,
        }
    }

    /// Append satellites after the umbrella row and clear the scanning flag.
    /// Re-clamps the list cursor.
    pub fn apply_satellites(&mut self, satellites: Vec<SatelliteRow>) {
        self.rows.truncate(1); // keep the umbrella row only
        self.rows.extend(satellites);
        self.scanning = false;
        self.list = ListCursor::new(self.rows.len(), false, self.list.pos);
    }

    /// Replace one row's git snapshot, matched by path. No-op if no row matches.
    pub fn apply_row_git(&mut self, path: &std::path::Path, git: RepoGit) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.path == path) {
            row.git = git;
        }
    }

    /// Replace the loaded detail for the selected row and reset the pane cursor
    /// + diff scroll. Sizes the pane cursor to whichever list the active op shows.
    pub fn apply_detail(&mut self, detail: RepoDetail) {
        self.detail = detail;
        self.diff_scroll = 0;
        let len = self.active_pane_len();
        self.pane_list = ListCursor::new(len, false, 0);
    }

    /// Total number of patch lines across all diff entries; used to clamp scroll.
    fn total_patch_lines(&self) -> usize {
        self.detail
            .diff
            .iter()
            .map(|e| e.patch.lines().count())
            .sum()
    }

    /// Number of items in the currently-shown pane list.
    fn active_pane_len(&self) -> usize {
        match self.split.top() {
            Some(DetailView::Op(DetailOp::Branches)) => self.detail.branches.len(),
            Some(DetailView::Op(DetailOp::Outgoing | DetailOp::Log)) => self.detail.commits.len(),
            _ => 0,
        }
    }

    /// Whether the satellite scan is still running.
    pub fn is_scanning(&self) -> bool {
        self.scanning
    }

    /// The row list (umbrella first, then satellites).
    pub fn rows(&self) -> &[SatelliteRow] {
        &self.rows
    }

    /// The umbrella workspace this detail tab represents.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The drill-in split state (focus + view stack).
    pub fn split(&self) -> &SplitView<DetailView> {
        &self.split
    }

    /// The currently-selected row, if any.
    pub fn selected_row(&self) -> Option<&SatelliteRow> {
        match self.list.target() {
            CursorTarget::Item(i) => self.rows.get(i),
            _ => None,
        }
    }

    /// Index into `detail.commits` the pane cursor points at (Outgoing/Log).
    pub fn selected_commit_index(&self) -> Option<usize> {
        match (self.split.top(), self.pane_list.target()) {
            (Some(DetailView::Op(DetailOp::Outgoing | DetailOp::Log)), CursorTarget::Item(i)) => {
                Some(i)
            }
            _ => None,
        }
    }

    /// Diff scroll offset.
    pub fn diff_scroll(&self) -> usize {
        self.diff_scroll
    }

    /// Move the list selection down; re-root the right pane (selecting a row
    /// resets the drill-in to that row's ops menu, list-focused).
    pub fn select_next(&mut self) {
        self.list.down();
        self.split.reset();
    }

    /// Move the list selection up; re-root the right pane.
    pub fn select_prev(&mut self) {
        self.list.up();
        self.split.reset();
    }

    /// Push an op view onto the stack (focuses the pane).
    pub fn enter_op(&mut self, op: DetailOp) {
        self.split.push(DetailView::Op(op));
        self.pane_list = ListCursor::new(self.active_pane_len(), false, 0);
        self.diff_scroll = 0;
    }

    /// From an Outgoing/Log commit list, drill into the selected commit's diff.
    pub fn drill_into_commit(&mut self) {
        if let Some(i) = self.selected_commit_index() {
            self.split.push(DetailView::Diff(i));
            self.diff_scroll = 0;
        }
    }

    /// Pop one drill-in level; when the stack empties, focus returns to the list.
    pub fn pop_view(&mut self) {
        self.split.pop();
        let preserved_pos = self.pane_list.pos;
        let len = self.active_pane_len();
        // Clamp the preserved position to the new list length, like apply_satellites does.
        let clamped = if len == 0 {
            0
        } else {
            preserved_pos.min(len - 1)
        };
        self.pane_list = ListCursor::new(len, false, clamped);
        self.diff_scroll = 0;
    }

    /// Move the active pane list cursor down.
    pub fn pane_next(&mut self) {
        self.pane_list.down();
    }

    /// Move the active pane list cursor up.
    pub fn pane_prev(&mut self) {
        self.pane_list.up();
    }

    /// Scroll the diff view down one line.
    pub fn diff_scroll_down(&mut self) {
        let max = self.total_patch_lines().saturating_sub(1);
        self.diff_scroll = self.diff_scroll.saturating_add(1).min(max);
    }

    /// Scroll the diff view up one line.
    pub fn diff_scroll_up(&mut self) {
        self.diff_scroll = self.diff_scroll.saturating_sub(1);
    }

    /// Borrow the loaded detail (for the renderer).
    pub fn detail(&self) -> &RepoDetail {
        &self.detail
    }

    /// Pane cursor (for the renderer to highlight the selected list row).
    pub fn pane_cursor(&self) -> &ListCursor {
        &self.pane_list
    }

    /// Compute the Paragraph scroll offset to keep the selected row inside
    /// the visible window. `inner_height` is the number of visible rows
    /// (block inner height = area height − 2 for the border).
    fn list_scroll_offset(&self, inner_height: u16) -> u16 {
        let sel = match self.list.target() {
            CursorTarget::Item(i) => i,
            _ => return 0,
        };
        let h = inner_height as usize;
        if sel < h { 0 } else { (sel - h + 1) as u16 }
    }

    /// Scroll offset to keep the pane cursor in view.
    fn pane_scroll_offset(&self, inner_height: u16) -> u16 {
        let sel = match self.pane_list.target() {
            CursorTarget::Item(i) => i,
            _ => return 0,
        };
        let h = inner_height as usize;
        if sel < h { 0 } else { (sel - h + 1) as u16 }
    }

    /// Draw the detail tab: umbrella header row, then a 40/60 list/pane split.
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);
        self.render_header(frame, rows[0], theme);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(rows[1]);
        self.render_list(frame, cols[0], theme);
        self.render_pane(frame, cols[1], theme);
    }

    fn render_header(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let umbrella_git = self
            .rows
            .first()
            .map(|r| r.git.header_summary())
            .unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", self.workspace.name),
                Style::default()
                    .fg(theme.background.into())
                    .bg(theme.accent_primary.into())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(umbrella_git, Style::default().fg(theme.foreground.into())),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_list(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let focused = self.split.focus() == SplitFocus::List;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Repos ");
        if self.scanning && self.rows.len() <= 1 {
            let body = Paragraph::new(Line::from(Span::styled(
                "  scanning for satellites…",
                Style::default().fg(theme.muted.into()),
            )))
            .block(block);
            frame.render_widget(body, area);
            return;
        }
        let sel = match self.list.target() {
            CursorTarget::Item(i) => Some(i),
            _ => None,
        };
        let lines: Vec<Line<'_>> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let glyph = if r.is_umbrella { '▾' } else { '·' };
                let marker = if Some(i) == sel { '>' } else { ' ' };
                let label = format!("{marker} {glyph} {}  {}", r.name, r.git.header_summary());
                let style = if Some(i) == sel {
                    Style::default()
                        .fg(theme.background.into())
                        .bg(theme.accent_primary.into())
                } else {
                    Style::default().fg(theme.foreground.into())
                };
                Line::from(Span::styled(label, style))
            })
            .collect();
        let scroll = self.list_scroll_offset(area.height.saturating_sub(2));
        frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
    }

    fn render_pane(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let focused = self.split.focus() == SplitFocus::Pane;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let title = match self.split.top() {
            None => " Ops ".to_string(),
            Some(DetailView::Op(op)) => format!(" {} ", op.label()),
            Some(DetailView::Diff(_)) => " Diff ".to_string(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(title);
        let body: Vec<Line<'_>> = match self.split.top() {
            None => DetailOp::ALL
                .iter()
                .map(|op| {
                    Line::from(Span::styled(
                        format!("  {}", op.label()),
                        Style::default().fg(theme.foreground.into()),
                    ))
                })
                .collect(),
            Some(DetailView::Op(DetailOp::Outgoing | DetailOp::Log)) => {
                self.render_commit_lines(theme)
            }
            Some(DetailView::Op(DetailOp::Branches)) => self.render_branch_lines(theme),
            Some(DetailView::Op(DetailOp::Stash)) => vec![Line::from(Span::styled(
                "  (no stash entries)",
                Style::default().fg(theme.muted.into()),
            ))],
            Some(DetailView::Op(DetailOp::Worktrees)) => vec![Line::from(Span::styled(
                "  (no linked worktrees)",
                Style::default().fg(theme.muted.into()),
            ))],
            Some(DetailView::Diff(idx)) => self.render_diff_lines(*idx, theme),
        };
        let pane_scroll = self.pane_scroll_offset(area.height.saturating_sub(2));
        frame.render_widget(
            Paragraph::new(body).block(block).scroll((pane_scroll, 0)),
            area,
        );
    }

    fn render_commit_lines(&self, theme: &Theme) -> Vec<Line<'_>> {
        if self.detail.commits.is_empty() {
            return vec![Line::from(Span::styled(
                "  (no commits)",
                Style::default().fg(theme.muted.into()),
            ))];
        }
        let sel = match self.pane_list.target() {
            CursorTarget::Item(i) => Some(i),
            _ => None,
        };
        self.detail
            .commits
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let short: String = c.oid.chars().take(8).collect();
                let marker = if Some(i) == sel { '>' } else { ' ' };
                Line::from(vec![
                    Span::styled(
                        format!("{marker} {short}"),
                        Style::default().fg(theme.accent_warning.into()),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        c.summary.clone(),
                        Style::default().fg(theme.foreground.into()),
                    ),
                ])
            })
            .collect()
    }

    fn render_branch_lines(&self, theme: &Theme) -> Vec<Line<'_>> {
        if self.detail.branches.is_empty() {
            return vec![Line::from(Span::styled(
                "  (no branches loaded)",
                Style::default().fg(theme.muted.into()),
            ))];
        }
        let sel = match self.pane_list.target() {
            CursorTarget::Item(i) => Some(i),
            _ => None,
        };
        self.detail
            .branches
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let glyph = if b.is_current { '●' } else { '○' };
                let marker = if Some(i) == sel { '>' } else { ' ' };
                Line::from(Span::styled(
                    format!("{marker} {glyph} {}", b.name),
                    Style::default().fg(theme.foreground.into()),
                ))
            })
            .collect()
    }

    fn render_diff_lines(&self, idx: usize, theme: &Theme) -> Vec<Line<'_>> {
        // The binary loads diff entries for the drilled commit into detail.diff.
        const MAX: usize = 200;
        let _ = idx; // diff is per-commit; the binary fills detail.diff for the drilled commit
        if self.detail.diff.is_empty() {
            return vec![Line::from(Span::styled(
                "  (no diff loaded)",
                Style::default().fg(theme.muted.into()),
            ))];
        }
        self.detail
            .diff
            .iter()
            .flat_map(|e| e.patch.lines())
            .skip(self.diff_scroll)
            .take(MAX)
            .map(|l| Line::from(Span::raw(l.to_string())))
            .collect()
    }
}

impl Widget for WorkspaceDetailWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }

    fn title(&self) -> &str {
        &self.workspace.name
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn render(&self, _: &mut dyn RenderTarget) {}

    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("j/k", "select"),
            FooterHint::new("→/⏎", "drill in"),
            FooterHint::new("←", "back"),
            FooterHint::new("Ctrl+W", "close"),
        ]
    }

    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        let Event::Key(chord) = ev else {
            return EventOutcome::Bubble;
        };
        match self.split.focus() {
            SplitFocus::List => match (chord.code, chord.mods) {
                (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                    self.select_next();
                    EventOutcome::Consumed
                }
                (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                    self.select_prev();
                    EventOutcome::Consumed
                }
                (KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter, KeyModifiers::NONE) => {
                    // Enter the ops menu: push the first op (Outgoing).
                    self.enter_op(DetailOp::Outgoing);
                    EventOutcome::Consumed
                }
                _ => EventOutcome::Bubble,
            },
            SplitFocus::Pane => match (chord.code, chord.mods) {
                (KeyCode::Char('h') | KeyCode::Left, KeyModifiers::NONE) => {
                    self.pop_view();
                    EventOutcome::Consumed
                }
                (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                    if matches!(self.split.top(), Some(DetailView::Diff(_))) {
                        self.diff_scroll_down();
                    } else {
                        self.pane_next();
                    }
                    EventOutcome::Consumed
                }
                (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                    if matches!(self.split.top(), Some(DetailView::Diff(_))) {
                        self.diff_scroll_up();
                    } else {
                        self.pane_prev();
                    }
                    EventOutcome::Consumed
                }
                (KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter, KeyModifiers::NONE) => {
                    // From a commit list, drill into the diff.
                    if matches!(
                        self.split.top(),
                        Some(DetailView::Op(DetailOp::Outgoing | DetailOp::Log))
                    ) {
                        self.drill_into_commit();
                    }
                    EventOutcome::Consumed
                }
                (KeyCode::Tab, _) => EventOutcome::Consumed,
                _ => EventOutcome::Bubble,
            },
        }
    }
}

/// Format a duration in seconds as a short human string (`5m`, `2h`, `3d`).
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail::format_age;
/// assert_eq!(format_age(30), "30s");
/// assert_eq!(format_age(120), "2m");
/// assert_eq!(format_age(7200), "2h");
/// assert_eq!(format_age(259_200), "3d");
/// ```
pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Render the widget into a fresh test buffer of `(width, height)` using
/// the cosmos theme. Used by tests + snapshots.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_core::workspace_metadata::WorkspaceKind;
/// use sid_store::Workspace;
/// use sid_widgets::workspace_detail::{render_to_string, WorkspaceDetailWidget};
///
/// let ws = Workspace {
///     path: PathBuf::from("/vcs/x"),
///     name: "x".into(),
///     kind: WorkspaceKind::Umbrella,
///     manifest_hash: 0,
///     last_seen: 0,
///     parent: None,
/// };
/// let w = WorkspaceDetailWidget::new(ws, None);
/// let s = render_to_string(&w, 100, 30);
/// assert!(s.contains("scanning"));
/// ```
pub fn render_to_string(widget: &WorkspaceDetailWidget, width: u16, height: u16) -> String {
    use ratatui::{Terminal, backend::TestBackend};
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

#[cfg(test)]
mod tests {
    use sid_core::{adapters::git::CommitInfo, workspace_metadata::WorkspaceKind};
    use sid_store::Workspace;

    use super::*;
    use crate::split_view::SplitFocus;

    fn umbrella() -> Workspace {
        Workspace {
            path: std::path::PathBuf::from("/stack"),
            name: "gen4-stack".into(),
            kind: WorkspaceKind::Umbrella,
            manifest_hash: 0,
            last_seen: 0,
            parent: None,
        }
    }

    #[test]
    fn new_seeds_umbrella_row_and_is_scanning() {
        let w = WorkspaceDetailWidget::new(umbrella(), None);
        assert!(w.is_scanning());
        // exactly the umbrella row until satellites land
        assert_eq!(w.rows().len(), 1);
        assert!(w.rows()[0].is_umbrella);
        assert_eq!(w.rows()[0].name, "gen4-stack");
    }

    #[test]
    fn apply_satellites_appends_after_umbrella_row() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_satellites(vec![
            SatelliteRow {
                name: "api".into(),
                path: "/stack/api".into(),
                is_umbrella: false,
                git: RepoGit::loading(),
            },
            SatelliteRow {
                name: "web".into(),
                path: "/stack/web".into(),
                is_umbrella: false,
                git: RepoGit::loading(),
            },
        ]);
        assert!(!w.is_scanning());
        assert_eq!(w.rows().len(), 3);
        assert!(w.rows()[0].is_umbrella);
        assert_eq!(w.rows()[1].name, "api");
    }

    #[test]
    fn apply_row_git_updates_matching_path_only() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_satellites(vec![SatelliteRow {
            name: "api".into(),
            path: "/stack/api".into(),
            is_umbrella: false,
            git: RepoGit::loading(),
        }]);
        w.apply_row_git(
            std::path::Path::new("/stack/api"),
            RepoGit::loaded("main".into(), 2, 1, 0),
        );
        let api = w.rows().iter().find(|r| r.name == "api").unwrap();
        assert!(!api.git.is_loading());
        assert_eq!(api.git.outgoing, 1);
        // unknown path is a no-op (no panic)
        w.apply_row_git(
            std::path::Path::new("/nope"),
            RepoGit::loaded("x".into(), 0, 0, 0),
        );
    }

    #[test]
    fn list_navigation_wraps_via_cursor() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_satellites(vec![SatelliteRow {
            name: "api".into(),
            path: "/stack/api".into(),
            is_umbrella: false,
            git: RepoGit::loading(),
        }]);
        assert_eq!(w.selected_row().unwrap().name, "gen4-stack");
        w.select_next();
        assert_eq!(w.selected_row().unwrap().name, "api");
        w.select_next(); // saturates at bottom (ListCursor::down does not wrap)
        assert_eq!(w.selected_row().unwrap().name, "api");
        w.select_prev();
        assert_eq!(w.selected_row().unwrap().name, "gen4-stack");
    }

    #[test]
    fn enter_op_drills_into_pane_and_left_pops_back_to_list() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        // start on the ops menu, focus list
        assert_eq!(w.split().focus(), SplitFocus::List);
        w.enter_op(DetailOp::Outgoing); // push Op(Outgoing)
        assert_eq!(w.split().focus(), SplitFocus::Pane);
        assert_eq!(w.split().top(), Some(&DetailView::Op(DetailOp::Outgoing)));
        // drill into a commit's diff
        w.apply_detail(RepoDetail {
            commits: vec![CommitInfo {
                oid: "abc".into(),
                summary: "s".into(),
                author_name: "a".into(),
                author_email: "a@b".into(),
                timestamp_secs: 0,
                parents: vec![],
            }],
            ..RepoDetail::default()
        });
        w.drill_into_commit();
        assert_eq!(w.split().top(), Some(&DetailView::Diff(0)));
        w.pop_view(); // back to Op(Outgoing)
        assert_eq!(w.split().top(), Some(&DetailView::Op(DetailOp::Outgoing)));
        w.pop_view(); // back to list
        assert_eq!(w.split().focus(), SplitFocus::List);
    }

    #[test]
    fn selecting_a_new_row_resets_the_drill_in() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_satellites(vec![SatelliteRow {
            name: "api".into(),
            path: "/stack/api".into(),
            is_umbrella: false,
            git: RepoGit::loading(),
        }]);
        w.enter_op(DetailOp::Branches);
        assert_eq!(w.split().focus(), SplitFocus::Pane);
        w.select_next(); // moving the list selection re-roots the right pane
        assert_eq!(w.split().focus(), SplitFocus::List);
        assert_eq!(w.split().depth(), 0);
    }

    fn loaded_widget() -> WorkspaceDetailWidget {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_row_git(
            std::path::Path::new("/stack"),
            RepoGit::loaded("main".into(), 2, 3, 0),
        );
        w.apply_satellites(vec![
            SatelliteRow {
                name: "api".into(),
                path: "/stack/api".into(),
                is_umbrella: false,
                git: RepoGit::loaded("main".into(), 0, 0, 0),
            },
            SatelliteRow {
                name: "web".into(),
                path: "/stack/web".into(),
                is_umbrella: false,
                git: RepoGit::loaded("feat/x".into(), 5, 0, 1),
            },
        ]);
        w
    }

    #[test]
    fn snapshot_detail_list_and_ops_menu() {
        let w = loaded_widget();
        let s = render_to_string(&w, 100, 24);
        insta::assert_snapshot!("detail_list_and_ops_menu", s);
    }

    #[test]
    fn snapshot_detail_outgoing_commits() {
        let mut w = loaded_widget();
        w.enter_op(DetailOp::Outgoing);
        w.apply_detail(RepoDetail {
            commits: vec![
                CommitInfo {
                    oid: "deadbeef0".into(),
                    summary: "feat: thing".into(),
                    author_name: "a".into(),
                    author_email: "a@b".into(),
                    timestamp_secs: 0,
                    parents: vec![],
                },
                CommitInfo {
                    oid: "cafebabe1".into(),
                    summary: "fix: bug".into(),
                    author_name: "a".into(),
                    author_email: "a@b".into(),
                    timestamp_secs: 0,
                    parents: vec![],
                },
            ],
            ..RepoDetail::default()
        });
        let s = render_to_string(&w, 100, 24);
        insta::assert_snapshot!("detail_outgoing_commits", s);
    }

    #[test]
    fn header_shows_umbrella_git_summary() {
        let w = loaded_widget();
        let s = render_to_string(&w, 100, 24);
        // umbrella header carries branch · dirty · outgoing
        assert!(s.contains("main"));
        assert!(s.contains("↑3"));
    }

    #[test]
    fn handle_enter_on_ops_menu_drills_in() {
        use sid_core::{context::WidgetCtx, event::Event};
        let mut w = loaded_widget();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let ev = Event::Key(sid_core::event::KeyChord {
            code: crossterm::event::KeyCode::Enter,
            mods: crossterm::event::KeyModifiers::NONE,
        });
        let _ = w.handle_event(&ev, &mut ctx);
        // Enter on the ops menu (default selection 0 = Outgoing) drills in
        assert_eq!(w.split().focus(), SplitFocus::Pane);
    }
}
