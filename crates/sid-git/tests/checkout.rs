use std::{fs, path::Path};

use sid_core::adapters::git::{GitError, GitProvider};
use sid_git::Git2ProviderFactory;
use tempfile::tempdir;

fn setup_two_branches(path: &Path) -> git2::Repository {
    let repo = git2::Repository::init(path).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    fs::write(path.join("a.txt"), b"v1\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    {
        let tree = repo.find_tree(tree_id).unwrap();
        let init = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let head = repo.find_commit(init).unwrap();
        repo.branch("feature", &head, false).unwrap();
        drop(head);
    }
    repo
}

#[test]
fn checkout_succeeds_when_clean() {
    let dir = tempdir().unwrap();
    setup_two_branches(dir.path());
    let factory = Git2ProviderFactory::new();
    let mut provider = factory.open(dir.path()).unwrap();
    provider.checkout_branch("feature").unwrap();
    let cur = provider.current_branch().unwrap().unwrap();
    assert_eq!(cur.name, "feature");
}

#[test]
fn checkout_refuses_when_dirty() {
    let dir = tempdir().unwrap();
    setup_two_branches(dir.path());
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let err = provider.checkout_branch("feature").unwrap_err();
    assert!(matches!(err, GitError::DirtyWorkingTree(_)));
}

#[test]
fn checkout_unknown_branch_errors() {
    let dir = tempdir().unwrap();
    setup_two_branches(dir.path());
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let err = provider.checkout_branch("nonexistent").unwrap_err();
    assert!(matches!(err, GitError::BranchNotFound(_)));
}

#[test]
fn checkout_to_current_branch_is_noop_and_succeeds() {
    let dir = tempdir().unwrap();
    setup_two_branches(dir.path());
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let current = provider.current_branch().unwrap().unwrap().name;
    provider.checkout_branch(&current).unwrap();
    let still = provider.current_branch().unwrap().unwrap();
    assert_eq!(still.name, current);
}

// Adversarial: checkout then checkout back verifies bidirectional switch
#[test]
fn checkout_round_trip_between_branches() {
    let dir = tempdir().unwrap();
    let repo = setup_two_branches(dir.path());
    let initial_name = repo.head().unwrap().shorthand().unwrap().to_string();
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    provider.checkout_branch("feature").unwrap();
    assert_eq!(provider.current_branch().unwrap().unwrap().name, "feature");
    provider.checkout_branch(&initial_name).unwrap();
    assert_eq!(
        provider.current_branch().unwrap().unwrap().name,
        initial_name
    );
}

// Adversarial: dirty guard counts the entries
#[test]
fn checkout_dirty_guard_reports_nonzero_count() {
    let dir = tempdir().unwrap();
    setup_two_branches(dir.path());
    // Create 3 untracked files
    for i in 0..3 {
        fs::write(dir.path().join(format!("dirty-{i}.txt")), b"x").unwrap();
    }
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let err = provider.checkout_branch("feature").unwrap_err();
    match err {
        GitError::DirtyWorkingTree(n) => assert!(n > 0),
        other => panic!("expected DirtyWorkingTree, got: {other:?}"),
    }
}

// Adversarial: branch name with slashes checkouts correctly
#[test]
fn checkout_branch_with_slash_in_name() {
    let dir = tempdir().unwrap();
    let repo = setup_two_branches(dir.path());
    let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("feat/my-feature", &head_commit, false).unwrap();
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    provider.checkout_branch("feat/my-feature").unwrap();
    let cur = provider.current_branch().unwrap().unwrap();
    assert_eq!(cur.name, "feat/my-feature");
}

// Verify checkout updates list_branches to reflect the new current
#[test]
fn checkout_updates_list_branches_is_current() {
    let dir = tempdir().unwrap();
    setup_two_branches(dir.path());
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    provider.checkout_branch("feature").unwrap();
    let branches = provider.list_branches().unwrap();
    let feature_branch = branches.iter().find(|b| b.name == "feature").unwrap();
    assert!(feature_branch.is_current);
    let other_current_count = branches.iter().filter(|b| b.is_current).count();
    assert_eq!(other_current_count, 1);
}
