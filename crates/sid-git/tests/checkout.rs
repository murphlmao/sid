//! Workspaces v1 §A — `checkout_branch`: happy path, dirty-tree refusal
//! (staged or unstaged tracked changes), and the untracked-files-don't-block
//! carve-out. This last point is a deliberate v1 deviation from the POC,
//! which counted untracked files toward the dirty guard too — see
//! `checkout_branch`'s doc comment in `src/lib.rs`.

use std::path::Path;

use sid_core::git::GitError;
use sid_git::Git2Provider;

fn init_repo_with_two_branches(path: &Path) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(path, &opts).unwrap();
    std::fs::write(path.join("a.txt"), "v1").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    // Scoped so `tree`/`head_commit` (both borrow `repo`) drop before `repo`
    // is moved out at the end of this function.
    {
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let head_commit = repo.find_commit(oid).unwrap();
        repo.branch("feature", &head_commit, false).unwrap();
    }
    repo
}

#[test]
fn checkout_switches_the_current_branch() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_two_branches(dir.path());
    let mut provider = Git2Provider::factory().open(dir.path()).unwrap();
    provider.checkout_branch("feature").unwrap();
    let cur = provider.current_branch().unwrap().unwrap();
    assert_eq!(cur.name, "feature");
}

#[test]
fn checkout_refuses_on_staged_change() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_with_two_branches(dir.path());
    std::fs::write(dir.path().join("a.txt"), "dirty").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();

    let mut provider = Git2Provider::factory().open(dir.path()).unwrap();
    let err = provider.checkout_branch("feature").unwrap_err();
    assert!(matches!(err, GitError::DirtyWorkingTree(1)));
}

#[test]
fn checkout_refuses_on_unstaged_change() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_two_branches(dir.path());
    std::fs::write(dir.path().join("a.txt"), "dirty").unwrap();

    let mut provider = Git2Provider::factory().open(dir.path()).unwrap();
    let err = provider.checkout_branch("feature").unwrap_err();
    assert!(matches!(err, GitError::DirtyWorkingTree(1)));
}

#[test]
fn refused_checkout_never_touches_head_or_the_uncommitted_edit() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_two_branches(dir.path());
    std::fs::write(dir.path().join("a.txt"), "dirty").unwrap();

    let mut provider = Git2Provider::factory().open(dir.path()).unwrap();
    assert!(provider.checkout_branch("feature").is_err());
    let cur = provider.current_branch().unwrap().unwrap();
    assert_eq!(cur.name, "main");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "dirty"
    );
}

#[test]
fn checkout_untracked_files_alone_do_not_block() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_two_branches(dir.path());
    std::fs::write(dir.path().join("scratch.txt"), "untracked").unwrap();

    let mut provider = Git2Provider::factory().open(dir.path()).unwrap();
    provider.checkout_branch("feature").unwrap();
    let cur = provider.current_branch().unwrap().unwrap();
    assert_eq!(cur.name, "feature");
}

#[test]
fn checkout_unknown_branch_errors() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_two_branches(dir.path());
    let mut provider = Git2Provider::factory().open(dir.path()).unwrap();
    let err = provider.checkout_branch("does-not-exist").unwrap_err();
    assert!(matches!(err, GitError::BranchNotFound(_)));
}

#[test]
fn checkout_updates_which_branch_list_branches_marks_current() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_two_branches(dir.path());
    let mut provider = Git2Provider::factory().open(dir.path()).unwrap();
    provider.checkout_branch("feature").unwrap();

    let branches = provider.list_branches().unwrap();
    let feature = branches.iter().find(|b| b.name == "feature").unwrap();
    assert!(feature.is_current);
    assert_eq!(branches.iter().filter(|b| b.is_current).count(), 1);
}
