//! Fetch against a local bare repo standing in for `origin`.

mod common;

use common::*;
use sid_core::git::GitError;

/// `(bare origin dir, clone dir)` with `main` tracking `origin/main`.
fn origin_and_clone() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    git2::Repository,
    tempfile::TempDir,
) {
    let origin_dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    // Seed the origin through a scratch working repo, then push into the bare one.
    let seed_dir = tempfile::tempdir().unwrap();
    let seed = init_repo(seed_dir.path());
    write(seed_dir.path(), "a.txt", "v1\n");
    commit_all(&seed, "init");
    let bare = git2::Repository::init_bare(origin_dir.path()).unwrap();
    bare.set_head("refs/heads/main").unwrap();
    let mut r = seed
        .remote("origin", origin_dir.path().to_str().unwrap())
        .unwrap();
    r.push(&["refs/heads/main:refs/heads/main"], None).unwrap();
    drop(r);

    let clone = git2::Repository::clone(origin_dir.path().to_str().unwrap(), work.path()).unwrap();
    let mut cfg = clone.config().unwrap();
    cfg.set_str("user.name", "Test").unwrap();
    cfg.set_str("user.email", "test@example.com").unwrap();
    drop(cfg);
    (origin_dir, work, seed, seed_dir)
}

#[test]
fn fetch_reports_new_commits_as_behind() {
    let (origin, work, seed, _seed_dir) = origin_and_clone();
    let p = open(work.path());
    assert_eq!(p.remotes().unwrap(), ["origin"]);

    // Nothing new yet.
    let r = p.fetch("origin").unwrap();
    assert_eq!((r.ahead, r.behind), (Some(0), Some(0)));
    assert!(r.updated_refs.is_empty());

    // Origin gains two commits.
    write(seed.workdir().unwrap(), "a.txt", "v2\n");
    commit_all(&seed, "two");
    write(seed.workdir().unwrap(), "a.txt", "v3\n");
    commit_all(&seed, "three");
    seed.find_remote("origin")
        .unwrap()
        .push(&["refs/heads/main:refs/heads/main"], None)
        .unwrap();
    let _ = &origin;

    let r = p.fetch("origin").unwrap();
    assert_eq!(r.remote, "origin");
    assert_eq!((r.ahead, r.behind), (Some(0), Some(2)));
    assert!(
        r.updated_refs.iter().any(|n| n == "origin/main"),
        "{:?}",
        r.updated_refs
    );
    assert!(r.received_objects > 0);
    assert_eq!(p.summary().unwrap().behind, Some(2));
}

#[test]
fn fetch_counts_local_ahead_too() {
    let (_origin, work, _seed, _sd) = origin_and_clone();
    let mut p = open(work.path());
    write(work.path(), "local.txt", "l");
    p.stage_all().unwrap();
    p.commit("local only", false).unwrap();
    let r = p.fetch("origin").unwrap();
    assert_eq!((r.ahead, r.behind), (Some(1), Some(0)));
}

#[test]
fn unknown_remote_and_unreachable_remote_fail_cleanly() {
    let (_o, work, _s, _sd) = origin_and_clone();
    let p = open(work.path());
    assert!(matches!(p.fetch("nope"), Err(GitError::RemoteNotFound(_))));

    let repo = git2::Repository::open(work.path()).unwrap();
    repo.remote("dead", "/definitely/not/a/repo").unwrap();
    let started = std::time::Instant::now();
    let err = p.fetch("dead").unwrap_err();
    assert!(matches!(err, GitError::Other(_)), "{err:?}");
    assert!(started.elapsed().as_secs() < 30, "must not hang");
}

#[test]
fn credential_policy_is_bounded_and_never_prompts() {
    // Over a transport that is not compiled in (or needs auth) the fetch must come
    // back with an error promptly, never a prompt or a hang.
    let (_o, work, _s, _sd) = origin_and_clone();
    let repo = git2::Repository::open(work.path()).unwrap();
    repo.remote("web", "https://127.0.0.1:1/none.git").unwrap();
    let p = open(work.path());
    let started = std::time::Instant::now();
    assert!(p.fetch("web").is_err());
    assert!(started.elapsed().as_secs() < 30);
}
