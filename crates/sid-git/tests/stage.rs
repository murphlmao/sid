//! Stage / unstage / discard / remove-untracked.

mod common;

use common::*;
use sid_core::git::{GitError, StatusKind};

#[test]
fn stage_and_unstage_a_path() {
    let (dir, _repo) = seeded();
    write(dir.path(), "a.txt", "v2\n");
    write(dir.path(), "b.txt", "b\n");
    let mut p = open(dir.path());
    p.stage_paths(&["a.txt".into(), "b.txt".into()]).unwrap();
    let s = p.status().unwrap();
    assert_eq!(s.counts.staged, 2);
    assert_eq!(s.counts.unstaged + s.counts.untracked, 0);

    p.unstage_paths(&["a.txt".into()]).unwrap();
    let s = p.status().unwrap();
    let a = s.files.iter().find(|f| f.path == "a.txt").unwrap();
    assert_eq!((a.index, a.worktree), (None, Some(StatusKind::Modified)));
    let b = s.files.iter().find(|f| f.path == "b.txt").unwrap();
    assert_eq!(b.index, Some(StatusKind::Added));
}

#[test]
fn stage_all_takes_untracked_and_deletions_and_unstage_all_undoes() {
    let (dir, _repo) = seeded();
    write(dir.path(), "new/deep.txt", "x");
    std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    let mut p = open(dir.path());
    p.stage_all().unwrap();
    let s = p.status().unwrap();
    assert_eq!(s.counts.staged, 2);
    assert_eq!(s.counts.unstaged + s.counts.untracked, 0);

    p.unstage_all().unwrap();
    let s = p.status().unwrap();
    assert_eq!(s.counts.staged, 0);
    assert_eq!(s.counts.untracked, 1);
    assert_eq!(s.counts.unstaged, 1);
}

#[test]
fn stage_a_directory_takes_everything_under_it() {
    let (dir, _repo) = seeded();
    write(dir.path(), "d/one.txt", "1");
    write(dir.path(), "d/two.txt", "2");
    let mut p = open(dir.path());
    p.stage_paths(&["d".into()]).unwrap();
    assert_eq!(p.status().unwrap().counts.staged, 2);
}

#[test]
fn unstage_on_unborn_head_removes_from_index() {
    let dir = tempfile::tempdir().unwrap();
    let _repo = init_repo(dir.path());
    write(dir.path(), "a.txt", "x");
    let mut p = open(dir.path());
    p.stage_paths(&["a.txt".into()]).unwrap();
    assert_eq!(p.status().unwrap().counts.staged, 1);
    p.unstage_paths(&["a.txt".into()]).unwrap();
    let s = p.status().unwrap();
    assert_eq!((s.counts.staged, s.counts.untracked), (0, 1));
    p.stage_all().unwrap();
    p.unstage_all().unwrap();
    assert_eq!(p.status().unwrap().counts.staged, 0);
}

#[test]
fn discard_restores_modified_and_deleted_files() {
    let (dir, _repo) = seeded();
    write(dir.path(), "a.txt", "scribble\n");
    let mut p = open(dir.path());
    p.discard_paths(&["a.txt".into()]).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "v1\n"
    );
    assert!(p.status().unwrap().is_clean);

    std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    p.discard_paths(&["a.txt".into()]).unwrap();
    assert!(dir.path().join("a.txt").exists());
}

#[test]
fn discard_keeps_staged_content_in_the_index() {
    let (dir, _repo) = seeded();
    write(dir.path(), "a.txt", "staged\n");
    let mut p = open(dir.path());
    p.stage_paths(&["a.txt".into()]).unwrap();
    write(dir.path(), "a.txt", "unstaged\n");
    p.discard_paths(&["a.txt".into()]).unwrap();
    // The worktree goes back to the INDEX version, not HEAD.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "staged\n"
    );
}

#[test]
fn discard_refuses_untracked_and_remove_untracked_refuses_tracked() {
    let (dir, _repo) = seeded();
    write(dir.path(), "u.txt", "u");
    let mut p = open(dir.path());
    assert!(matches!(
        p.discard_paths(&["u.txt".into()]),
        Err(GitError::InvalidRef(_))
    ));
    assert!(matches!(
        p.remove_untracked(&["a.txt".into()]),
        Err(GitError::InvalidRef(_))
    ));
    // Nothing was touched by either refusal.
    assert!(dir.path().join("u.txt").exists() && dir.path().join("a.txt").exists());
}

#[test]
fn remove_untracked_deletes_files_and_dirs() {
    let (dir, _repo) = seeded();
    write(dir.path(), "u.txt", "u");
    write(dir.path(), "junk/x.txt", "x");
    let mut p = open(dir.path());
    p.remove_untracked(&["u.txt".into(), "junk".into()])
        .unwrap();
    assert!(!dir.path().join("u.txt").exists());
    assert!(!dir.path().join("junk").exists());
    assert!(p.status().unwrap().is_clean);
}

#[test]
fn paths_escaping_the_repo_are_refused() {
    let (dir, _repo) = seeded();
    let outside = dir.path().parent().unwrap().join("sid-git-escape-probe");
    std::fs::write(&outside, "keep").unwrap();
    let mut p = open(dir.path());
    for bad in ["../sid-git-escape-probe", "/etc/hostname", ""] {
        assert!(
            matches!(p.stage_paths(&[bad.into()]), Err(GitError::InvalidRef(_))),
            "{bad}"
        );
        assert!(
            matches!(
                p.remove_untracked(&[bad.into()]),
                Err(GitError::InvalidRef(_))
            ),
            "{bad}"
        );
        assert!(
            matches!(p.discard_paths(&[bad.into()]), Err(GitError::InvalidRef(_))),
            "{bad}"
        );
        assert!(
            matches!(p.unstage_paths(&[bad.into()]), Err(GitError::InvalidRef(_))),
            "{bad}"
        );
    }
    assert!(outside.exists());
    std::fs::remove_file(outside).unwrap();
}
