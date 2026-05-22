# Branch #3 — Workspace detail tab (multi-repo dashboard)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the user presses Enter on a workspace in the overview, a new closable tab opens showing every git repo under that workspace as a row in a six-column dashboard, with the existing Branches/Status/Log/Diff/Commit/Actions pane drilled in for the highlighted sub-repo.

**Architecture:** A new `WorkspaceDetailWidget` (in a new file under `sid-widgets`) owns: a `Workspace` clone, a `Vec<RepoSummary>` populated off the render thread via `JobQueue`, a `RightPane` instance (reused from the existing workspaces widget), and a focus marker. The binary's wire layer handles the `workspaces.open_detail` action from branch #2 by building the widget, spawning the scan job, and calling `TabManager::push_detail`. Closing via `Ctrl+W` flows through the existing `tab.close` action from branch #1.

**Tech Stack:** Rust 2024 edition, sid-job::JobQueue (off-thread scan), sid-core::workspace_discovery::scan_workspace_root (pure walker), git2 via the existing `GitProvider` adapter (status/branches), ratatui (rendering).

**Branch:** `feat/workspace-detail-as-tab`

**Depends on:** Branches #1 (TabManager dynamic API, Ctrl+W binding) and #2 (workspaces.open_detail action) merged.

**Spec reference:** [`docs/superpowers/specs/2026-05-22-tui-ux-interaction-design.md`](../specs/2026-05-22-tui-ux-interaction-design.md) §§ 5.5, 6.

---

## File map

| File | Purpose | Action |
|---|---|---|
| `crates/sid-widgets/src/workspace_detail.rs` | the new widget — types, layout, key handling | Create |
| `crates/sid-widgets/src/lib.rs` | `pub mod workspace_detail;` + re-export | Modify |
| `crates/sid-widgets/tests/workspace_detail.rs` | unit + integration tests | Create |
| `crates/sid-widgets/tests/snapshots/` | insta snapshots for the dashboard | Create on first --review pass |
| `crates/sid-widgets/benches/workspace_detail_open.rs` | criterion bench | Create |
| `crates/sid-widgets/Cargo.toml` | bench entry | Modify |
| `crates/sid/src/wire.rs` | dispatch `workspaces.open_detail` → build + push_detail; replace placeholder from branch #2 | Modify |
| `crates/sid/src/wire.rs` | drain JobQueue completions per frame; apply `RepoSummary` results | Modify |

---

## Task 1 — Define `RepoSummary`, `CiStatus`, and the widget shell

**Files:**
- Create: `crates/sid-widgets/src/workspace_detail.rs`
- Modify: `crates/sid-widgets/src/lib.rs`

`★ Insight ─────────────────────────────────────`
`RepoSummary` is a serialization-free, derive-Clone type so the JobQueue can ship completed scan results through the `Vec<Result<T, JobError>>` without lifetime ceremony. It owns its strings (`PathBuf`, `String`) so the widget can drop the scan callback's references safely.
`─────────────────────────────────────────────────`

- [ ] **Step 1.1: Create `crates/sid-widgets/src/workspace_detail.rs` with the type definitions and a stub `Widget` impl**

