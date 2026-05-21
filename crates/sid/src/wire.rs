//! Wires together concrete implementations — RedbStore, all six widgets, the
//! keybind map and action registry — into a running [`App`], and contains the
//! Ratatui render loop.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use directories::{ProjectDirs, UserDirs};
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};
use sid_core::Result as SidResult;
use sid_core::action::{Action, ActionRegistry};
use sid_core::app::{App, Dispatch};
use sid_core::event::Event as SidEvent;
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabManager};
use sid_core::widget::Widget;
use sid_core::workspace_discovery::{WorkspaceUpserter, merge_discoveries_into, scan_workspace_root};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_git::Git2ProviderFactory;
use sid_store::{RedbStore, SessionRecord, Store, Workspace, now_epoch};
use sid_ui::helpers::styled_block;
use sid_ui::themes::cosmos;
use sid_widgets::{
    DatabaseWidget, NetworkWidget, SettingsWidget, SshWidget, SystemWidget, WorkspacesWidget,
};
use tokio::sync::mpsc::Receiver;

/// Top-level wired application state owned by the binary.
///
/// Bundles together the [`App`] (tab manager + keybinds + action registry),
/// the backing [`RedbStore`], and the current session identifier.  Owned
/// exclusively by the main task for the duration of a sid invocation.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use std::sync::Arc;
/// use sid::wire::{SidApp, build_app};
/// use sid_store::{OpenStore, RedbStore};
///
/// let store = Arc::new(RedbStore::open(Path::new("/tmp/test.redb")).unwrap());
/// let app = build_app(None, vec![]);
/// let sid_app = SidApp { app, store, session_id: "sess-1".to_string() };
/// ```
pub struct SidApp {
    pub app: App,
    pub store: Arc<RedbStore>,
    pub session_id: String,
}

/// Return the path to the redb database file.
///
/// Uses `override_path` when provided; otherwise derives from the XDG/platform
/// data directory via [`directories::ProjectDirs`]. Falls back to `./sid.redb`
/// if the platform dirs cannot be determined.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid::wire::db_path;
///
/// // With an override, the override is returned unchanged.
/// let p = db_path(Some(PathBuf::from("/tmp/test.redb")));
/// assert_eq!(p, PathBuf::from("/tmp/test.redb"));
///
/// // Without an override, an XDG-derived path is returned.
/// let p = db_path(None);
/// assert!(p.to_string_lossy().contains("sid"));
/// ```
pub fn db_path(override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    if let Some(dirs) = ProjectDirs::from("dev", "sid", "sid") {
        let data = dirs.data_local_dir().to_path_buf();
        std::fs::create_dir_all(&data).ok();
        return data.join("sid.redb");
    }
    PathBuf::from("./sid.redb")
}

/// Build an [`App`] with the six Plan-1 tabs pre-wired.
///
/// Injects the real [`Git2ProviderFactory`] into the [`WorkspacesWidget`] and
/// pre-populates it with `workspaces` loaded from the store.
///
/// Optionally switches to `start_tab` if a matching tab id is found.
///
/// # Examples
///
/// ```
/// use sid::wire::build_app;
///
/// let app = build_app(None, vec![]);
/// assert_eq!(app.tabs().tabs().len(), 6);
/// assert_eq!(app.tabs().active().id.as_str(), "workspaces");
/// ```
pub fn build_app(start_tab: Option<&str>, workspaces: Vec<Workspace>) -> App {
    let git_factory = Arc::new(Git2ProviderFactory::new());
    let tabs = TabManager::new(vec![
        tab(
            "workspaces",
            "Workspaces",
            Box::new(WorkspacesWidget::new(workspaces, Some(git_factory))),
            Some('1'),
        ),
        tab("ssh", "SSH", Box::new(SshWidget::new()), Some('2')),
        tab(
            "database",
            "Database",
            Box::new(DatabaseWidget::new()),
            Some('3'),
        ),
        tab(
            "network",
            "Network",
            Box::new(NetworkWidget::new()),
            Some('4'),
        ),
        tab("system", "System", Box::new(SystemWidget::new()), Some('5')),
        tab(
            "settings",
            "Settings",
            Box::new(SettingsWidget::new()),
            Some('6'),
        ),
    ]);
    let kb = KeybindMap::cosmos_default();
    let mut reg = ActionRegistry::new();
    for a in [
        "app.quit",
        "palette.open",
        "tabs.next",
        "tabs.prev",
        "app.open_settings",
        "tab.detach",
        "tab.attach",
        "tab.reload",
    ] {
        reg.register(Action::new(a, pretty_label(a)));
    }
    for i in 1..=6 {
        reg.register(Action::new(
            format!("tabs.jump.{i}"),
            format!("Jump to tab {i}"),
        ));
    }
    let mut app = App::new(tabs, kb, reg);
    if let Some(id) = start_tab {
        let _ = app.tabs_mut().switch_to(&TabId::new(id));
    }
    app
}

