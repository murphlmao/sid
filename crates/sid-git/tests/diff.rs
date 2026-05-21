use std::fs;
use std::path::Path;

use sid_core::adapters::git::GitProvider;
use sid_git::Git2ProviderFactory;
use tempfile::tempdir;

fn commit_initial(repo: &git2::Repository) {
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let mut idx = repo.index().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
}

#[test]
fn unstaged_modification_appears_in_unstaged_diff() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.txt"), b"hello\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    commit_initial(&repo);
    fs::write(dir.path().join("a.txt"), b"hello\nworld\n").unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let diff = provider.diff(false).unwrap();
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].path, "a.txt");
    assert!(diff[0].patch.contains("+world"), "patch: {}", diff[0].patch);
    assert_eq!(diff[0].added, 1);
}

#[test]
fn staged_modification_appears_in_staged_diff_not_unstaged() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.txt"), b"v1\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    commit_initial(&repo);
    fs::write(dir.path().join("a.txt"), b"v2\n").unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    assert_eq!(provider.diff(true).unwrap().len(), 1);
    assert_eq!(provider.diff(false).unwrap().len(), 0);
}

#[test]
fn clean_repo_diff_is_empty() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    commit_initial(&repo);
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    assert!(provider.diff(true).unwrap().is_empty());
    assert!(provider.diff(false).unwrap().is_empty());
}

#[test]
fn binary_file_diff_does_not_panic() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("bin"), [0u8, 1, 2, 3, 0, 5, 6, 7]).unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("bin")).unwrap();
    idx.write().unwrap();
    commit_initial(&repo);
    fs::write(dir.path().join("bin"), [0u8, 1, 2, 3, 99, 5, 6, 7]).unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let _ = provider.diff(false).unwrap();
}

// Adversarial: diff_entry fields are populated correctly
#[test]
fn diff_entry_has_correct_added_removed_counts() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.txt"), b"line1\nline2\nline3\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    commit_initial(&repo);
    // Remove line2, add line4
    fs::write(dir.path().join("a.txt"), b"line1\nline3\nline4\n").unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let diff = provider.diff(true).unwrap();
    assert_eq!(diff.len(), 1);
    let e = &diff[0];
    assert_eq!(e.path, "a.txt");
    assert!(e.added >= 1, "expected >= 1 added");
    assert!(e.removed >= 1, "expected >= 1 removed");
}

// Adversarial: multiple modified files appear as multiple diff entries
#[test]
fn diff_multiple_files_returns_multiple_entries() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(dir.path().join(name), b"original\n").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new(name)).unwrap();
        idx.write().unwrap();
    }
    commit_initial(&repo);
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(dir.path().join(name), b"modified\n").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new(name)).unwrap();
        idx.write().unwrap();
    }
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let diff = provider.diff(true).unwrap();
    assert_eq!(diff.len(), 3);
}

// Adversarial: unicode filename in diff
#[test]
fn diff_with_unicode_filename_does_not_panic() {
    let dir = tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("🐕.txt"), b"woof\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("🐕.txt")).unwrap();
    idx.write().unwrap();
    commit_initial(&repo);
    fs::write(dir.path().join("🐕.txt"), b"bark\n").unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let diff = provider.diff(false).unwrap();
    assert!(!diff.is_empty());
}

// Adversarial: diff on repo with no head commit returns empty staged diff
#[test]
fn diff_on_repo_without_head_returns_empty_or_error() {
    let dir = tempdir().unwrap();
    let _repo = git2::Repository::init(dir.path()).unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    // No commits yet; staged diff against empty HEAD = empty
    let result = provider.diff(true);
    if let Ok(d) = result {
        assert!(d.is_empty());
    }
    // unstaged diff is also fine
    let _ = provider.diff(false);
}
