//! Workspaces v1 §A — `summary()`: the one-call fleet rollup, across the
//! repo states the trait's doc comment calls out — unborn HEAD, a clean repo
//! with an upstream (ahead/behind), a dirty repo, and detached HEAD.

use std::path::Path;

use sid_git::Git2Provider;

fn init_repo(path: &Path) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    git2::Repository::init_opts(path, &opts).unwrap()
}

fn commit_all(repo: &git2::Repository, message: &str) -> git2::Oid {
    let mut idx = repo.index().unwrap();
    idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("Test", "test@example.com").unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .unwrap()
}

/// Fake an upstream by pointing `branch_name`'s tracking config at another
/// *local* branch. libgit2 supports the "." pseudo-remote for exactly this —
/// no real remote or fetch is needed to exercise `graph_ahead_behind`.
fn set_fake_upstream(repo: &git2::Repository, branch_name: &str, upstream_branch: &str) {
    let mut config = repo.config().unwrap();
    config
        .set_str(&format!("branch.{branch_name}.remote"), ".")
        .unwrap();
    config
        .set_str(
            &format!("branch.{branch_name}.merge"),
            &format!("refs/heads/{upstream_branch}"),
        )
        .unwrap();
}

#[test]
fn summary_on_unborn_head_has_no_branch_and_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.summary().unwrap();
    assert_eq!(s.branch, None);
    assert!(!s.detached);
    assert!(s.is_clean());
    assert_eq!(s.ahead, None);
    assert_eq!(s.behind, None);
    assert!(s.last_commit.is_none());
}

#[test]
fn summary_with_no_upstream_has_no_ahead_behind() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    commit_all(&repo, "init");

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.summary().unwrap();
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.ahead, None);
    assert_eq!(s.behind, None);
}

#[test]
fn summary_on_clean_repo_with_upstream_reports_ahead_and_behind() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    let base = commit_all(&repo, "base");

    // "upstream-fake" starts at the same commit as main, then gets one
    // commit main never receives. This is a manual ref update (not a
    // checkout), so it never touches the working tree or HEAD.
    let base_commit = repo.find_commit(base).unwrap();
    repo.branch("upstream-fake", &base_commit, false).unwrap();
    let sig = git2::Signature::now("Test", "test@example.com").unwrap();
    repo.commit(
        Some("refs/heads/upstream-fake"),
        &sig,
        &sig,
        "only on upstream",
        &base_commit.tree().unwrap(),
        &[&base_commit],
    )
    .unwrap();
    set_fake_upstream(&repo, "main", "upstream-fake");

    // main advances by two commits the fake upstream never gets.
    std::fs::write(dir.path().join("b.txt"), "v1").unwrap();
    commit_all(&repo, "second");
    std::fs::write(dir.path().join("c.txt"), "v1").unwrap();
    commit_all(&repo, "third");

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.summary().unwrap();
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert!(!s.detached);
    assert!(s.is_clean());
    assert_eq!(s.ahead, Some(2));
    assert_eq!(s.behind, Some(1));
    assert!(s.last_commit.is_some());
    assert_eq!(s.last_commit.unwrap().summary, "third");
}

#[test]
fn summary_on_dirty_repo_reports_change_counts() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    std::fs::write(dir.path().join("b.txt"), "v1").unwrap();
    commit_all(&repo, "init");

    // one staged change, one unstaged change, one untracked file
    std::fs::write(dir.path().join("a.txt"), "v2").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    std::fs::write(dir.path().join("b.txt"), "v2").unwrap();
    std::fs::write(dir.path().join("c.txt"), "new").unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.summary().unwrap();
    assert!(!s.is_clean());
    assert_eq!(s.staged, 1);
    assert_eq!(s.unstaged, 1);
    assert_eq!(s.untracked, 1);
}

#[test]
fn summary_on_detached_head_reports_short_oid_and_detached_true() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    let oid = commit_all(&repo, "init");
    repo.set_head_detached(oid).unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.summary().unwrap();
    assert!(s.detached);
    assert_eq!(s.branch.as_deref(), Some(&oid.to_string()[..7]));
    assert_eq!(s.ahead, None);
    assert_eq!(s.behind, None);
    assert!(s.last_commit.is_some());
}