fn tab(id: &str, title: &str, widget: Box<dyn Widget>, hotkey: Option<char>) -> Tab {
    Tab {
        id: TabId::new(id),
        title: title.to_string(),
        layout: Layout::Single(widget),
        hotkey,
    }
}

/// Convert a known action id to a human-readable label.
///
/// Unknown action ids are returned unchanged — this function never panics.
///
/// # Examples
///
/// ```
/// use sid::wire::pretty_label;
///
/// assert_eq!(pretty_label("app.quit"), "Quit");
/// assert_eq!(pretty_label("tabs.next"), "Next tab");
/// assert_eq!(pretty_label("unknown.action"), "unknown.action");
/// ```
pub fn pretty_label(action_id: &str) -> String {
    match action_id {
        "app.quit" => "Quit".into(),
        "palette.open" => "Open command palette".into(),
        "tabs.next" => "Next tab".into(),
        "tabs.prev" => "Previous tab".into(),
        "app.open_settings" => "Open Settings".into(),
        "tab.detach" => "Detach tab (Plan 8)".into(),
        "tab.attach" => "Attach widget (Plan 8)".into(),
        "tab.reload" => "Reload tab data".into(),
        other => other.to_string(),
    }
}

/// Persist the current active tab into the session record.
///
/// Creates or updates the session identified by `session_id`.  The session
/// record stores the active tab id and the full ordered list of open tab ids.
///
/// # Errors
///
/// Returns a [`sid_core::Error`] if the underlying store write fails (e.g.,
/// redb I/O error).
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid::wire::{build_app, save_active_tab};
/// use sid_store::{OpenStore, RedbStore};
///
/// let store = RedbStore::open(Path::new("/tmp/test.redb")).unwrap();
/// let app = build_app(Some("ssh"), vec![]);
/// save_active_tab(&store, "sess-1", &app).unwrap();
/// // The session record is now persisted; open the store again to verify.
/// ```
pub fn save_active_tab(store: &dyn Store, session_id: &str, app: &App) -> SidResult<()> {
    let sess = SessionRecord {
        id: session_id.to_string(),
        started_at: now_epoch(),
        last_active: now_epoch(),
        ended_at: None,
        active_tab: Some(app.tabs().active().id.clone()),
        open_tabs: app.tabs().tabs().iter().map(|t| t.id.clone()).collect(),
    };
    store.upsert_session(&sess)
}