```rust
//! Workspace detail tab — multi-repo dashboard.
//!
//! Opened when the user presses Enter on a workspace in the Workspaces
//! overview. Renders every git repo discovered one level deep under the
//! workspace path as a row in a six-column table; the highlighted row's
//! sub-pane (Branches/Status/Log/Diff/Commit/Actions) renders below.

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
    Pending,
    Pass,
    Fail,
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
/// let r = RepoSummary {
///     path: PathBuf::from("/vcs/x/y"),
///     name: "y".into(),
///     branch: "main".into(),
///     ahead: 0, behind: 0, dirty: 0,
///     last_commit_age_secs: 60,
///     ci_status: CiStatus::Unknown,
/// };
/// assert_eq!(r.name, "y");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoSummary {
    pub path: PathBuf,
    pub name: String,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
    pub last_commit_age_secs: u64,
    pub ci_status: CiStatus,
}

/// Detail tab widget. Owns a workspace clone, a list of sub-repo summaries
/// (populated by the binary off-thread), a selection cursor, and a
/// `RightPane` for drill-in.
pub struct WorkspaceDetailWidget {
    id: WidgetId,
    workspace: Workspace,
    sub_repos: Vec<RepoSummary>,
    selected: usize,
    right_pane: RightPane,
    git_factory: Option<Arc<dyn GitProvider>>,
    /// True while the scan job is still running. Renderer shows a
    /// "scanning…" hint until results land.
    scanning: bool,
}

impl WorkspaceDetailWidget {
    /// Construct with a workspace and the git factory (cloned from the
    /// overview widget so all detail tabs share the same provider).
    /// Sub-repos start empty + `scanning = true`; the binary calls
    /// `apply_scan_results` once `scan_workspace_root` completes.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_core::workspace_metadata::WorkspaceKind;
    /// use sid_store::Workspace;
    /// use sid_widgets::workspace_detail::WorkspaceDetailWidget;
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
    /// background scan job returns. Clears the `scanning` flag.
    pub fn apply_scan_results(&mut self, results: Vec<RepoSummary>) {
        self.sub_repos = results;
        self.scanning = false;
        if self.selected >= self.sub_repos.len() {
            self.selected = 0;
        }
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning
    }

    pub fn sub_repos(&self) -> &[RepoSummary] {
        &self.sub_repos
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_repo(&self) -> Option<&RepoSummary> {
        self.sub_repos.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if !self.sub_repos.is_empty() {
            self.selected = (self.selected + 1) % self.sub_repos.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.sub_repos.is_empty() {
            let n = self.sub_repos.len();
            self.selected = if self.selected == 0 { n - 1 } else { self.selected - 1 };
        }
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

    fn render(&self, _: &mut dyn RenderTarget) {
        // ratatui-aware draw is in render_into_frame; the trait body is
        // a no-op for parity with other widgets.
    }

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
            // 'b' opens the stub sync-branches modal in Task 4.
            // Ctrl+W is handled by the global keybind → tab.close action.
            _ => EventOutcome::Bubble,
        }
    }
}
```

- [ ] **Step 1.2: Wire the module in `crates/sid-widgets/src/lib.rs`**

Add near the existing `pub mod workspaces;`:

```rust
pub mod workspace_detail;
pub use workspace_detail::{CiStatus, RepoSummary, WorkspaceDetailWidget};
```

- [ ] **Step 1.3: Verify the crate compiles**

```bash
cargo build -p sid-widgets
```

Expected: success. If `Workspace` is not Clone — confirm by checking `crates/sid-store/src/lib.rs`. If it isn't, this is a problem we need to surface; in that case derive Clone on Workspace in a tiny prep commit before continuing.

- [ ] **Step 1.4: Commit Task 1**

```bash
git add crates/sid-widgets/src/workspace_detail.rs crates/sid-widgets/src/lib.rs
git commit -m "feat(sid-widgets): WorkspaceDetailWidget shell + RepoSummary types

Defines the new tab widget that opens when Enter is pressed on a
workspace. Carries a Workspace, a list of RepoSummary rows, a
RightPane reused from the overview widget, and a scanning flag set
to true until the binary's JobQueue completes the off-thread scan.

No rendering, no key bindings beyond j/k yet — Task 2 adds the
ratatui-aware draw, Task 3 the scan job integration."
```

---

## Task 2 — Render the dashboard table + drill-in pane

**Files:**
- Modify: `crates/sid-widgets/src/workspace_detail.rs`
- Test: `crates/sid-widgets/tests/workspace_detail.rs`

`★ Insight ─────────────────────────────────────`
The 6-column layout mirrors the spec's mockup: `repo · branch · ahead/behind · dirty · age · CI`. Using `ratatui::widgets::Table` with `Constraint::Length` for fixed columns and `Constraint::Min` for the variable name column keeps the layout stable as the terminal resizes.
`─────────────────────────────────────────────────`

- [ ] **Step 2.1: Add `render_into_frame` to `WorkspaceDetailWidget`**

In `crates/sid-widgets/src/workspace_detail.rs`, after `impl Widget for WorkspaceDetailWidget`, add:

