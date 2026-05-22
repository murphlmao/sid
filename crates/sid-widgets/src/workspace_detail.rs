//! Workspace detail tab — multi-repo dashboard.
//!
//! Opened when the user presses Enter on a workspace in the Workspaces
//! overview. Renders every git repo discovered one level deep under the
//! workspace path as a row in a six-column table; the highlighted row's
//! sub-pane drill-in is a placeholder in v1 (the full Branches / Status /
//! Log / Diff / Commit / Actions refactor is a follow-up).

use std::path::PathBuf;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use sid_core::adapters::git::GitProvider;
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, FooterHint, RenderTarget, Widget, WidgetId};
use sid_store::Workspace;
use sid_ui::Theme;

use crate::workspaces::RightPane;

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

/// Tab widget for the workspace detail view. Owns a workspace clone, a
/// list of sub-repo summaries populated by the binary off-thread, a
/// selection cursor, and a `RightPane` for drill-in.
pub struct WorkspaceDetailWidget {
    id: WidgetId,
    workspace: Workspace,
    sub_repos: Vec<RepoSummary>,
    selected: usize,
    right_pane: RightPane,
    #[allow(dead_code)] // Used by a follow-up that wires the real per-sub-repo drill-in.
    git_factory: Option<Arc<dyn GitProvider>>,
    /// True while the scan job is still running. Renderer shows a
    /// "scanning…" hint until results land.
    scanning: bool,
}

impl WorkspaceDetailWidget {
    /// Construct with a workspace and the git factory (cloned from the
    /// overview widget so all detail tabs share the same provider).
    /// Sub-repos start empty + `scanning = true`; the binary calls
    /// [`Self::apply_scan_results`] once the off-thread scan completes.
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
    ///     path: PathBuf::from("/vcs/x"),
    ///     name: "x".into(),
    ///     kind: WorkspaceKind::Umbrella,
    ///     manifest_hash: 0,
    ///     last_seen: 0,
    ///     parent: None,
    /// };
    /// let w = WorkspaceDetailWidget::new(ws, None);
    /// assert!(w.is_scanning());
    /// assert_eq!(w.sub_repos().len(), 0);
    /// ```
    pub fn new(workspace: Workspace, git_factory: Option<Arc<dyn GitProvider>>) -> Self {
        let tab_id = format!("workspace_detail:{}", workspace.path.display());
        Self {
            id: WidgetId::new(tab_id),
            workspace,
            sub_repos: Vec::new(),
            selected: 0,
            right_pane: RightPane::default(),
            git_factory,
            scanning: true,
        }
    }

    /// Apply a completed scan's results. Called by the binary once the
    /// background scan job returns. Clears the `scanning` flag and
    /// clamps `selected` if the new result set is smaller.
    pub fn apply_scan_results(&mut self, results: Vec<RepoSummary>) {
        self.sub_repos = results;
        self.scanning = false;
        if self.selected >= self.sub_repos.len() {
            self.selected = 0;
        }
    }

    /// Whether the off-thread scan is still running.
    pub fn is_scanning(&self) -> bool {
        self.scanning
    }

    /// Borrow the loaded sub-repo summaries.
    pub fn sub_repos(&self) -> &[RepoSummary] {
        &self.sub_repos
    }

    /// Borrow the workspace this detail tab represents.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Current selection cursor.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Currently-selected sub-repo, or `None` if the list is empty.
    pub fn selected_repo(&self) -> Option<&RepoSummary> {
        self.sub_repos.get(self.selected)
    }

    /// Advance selection (wraps).
    pub fn select_next(&mut self) {
        if !self.sub_repos.is_empty() {
            self.selected = (self.selected + 1) % self.sub_repos.len();
        }
    }

    /// Step selection back (wraps).
    pub fn select_prev(&mut self) {
        if !self.sub_repos.is_empty() {
            let n = self.sub_repos.len();
            self.selected = if self.selected == 0 {
                n - 1
            } else {
                self.selected - 1
            };
        }
    }