/// Draw one frame: tab strip at the top, active widget body below, optional
/// palette overlay centred over everything.
///
/// Renders into the provided [`Frame`].  Uses the cosmos theme throughout.
/// This function is pure layout — it does not mutate `app`.
///
/// # Examples
///
/// ```no_run
/// use ratatui::Terminal;
/// use ratatui::backend::TestBackend;
/// use sid::wire::{build_app, draw};
///
/// let app = build_app(None, vec![]);
/// let backend = TestBackend::new(120, 40);
/// let mut terminal = Terminal::new(backend).unwrap();
/// terminal.draw(|frame| draw(frame, &app)).unwrap();
/// ```
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let theme = cosmos();
    let size = frame.area();

    // Top bar with tab labels.
    let labels: String = app
        .tabs()
        .tabs()
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let marker = if i == app.tabs().active_index() {
                '●'
            } else {
                '·'
            };
            format!("{marker} {} ", t.title)
        })
        .collect();
    let bar = Paragraph::new(labels).block(styled_block(&theme, "sid"));
    let bar_rect = Rect {
        x: 0,
        y: 0,
        width: size.width,
        height: 3,
    };
    frame.render_widget(bar, bar_rect);

    // Active widget body — stubs render a centred placeholder.
    let body_rect = Rect {
        x: 0,
        y: 3,
        width: size.width,
        height: size.height.saturating_sub(3),
    };
    let title = app.tabs().active().title.clone();
    let body =
        Paragraph::new(format!("{title}\n\n(coming soon)")).block(styled_block(&theme, "panel"));
    frame.render_widget(body, body_rect);

    // Palette overlay if open.
    if app.palette().is_open() {
        let overlay_rect = centered(size, 60, 40);
        let mut lines = vec![format!("> {}", app.palette().query())];
        for (i, a) in app.palette().matches(app.actions()).into_iter().enumerate() {
            let prefix = if i == app.palette().selected_index() {
                ">"
            } else {
                " "
            };
            lines.push(format!("{prefix} {} ({})", a.label, a.id));
        }
        let p = Paragraph::new(lines.join("\n")).block(styled_block(&theme, "command palette"));
        frame.render_widget(p, overlay_rect);
    }
}

/// Return a [`Rect`] centred within `area` that is `pct_w`% wide and `pct_h`%
/// tall.
///
/// When the computed dimensions exceed `area`, the original `area` is returned
/// unchanged (e.g., when `pct_w >= 100` or `pct_h >= 100`).  Zero-percent
/// dimensions produce a zero-size rect pinned to the centre.
///
/// # Examples
///
/// ```
/// use ratatui::layout::Rect;
/// use sid::wire::centered;
///
/// let area = Rect { x: 0, y: 0, width: 100, height: 50 };
///
/// // 100% = original area returned.
/// assert_eq!(centered(area, 100, 100), area);
///
/// // 0% = zero-size rect at the centre.
/// let z = centered(area, 0, 0);
/// assert_eq!(z.width, 0);
/// assert_eq!(z.height, 0);
///
/// // Normal usage.
/// let c = centered(area, 60, 40);
/// assert!(c.width < area.width);
/// assert!(c.height < area.height);
/// ```
pub fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = area.width.saturating_mul(pct_w.min(100)) / 100;
    let h = area.height.saturating_mul(pct_h.min(100)) / 100;
    // Guard: if computed size is larger than or equal to area, return area.
    if w >= area.width && h >= area.height {
        return area;
    }
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Return the default workspace discovery roots.
///
/// Scans `~/vcs/` if `$HOME` resolves via [`directories::UserDirs`], otherwise
/// returns an empty list (discovery is a best-effort operation).
///
/// # Examples
///
/// ```
/// use sid::wire::default_discovery_roots;
///
/// let roots = default_discovery_roots();
/// // May be empty when $HOME is unset; never panics.
/// assert!(roots.len() <= 1);
/// ```
pub fn default_discovery_roots() -> Vec<PathBuf> {
    UserDirs::new()
        .map(|u| u.home_dir().join("vcs"))
        .into_iter()
        .collect()
}

