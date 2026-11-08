//! Structured diffs: all four targets, renames, binary, and the hard caps.

mod common;

use common::*;
use sid_core::git::{DiffLimits, DiffLineKind, DiffStatus, DiffTarget, GitError};

#[test]
fn workdir_vs_index_has_hunks_lines_and_numbers() {
    let (dir, _repo) = seeded();
    write(dir.path(), "a.txt", "v1\nv2\n");
    let p = open(dir.path());
    let d = p
        .diff(&DiffTarget::WorkdirVsIndex, &DiffLimits::default())
        .unwrap();
    assert_eq!(d.files.len(), 1);
    assert!(!d.truncated);
    let f = &d.files[0];
    assert_eq!(f.status, DiffStatus::Modified);
    assert_eq!((f.additions, f.deletions), (1, 0));
    assert!(!f.binary);
    let h = &f.hunks[0];
    assert!(h.header.starts_with("@@ -1"));
    assert_eq!(
        (h.old_start, h.old_lines, h.new_start, h.new_lines),
        (1, 1, 1, 2)
    );
    assert_eq!(h.lines.len(), 2);
    assert_eq!(h.lines[0].kind, DiffLineKind::Context);
    assert_eq!(
        (h.lines[0].old_lineno, h.lines[0].new_lineno),
        (Some(1), Some(1))
    );
    assert_eq!(h.lines[1].kind, DiffLineKind::Added);
    assert_eq!(
        (h.lines[1].old_lineno, h.lines[1].new_lineno),
        (None, Some(2))
    );
    assert_eq!(h.lines[1].text, "v2");
}

#[test]
fn workdir_diff_includes_untracked_as_additions() {
    let (dir, _repo) = seeded();
    write(dir.path(), "new.txt", "hello\n");
    let p = open(dir.path());
    let d = p
        .diff(&DiffTarget::WorkdirVsIndex, &DiffLimits::default())
        .unwrap();
    let f = &d.files[0];
    assert_eq!(f.display_path(), "new.txt");
    assert!(f.old_path.is_none());
    assert_eq!(f.additions, 1);
}

#[test]
fn index_vs_head_shows_staged_only() {
    let (dir, _repo) = seeded();
    write(dir.path(), "a.txt", "staged\n");
    let mut p = open(dir.path());
    p.stage_paths(&["a.txt".into()]).unwrap();
    write(dir.path(), "a.txt", "staged\nmore\n");
    let d = p
        .diff(&DiffTarget::IndexVsHead, &DiffLimits::default())
        .unwrap();
    assert_eq!(d.files.len(), 1);
    let texts: Vec<_> = d.files[0].hunks[0]
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(texts, ["v1", "staged"]);
}

#[test]
fn index_vs_head_on_unborn_head_is_all_additions() {
    let dir = tempfile::tempdir().unwrap();
    let _repo = init_repo(dir.path());
    write(dir.path(), "a.txt", "x\n");
    let mut p = open(dir.path());
    p.stage_all().unwrap();
    let d = p
        .diff(&DiffTarget::IndexVsHead, &DiffLimits::default())
        .unwrap();
    assert_eq!(d.files[0].status, DiffStatus::Added);
}

#[test]
fn commit_diff_against_parent_and_root_commit() {
    let (dir, repo) = seeded();
    let root = repo.head().unwrap().target().unwrap().to_string();
    write(dir.path(), "a.txt", "v1\nv2\n");
    let second = commit_all(&repo, "second").to_string();
    let p = open(dir.path());

    let d = p
        .diff(&DiffTarget::Commit(second), &DiffLimits::default())
        .unwrap();
    assert_eq!(d.files.len(), 1);
    assert_eq!(d.files[0].additions, 1);

    let d = p
        .diff(&DiffTarget::Commit(root), &DiffLimits::default())
        .unwrap();
    assert_eq!(d.files[0].status, DiffStatus::Added);

    assert!(matches!(
        p.diff(&DiffTarget::Commit("nope".into()), &DiffLimits::default()),
        Err(GitError::InvalidRef(_))
    ));
}

