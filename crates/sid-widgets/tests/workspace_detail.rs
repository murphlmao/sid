//! Tests for `WorkspaceDetailWidget` — types, navigation, and rendering.

use std::path::PathBuf;

use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspace_detail::{
    CiStatus, RepoSummary, WorkspaceDetailWidget, format_age, render_to_string,
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
    let s = render_to_string(&w, 200, 30);
    assert!(
        s.contains("no sub-repos found"),
        "expected empty-state hint:\n{s}",
    );
}

#[test]
fn render_shows_each_subrepo_row() {
    let mut w =
        WorkspaceDetailWidget::new(umbrella("/vcs/eggsight-stack", "eggsight-stack"), None);
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
fn select_prev_wraps_to_last() {
    let mut w = WorkspaceDetailWidget::new(umbrella("/vcs/x", "x"), None);
    w.apply_scan_results(vec![
        summary("a", "main", 0, 0, 60),
        summary("b", "main", 0, 0, 60),
        summary("c", "main", 0, 0, 60),
    ]);
    assert_eq!(w.selected_index(), 0);
    w.select_prev();
    assert_eq!(w.selected_index(), 2);
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
fn missing_path_still_renders_empty_state_after_scan_completes() {
    let mut w = WorkspaceDetailWidget::new(
        umbrella("/nonexistent/path/never/exists", "ghost"),
        None,
    );
    // The binary's scan job returns an empty vec on filesystem error.
    w.apply_scan_results(vec![]);
    let s = render_to_string(&w, 200, 30);
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

#[test]
fn ci_status_glyphs() {
    assert_eq!(CiStatus::Pending.glyph(), "*");
    assert_eq!(CiStatus::Pass.glyph(), "✓");
    assert_eq!(CiStatus::Fail.glyph(), "x");
    assert_eq!(CiStatus::Unknown.glyph(), "-");
}