/// Scan each root for workspaces and merge discoveries into the store.
///
/// Uses [`scan_workspace_root`] with a max depth of 2, then calls
/// [`merge_discoveries_into`] with a [`WorkspaceUpserter`] adapter that
/// delegates to the store.  Discovery is best-effort: errors from scanning an
/// individual root are propagated to the caller; errors from a single upsert
/// are surfaced as `Err(String)` from [`merge_discoveries_into`].
///
/// Returns the total number of workspaces upserted across all roots.
///
/// # Errors
///
/// Returns `anyhow::Error` if any scan or upsert fails.
///
/// # Examples
///
/// ```no_run
/// use std::path::PathBuf;
/// use sid::wire::startup_discover;
/// use sid_store::{OpenStore, RedbStore};
///
/// let store = RedbStore::open(std::path::Path::new("/tmp/discover_test.redb")).unwrap();
/// let roots = vec![PathBuf::from("/tmp/vcs-roots")];
/// let count = startup_discover(&store, &roots).unwrap_or(0);
/// // count is how many workspaces were upserted.
/// let _ = count;
/// ```
pub fn startup_discover(store: &dyn Store, roots: &[PathBuf]) -> anyhow::Result<usize> {
    struct Bridge<'a> {
        store: &'a dyn Store,
    }

    impl<'a> WorkspaceUpserter for Bridge<'a> {
        fn upsert(
            &self,
            path: &Path,
            kind: WorkspaceKind,
            name: &str,
        ) -> Result<(), String> {
            let w = Workspace {
                path: path.to_path_buf(),
                name: name.to_string(),
                kind,
                manifest_hash: 0,
                last_seen: now_epoch(),
                parent: None,
            };
            self.store.upsert_workspace(&w).map_err(|e| format!("{e}"))
        }
    }

    let mut total = 0usize;
    for root in roots {
        if !root.exists() {
            continue;
        }
        let discovered = scan_workspace_root(root, 2)
            .map_err(|e| anyhow::anyhow!("scan {:?}: {e}", root))?;
        let n = merge_discoveries_into(&Bridge { store }, &discovered)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        total += n;
    }
    Ok(total)
}

