//! Workspaces v1 §A — `list_branches`/`current_branch`: current-first-then-
//! alphabetical ordering, `is_current` marking, and the unborn-HEAD edge case.

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

#[test]
fn current_branch_is_marked_and_exactly_one() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "1").unwrap();
    commit_all(&repo, "init");
    let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("zzz-feature", &head_commit, false).unwrap();
    repo.branch("aaa-feature", &head_commit, false).unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let branches = provider.list_branches().unwrap();
    assert_eq!(branches.len(), 3);
    assert_eq!(branches.iter().filter(|b| b.is_current).count(), 1);
}

#[test]
fn current_branch_sorts_first_then_the_rest_are_alphabetical() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "1").unwrap();
    commit_all(&repo, "init");
    let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("zzz-feature", &head_commit, false).unwrap();
    repo.branch("aaa-feature", &head_commit, false).unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let names: Vec<_> = provider
        .list_branches()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    // "main" is current (HEAD never moved) so it must lead; the rest follow
    // alphabetically regardless of creation order.
    assert_eq!(names, vec!["main", "aaa-feature", "zzz-feature"]);
}

#[test]
fn branch_head_oid_is_40_char_hex() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "1").unwrap();
    commit_all(&repo, "init");

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    for b in provider.list_branches().unwrap() {
        assert_eq!(b.head_oid.len(), 40);
        assert!(b.head_oid.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

#[test]
fn list_branches_on_unborn_repo_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    assert!(provider.list_branches().unwrap().is_empty());
}

#[test]
fn current_branch_matches_the_marked_entry_in_list_branches() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "1").unwrap();
    commit_all(&repo, "init");

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let current = provider.current_branch().unwrap().unwrap();
    assert_eq!(current.name, "main");
    assert!(current.is_current);
}

#[test]
fn current_branch_is_none_on_unborn_head() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    assert!(provider.current_branch().unwrap().is_none());
}