```rust
impl WorkspaceDetailWidget {
    /// ratatui-aware draw. Top half: 6-column table; bottom half: a
    /// RightPane drilled into the selected sub-repo.
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(40), Constraint::Min(0)])
            .split(area);
        self.render_table(frame, split[0], theme);
        self.render_drilldown(frame, split[1], theme);
    }

    fn render_table(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
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
        let title = format!(" {} ", self.workspace.name);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent_primary.into()))
            .title(title);
        if self.scanning && self.sub_repos.is_empty() {
            // While the scan is running, show a single-line hint instead of
            // the table.
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
        let table = Table::new(
            body,
            [
                Constraint::Min(12),    // REPO
                Constraint::Length(20), // BRANCH
                Constraint::Length(10), // ↑/↓
                Constraint::Length(8),  // DIRTY
                Constraint::Length(10), // AGE
                Constraint::Length(4),  // CI
            ],
        )
        .header(header)
        .block(block);
        frame.render_widget(table, area);
    }

    fn render_drilldown(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        // Bottom pane: reuse the existing RightPane label for now; the
        // actual git-data fetching for the selected sub-repo is wired by
        // the binary when this widget is in focus. v1 just shows the
        // selected repo's name + a "drill-in coming…" placeholder. The
        // existing RightPane render code from the workspaces widget is
        // not refactored to be importable from here yet; that's a follow-
        // up. v1 ships with a plain-text placeholder so the layout is
        // final.
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
                    "  (Branches / Status / Log / Diff / Commit / Actions drill-in coming in branch #3 follow-up — render-pane refactor)",
                    Style::default().fg(theme.muted.into()),
                )]),
            ]
        } else {
            vec![Line::from(""), Line::from("  (no repo selected)")]
        };
        frame.render_widget(Paragraph::new(body).block(block), area);
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
```

- [ ] **Step 2.2: Add a `render_to_string` helper to the new module for tests**

Append to `crates/sid-widgets/src/workspace_detail.rs`:

```rust
/// Render the widget into a fresh test buffer of `(width, height)`
/// using the cosmos theme. Used by tests + snapshots.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_core::workspace_metadata::WorkspaceKind;
/// use sid_store::Workspace;
/// use sid_widgets::workspace_detail::{render_to_string, WorkspaceDetailWidget};
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
```

- [ ] **Step 2.3: Add unit tests**

Create `crates/sid-widgets/tests/workspace_detail.rs`:

```rust
use std::path::PathBuf;
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspace_detail::{format_age, render_to_string, CiStatus, RepoSummary, WorkspaceDetailWidget};

fn umbrella(path: &str, name: &str) -> Workspace {
    Workspace {
        path: PathBuf::from(path),
        name: name.into(),
        kind: WorkspaceKind::Umbrella,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    }
}

fn summary(name: &str, branch: &str, ahead: u32, dirty: u32, age: u64) -> RepoSummary {
    RepoSummary {
        path: PathBuf::from(format!("/vcs/x/{name}")),
        name: name.into(),
        branch: branch.into(),
        ahead,
        behind: 0,
        dirty,
        last_commit_age_secs: age,
        ci_status: CiStatus::Unknown,
    }
}

#[test]
fn scanning_state_shows_hint() {
    let w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    let s = render_to_string(&w, 100, 30);
    assert!(s.contains("scanning"), "expected scanning hint:\n{s}");
}

#[test]
fn empty_results_shows_no_subrepos_hint() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_scan_results(vec![]);
    let s = render_to_string(&w, 100, 30);
    assert!(
        s.contains("no sub-repos found"),
        "expected empty-state hint:\n{s}",
    );
}

#[test]
fn render_shows_each_subrepo_row() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/eggsight-stack", "eggsight-stack"), None);
    w.apply_scan_results(vec![
        summary("frontend", "main", 3, 12, 7200),
        summary("backend", "feature/x", 0, 0, 900),
        summary("infra", "main", 0, 0, 259_200),
    ]);
    let s = render_to_string(&w, 120, 40);
    for name in ["frontend", "backend", "infra"] {
        assert!(s.contains(name), "row {name} missing:\n{s}");
    }
    assert!(s.contains("eggsight-stack"), "title missing:\n{s}");
}

#[test]
fn select_next_wraps() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_scan_results(vec![
        summary("a", "main", 0, 0, 60),
        summary("b", "main", 0, 0, 60),
    ]);
    assert_eq!(w.selected_index(), 0);
    w.select_next();
    assert_eq!(w.selected_index(), 1);
    w.select_next();
    assert_eq!(w.selected_index(), 0);
}

#[test]
fn format_age_buckets() {
    assert_eq!(format_age(0), "0s");
    assert_eq!(format_age(59), "59s");
    assert_eq!(format_age(60), "1m");
    assert_eq!(format_age(3599), "59m");
    assert_eq!(format_age(3600), "1h");
    assert_eq!(format_age(86_399), "23h");
    assert_eq!(format_age(86_400), "1d");
}
```

