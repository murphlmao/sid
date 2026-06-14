//! Integration: workspace detail tab open/close round-trip through
//! `TabManager` + `App::handle_event` + the `tab.close` action.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::{
    action::ActionRegistry,
    app::App,
    event::{Event, KeyChord},
    keybind::KeybindMap,
    layout::Layout,
    tab::{Tab, TabId, TabKind, TabManager},
    workspace_metadata::WorkspaceKind,
};
use sid_store::Workspace;
use sid_widgets::{WorkspacesWidget, workspace_detail::WorkspaceDetailWidget};

fn workspaces_tab(workspaces: Vec<Workspace>) -> Tab {
    Tab {
        id: TabId::new("workspaces"),
        title: "Workspaces".into(),
        layout: Layout::Single(Box::new(WorkspacesWidget::new(workspaces, None))),
        hotkey: Some('1'),
        kind: TabKind::Core,
    }
}

fn repo(path: &str, name: &str) -> Workspace {
    Workspace {
        path: PathBuf::from(path),
        name: name.into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    }
}

#[test]
fn enter_then_push_detail_then_ctrl_w_closes() {
    let ws = repo("/vcs/eggsight-stack", "eggsight-stack");
    let tabs = TabManager::new(vec![workspaces_tab(vec![ws.clone()])]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());

    // Press Enter on the Workspaces tab — the widget emits + sets pending.
    let _ = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));

    // Simulate the wire layer's response: detect pending_open_detail and
    // push a Detail tab.
    let pending = {
        let workspaces_tab_ref = app
            .tabs_mut()
            .tabs_mut()
            .iter_mut()
            .find(|t| t.id.as_str() == "workspaces")
            .expect("workspaces tab present");
        let Layout::Single(w) = &mut workspaces_tab_ref.layout else {
            panic!("workspaces layout should be Single")
        };
        let ws_widget = w
            .as_any_mut()
            .downcast_mut::<WorkspacesWidget>()
            .expect("workspaces widget downcast");
        ws_widget.take_pending_open_detail()
    };
    let pending = pending.expect("Enter on a Repo must set pending_open_detail");
    assert_eq!(pending.path, ws.path);

    let detail_tab = Tab {
        id: TabId::new("workspace_detail:/vcs/eggsight-stack"),
        title: "eggsight-stack".into(),
        layout: Layout::Single(Box::new(WorkspaceDetailWidget::new(pending.clone(), None))),
        hotkey: None,
        kind: TabKind::Detail { parent_idx: 0 },
    };
    app.tabs_mut().push_detail(detail_tab).unwrap();
    let _ = app
        .tabs_mut()
        .switch_to(&TabId::new("workspace_detail:/vcs/eggsight-stack"));
    assert_eq!(
        app.tabs().active().id.as_str(),
        "workspace_detail:/vcs/eggsight-stack"
    );
    assert_eq!(app.tabs().detail_count(), 1);

    // Press Ctrl+W — global keybind dispatches tab.close → close_active.
    let _ = app.handle_event(&Event::Key(KeyChord::new(
        KeyCode::Char('w'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.tabs().active().id.as_str(), "workspaces");
    assert_eq!(app.tabs().detail_count(), 0);
}

#[test]
fn alt_w_also_closes_detail_tab() {
    let ws = repo("/vcs/x", "x");
    let tabs = TabManager::new(vec![workspaces_tab(vec![ws.clone()])]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    app.tabs_mut()
        .push_detail(Tab {
            id: TabId::new("workspace_detail:/vcs/x"),
            title: "x".into(),
            layout: Layout::Single(Box::new(WorkspaceDetailWidget::new(ws, None))),
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
