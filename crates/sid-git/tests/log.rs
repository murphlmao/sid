//! Workspaces v1 §A — `commit_log`: newest-first ordering, the `max` cap,
//! and the unborn-HEAD edge case (no commits yet, no panic).

use std::path::Path;

use sid_git::Git2Provider;

fn init_repo(path: &Path) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    git2::Repository::init_opts(path, &opts).unwrap()
}

fn commit(repo: &git2::Repository, file_name: &str, message: &str) -> git2::Oid {
    std::fs::write(repo.workdir().unwrap().join(file_name), message).unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new(file_name)).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("Test", "test@example.com").unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .unwrap()
}

#[test]
fn commit_log_is_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    let a = commit(&repo, "a.txt", "first");
    let b = commit(&repo, "b.txt", "second");
    let c = commit(&repo, "c.txt", "third");

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let log = provider.commit_log(10).unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].oid, c.to_string());
    assert_eq!(log[1].oid, b.to_string());
    assert_eq!(log[2].oid, a.to_string());
}

#[test]
fn commit_log_respects_max() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    for i in 0..5 {
        commit(&repo, &format!("f-{i}.txt"), &format!("commit {i}"));
    }

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    assert_eq!(provider.commit_log(3).unwrap().len(), 3);
    assert_eq!(provider.commit_log(1).unwrap().len(), 1);
}

#[test]
fn commit_log_max_zero_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    commit(&repo, "a.txt", "init");

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    assert!(provider.commit_log(0).unwrap().is_empty());
}

#[test]
fn commit_log_on_unborn_repo_is_empty_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    assert!(provider.commit_log(10).unwrap().is_empty());
}

#[test]
fn commit_log_fields_are_populated() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    commit(&repo, "a.txt", "feat: hello world");

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let log = provider.commit_log(1).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].summary, "feat: hello world");
    assert_eq!(log[0].author_name, "Test");
    assert_eq!(log[0].author_email, "test@example.com");
    assert!(log[0].timestamp_secs > 0);
    assert_eq!(log[0].oid.len(), 40);
}