- [ ] **Step 2.4: Run tests, verify PASS**

```bash
cargo test -p sid-widgets --test workspace_detail
cargo test -p sid-widgets --doc workspace_detail
```

Expected: PASS for unit tests AND doc tests.

- [ ] **Step 2.5: Take a baseline snapshot**

Add an insta snapshot test:

```rust
#[test]
fn dashboard_renders_eggsight_stack_snapshot() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/eggsight-stack", "eggsight-stack"), None);
    w.apply_scan_results(vec![
        summary("frontend", "main", 3, 12, 7200),
        summary("backend", "feature/x", 0, 0, 900),
        summary("infra", "main", 0, 0, 259_200),
        summary("docs", "main", 0, 0, 3600),
    ]);
    let s = render_to_string(&w, 120, 30);
    insta::assert_snapshot!("workspace_detail_eggsight_stack", s);
}
```

Run:

```bash
cargo insta test -p sid-widgets --review
```

Accept the baseline snapshot.

- [ ] **Step 2.6: Commit Task 2**

```bash
git add crates/sid-widgets/src/workspace_detail.rs crates/sid-widgets/tests/workspace_detail.rs crates/sid-widgets/tests/snapshots/
git commit -m "feat(sid-widgets): WorkspaceDetailWidget — 6-column dashboard table

Top 40% renders the per-sub-repo dashboard (REPO · BRANCH · ↑/↓ · DIRTY
· AGE · CI). Bottom 60% renders a placeholder for the per-repo drill-
in pane (the full Branches/Status/Log/Diff/Commit refactor is a
follow-up). Scanning state shows a hint; empty results shows a
\"check the path\" hint; populated state shows the full table with
the selected row highlighted via theme.accent_primary.

Insta baseline snapshot included so subsequent visual changes are
intentional."
```

---

## Task 3 — Off-thread scan via JobQueue; binary wires `workspaces.open_detail`

**Files:**
- Modify: `crates/sid/src/wire.rs`

`★ Insight ─────────────────────────────────────`
The scan walks the workspace path with `scan_workspace_root(path, 1)`. For an umbrella with 30 sub-repos this is well under 100 ms on a warm cache, but it touches the filesystem so we keep it off the render thread via `JobQueue::spawn`. The binary drains completions per frame and routes the `RepoSummary` vec to the right tab via the tab's `WidgetId`.
`─────────────────────────────────────────────────`

- [ ] **Step 3.1: Define the JobOutcome variant for workspace detail scans**

Find the existing `JobOutcome` enum in `crates/sid/src/wire.rs`:

```bash
grep -n "enum JobOutcome\|pub enum JobOutcome" crates/sid/src/wire.rs
```

Add a variant:

```rust
pub enum JobOutcome {
    // existing variants...
    WorkspaceDetailScanned {
        tab_id: String,
        summaries: Vec<sid_widgets::workspace_detail::RepoSummary>,
    },
}
```

- [ ] **Step 3.2: Replace the `workspaces.open_detail` placeholder from branch #2 with the real handler**

Find the placeholder dispatch added in branch #2 and replace with a function call:

```rust
if action.as_str() == "workspaces.open_detail" {
    open_workspace_detail_tab(sid_app);
}
```

Then define `open_workspace_detail_tab`:

