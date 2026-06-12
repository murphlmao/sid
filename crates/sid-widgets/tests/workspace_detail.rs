//! Tests for `WorkspaceDetailWidget` (UX-v2 rework) — satellite rows,
//! navigation, rendering, plus the still-public `format_age` / `CiStatus`.

use std::path::PathBuf;

use sid_widgets::workspace_detail::{
    CiStatus, WorkspaceDetailWidget, format_age, render_to_string,
};
use sid_widgets::{RepoGit, SatelliteRow};

use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;

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
    assert!(w.selected_row().is_some());
}

#[test]
fn ci_status_glyphs() {
    assert_eq!(CiStatus::Pending.glyph(), "*");
    assert_eq!(CiStatus::Pass.glyph(), "✓");
    assert_eq!(CiStatus::Fail.glyph(), "x");
    assert_eq!(CiStatus::Unknown.glyph(), "-");
}
