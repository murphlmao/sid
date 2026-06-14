use std::{fs, path::Path};

use sid_core::adapters::git::{GitError, GitProvider};
use sid_git::Git2ProviderFactory;
use tempfile::tempdir;

fn commit(repo: &git2::Repository, msg: &str) -> git2::Oid {
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let mut idx = repo.index().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let parents: Vec<_> = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .into_iter()
        .collect();
    let parent_refs: Vec<_> = parents.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)
        .unwrap();
    drop(tree);
    oid
}

#[test]
fn commit_log_returns_recent_commits_first() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    let a = commit(&repo, "first");
    fs::write(dir.path().join("x.txt"), b"x").unwrap();
    let mut i = repo.index().unwrap();
    i.add_path(Path::new("x.txt")).unwrap();
    i.write().unwrap();
    let b = commit(&repo, "second");
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let log = provider.commit_log(10, None).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].oid, b.to_string());
    assert_eq!(log[1].oid, a.to_string());
}

#[test]
fn commit_log_respects_max() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    for i in 0..5 {
        fs::write(dir.path().join(format!("f-{i}.txt")), b"x").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new(&format!("f-{i}.txt"))).unwrap();
        idx.write().unwrap();
        commit(&repo, &format!("commit {i}"));
    }
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let log = provider.commit_log(3, None).unwrap();
    assert_eq!(log.len(), 3);
}

#[test]
fn commit_log_from_specific_oid() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    let a = commit(&repo, "a");
    let _b = commit(&repo, "b");
    let _c = commit(&repo, "c");
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let log = provider.commit_log(10, Some(&a.to_string())).unwrap();
    // Walking from `a` should give just `a` (no parents).
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].oid, a.to_string());
}

#[test]
fn commit_log_zero_max_returns_empty() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    let _ = commit(&repo, "init");
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let log = provider.commit_log(0, None).unwrap();
    assert!(log.is_empty());
}

#[test]
fn invalid_oid_returns_invalid_ref_error() {
    let dir = tempdir().unwrap();
    let _repo = git2::Repository::init(dir.path()).unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let err = provider.commit_log(5, Some("not-a-valid-oid")).unwrap_err();
    assert!(matches!(err, GitError::InvalidRef(_)));
}

// Adversarial: commit log on repo with no commits returns empty (not panic)
#[test]
fn commit_log_on_unborn_repo_returns_empty() {
    let dir = tempdir().unwrap();
    let _repo = git2::Repository::init(dir.path()).unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    // push_head() will fail if there's no HEAD — expect an error OR empty
    let result = provider.commit_log(10, None);
    // Either an empty list (treated gracefully) or an error — no panic
    if let Ok(log) = result {
        assert!(log.is_empty());
    }
    // Err is also acceptable — the key guarantee is no panic
}

// Verify commit metadata fields are populated
#[test]
fn commit_log_fields_are_populated() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    let sig = git2::Signature::now("Author Name", "author@example.com").unwrap();
    let tree_id = {
        let mut idx = repo.index().unwrap();
        idx.write().unwrap();
        idx.write_tree().unwrap()
    };
    {
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "feat: hello world", &tree, &[])
            .unwrap();
    }
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let log = provider.commit_log(1, None).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].summary, "feat: hello world");
    assert_eq!(log[0].author_name, "Author Name");
    assert_eq!(log[0].author_email, "author@example.com");
    assert!(log[0].timestamp_secs > 0);
    assert_eq!(log[0].oid.len(), 40);
    assert!(log[0].parents.is_empty()); // first commit has no parents
}

// Adversarial: max=1 returns exactly one commit even when there are many
#[test]
fn commit_log_max_1_returns_single_entry() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    for _ in 0..10 {
        commit(&repo, "x");
    }
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let log = provider.commit_log(1, None).unwrap();
    assert_eq!(log.len(), 1);
}
