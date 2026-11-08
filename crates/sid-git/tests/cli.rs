//! Fetch + push through the `git` CLI path, against a local bare repo.

mod common;

use std::path::Path;
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

    // Local remotes route through libgit2 (the hardened CLI only speaks ssh/https).
    let r = open(work.path()).fetch("origin").unwrap();
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
    let e = open(work.path()).fetch("dead").unwrap_err();
    assert!(!matches!(e, GitError::TimedOut(_)), "{e:?}");
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
    assert_eq!(c.summary().ok().flatten(), Some("add b"));

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
    let r = p.push();
    assert!(matches!(r, Err(GitError::Conflict(_))), "{r:?}");
}

#[test]
fn push_without_upstream_is_a_clear_error() {
    let (dir, _repo) = seeded();
    let p = open(dir.path());
    assert!(matches!(p.push(), Err(GitError::Other(m)) if m.contains("no upstream")));
}

// ---- hostile repo config must never run a command ------------------------------------

fn marker_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn set(work: &Path, key: &str, val: &str) {
    git2::Repository::open(work)
        .unwrap()
        .config()
        .unwrap()
        .set_str(key, val)
        .unwrap();
}

#[cfg(unix)]
fn hook(work: &Path, name: &str, marker: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let dir = work.join(".git/hooks");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join(name);
    std::fs::write(&f, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A local-path remote whose `receivepack`/`uploadpack` run a command, plus hooks.
#[cfg(unix)]
#[test]
fn hostile_local_config_runs_nothing_on_push_or_fetch() {
    let (_origin, work, _s) = origin_and_clone();
    let m = marker_dir();
    let pack = m.path().join("pack");
    let pre_push = m.path().join("pre-push");
    let reftx = m.path().join("ref-tx");
    let ssh = m.path().join("ssh");
    set(
        work.path(),
        "remote.origin.receivepack",
        &format!("sh -c 'touch {}'", pack.display()),
    );
    set(
        work.path(),
        "remote.origin.uploadpack",
        &format!("sh -c 'touch {}'", pack.display()),
    );
    set(
        work.path(),
        "core.sshCommand",
        &format!("sh -c 'touch {}'", ssh.display()),
    );
    hook(work.path(), "pre-push", &pre_push);
    hook(work.path(), "reference-transaction", &reftx);

    let mut p = open(work.path());
    write(work.path(), "h.txt", "h\n");
    p.stage_all().unwrap();
    p.commit("hostile", false).unwrap();
    let _ = p.push();
    let _ = p.fetch("origin");
    // The CLI path itself (as if the remote were ssh) must scrub the same things.
    let _ = cli::fetch(work.path(), "origin", Duration::from_secs(20));
    let _ = cli::push(work.path(), Duration::from_secs(20));

    for f in [&pack, &pre_push, &reftx, &ssh] {
        assert!(!f.exists(), "{} was created by repo config", f.display());
    }
}

#[cfg(unix)]
#[test]
fn ext_remote_is_refused_and_runs_nothing() {
    let (_origin, work, _s) = origin_and_clone();
    let m = marker_dir();
    let mark = m.path().join("ext");
    let url = format!("ext::sh -c 'touch {}'", mark.display());
    git2::Repository::open(work.path())
        .unwrap()
        .remote("evil", &url)
        .unwrap();
    // `protocol.ext.allow=always` is what a hostile repo would set.
    set(work.path(), "protocol.ext.allow", "always");
    set(work.path(), "branch.main.remote", "evil");

    let p = open(work.path());
    let f = p.fetch("evil");
    assert!(f.is_err(), "{f:?}");
    let pu = p.push();
    assert!(
        matches!(&pu, Err(GitError::Other(m)) if m.contains("not supported")),
        "{pu:?}"
    );
    // And directly on the CLI, where GIT_ALLOW_PROTOCOL must still block it.
    let _ = cli::fetch(work.path(), "evil", Duration::from_secs(20));
    assert!(!mark.exists(), "ext:: command ran");
}

#[test]
fn plain_http_push_is_refused_clearly() {
    let (_origin, work, _s) = origin_and_clone();
    set(
        work.path(),
        "remote.origin.url",
        "http://example.invalid/x.git",
    );
    let r = open(work.path()).push();
    assert!(
        matches!(&r, Err(GitError::Other(m)) if m.contains("plain http")),
        "{r:?}"
    );
}
