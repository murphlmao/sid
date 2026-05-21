use std::path::Path;

use sid_core::adapters::git::{GitError, GitProvider};
use sid_git::Git2ProviderFactory;
use tempfile::tempdir;

fn init_repo_at(path: &Path) {
    git2::Repository::init(path).expect("init repo");
}

/// Helper to extract error from a Result where T doesn't implement Debug.
fn expect_err<T, E>(result: Result<T, E>, msg: &str) -> E {
    match result {
        Ok(_) => panic!("{}", msg),
        Err(e) => e,
    }
}

#[test]
fn open_succeeds_on_initialized_repo() {
    let dir = tempdir().unwrap();
    init_repo_at(dir.path());
    let factory = Git2ProviderFactory::new();
    let _provider = factory.open(dir.path()).expect("open repo");
}

#[test]
fn open_fails_on_non_repo_directory() {
    let dir = tempdir().unwrap();
    let factory = Git2ProviderFactory::new();
    let err = expect_err(factory.open(dir.path()), "expected error opening non-repo directory");
    let msg = format!("{err}");
    assert!(msg.contains("repository not found") || msg.contains("not"), "msg was: {msg}");
}

#[test]
fn open_fails_on_nonexistent_path() {
    let factory = Git2ProviderFactory::new();
    let err = expect_err(
        factory.open(Path::new("/nonexistent/path/should-not-exist")),
        "expected error opening nonexistent path",
    );
    let msg = format!("{err}");
    let _ = msg; // just confirm it doesn't panic
}

// Adversarial: open via a symlink to a real repo (should succeed if symlinks resolve)
#[test]
#[cfg(unix)]
fn open_succeeds_via_symlink_to_repo() {
    let dir = tempdir().unwrap();
    let real = dir.path().join("real_repo");
    std::fs::create_dir(&real).unwrap();
    init_repo_at(&real);
    let link = dir.path().join("link_to_repo");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let factory = Git2ProviderFactory::new();
    let _provider = factory.open(&link).expect("open via symlink should succeed");
}

// Adversarial: open at a path that is a regular file, not a directory (should error gracefully)
#[test]
fn open_at_file_errors_gracefully() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("file.txt");
    std::fs::write(&file_path, b"not a dir").unwrap();
    // Treat the file itself as a "directory" — git2 should error, not panic
    let factory = Git2ProviderFactory::new();
    let result = factory.open(&file_path);
    assert!(result.is_err(), "opening a file as a repo should fail");
    let err: GitError = expect_err(result, "expected error");
    let msg = format!("{err}");
    assert!(!msg.is_empty());
}

// Adversarial: factory methods other than open() return clear errors
#[test]
fn factory_methods_other_than_open_return_error() {
    let factory = Git2ProviderFactory::new();
    assert!(factory.list_branches().is_err());
    assert!(factory.current_branch().is_err());
    assert!(factory.status().is_err());
    assert!(factory.commit_log(5, None).is_err());
    assert!(factory.diff(false).is_err());
}
