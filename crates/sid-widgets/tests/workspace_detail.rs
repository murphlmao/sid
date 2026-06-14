//! Tests for `WorkspaceDetailWidget` (UX-v2 rework) — satellite rows,
//! navigation, rendering, plus the still-public `format_age` / `CiStatus`.

use std::path::PathBuf;

use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::{
    RepoGit, SatelliteRow,
    workspace_detail::{CiStatus, WorkspaceDetailWidget, format_age, render_to_string},
};

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

fn sat(name: &str, branch: &str, dirty: u32) -> SatelliteRow {
    SatelliteRow {
        name: name.into(),
        path: PathBuf::from(format!("/vcs/x/{name}")),
        is_umbrella: false,
        git: RepoGit::loaded(branch.into(), dirty, 0, 0),
    }
}

#[test]
fn scanning_state_shows_hint() {
    let w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    let s = render_to_string(&w, 100, 30);
    assert!(s.contains("scanning"), "expected scanning hint:\n{s}");
}

#[test]
fn apply_satellites_clears_scanning_flag() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    assert!(w.is_scanning());
    w.apply_satellites(vec![]);
    assert!(!w.is_scanning());
}

#[test]
fn render_shows_each_satellite_row() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/eggsight-stack", "eggsight-stack"), None);
    w.apply_satellites(vec![
        sat("frontend", "main", 12),
        sat("backend", "feature/x", 0),
        sat("infra", "main", 0),
    ]);
    let s = render_to_string(&w, 120, 40);
    for name in ["frontend", "backend", "infra"] {
        assert!(s.contains(name), "row {name} missing:\n{s}");
    }
    assert!(s.contains("eggsight-stack"), "umbrella name missing:\n{s}");
}

#[test]
fn select_next_saturates_at_bottom() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![sat("a", "main", 0), sat("b", "main", 0)]);
    // umbrella row first
    assert_eq!(w.selected_row().unwrap().name, "x");
    w.select_next();
    assert_eq!(w.selected_row().unwrap().name, "a");
    w.select_next();
    assert_eq!(w.selected_row().unwrap().name, "b");
    w.select_next(); // ListCursor::down saturates
    assert_eq!(w.selected_row().unwrap().name, "b");
}

#[test]
fn select_prev_saturates_at_top() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![sat("a", "main", 0), sat("b", "main", 0)]);
    w.select_next();
    w.select_next();
    assert_eq!(w.selected_row().unwrap().name, "b");
    w.select_prev();
    w.select_prev();
    w.select_prev(); // saturates at the umbrella row
    assert_eq!(w.selected_row().unwrap().name, "x");
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

// Adversarial cases

#[test]
fn select_on_lone_umbrella_is_noop() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![]); // only the umbrella row remains
    w.select_next(); // must not panic on the single-row list
    w.select_prev();
    assert_eq!(w.selected_row().unwrap().name, "x");
}

#[test]
fn apply_satellites_keeps_selection_in_range_on_smaller_set() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![
        sat("a", "main", 0),
        sat("b", "main", 0),
        sat("c", "main", 0),
    ]);
    w.select_next();
    w.select_next();
    w.select_next();
    assert_eq!(w.selected_row().unwrap().name, "c");
    // A rescan returns fewer rows; the cursor re-clamps.
    w.apply_satellites(vec![sat("a", "main", 0)]);
    assert_eq!(
        w.selected_row().map(|r| r.name.as_str()),
        Some("a"),
        "ListCursor must clamp to last valid row on shrink"
    );
}

#[test]
fn ci_status_glyphs() {
    assert_eq!(CiStatus::Pending.glyph(), "*");
    assert_eq!(CiStatus::Pass.glyph(), "✓");
    assert_eq!(CiStatus::Fail.glyph(), "x");
    assert_eq!(CiStatus::Unknown.glyph(), "-");
}

// ── Fix 1: Tab consumed while pane focused ──────────────────────────────────

#[test]
fn tab_is_consumed_while_pane_focused() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::{
        context::WidgetCtx,
        event::{Event, KeyChord},
        widget::{EventOutcome, Widget},
    };
    use sid_widgets::split_view::SplitFocus;

    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![sat("api", "main", 0)]);
    // Drill into pane via Right
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    let enter = Event::Key(KeyChord {
        code: KeyCode::Right,
        mods: KeyModifiers::NONE,
    });
    let _ = w.handle_event(&enter, &mut ctx);
    assert_eq!(w.split().focus(), SplitFocus::Pane);

    let tab_ev = Event::Key(KeyChord {
        code: KeyCode::Tab,
        mods: KeyModifiers::NONE,
    });
    let outcome = w.handle_event(&tab_ev, &mut ctx);
    assert_eq!(
        outcome,
        EventOutcome::Consumed,
        "Tab must be consumed in Pane focus to prevent tab-strip cycling"
    );
}

// ── Fix 2: Diff scroll clamps at last line ──────────────────────────────────