    /// ratatui-aware draw. Top 40%: 6-column table. Bottom: a placeholder
    /// for the per-sub-repo drill-in pane.
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(40), Constraint::Min(0)])
            .split(area);
        self.render_table(frame, split[0], theme);
        self.render_drilldown(frame, split[1], theme);
    }

    fn render_table(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let title = format!(" {} ", self.workspace.name);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent_primary.into()))
            .title(title);

        if self.scanning && self.sub_repos.is_empty() {
            let para = Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!(
                        "scanning {} for sub-repos…",
                        self.workspace.path.display()
                    ),
                    Style::default().fg(theme.muted.into()),
                ),
            ]))
            .block(block);
            frame.render_widget(para, area);
            return;
        }

        if self.sub_repos.is_empty() {
            let para = Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!(
                        "no sub-repos found under {} — is the path correct?",
                        self.workspace.path.display()
                    ),
                    Style::default().fg(theme.muted.into()),
                ),
            ]))
            .block(block);
            frame.render_widget(para, area);
            return;
        }

        let header = Row::new(["REPO", "BRANCH", "↑/↓", "DIRTY", "AGE", "CI"]).style(
            Style::default()
                .fg(theme.muted.into())
                .add_modifier(Modifier::BOLD),
        );
        let body: Vec<Row> = self
            .sub_repos
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let ahead_behind = match (r.ahead, r.behind) {
                    (0, 0) => "—".to_string(),
                    (a, 0) => format!("↑{a}"),
                    (0, b) => format!("↓{b}"),
                    (a, b) => format!("↑{a} ↓{b}"),
                };
                let dirty = if r.dirty == 0 {
                    "clean".to_string()
                } else {
                    format!("●{}", r.dirty)
                };
                let age = format_age(r.last_commit_age_secs);
                let style = if i == self.selected {
                    Style::default()
                        .fg(theme.background.into())
                        .bg(theme.accent_primary.into())
                } else {
                    Style::default().fg(theme.foreground.into())
                };
                Row::new(vec![
                    r.name.clone(),
                    r.branch.clone(),
                    ahead_behind,
                    dirty,
                    age,
                    r.ci_status.glyph().to_string(),
                ])
                .style(style)
            })
            .collect();
        let table = Table::new(
            body,
            [
                Constraint::Min(12),
                Constraint::Length(20),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(4),
            ],
        )
        .header(header)
        .block(block);
        frame.render_widget(table, area);
    }

    fn render_drilldown(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let title = match self.selected_repo() {
            Some(r) => format!(" {} — {} ", r.name, self.right_pane.label()),
            None => format!(" {} ", self.right_pane.label()),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.muted.into()))
            .title(title);
        let body = if let Some(r) = self.selected_repo() {
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::raw("  branch: "),
                    Span::styled(
                        r.branch.clone(),
                        Style::default().fg(theme.accent_primary.into()),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "  (Branches / Status / Log / Diff / Commit / Actions drill-in coming in a follow-up)",
                    Style::default().fg(theme.muted.into()),
                )]),
            ]
        } else {
            vec![Line::from(""), Line::from("  (no repo selected)")]
        };
        frame.render_widget(Paragraph::new(body).block(block), area);
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

    fn render(&self, _: &mut dyn RenderTarget) {}

    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("Ctrl+W", "close"),
            FooterHint::new("Enter", "focus right"),
            FooterHint::new("b", "sync branches (stub)"),
            FooterHint::new("j/k", "select repo"),
        ]
    }

    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        let Event::Key(chord) = ev else {
            return EventOutcome::Bubble;
        };
        match (chord.code, chord.mods) {
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                self.select_next();
                EventOutcome::Consumed
            }
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                self.select_prev();
                EventOutcome::Consumed
            }
            _ => EventOutcome::Bubble,
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