/// Run the main render + event loop until the app requests to quit.
///
/// Draws each frame, waits for the next event, dispatches it through the
/// [`App`], and persists the session after each event.  The loop exits when
/// [`App::handle_event`] returns [`Dispatch::Quit`] or the event channel is
/// closed.
///
/// # Errors
///
/// Returns an error if any terminal draw call fails.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use std::sync::Arc;
/// use ratatui::Terminal;
/// use ratatui::backend::TestBackend;
/// use sid::wire::{SidApp, build_app, run_event_loop};
/// use sid_store::{OpenStore, RedbStore};
///
/// #[tokio::main]
/// async fn main() {
///     let backend = TestBackend::new(120, 40);
///     let mut terminal = Terminal::new(backend).unwrap();
///     let store = Arc::new(RedbStore::open(Path::new("/tmp/test.redb")).unwrap());
///     let app = build_app(None, vec![]);
///     let mut sid_app = SidApp { app, store, session_id: "sess-1".to_string() };
///     let (tx, mut rx) = tokio::sync::mpsc::channel(1);
///     // Drop the sender to close the channel so the loop exits immediately.
///     drop(tx);
///     run_event_loop(&mut terminal, &mut sid_app, &mut rx).await.unwrap();
/// }
/// ```
pub async fn run_event_loop<B>(
    terminal: &mut Terminal<B>,
    sid_app: &mut SidApp,
    rx: &mut Receiver<SidEvent>,
) -> Result<()>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
{
    let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
    loop {
        terminal.draw(|f| draw(f, &sid_app.app))?;
        let ev = match rx.recv().await {
            Some(e) => e,
            None => break,
        };
        let dispatch = sid_app.app.handle_event(&ev);
        let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
        if matches!(dispatch, Dispatch::Quit) {
            break;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use sid_core::tab::TabId;
    use sid_store::{OpenStore, RedbStore, Store};
    use tempfile::tempdir;

    use super::*;

    // ---- build_app ----

    /// `build_app` creates a TabManager with exactly 6 tabs in the correct order.
    #[test]
    fn build_app_has_six_tabs_in_order() {
        let app = build_app(None, vec![]);
        let ids: Vec<&str> = app.tabs().tabs().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            &[
                "workspaces",
                "ssh",
                "database",
                "network",
                "system",
                "settings"
            ]
        );
    }

    /// `build_app` defaults to the first tab (workspaces).
    #[test]
    fn build_app_defaults_to_workspaces() {
        let app = build_app(None, vec![]);
        assert_eq!(app.tabs().active().id.as_str(), "workspaces");
    }

    /// `build_app` with a valid start_tab switches to that tab.
    #[test]
    fn build_app_start_tab_switches() {
        let app = build_app(Some("settings"), vec![]);
        assert_eq!(app.tabs().active().id.as_str(), "settings");
    }

    /// `build_app` with an unknown start_tab falls back to the first tab.
    #[test]
    fn build_app_unknown_start_tab_falls_back() {
        let app = build_app(Some("does-not-exist"), vec![]);
        // switch_to returns false but doesn't panic; active stays at 0.
        assert_eq!(app.tabs().active_index(), 0);
    }

    // ---- pretty_label ----

    /// Known action ids map to friendly labels.
    #[test]
    fn pretty_label_known_actions() {
        assert_eq!(pretty_label("app.quit"), "Quit");
        assert_eq!(pretty_label("palette.open"), "Open command palette");
        assert_eq!(pretty_label("tabs.next"), "Next tab");
        assert_eq!(pretty_label("tabs.prev"), "Previous tab");
        assert_eq!(pretty_label("app.open_settings"), "Open Settings");
        assert_eq!(pretty_label("tab.detach"), "Detach tab (Plan 8)");
        assert_eq!(pretty_label("tab.attach"), "Attach widget (Plan 8)");
        assert_eq!(pretty_label("tab.reload"), "Reload tab data");
    }

    /// Unknown action ids are returned as-is.
    #[test]
    fn pretty_label_unknown_returns_action_id() {
        assert_eq!(pretty_label("unknown.action"), "unknown.action");
        assert_eq!(pretty_label(""), "");
        assert_eq!(
            pretty_label("some.deeply.nested.action.id"),
            "some.deeply.nested.action.id"
        );
    }

    // ---- centered ----

    /// `centered(area, 100, 100)` returns the original area.
    #[test]
    fn centered_100pct_returns_original() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(centered(area, 100, 100), area);
    }

    /// `centered(area, 0, 0)` returns a zero-size rect.
    #[test]
    fn centered_0pct_returns_zero_size() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let z = centered(area, 0, 0);
        assert_eq!(z.width, 0);
        assert_eq!(z.height, 0);
    }

    /// `centered` on a normal area returns something smaller than the area.
    #[test]
    fn centered_normal_is_smaller() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let c = centered(area, 60, 40);
        assert!(c.width < area.width);
        assert!(c.height < area.height);
        // The rect must fit inside area.
        assert!(c.x >= area.x);
        assert!(c.y >= area.y);
        assert!(c.x + c.width <= area.x + area.width);
        assert!(c.y + c.height <= area.y + area.height);
    }

    /// A small area with large pct still returns the area (not a zero-size rect).
    #[test]
    fn centered_small_area_large_pct_returns_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 3,
        };
        let c = centered(area, 100, 100);
        assert_eq!(c, area);
    }

    /// `centered` with a 1×1 area and 50% returns a zero-size rect.
    #[test]
    fn centered_1x1_50pct() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let c = centered(area, 50, 50);
        // 1 * 50 / 100 = 0; so width and height are 0
        assert_eq!(c.width, 0);
        assert_eq!(c.height, 0);
    }

    // ---- db_path ----

    /// `db_path(Some(p))` returns `p` unchanged.
    #[test]
    fn db_path_override_returned_unchanged() {
        let p = PathBuf::from("/tmp/explicit.redb");
        assert_eq!(db_path(Some(p.clone())), p);
    }

    /// `db_path(None)` returns an XDG-derived path containing "sid".
    #[test]
    fn db_path_none_contains_sid() {
        let p = db_path(None);
        assert!(
            p.to_string_lossy().contains("sid"),
            "XDG path should contain 'sid': {p:?}"
        );
    }

    /// `db_path(None)` returns a `.redb` path.
    #[test]
    fn db_path_none_ends_with_redb() {
        let p = db_path(None);
        assert!(
            p.to_string_lossy().ends_with(".redb"),
            "path should end with .redb: {p:?}"
        );
    }

    // ---- save_active_tab ----

    /// `save_active_tab` persists the active tab and it round-trips through the store.
    #[test]
    fn save_active_tab_round_trips() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app = build_app(Some("ssh"), vec![]);

        save_active_tab(&store, "sess-1", &app).unwrap();

        let loaded = store.current_session().unwrap().unwrap();
        assert_eq!(loaded.id, "sess-1");
        assert_eq!(loaded.active_tab.unwrap().as_str(), "ssh");
    }

    /// `save_active_tab` records all 6 open tabs.
    #[test]
    fn save_active_tab_records_open_tabs() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app = build_app(None, vec![]);

        save_active_tab(&store, "sess-2", &app).unwrap();

        let loaded = store.current_session().unwrap().unwrap();
        assert_eq!(loaded.open_tabs.len(), 6);
    }

    /// `save_active_tab` called twice overwrites the first record (upsert semantics).
    #[test]
    fn save_active_tab_upsert_semantics() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app1 = build_app(Some("ssh"), vec![]);
        let app2 = build_app(Some("database"), vec![]);

        save_active_tab(&store, "sess-3", &app1).unwrap();
        save_active_tab(&store, "sess-3", &app2).unwrap();

        let loaded = store.current_session().unwrap().unwrap();
        assert_eq!(loaded.active_tab.unwrap().as_str(), "database");
    }

    /// Adversarial: `save_active_tab` with different session IDs creates distinct records.
    #[test]
    fn save_active_tab_distinct_sessions() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app = build_app(None, vec![]);

        save_active_tab(&store, "sess-A", &app).unwrap();
        save_active_tab(&store, "sess-B", &app).unwrap();

        // current_session should point to the last upserted one.
        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"sess-A"), "should contain sess-A");
        assert!(ids.contains(&"sess-B"), "should contain sess-B");
    }

    // ---- build_app additional: all 6 tabs have titles ----

    #[test]
    fn build_app_all_tabs_have_titles() {
        let app = build_app(None, vec![]);
        let expected_titles = [
            "Workspaces",
            "SSH",
            "Database",
            "Network",
            "System",
            "Settings",
        ];
        for (tab, expected) in app.tabs().tabs().iter().zip(expected_titles.iter()) {
            assert_eq!(tab.title, *expected);
        }
    }

    /// `build_app` registers 14 actions (8 named + 6 jump).
    #[test]
    fn build_app_registers_expected_actions() {
        let app = build_app(None, vec![]);
        // 8 named + 6 jump actions
        let all: Vec<_> = app.actions().all().collect();
        assert_eq!(all.len(), 14, "expected 14 actions, got {}", all.len());
    }

    /// start_tab with "workspaces" ID stays at index 0.
    #[test]
    fn build_app_start_tab_workspaces_is_index_0() {
        let app = build_app(Some("workspaces"), vec![]);
        assert_eq!(app.tabs().active_index(), 0);
    }

    /// switch_to with each valid tab id works.
    #[test]
    fn build_app_can_switch_to_all_tabs() {
        let expected = [
            ("workspaces", 0usize),
            ("ssh", 1),
            ("database", 2),
            ("network", 3),
            ("system", 4),
            ("settings", 5),
        ];
        for (id, idx) in expected {
            let app = build_app(Some(id), vec![]);
            assert_eq!(app.tabs().active_index(), idx, "for tab id={id}");
        }
    }

    // ---- db_path: empty override doesn't fail ----

    #[test]
    fn db_path_empty_pathbuf_is_returned_as_is() {
        let p = PathBuf::from("");
        let result = db_path(Some(p.clone()));
        assert_eq!(result, p);
    }

    // ---- centered: non-zero origin ----

    #[test]
    fn centered_handles_non_zero_origin() {
        let area = Rect {
            x: 10,
            y: 5,
            width: 80,
            height: 40,
        };
        let c = centered(area, 50, 50);
        // Result must be within the area bounds.
        assert!(c.x >= area.x);
        assert!(c.y >= area.y);
        assert!(c.x + c.width <= area.x + area.width);
        assert!(c.y + c.height <= area.y + area.height);
    }

    // ---- centered: TabId round-trip ----

    #[test]
    fn tab_id_round_trips_through_save() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.redb");
        let store = RedbStore::open(&db_file).unwrap();
        let app = build_app(Some("network"), vec![]);

        save_active_tab(&store, "sess-net", &app).unwrap();
        let loaded = store.current_session().unwrap().unwrap();
        assert_eq!(
            loaded.active_tab.as_ref().map(TabId::as_str),
            Some("network")
        );
    }

    // ---- build_app adversarial: whitespace / edge-case start_tab values ----

    /// `build_app` with a start_tab containing only whitespace falls back to
    /// the default tab (no match for " " as a tab id).
    #[test]
    fn build_app_start_tab_whitespace_falls_back_to_default() {
        let app = build_app(Some("   "), vec![]);
        assert_eq!(
            app.tabs().active_index(),
            0,
            "whitespace should not match any tab"
        );
    }

    /// `build_app` with a start_tab containing a newline character falls back
    /// to the default tab — tab ids never contain newlines.
    #[test]
    fn build_app_start_tab_with_newline_falls_back_to_default() {
        let app = build_app(Some("settings\n"), vec![]);
        // "settings\n" is not a valid tab id; active index stays at 0.
        assert_eq!(
            app.tabs().active_index(),
            0,
            "newline-suffixed id should not match"
        );
    }

    /// `build_app` with a very long start_tab string does not panic.
    #[test]
    fn build_app_start_tab_very_long_does_not_panic() {
        let long = "x".repeat(100_000);
        let app = build_app(Some(&long), vec![]);
        // No known tab id matches; falls back to default.
        assert_eq!(app.tabs().active_index(), 0);
    }

    /// `build_app` with a start_tab that contains a dot does not panic and
    /// falls back to the default (dots are not part of any tab id).
    #[test]
    fn build_app_start_tab_with_dot_falls_back() {
        let app = build_app(Some("settings.extra"), vec![]);
        assert_eq!(app.tabs().active_index(), 0);
    }

    // ---- pretty_label edge cases ----

    /// `pretty_label` on an empty string returns an empty string (no panic).
    #[test]
    fn pretty_label_empty_string_returns_empty() {
        assert_eq!(pretty_label(""), "");
    }

    /// `pretty_label` on an action id that contains a dot in an unexpected
    /// position is returned unchanged.
    #[test]
    fn pretty_label_dot_in_id_returned_unchanged() {
        assert_eq!(pretty_label("a.b.c.d.e"), "a.b.c.d.e");
        assert_eq!(pretty_label(".leading.dot"), ".leading.dot");
        assert_eq!(pretty_label("trailing.dot."), "trailing.dot.");
    }

    /// `pretty_label` on a string containing a newline returns it unchanged.
    #[test]
    fn pretty_label_newline_returned_unchanged() {
        let s = "app\nquit";
        assert_eq!(
            pretty_label(s),
            s,
            "newline in action_id returned unchanged"
        );
    }

    /// `pretty_label` on a string containing unicode is returned unchanged
    /// (no known mapping).
    #[test]
    fn pretty_label_unicode_returned_unchanged() {
        let s = "app.日本語";
        assert_eq!(pretty_label(s), s);
        let s2 = "😀.action";
        assert_eq!(pretty_label(s2), s2);
    }

    // ---- centered adversarial: zero-dimension areas ----

    /// `centered` with 0-width area returns a zero-width rect (no overflow).
    #[test]
    fn centered_zero_width_area_does_not_panic() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 24,
        };
        let c = centered(area, 60, 60);
        // 0 * 60 / 100 = 0 width; must not overflow or panic.
        assert_eq!(c.width, 0);
    }

    /// `centered` with 0-height area returns a zero-height rect.
    #[test]
    fn centered_zero_height_area_does_not_panic() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 0,
        };
        let c = centered(area, 60, 60);
        assert_eq!(c.height, 0);
    }

    /// `centered` with both dimensions 0 returns a zero-size rect at the origin.
    #[test]
    fn centered_zero_both_dimensions_does_not_panic() {
        let area = Rect {
            x: 5,
            y: 3,
            width: 0,
            height: 0,
        };
        let c = centered(area, 50, 50);
        assert_eq!(c.width, 0);
        assert_eq!(c.height, 0);
    }

    /// `centered` with pct_w = 0 produces a zero-width result (even on large area).
    #[test]
    fn centered_zero_pct_w_produces_zero_width() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        };
        let c = centered(area, 0, 50);
        assert_eq!(c.width, 0, "0% width should yield width=0");
        // Height should be non-zero.
        assert!(c.height > 0, "height should be > 0 for 50%");
    }

    /// `centered` with pct_h = 0 produces a zero-height result.
    #[test]
    fn centered_zero_pct_h_produces_zero_height() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        };
        let c = centered(area, 50, 0);
        assert_eq!(c.height, 0, "0% height should yield height=0");
        assert!(c.width > 0, "width should be > 0 for 50%");
    }

    /// `centered` with pct_w = 200 (> 100) is clamped to 100; the guard
    /// returns the original area when the computed rect equals or exceeds it.
    #[test]
    fn centered_oversized_pct_returns_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        // 200% is clamped to 100% internally via `pct_w.min(100)`.
        let c = centered(area, 200, 200);
        assert_eq!(
            c, area,
            "200% should be clamped to 100% and return the area"
        );
    }

    /// `centered` where only one dimension is > 100% still uses the clamped
    /// value and may or may not return the full area depending on the other
    /// dimension.
    #[test]
    fn centered_partial_oversized_pct_clamped() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        // 200% width → clamped to 100 → w = 100 = area.width.
        // 50% height → h = 25 < area.height.
        // Guard: w >= area.width AND h >= area.height → false (h=25 < 50).
        // So we get a partially-centred rect.
        let c = centered(area, 200, 50);
        assert_eq!(
            c.width, area.width,
            "width should equal area.width at 200% clamped"
        );
        assert!(
            c.height < area.height,
            "height should be < area.height at 50%"
        );
        // Must still fit inside area.
        assert!(c.x + c.width <= area.x + area.width);
        assert!(c.y + c.height <= area.y + area.height);
    }

    // ---- draw: TestBackend smoke ----

    /// `draw` renders without panicking on a normal-sized terminal.
    #[test]
    fn draw_does_not_panic_on_normal_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let app = build_app(None, vec![]);
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
    }

    /// `draw` renders without panicking on a very small (1×1) terminal.
    #[test]
    fn draw_does_not_panic_on_tiny_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let app = build_app(None, vec![]);
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
    }

    /// `draw` renders without panicking when the terminal is smaller than the
    /// tab bar (height = 2, which is less than the 3-row bar height).
    #[test]
    fn draw_does_not_panic_when_shorter_than_bar() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let app = build_app(None, vec![]);
        // Height 2 < bar height 3; body_rect will have saturating_sub(3) = 0 height.
        let backend = TestBackend::new(80, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
    }

    /// `draw` renders all six tabs without panicking.
    #[test]
    fn draw_all_tabs_render_without_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for tab_id in [
            "workspaces",
            "ssh",
            "database",
            "network",
            "system",
            "settings",
        ] {
            let app = build_app(Some(tab_id), vec![]);
            let backend = TestBackend::new(120, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw(frame, &app))
                .unwrap_or_else(|e| panic!("draw panicked for tab '{tab_id}': {e}"));
        }
    }
}
