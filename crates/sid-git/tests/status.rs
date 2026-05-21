use std::fs;
use std::path::Path;

use sid_core::adapters::git::{GitProvider, StatusKind};
use sid_git::Git2ProviderFactory;
use tempfile::tempdir;

fn init_repo_with_initial_commit(path: &Path) -> git2::Repository {
    let repo = git2::Repository::init(path).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let tree_id = {
        let mut i = repo.index().unwrap();
        i.write().unwrap();
        i.write_tree().unwrap()
    };
    {
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
    }
    repo
}

#[test]
fn clean_repo_reports_clean() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    let factory = Git2ProviderFactory::new();
    let provider = factory.open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    assert!(s.is_clean);
    assert!(s.entries.is_empty());
}

#[test]
fn untracked_file_appears_as_untracked() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("hello.txt"), b"hi").unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    assert!(!s.is_clean);
    let e = s.entries.iter().find(|e| e.path == "hello.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Untracked);
    assert!(!e.staged);
}

#[test]
fn staged_added_file_reports_added_and_staged() {
    let dir = tempdir().unwrap();
    let repo = init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("new.txt"), b"new").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("new.txt")).unwrap();
    idx.write().unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let e = s.entries.iter().find(|e| e.path == "new.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Added);
    assert!(e.staged);
}

#[test]
fn modified_unstaged_file_reports_modified_unstaged() {
    let dir = tempdir().unwrap();
    let repo = init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("a.txt"), b"v1").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "add a", &tree, &[&parent])
        .unwrap();
    fs::write(dir.path().join("a.txt"), b"v2").unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let e = s.entries.iter().find(|e| e.path == "a.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Modified);
    assert!(!e.staged);
}

#[test]
fn unicode_filename_appears_correctly() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("hello-🐕.txt"), b"woof").unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    assert!(s.entries.iter().any(|e| e.path == "hello-🐕.txt"));
}

#[test]
fn many_files_does_not_panic() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    for i in 0..200 {
        fs::write(dir.path().join(format!("f-{i}.txt")), b"x").unwrap();
    }
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    assert!(!s.is_clean);
    assert!(s.entries.len() >= 200);
}

// Adversarial: deleted file shows as Deleted
#[test]
fn deleted_tracked_file_shows_as_deleted() {
    let dir = tempdir().unwrap();
    let repo = init_repo_with_initial_commit(dir.path());
    // Create and commit a file
    fs::write(dir.path().join("d.txt"), b"del").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("d.txt")).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "add d", &tree, &[&parent])
        .unwrap();
    // Now delete it
    fs::remove_file(dir.path().join("d.txt")).unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let e = s.entries.iter().find(|e| e.path == "d.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Deleted);
    assert!(!s.is_clean);
}

// Adversarial: staged deletion shows staged=true
#[test]
fn staged_deletion_shows_staged() {
    let dir = tempdir().unwrap();
    let repo = init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("s.txt"), b"x").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("s.txt")).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "add s", &tree, &[&parent])
        .unwrap();
    // Stage the deletion
    idx.remove_path(Path::new("s.txt")).unwrap();
    idx.write().unwrap();
    fs::remove_file(dir.path().join("s.txt")).unwrap();
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let staged_del = s
        .entries
        .iter()
        .find(|e| e.path == "s.txt" && e.staged)
        .unwrap();
    assert_eq!(staged_del.kind, StatusKind::Deleted);
}

// Adversarial: is_clean mirrors entries.is_empty()
#[test]
fn is_clean_is_consistent_with_entries() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    let provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    assert_eq!(s.is_clean, s.entries.is_empty());
    // Add a file and re-check
    fs::write(dir.path().join("x.txt"), b"x").unwrap();
    let provider2 = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let s2 = provider2.status().unwrap();
    assert_eq!(s2.is_clean, s2.entries.is_empty());
}
