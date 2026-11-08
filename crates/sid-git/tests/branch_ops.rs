//! Branch create / delete / rename.

mod common;

use common::*;
use sid_core::git::GitError;

#[test]
fn create_from_head_and_from_a_ref() {
    let (dir, repo) = seeded();
    let first = repo.head().unwrap().target().unwrap();
    write(dir.path(), "a.txt", "v2\n");
    let second = commit_all(&repo, "second");
    let mut p = open(dir.path());

    let b = p.create_branch("topic", None).unwrap();
    assert_eq!(b.name, "topic");
    assert_eq!(b.head_oid, second.to_string());
    assert!(!b.is_current, "create does not switch");

    let b = p.create_branch("old", Some(&first.to_string())).unwrap();
    assert_eq!(b.head_oid, first.to_string());
    let b = p.create_branch("from-topic", Some("topic")).unwrap();
    assert_eq!(b.head_oid, second.to_string());
    assert_eq!(p.list_branches().unwrap().len(), 4);
}

#[test]
fn create_errors() {
    let (dir, _repo) = seeded();
    let mut p = open(dir.path());
    assert!(matches!(
        p.create_branch("main", None),
        Err(GitError::BranchExists(_))
    ));
    assert!(matches!(
        p.create_branch("x", Some("nope")),
        Err(GitError::InvalidRef(_))
    ));
    assert!(matches!(
        p.create_branch("bad name~", None),
        Err(GitError::InvalidRef(_))
    ));

    let empty = tempfile::tempdir().unwrap();
    let _r = init_repo(empty.path());
    let mut p = open(empty.path());
    assert!(matches!(
        p.create_branch("x", None),
        Err(GitError::InvalidRef(_))
    ));
}

#[test]
fn delete_refuses_current_and_unmerged_but_force_works() {
    let (dir, repo) = seeded();
    let mut p = open(dir.path());
    p.create_branch("merged", None).unwrap();
    assert!(matches!(
        p.delete_branch("main", true),
        Err(GitError::BranchIsCurrent(_))
    ));
    assert!(matches!(
        p.delete_branch("ghost", false),
        Err(GitError::BranchNotFound(_))
    ));

    // merged (same tip as HEAD) deletes without force
    p.delete_branch("merged", false).unwrap();

    // unmerged: a branch with a commit main does not have
    p.create_branch("wip", None).unwrap();
    repo.set_head("refs/heads/wip").unwrap();
    write(dir.path(), "a.txt", "wip\n");
    commit_all(&repo, "wip commit");
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    let err = p.delete_branch("wip", false).unwrap_err();
    assert!(matches!(err, GitError::BranchNotMerged(_)), "{err:?}");
    assert!(p.list_branches().unwrap().iter().any(|b| b.name == "wip"));
    p.delete_branch("wip", true).unwrap();
    assert!(!p.list_branches().unwrap().iter().any(|b| b.name == "wip"));
}

#[test]
fn rename_other_and_current_branch() {
    let (dir, _repo) = seeded();
    let mut p = open(dir.path());
    p.create_branch("topic", None).unwrap();
    p.rename_branch("topic", "topic2").unwrap();
    let names: Vec<_> = p
        .list_branches()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"topic2".to_string()) && !names.contains(&"topic".to_string()));

    p.rename_branch("main", "trunk").unwrap();
    assert_eq!(p.current_branch().unwrap().unwrap().name, "trunk");

    assert!(matches!(
        p.rename_branch("trunk", "topic2"),
        Err(GitError::BranchExists(_))
    ));
    assert!(matches!(
        p.rename_branch("ghost", "x"),
        Err(GitError::BranchNotFound(_))
    ));
}
