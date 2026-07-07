//! Workspaces v1 §A — `open()`'s `NotARepo` contract: a plain directory (not
//! a git repository) must map to `GitError::NotARepo`, not a generic
//! `Other`, so callers can tell "fine, just not a repo" from a real failure.

use sid_core::git::GitError;
use sid_git::Git2Provider;

/// `Box<dyn GitProvider>` doesn't implement `Debug`, so `Result::unwrap_err`
/// (which requires the `Ok` side to be `Debug`) isn't usable here directly.
fn expect_err<T, E>(result: Result<T, E>, msg: &str) -> E {
    match result {
        Ok(_) => panic!("{msg}"),
        Err(e) => e,
    }
}

#[test]
fn open_on_plain_directory_is_not_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let factory = Git2Provider::factory();
    let err = expect_err(factory.open(dir.path()), "expected NotARepo error");
    assert!(
        matches!(err, GitError::NotARepo(_)),
        "expected NotARepo, got: {err:?}"
    );
}

#[test]
fn open_on_initialized_repo_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    git2::Repository::init(dir.path()).unwrap();
    let factory = Git2Provider::factory();
    let _provider = factory.open(dir.path()).unwrap();
}