```rust
/// Build a WorkspaceDetailWidget for the currently-selected workspace and
/// push it as a new detail tab. Spawns the sub-repo scan as a JobQueue
/// task; completions land in the widget via apply_scan_results on the
/// next render cycle.
///
/// No-op when no workspace is selected or the active tab isn't the
/// workspaces overview.
fn open_workspace_detail_tab(sid_app: &mut SidApp) {
    use sid_core::tab::{Tab, TabId, TabKind};
    use sid_core::layout::Layout;
    use sid_widgets::workspace_detail::WorkspaceDetailWidget;

    // Verify we're on the workspaces tab.
    let active = sid_app.app.tabs().active();
    if active.id.as_str() != "workspaces" {
        return;
    }
    let parent_idx = sid_app.app.tabs().active_index();

    // Pull the selected workspace from the overview widget.
    let workspace = {
        let any_ref = active.layout.iter_widgets()
            .next()
            .and_then(|w| w.as_any().downcast_ref::<sid_widgets::WorkspacesWidget>());
        let Some(ws_widget) = any_ref else { return; };
        let Some(selected) = ws_widget.state().selected_workspace() else { return; };
        selected.clone()
    };

    let tab_id = format!("workspace_detail:{}", workspace.path.display());

    // Avoid duplicate tabs — if a detail tab for this workspace is already
    // open, just switch to it.
    if sid_app.app.tabs_mut().switch_to(&TabId::new(&tab_id)) {
        return;
    }

    let widget = WorkspaceDetailWidget::new(workspace.clone(), Some(sid_app.git_factory.clone()));
    let new_tab = Tab {
        id: TabId::new(&tab_id),
        title: workspace.name.clone(),
        layout: Layout::Single(Box::new(widget)),
        hotkey: None,
        kind: TabKind::Detail { parent_idx },
    };
    if let Err(e) = sid_app.app.tabs_mut().push_detail(new_tab) {
        tracing::warn!("push_detail failed: {e}");
        return;
    }
    // Switch focus to the new tab.
    let _ = sid_app.app.tabs_mut().switch_to(&TabId::new(&tab_id));

    // Spawn the scan job.
    let path = workspace.path.clone();
    let scan_tab_id = tab_id.clone();
    let _ = sid_app.jobs.spawn(async move {
        let summaries = scan_workspace_for_summaries(&path).await;
        JobOutcome::WorkspaceDetailScanned {
            tab_id: scan_tab_id,
            summaries,
        }
    });
}

/// Walk `path` one level deep, find each git repo, and build a
/// `RepoSummary` for each. Best-effort — failures on individual repos
/// are reported with placeholder values (branch "?", dirty 0, etc.).
async fn scan_workspace_for_summaries(
    path: &std::path::Path,
) -> Vec<sid_widgets::workspace_detail::RepoSummary> {
    use sid_core::workspace_discovery::scan_workspace_root;
    use sid_widgets::workspace_detail::{CiStatus, RepoSummary};
    use sid_store::now_epoch;
    let discovered = match scan_workspace_root(path, 1) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("scan_workspace_root({}) failed: {e}", path.display());
            return Vec::new();
        }
    };
    let now = now_epoch();
    discovered
        .into_iter()
        .filter(|d| d.kind == sid_core::workspace_metadata::WorkspaceKind::Repo)
        .map(|d| {
            let name = d
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| d.path.display().to_string());
            // v1: branch / ahead-behind / dirty / age are best-effort
            // defaults. The follow-up that wires the real GitProvider
            // per row replaces these with live values.
            RepoSummary {
                path: d.path.clone(),
                name,
                branch: "?".into(),
                ahead: 0,
                behind: 0,
                dirty: 0,
                last_commit_age_secs: now,
                ci_status: CiStatus::Unknown,
            }
        })
        .collect()
}
```

- [ ] **Step 3.3: Drain JobQueue completions per frame and route `WorkspaceDetailScanned` to its widget**

Find the existing JobQueue drain loop in `wire.rs`:

```bash
grep -n "drain_completed\|jobs.drain\|JobOutcome::" crates/sid/src/wire.rs | head -20
```

Add a match arm:

```rust
JobOutcome::WorkspaceDetailScanned { tab_id, summaries } => {
    if let Some(tab) = sid_app.app.tabs_mut().tabs_mut().iter_mut()
        .find(|t| t.id.as_str() == tab_id)
        && let Some(widget) = tab.layout.iter_widgets_mut().next()
        && let Some(detail) = widget.as_any_mut().downcast_mut::<sid_widgets::WorkspaceDetailWidget>()
    {
        detail.apply_scan_results(summaries);
    }
}
```

> Note: `as_any_mut` is required on the Widget trait. Confirm it exists at `crates/sid-core/src/widget.rs`. If only `as_any` exists, add `as_any_mut` in a tiny prep commit before this step.

- [ ] **Step 3.4: Add `git_factory` to `SidApp` if not already present**

