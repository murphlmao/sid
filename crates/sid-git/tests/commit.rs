//! Commit: validation, unborn HEAD, amend, nothing-to-commit, conflicts.

mod common;

use common::*;
use sid_core::git::GitError;

#[test]
fn first_commit_on_unborn_head() {
    let dir = tempfile::tempdir().unwrap();
    let _repo = init_repo(dir.path());
    write(dir.path(), "a.txt", "x");
    let mut p = open(dir.path());
    p.stage_all().unwrap();
    let c = p.commit("first", false).unwrap();
    assert_eq!(c.summary, "first");
    assert_eq!(c.author_name, "Test");
    assert_eq!(c.author_email, "test@example.com");
    assert_eq!(c.oid.len(), 40);
    let s = p.summary().unwrap();
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.last_commit.unwrap().oid, c.oid);
    assert!(p.status().unwrap().is_clean);
}

#[test]
fn blank_message_is_refused_and_nothing_is_committed() {
    let (dir, repo) = seeded();
    write(dir.path(), "a.txt", "v2");
    let mut p = open(dir.path());
    p.stage_all().unwrap();
    for m in ["", "   ", "\n\t \n"] {
        assert!(
            matches!(p.commit(m, false), Err(GitError::EmptyMessage)),
            "{m:?}"
        );
    }
    assert_eq!(
        repo.head().unwrap().peel_to_commit().unwrap().summary(),
        Some("init")
    );
}

#[test]
fn nothing_staged_is_refused() {
    let (dir, _repo) = seeded();
    write(dir.path(), "a.txt", "unstaged only");
    let mut p = open(dir.path());
    assert!(matches!(
        p.commit("x", false),
        Err(GitError::NothingToCommit)
    ));

    let empty = tempfile::tempdir().unwrap();
    let _r = init_repo(empty.path());
    let mut p = open(empty.path());
    assert!(matches!(
        p.commit("x", false),
        Err(GitError::NothingToCommit)
    ));
}

#[test]
fn commit_creates_child_of_head_with_trimmed_message() {
    let (dir, repo) = seeded();
    let before = repo.head().unwrap().target().unwrap();
    write(dir.path(), "a.txt", "v2\n");
    let mut p = open(dir.path());
    p.stage_paths(&["a.txt".into()]).unwrap();
    let c = p.commit("  feat: x\n\nbody here  \n", false).unwrap();
    assert_eq!(c.summary, "feat: x");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.id().to_string(), c.oid);
    assert_eq!(head.parent_id(0).unwrap(), before);
    assert_eq!(head.message(), Some("feat: x\n\nbody here\n"));
}

#[test]
fn amend_rewrites_head_keeping_author() {
    let (dir, repo) = seeded();
    let parent_count_before = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .parent_count();
    write(dir.path(), "a.txt", "v2\n");
    let mut p = open(dir.path());
    p.stage_all().unwrap();
    // Message-only amend works without extra staging too, so first fold the change in.
    let c = p.commit("init (amended)", true).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.id().to_string(), c.oid);
    assert_eq!(head.parent_count(), parent_count_before);
    assert_eq!(head.summary(), Some("init (amended)"));
    assert_eq!(
        repo.revwalk()
            .map(|mut w| {
                w.push_head().unwrap();
                w.count()
            })
            .unwrap(),
        1
    );
    assert!(p.status().unwrap().is_clean);

    // Reword-only amend (nothing new staged) is allowed.
    let c2 = p.commit("reworded", true).unwrap();
    assert_ne!(c2.oid, c.oid);
    assert_eq!(p.commit_log(10).unwrap().len(), 1);
}

#[test]
fn amend_on_unborn_head_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let _repo = init_repo(dir.path());
    write(dir.path(), "a.txt", "x");
    let mut p = open(dir.path());
    p.stage_all().unwrap();
    assert!(matches!(p.commit("m", true), Err(GitError::Other(_))));
}

#[test]
fn unresolved_conflicts_block_commit() {
    let (dir, repo) = seeded();
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

    let mut p = open(dir.path());
    assert!(matches!(
        p.commit("merge", false),
        Err(GitError::Conflicts(1))
    ));
}
