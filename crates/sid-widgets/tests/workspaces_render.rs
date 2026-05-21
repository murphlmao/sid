//! Insta snapshot tests for [`WorkspacesWidget::render_into_frame`].
//!
//! Renders the widget into a fixed `TestBackend` using the cosmos theme and
//! snapshots the resulting ASCII buffer. The widget is the canonical render
//! path for the Workspaces tab in `sid` after the visual hierarchy fix that
//! wrapped every pane in a bordered Block titled with its purpose.
//!
//! Two scenarios are covered:
//!
//! - empty state — no workspaces registered yet,
//! - one repo + default Branches sub-view (no branches loaded).

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_ui::themes::cosmos;
use sid_widgets::WorkspacesWidget;

fn render(widget: &WorkspacesWidget, width: u16, height: u16) -> String {
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

#[test]
fn snapshot_empty_workspaces_default_panes() {
    let w = WorkspacesWidget::new(vec![], None);
    let s = render(&w, 80, 12);
    insta::assert_snapshot!("workspaces_empty_default", s);
}

#[test]
fn snapshot_one_repo_branches_pane() {
    let ws = Workspace {
        path: PathBuf::from("/vcs/myrepo"),
        name: "myrepo".into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    };
    let w = WorkspacesWidget::new(vec![ws], None);
    let s = render(&w, 80, 12);
    insta::assert_snapshot!("workspaces_one_repo_branches", s);
}