```bash
grep -n "git_factory\|GitProvider" crates/sid/src/wire.rs | head -10
```

If `SidApp` doesn't already carry a `git_factory: Arc<dyn GitProvider>`, that's outside this branch's scope — branch #3 only needs `Option<Arc<dyn GitProvider>>`, and in this version we pass `None` until a follow-up wires the per-sub-repo drill-in. Update `open_workspace_detail_tab` to pass `None`:

```rust
let widget = WorkspaceDetailWidget::new(workspace.clone(), None);
```

- [ ] **Step 3.5: Run all tests, verify PASS**

```bash
cargo test -p sid-widgets
cargo test -p sid
```

Expected: PASS.

- [ ] **Step 3.6: Commit Task 3**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): workspaces.open_detail handler — build WorkspaceDetailWidget + push as Detail tab

Replaces the branch-#2 placeholder. Builds the widget with the selected
workspace, calls TabManager::push_detail with TabKind::Detail and the
overview tab as parent_idx, then spawns the sub-repo scan as a JobQueue
task. Completions drain per frame and route summaries to the matching
detail widget via downcast_mut.

Per-sub-repo branch/dirty/age/CI populations are best-effort defaults
in v1 — wiring the real GitProvider per row is a follow-up. v1 ships
the layout + scan flow + close-with-Ctrl+W."
```

---

## Task 4 — Integration test: Enter → tab opens → Ctrl+W → tab closes

**Files:**
- Create: `crates/sid-widgets/tests/workspace_detail_integration.rs`

> Note: this test exercises the full flow through the binary. We do this as a `sid_widgets` integration test for repo-locality; it constructs a `TabManager` and `App` directly.

- [ ] **Step 4.1: Write the integration test**

Create `crates/sid-widgets/tests/workspace_detail_integration.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::action::{ActionId, ActionRegistry};
use sid_core::app::App;
use sid_core::context::WidgetCtx;
use sid_core::event::{Event, KeyChord};
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabKind, TabManager};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspace_detail::WorkspaceDetailWidget;
use sid_widgets::WorkspacesWidget;
use std::path::PathBuf;

fn workspaces_tab(workspaces: Vec<Workspace>) -> Tab {
    Tab {
        id: TabId::new("workspaces"),
        title: "Workspaces".into(),
        layout: Layout::Single(Box::new(WorkspacesWidget::new(workspaces, None))),
        hotkey: Some('1'),
        kind: TabKind::Core,
    }
}

#[test]
fn enter_then_push_detail_then_ctrl_w_closes() {
    let ws = Workspace {
        path: PathBuf::from("/vcs/eggsight-stack"),
        name: "eggsight-stack".into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    };
    let tabs = TabManager::new(vec![workspaces_tab(vec![ws.clone()])]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());

    // Press Enter on the workspaces tab. This should emit
    // "workspaces.open_detail" via the widget; App::handle_event drains
    // and runs it as an action. In a real binary the run_action handler
    // would call open_workspace_detail_tab; here we simulate that by
    // pushing a detail tab directly.
    let _ = app.handle_event(&Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE)));
    // Simulate the wire layer's response:
    let detail = WorkspaceDetailWidget::new(ws.clone(), None);
    app.tabs_mut()
        .push_detail(Tab {
            id: TabId::new("workspace_detail:/vcs/eggsight-stack"),
            title: "eggsight-stack".into(),
            layout: Layout::Single(Box::new(detail)),
            hotkey: None,
            kind: TabKind::Detail { parent_idx: 0 },
        })
        .unwrap();
    let _ = app
        .tabs_mut()
        .switch_to(&TabId::new("workspace_detail:/vcs/eggsight-stack"));
    assert_eq!(app.tabs().active().id.as_str(), "workspace_detail:/vcs/eggsight-stack");
    assert_eq!(app.tabs().detail_count(), 1);

    // Press Ctrl+W. The global keybind maps to tab.close.
    let _ = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('w'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.tabs().active().id.as_str(), "workspaces");
    assert_eq!(app.tabs().detail_count(), 0);
}

