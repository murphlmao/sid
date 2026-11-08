//! Stash: list/push/apply/pop/drop and the error paths.

mod common;

use common::*;
use sid_core::git::GitError;

#[test]
fn push_list_apply_pop_drop() {
    let (dir, _repo) = seeded();
    let mut p = open(dir.path());
    assert!(p.stash_list().unwrap().is_empty());

    write(dir.path(), "a.txt", "work1\n");
    let e = p.stash_push(Some("first work"), false).unwrap();
    assert_eq!(e.index, 0);
    assert_eq!(e.branch.as_deref(), Some("main"));
    assert!(e.message.contains("first work"));
    assert_eq!(e.oid.len(), 40);
    assert!(p.status().unwrap().is_clean, "push cleans the tree");

    write(dir.path(), "a.txt", "work2\n");
    p.stash_push(None, false).unwrap();
    let list = p.stash_list().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].message.starts_with("WIP on main"));
    assert_eq!(list[0].branch.as_deref(), Some("main"));
    assert!(list[1].message.contains("first work"));

    // apply keeps the entry
    p.stash_apply(1).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "work1\n"
    );
    assert_eq!(p.stash_list().unwrap().len(), 2);
    p.discard_paths(&["a.txt".into()]).unwrap();

    // pop removes it
    p.stash_pop(0).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "work2\n"
    );
    assert_eq!(p.stash_list().unwrap().len(), 1);

    p.stash_drop(0).unwrap();
    assert!(p.stash_list().unwrap().is_empty());
}

#[test]
fn include_untracked_controls_whether_new_files_are_stashed() {
    let (dir, _repo) = seeded();
    let mut p = open(dir.path());
    write(dir.path(), "a.txt", "mod\n");
    write(dir.path(), "u.txt", "u");
    p.stash_push(None, false).unwrap();
    assert!(
        dir.path().join("u.txt").exists(),
        "untracked stays without the flag"
    );
    p.stash_drop(0).unwrap();

    write(dir.path(), "a.txt", "mod\n");
    p.stash_push(None, true).unwrap();
    assert!(!dir.path().join("u.txt").exists());
    p.stash_pop(0).unwrap();
    assert!(dir.path().join("u.txt").exists());
}

#[test]
fn nothing_to_stash_and_bad_index_errors() {
    let (dir, _repo) = seeded();
    let mut p = open(dir.path());
    assert!(matches!(
        p.stash_push(None, false),
        Err(GitError::NothingToStash)
    ));
    assert!(matches!(p.stash_apply(0), Err(GitError::StashNotFound(0))));
    assert!(matches!(p.stash_pop(3), Err(GitError::StashNotFound(3))));
    assert!(matches!(p.stash_drop(0), Err(GitError::StashNotFound(0))));
}

#[test]
fn conflicting_apply_errors_and_pop_keeps_the_entry() {
    let (dir, _repo) = seeded();
    let mut p = open(dir.path());
    write(dir.path(), "a.txt", "stashed side\n");
    p.stash_push(Some("s"), false).unwrap();
    // Diverge: commit a different change to the same line.
    write(dir.path(), "a.txt", "committed side\n");
    p.stage_all().unwrap();
    p.commit("diverge", false).unwrap();

    let err = p.stash_pop(0).unwrap_err();
    assert!(matches!(err, GitError::Conflicts(_)), "{err:?}");
    assert_eq!(
        p.stash_list().unwrap().len(),
        1,
        "a failed pop must not drop the stash"
    );
}

#[test]
fn apply_over_colliding_local_edit_is_refused_and_tree_untouched() {
    let (dir, _repo) = seeded();
    let mut p = open(dir.path());
    write(dir.path(), "a.txt", "stashed\n");
    p.stash_push(None, false).unwrap();
    write(dir.path(), "a.txt", "local edit\n");
    let err = p.stash_apply(0).unwrap_err();
    assert!(matches!(err, GitError::Conflict(_)), "{err:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "local edit\n"
    );
}