#[test]
fn diff_between_two_refs_with_rename() {
    let (dir, repo) = seeded();
    let base = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("feature", &base, false).unwrap();
    // Give the rename enough content to be similarity-detected.
    let body: String = (0..20).map(|i| format!("line {i}\n")).collect();
    write(dir.path(), "a.txt", &body);
    commit_all(&repo, "bulk up a");
    repo.set_head("refs/heads/feature").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    write(dir.path(), "a.txt", &body);
    commit_all(&repo, "same on feature");
    std::fs::rename(dir.path().join("a.txt"), dir.path().join("renamed.txt")).unwrap();
    commit_all(&repo, "rename");

    let p = open(dir.path());
    let d = p
        .diff(
            &DiffTarget::Refs {
                from: "main".into(),
                to: "feature".into(),
            },
            &DiffLimits::default(),
        )
        .unwrap();
    assert_eq!(d.files.len(), 1);
    let f = &d.files[0];
    assert_eq!(f.status, DiffStatus::Renamed);
    assert_eq!(f.old_path.as_deref(), Some("a.txt"));
    assert_eq!(f.new_path.as_deref(), Some("renamed.txt"));

    assert!(matches!(
        p.diff(
            &DiffTarget::Refs {
                from: "main".into(),
                to: "ghost".into()
            },
            &DiffLimits::default()
        ),
        Err(GitError::InvalidRef(_))
    ));
}

#[test]
fn binary_files_have_no_hunks() {
    let (dir, _repo) = seeded();
    std::fs::write(dir.path().join("blob.bin"), [0u8, 159, 146, 150, 0, 0, 1]).unwrap();
    let p = open(dir.path());
    let d = p
        .diff(&DiffTarget::WorkdirVsIndex, &DiffLimits::default())
        .unwrap();
    let f = d
        .files
        .iter()
        .find(|f| f.display_path() == "blob.bin")
        .unwrap();
    assert!(f.binary);
    assert!(f.hunks.is_empty());
}

#[test]
fn caps_set_truncated_flags() {
    let (dir, _repo) = seeded();
    for i in 0..5 {
        write(dir.path(), &format!("f{i}.txt"), "x\n");
    }
    let p = open(dir.path());

    // max_files
    let d = p
        .diff(
            &DiffTarget::WorkdirVsIndex,
            &DiffLimits {
                max_files: 2,
                ..DiffLimits::default()
            },
        )
        .unwrap();
    assert_eq!(d.files.len(), 2);
    assert_eq!(d.total_files, 5);
    assert!(d.truncated);

    // max_bytes_per_file
    let big: String = (0..200)
        .map(|i| format!("this is line number {i}\n"))
        .collect();
    write(dir.path(), "big.txt", &big);
    let d = p
        .diff(
            &DiffTarget::WorkdirVsIndex,
            &DiffLimits {
                max_bytes_per_file: 100,
                ..DiffLimits::default()
            },
        )
        .unwrap();
    let f = d
        .files
        .iter()
        .find(|f| f.display_path() == "big.txt")
        .unwrap();
    assert!(f.truncated && d.truncated);
    assert_eq!(
        f.additions, 200,
        "totals are counted even when lines are cut"
    );
    let kept: usize = f
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .map(|l| l.text.len())
        .sum();
    assert!(kept <= 100);

    // max_total_bytes
    let d = p
        .diff(
            &DiffTarget::WorkdirVsIndex,
            &DiffLimits {
                max_total_bytes: 10,
                ..DiffLimits::default()
            },
        )
        .unwrap();
    assert!(d.truncated);
}

#[test]
fn clean_tree_diffs_empty() {
    let (dir, _repo) = seeded();
    let p = open(dir.path());
    for t in [DiffTarget::WorkdirVsIndex, DiffTarget::IndexVsHead] {
        let d = p.diff(&t, &DiffLimits::default()).unwrap();
        assert!(d.files.is_empty() && !d.truncated);
    }
}
