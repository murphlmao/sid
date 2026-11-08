//! Fetch + push through the `git` CLI path, against a local bare repo.

mod common;

use std::time::Duration;

use common::*;
use sid_core::git::GitError;
use sid_git::cli;

/// `(bare origin, clone, seed)`; `main` tracks `origin/main`.
fn origin_and_clone() -> (tempfile::TempDir, tempfile::TempDir, tempfile::TempDir) {
    let origin = tempfile::tempdir().unwrap();
    let seed_dir = tempfile::tempdir().unwrap();
    let seed = init_repo(seed_dir.path());
    write(seed_dir.path(), "a.txt", "v1\n");
    commit_all(&seed, "init");
    let bare = git2::Repository::init_bare(origin.path()).unwrap();
    bare.set_head("refs/heads/main").unwrap();
    seed.remote("origin", origin.path().to_str().unwrap())
        .unwrap()
        .push(&["refs/heads/main:refs/heads/main"], None)
        .unwrap();
    let work = tempfile::tempdir().unwrap();
    let clone = git2::Repository::clone(origin.path().to_str().unwrap(), work.path()).unwrap();
    let mut cfg = clone.config().unwrap();
    cfg.set_str("user.name", "Test").unwrap();
    cfg.set_str("user.email", "test@example.com").unwrap();
    (origin, work, seed_dir)
}

fn push_seed(seed_dir: &std::path::Path, content: &str, msg: &str) {
    let seed = git2::Repository::open(seed_dir).unwrap();
    write(seed_dir, "a.txt", content);
    commit_all(&seed, msg);
    seed.find_remote("origin")
        .unwrap()
        .push(&["refs/heads/main:refs/heads/main"], None)
        .unwrap();
}

#[test]
fn cli_fetch_sees_new_commits() {
    let (_origin, work, seed_dir) = origin_and_clone();
    push_seed(seed_dir.path(), "v2\n", "two");

    let r = cli::fetch(work.path(), "origin", Duration::from_secs(30)).unwrap();
    assert_eq!((r.ahead, r.behind), (Some(0), Some(1)));
    assert_eq!(r.updated_refs, ["origin/main"]);
}

#[test]
fn cli_fetch_unreachable_remote_errors_cleanly() {
    let (_o, work, _s) = origin_and_clone();
    git2::Repository::open(work.path())
        .unwrap()
        .remote("dead", "/definitely/not/a/repo")
        .unwrap();
    let e = cli::fetch(work.path(), "dead", Duration::from_secs(30)).unwrap_err();
    assert!(matches!(e, GitError::Other(_)), "{e:?}");
}

#[test]
fn cli_push_publishes_and_then_is_up_to_date() {
    let (origin, work, _s) = origin_and_clone();
    let mut p = open(work.path());
    write(work.path(), "b.txt", "b\n");
    p.stage_all().unwrap();
    p.commit("add b", false).unwrap();

    let r = p.push().unwrap();
    assert_eq!((r.remote.as_str(), r.branch.as_str()), ("origin", "main"));
    let bare = git2::Repository::open(origin.path()).unwrap();
    let c = bare
        .find_reference("refs/heads/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    assert_eq!(c.summary(), Some("add b"));

    assert!(p.push().unwrap().up_to_date);
}

#[test]
fn cli_push_rejected_when_remote_is_ahead() {
    let (_origin, work, seed_dir) = origin_and_clone();
    push_seed(seed_dir.path(), "remote\n", "remote change");

    let mut p = open(work.path());
    write(work.path(), "c.txt", "c\n");
    p.stage_all().unwrap();
    p.commit("local change", false).unwrap();
    assert!(matches!(p.push(), Err(GitError::Conflict(_))));
}

#[test]
fn push_without_upstream_is_a_clear_error() {
    let (dir, _repo) = seeded();
    let p = open(dir.path());
    assert!(matches!(p.push(), Err(GitError::Other(m)) if m.contains("no upstream")));
}