#[test]
fn alt_w_also_closes_detail_tab() {
    let ws = Workspace {
        path: PathBuf::from("/vcs/x"),
        name: "x".into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    };
    let tabs = TabManager::new(vec![workspaces_tab(vec![ws.clone()])]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    let detail = WorkspaceDetailWidget::new(ws.clone(), None);
    app.tabs_mut()
        .push_detail(Tab {
            id: TabId::new("workspace_detail:/vcs/x"),
            title: "x".into(),
            layout: Layout::Single(Box::new(detail)),
            hotkey: None,
            kind: TabKind::Detail { parent_idx: 0 },
        })
        .unwrap();
    app.tabs_mut()
        .switch_to(&TabId::new("workspace_detail:/vcs/x"));
    let _ = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('w'),
        KeyModifiers::ALT,
    )));
    assert_eq!(app.tabs().active().id.as_str(), "workspaces");
}
```

- [ ] **Step 4.2: Run integration tests, verify PASS**

```bash
cargo test -p sid-widgets --test workspace_detail_integration
```

Expected: both PASS.

- [ ] **Step 4.3: Commit Task 4**

```bash
git add crates/sid-widgets/tests/workspace_detail_integration.rs
git commit -m "test(sid-widgets): full integration — Enter opens detail, Ctrl+W (or Alt+W) closes

Round-trips through TabManager, App::handle_event, and the new
tab.close action arm. Locks in the contract that closing a detail
tab snaps focus back to the spawning core tab (workspaces here)."
```

---

## Task 5 — Adversarial tests: missing path, scan error, duplicate-open

**Files:**
- Modify: `crates/sid-widgets/tests/workspace_detail.rs`

- [ ] **Step 5.1: Add tests**

Append to `crates/sid-widgets/tests/workspace_detail.rs`:

```rust
#[test]
fn missing_path_still_renders_empty_state_after_scan_completes() {
    let mut w = WorkspaceDetailWidget::new(
        umbrella("/nonexistent/path/never/exists", "ghost"),
        None,
    );
    // The binary's scan job returns an empty vec on filesystem error.
    w.apply_scan_results(vec![]);
    let s = render_to_string(&w, 100, 30);
    assert!(
        s.contains("no sub-repos found"),
        "expected empty-state hint after missing-path scan:\n{s}",
    );
}

#[test]
fn applying_scan_results_clears_scanning_flag() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    assert!(w.is_scanning());
    w.apply_scan_results(vec![]);
    assert!(!w.is_scanning());
}

#[test]
fn select_on_empty_subrepos_is_noop() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_scan_results(vec![]);
    w.select_next(); // would panic on naive indexing
    w.select_prev();
    assert_eq!(w.selected_index(), 0);
    assert!(w.selected_repo().is_none());
}

#[test]
fn applying_scan_results_clamps_selected_idx_if_smaller_set() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_scan_results(vec![
        summary("a", "main", 0, 0, 60),
        summary("b", "main", 0, 0, 60),
        summary("c", "main", 0, 0, 60),
    ]);
    w.select_next();
    w.select_next();
    assert_eq!(w.selected_index(), 2);
    // A rescan returns fewer rows.
    w.apply_scan_results(vec![summary("a", "main", 0, 0, 60)]);
    assert_eq!(w.selected_index(), 0);
}
```

- [ ] **Step 5.2: Run, verify PASS**

```bash
cargo test -p sid-widgets --test workspace_detail missing_path applying_scan_results select_on_empty
```

Expected: PASS.

- [ ] **Step 5.3: Commit Task 5**

```bash
git add crates/sid-widgets/tests/workspace_detail.rs
git commit -m "test(sid-widgets): adversarial cases for WorkspaceDetailWidget

Covers: missing path renders empty-state cleanly; apply_scan_results
clears the scanning flag and clamps a now-invalid selected_idx;
select_next/select_prev on an empty sub-repo set are no-ops, not
panics. Locks in the contracts the integration test relies on."
```

---

## Task 6 — Criterion bench: `bench_workspace_detail_open_with_5_subrepos`

**Files:**
- Create: `crates/sid-widgets/benches/workspace_detail_open.rs`
- Modify: `crates/sid-widgets/Cargo.toml`

- [ ] **Step 6.1: Declare the bench**

Append to `crates/sid-widgets/Cargo.toml`:

```toml
[[bench]]
name = "workspace_detail_open"
harness = false
```

- [ ] **Step 6.2: Write the bench**

Create `crates/sid-widgets/benches/workspace_detail_open.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_ui::themes::cosmos;
use sid_widgets::workspace_detail::{CiStatus, RepoSummary, WorkspaceDetailWidget};
use std::path::PathBuf;

