use std::path::Path;

use sid_core::adapters::git::GitProvider;
use sid_git::Git2ProviderFactory;
use tempfile::tempdir;

fn init_repo_with_initial_commit(path: &Path) -> git2::Repository {
    let repo = git2::Repository::init(path).unwrap();
    {
        let sig = git2::Signature::now("test", "test@test").unwrap();
        let tree_id = {
            let mut idx = repo.index().unwrap();
            idx.write().unwrap();
            idx.write_tree().unwrap()
        };
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
    }
    repo
}

#[test]
fn list_branches_returns_initial_branch() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let branches = provider.list_branches().unwrap();
    assert!(
        !branches.is_empty(),
        "expected at least one branch after initial commit"
    );
    // The default may be `master` or `main` depending on git config; just check exactly one is current.
    let current_count = branches.iter().filter(|b| b.is_current).count();
    assert_eq!(
        current_count, 1,
        "exactly one branch should be marked current"
    );
}

#[test]
fn current_branch_matches_list_branches_current() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let listed_current = provider
        .list_branches()
        .unwrap()
        .into_iter()
        .find(|b| b.is_current)
        .unwrap();
    let cur = provider.current_branch().unwrap().unwrap();
    assert_eq!(cur.name, listed_current.name);
    assert_eq!(cur.head_oid, listed_current.head_oid);
}

#[test]
fn current_branch_returns_none_on_unborn_head() {
    let dir = tempdir().unwrap();
    let _repo = git2::Repository::init(dir.path()).unwrap();
    // No commits yet — HEAD is unborn.
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let cur = provider.current_branch().unwrap();
    assert!(
        cur.is_none(),
        "unborn HEAD should yield current_branch = None"
    );
}

#[test]
fn list_branches_finds_a_second_branch() {
    let dir = tempdir().unwrap();
    let repo = init_repo_with_initial_commit(dir.path());
    let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("feature-x", &head_commit, false).unwrap();
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let names: Vec<_> = provider
        .list_branches()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"feature-x".to_string()));
}

#[test]
fn branch_head_oid_is_40_char_hex() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let branches = provider.list_branches().unwrap();
    for b in &branches {
        assert_eq!(
            b.head_oid.len(),
            40,
            "OID should be 40 hex chars, got: {}",
            b.head_oid
        );
        assert!(
            b.head_oid.chars().all(|c| c.is_ascii_hexdigit()),
            "OID should be hex, got: {}",
            b.head_oid
        );
    }
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_list_branches_count_matches_creation(extra_branches in 0usize..6) {
        let dir = tempdir().unwrap();
        let repo = init_repo_with_initial_commit(dir.path());
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        for i in 0..extra_branches {
            repo.branch(&format!("b{i}"), &head_commit, false).unwrap();
        }
        let factory = Git2ProviderFactory::new();
        let provider = factory.open(dir.path()).unwrap();
        let listed = provider.list_branches().unwrap();
        prop_assert_eq!(listed.len(), extra_branches + 1);
    }
}

#[test]
fn list_branches_handles_branch_with_slash_in_name() {
    let dir = tempdir().unwrap();
    let repo = init_repo_with_initial_commit(dir.path());
    let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("feat/auth-refactor", &head_commit, false)
        .unwrap();
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let names: Vec<_> = provider
        .list_branches()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"feat/auth-refactor".to_string()));
}

// Adversarial: list branches on repo with no commits but also check unborn doesn't panic
#[test]
fn list_branches_on_unborn_repo_returns_empty() {
    let dir = tempdir().unwrap();
    let _repo = git2::Repository::init(dir.path()).unwrap();
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let branches = provider.list_branches().unwrap();
    // No commits = no branches
    assert!(
        branches.is_empty(),
        "unborn repo should have no local branches"
    );
}

// Adversarial: deeply nested branch name (multiple slashes)
#[test]
fn list_branches_handles_deeply_nested_name() {
    let dir = tempdir().unwrap();
    let repo = init_repo_with_initial_commit(dir.path());
    let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("fix/auth/token/refresh", &head_commit, false)
        .unwrap();
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let names: Vec<_> = provider
        .list_branches()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"fix/auth/token/refresh".to_string()));
}