#[test]
fn diff_scroll_clamps_at_last_line() {
    use sid_core::adapters::git::{CommitInfo, DiffEntry};
    use sid_widgets::RepoDetail;

    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![sat("api", "main", 0)]);
    // Load a detail with a 3-line diff patch
    let detail = RepoDetail {
        commits: vec![CommitInfo {
            oid: "abc".into(),
            summary: "fix".into(),
            author_name: "a".into(),
            author_email: "a@b".into(),
            timestamp_secs: 0,
            parents: vec![],
        }],
        diff: vec![DiffEntry {
            path: "a.rs".into(),
            old_path: None,
            patch: "line1\nline2\nline3".into(),
            added: 0,
            removed: 0,
        }],
        ..RepoDetail::default()
    };
    w.apply_detail(detail);
    // Scroll way past the end
    for _ in 0..100 {
        w.diff_scroll_down();
    }
    // Must clamp to last line index (3 lines → max index = 2)
    assert_eq!(w.diff_scroll(), 2, "scroll must clamp to last line index");
    // Render must not panic
    let s = render_to_string(&w, 120, 40);
    let _ = s; // render succeeded without panic
}

// ── Fix 3: Pop from Diff preserves commit cursor ────────────────────────────

#[test]
fn pop_from_diff_preserves_commit_cursor() {
    use sid_core::adapters::git::CommitInfo;
    use sid_widgets::{DetailOp, RepoDetail};

    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![sat("api", "main", 0)]);
    // Enter the op first so the stack and pane context are correct.
    w.enter_op(DetailOp::Outgoing);
    let detail = RepoDetail {
        commits: vec![
            CommitInfo {
                oid: "a".into(),
                summary: "first".into(),
                author_name: "a".into(),
                author_email: "a@b".into(),
                timestamp_secs: 0,
                parents: vec![],
            },
            CommitInfo {
                oid: "b".into(),
                summary: "second".into(),
                author_name: "a".into(),
                author_email: "a@b".into(),
                timestamp_secs: 0,
                parents: vec![],
            },
        ],
        ..RepoDetail::default()
    };
    w.apply_detail(detail);
    // Advance pane cursor to the second commit
    w.pane_next();
    assert_eq!(w.selected_commit_index(), Some(1));
    // Drill into commit diff
    w.drill_into_commit();
    // Pop back out
    w.pop_view();
    // Cursor must still point at commit index 1
    assert_eq!(
        w.selected_commit_index(),
        Some(1),
        "commit cursor must survive pop_view"
    );
}

#[test]
fn jk_in_diff_scrolls_diff_not_commit_cursor() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::{
        adapters::git::{CommitInfo, DiffEntry},
        context::WidgetCtx,
        event::{Event, KeyChord},
        widget::Widget,
    };
    use sid_widgets::{DetailOp, RepoDetail};

    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![sat("api", "main", 0)]);
    // Enter the op so the pane stack is correct.
    w.enter_op(DetailOp::Outgoing);
    let detail = RepoDetail {
        commits: vec![
            CommitInfo {
                oid: "a".into(),
                summary: "first".into(),
                author_name: "a".into(),
                author_email: "a@b".into(),
                timestamp_secs: 0,
                parents: vec![],
            },
            CommitInfo {
                oid: "b".into(),
                summary: "second".into(),
                author_name: "a".into(),
                author_email: "a@b".into(),
                timestamp_secs: 0,
                parents: vec![],
            },
        ],
        diff: vec![DiffEntry {
            path: "x.rs".into(),
            old_path: None,
            patch: "line1\nline2\nline3\nline4\nline5".into(),
            added: 0,
            removed: 0,
        }],
        ..RepoDetail::default()
    };
    w.apply_detail(detail);
    // Advance to second commit, drill in
    w.pane_next();
    let commit_idx_before = w.selected_commit_index();
    w.drill_into_commit();
    // Now in Diff view — send j
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    let j = Event::Key(KeyChord {
        code: KeyCode::Char('j'),
        mods: KeyModifiers::NONE,
    });
    let _ = w.handle_event(&j, &mut ctx);
    // diff_scroll should advance
    assert_eq!(w.diff_scroll(), 1, "j in Diff view must scroll diff");
    // Pop back out and check commit cursor did not move
    w.pop_view();
    assert_eq!(
        w.selected_commit_index(),
        commit_idx_before,
        "pane cursor must not move while in Diff view"
    );
}

// ── Fix 5: Scroll-into-view keeps last row visible ──────────────────────────

#[test]
fn scroll_into_view_keeps_last_row_visible() {
    // Create many satellites so that not all fit in a small viewport.
    let satellites: Vec<_> = (0..20)
        .map(|i| sat(&format!("repo{i:02}"), "main", 0))
        .collect();
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(satellites);
    // Select the last row
    for _ in 0..20 {
        w.select_next();
    }
    let last_name = w.selected_row().unwrap().name.clone();
    // Render in a small viewport (10 rows — fewer than 21 total items)
    let s = render_to_string(&w, 80, 10);
    assert!(
        s.contains(&last_name),
        "last selected row '{}' must appear in the rendered output:\n{}",
        last_name,
        s
    );
}

// ── Fix 6b: Empty satellite state renders without panic ─────────────────────

#[test]
fn empty_satellite_state_renders_umbrella_and_no_panic() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_satellites(vec![]); // scanning completes with zero satellites
    // Must not panic; umbrella row must appear
    let s = render_to_string(&w, 80, 20);
    assert!(
        s.contains("x"),
        "umbrella name must appear in empty-satellite render:\n{s}"
    );
    // Must not say "scanning" anymore
    assert!(
        !s.contains("scanning"),
        "scanning hint must clear after apply_satellites:\n{s}"
    );
}