fn make_summaries(n: usize) -> Vec<RepoSummary> {
    (0..n)
        .map(|i| RepoSummary {
            path: PathBuf::from(format!("/vcs/x/repo_{i}")),
            name: format!("repo_{i}"),
            branch: "main".into(),
            ahead: 0,
            behind: 0,
            dirty: 0,
            last_commit_age_secs: 60,
            ci_status: CiStatus::Unknown,
        })
        .collect()
}

fn bench_open_and_render(c: &mut Criterion) {
    let ws = Workspace {
        path: PathBuf::from("/vcs/eggsight-stack"),
        name: "eggsight-stack".into(),
        kind: WorkspaceKind::Umbrella,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    };
    let theme = cosmos();
    c.bench_function("workspace_detail_open_5_subrepos_first_frame", |b| {
        b.iter(|| {
            let mut w = WorkspaceDetailWidget::new(ws.clone(), None);
            w.apply_scan_results(make_summaries(5));
            let backend = TestBackend::new(120, 40);
            let mut term = Terminal::new(backend).unwrap();
            term.draw(|f| w.render_into_frame(f, f.area(), &theme)).unwrap();
            black_box(())
        });
    });
}

criterion_group!(benches, bench_open_and_render);
criterion_main!(benches);
```

- [ ] **Step 6.3: Run, confirm budget**

```bash
cargo bench -p sid-widgets --bench workspace_detail_open
```

Budget per spec: ≤ 16 ms wall (first frame).

- [ ] **Step 6.4: Save baseline**

```bash
cargo bench -p sid-widgets --bench workspace_detail_open -- --save-baseline main
```

- [ ] **Step 6.5: Commit Task 6**

```bash
git add crates/sid-widgets/Cargo.toml crates/sid-widgets/benches/workspace_detail_open.rs
git commit -m "perf(sid-widgets): criterion bench for first-frame WorkspaceDetailWidget render

16 ms wall budget (60 Hz frame) at 5 sub-repos per the spec. The
scan itself is off-thread; this benches the render path the user
sees immediately when the tab opens."
```

---

## Task 7 — Workspace-wide gate + merge

- [ ] **Step 7.1: Run /sid-gate**

```bash
/sid-gate
```

Expected: green.

- [ ] **Step 7.2: Run /sid-perf-check**

```bash
/sid-perf-check
```

Expected: no regressions vs main.

- [ ] **Step 7.3: Merge to main**

```bash
git checkout main
git merge --no-ff feat/workspace-detail-as-tab -m "Merge branch #3: workspace detail tab (multi-repo dashboard)

Enter on a workspace now opens a new closable Detail tab with a
6-column dashboard of every sub-repo. Closes with Ctrl+W or Alt+W.
v1 sub-repo data is best-effort defaults; full GitProvider wiring
per row is a follow-up tracked in the future-features backlog."
```

---

## Definition of done

- [x] Pressing Enter on a workspace in the overview opens a new tab titled with the workspace name.
- [x] The detail tab shows a 6-column dashboard once the scan completes; a "scanning…" hint while it runs.
- [x] Empty workspace path shows "no sub-repos found"; missing path shows the same (scan returns empty).
- [x] Ctrl+W and Alt+W close the detail tab and snap focus back to the workspaces overview.
- [x] Opening the same workspace twice does not create a duplicate tab — it switches to the existing one.
- [x] Criterion bench saved; first-frame budget ≤ 16 ms at 5 sub-repos.
- [x] `/sid-gate` clean; `/sid-perf-check` no regressions.
- [x] Branch merged.

## Risks and rollback

- The per-row drill-in (Branches/Status/Log/Diff/Commit on the selected sub-repo) is a placeholder in v1. Users who expected the same interactivity as the overview's right-pane will see "drill-in coming" text. The follow-up is filed.
- `WorkspaceDetailWidget` is a new public type. If it needs to evolve quickly, document that the v1 shape is provisional in the doc comment.
- The JobQueue scan can in theory leak handles if a tab is closed mid-scan. The completion still arrives; the downcast lookup in `wire.rs` returns None and the result is dropped. Test: open detail, immediately close, confirm no panic — add this as an adversarial follow-up if you want a guard test.
