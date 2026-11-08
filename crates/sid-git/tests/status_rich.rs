//! Rich status: folded per-path rows, counts, typechange, rename, conflicts, ignored.

mod common;

use common::*;
use sid_core::git::{StatusKind, StatusOptions};

#[test]
fn both_sides_fold_into_one_row_with_counts() {
    let (dir, _repo) = seeded();
    write(dir.path(), "a.txt", "v2\n");
    let mut p = open(dir.path());
    p.stage_paths(&["a.txt".into()]).unwrap();
    write(dir.path(), "a.txt", "v3\n");
    write(dir.path(), "new.txt", "n");

    let s = p.status().unwrap();
    assert!(!s.is_clean);
    let a = s.files.iter().find(|f| f.path == "a.txt").unwrap();
    assert_eq!(a.index, Some(StatusKind::Modified));
    assert_eq!(a.worktree, Some(StatusKind::Modified));
    let n = s.files.iter().find(|f| f.path == "new.txt").unwrap();
    assert!(n.is_untracked() && n.index.is_none());
    assert_eq!(s.counts.staged, 1);
    assert_eq!(s.counts.unstaged, 1);
    assert_eq!(s.counts.untracked, 1);
    // The per-side rows are still there: a.txt twice, new.txt once.
    assert_eq!(s.entries.len(), 3);
}

#[test]
fn staged_rename_reports_old_path() {
    let (dir, _repo) = seeded();
    std::fs::rename(dir.path().join("a.txt"), dir.path().join("b.txt")).unwrap();
    let mut p = open(dir.path());
    p.stage_all().unwrap();
    let s = p.status().unwrap();
    let f = s.files.iter().find(|f| f.path == "b.txt").unwrap();
    assert_eq!(f.index, Some(StatusKind::Renamed));
    assert_eq!(f.old_path.as_deref(), Some("a.txt"));
}

#[test]
fn deleted_in_worktree_and_in_index() {
    let (dir, _repo) = seeded();
    std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    let mut p = open(dir.path());
    let f = &p.status().unwrap().files[0];
    assert_eq!((f.index, f.worktree), (None, Some(StatusKind::Deleted)));
    p.stage_paths(&["a.txt".into()]).unwrap();
    let f = &p.status().unwrap().files[0];
    assert_eq!((f.index, f.worktree), (Some(StatusKind::Deleted), None));
}

#[cfg(unix)]
#[test]
fn typechange_is_reported() {
    let (dir, _repo) = seeded();
    std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    std::os::unix::fs::symlink("elsewhere", dir.path().join("a.txt")).unwrap();
    let p = open(dir.path());
    let f = &p.status().unwrap().files[0];
    assert_eq!(f.worktree, Some(StatusKind::Typechange));
}

#[test]
fn ignored_is_off_by_default_and_opt_in() {
    let (dir, _repo) = seeded();
    write(dir.path(), ".gitignore", "*.log\n");
    write(dir.path(), "x.log", "noise");
    let p = open(dir.path());
    let s = p.status().unwrap();
    assert!(s.files.iter().all(|f| f.path != "x.log"));
    assert_eq!(s.counts.ignored, 0);

    let s = p
        .status_with(&StatusOptions {
            include_untracked: true,
            include_ignored: true,
        })
        .unwrap();
    let x = s.files.iter().find(|f| f.path == "x.log").unwrap();
    assert_eq!(x.worktree, Some(StatusKind::Ignored));
    assert_eq!(s.counts.ignored, 1);
}

#[test]
fn untracked_can_be_excluded() {
    let (dir, _repo) = seeded();
    write(dir.path(), "new.txt", "n");
    let p = open(dir.path());
    let s = p
        .status_with(&StatusOptions {
            include_untracked: false,
            include_ignored: false,
        })
        .unwrap();
    assert!(s.is_clean);
}

#[test]
fn conflicted_path_is_flagged() {
    let (dir, repo) = seeded();
    // Build a real merge conflict: main and topic both edit a.txt.
    let base = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("topic", &base, false).unwrap();
    write(dir.path(), "a.txt", "main-side\n");
    commit_all(&repo, "main edit");
    repo.set_head("refs/heads/topic").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    write(dir.path(), "a.txt", "topic-side\n");
    commit_all(&repo, "topic edit");
    let main = repo.find_branch("main", git2::BranchType::Local).unwrap();
    let ann = repo.reference_to_annotated_commit(main.get()).unwrap();
    repo.merge(&[&ann], None, None).unwrap();

    let p = open(dir.path());
    let s = p.status().unwrap();
    assert_eq!(s.counts.conflicted, 1);
    assert!(
        s.files
            .iter()
            .any(|f| f.is_conflicted() && f.path == "a.txt")
    );
}
