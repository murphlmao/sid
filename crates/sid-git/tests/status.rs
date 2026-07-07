//! Workspaces v1 §A — `status()`: staged vs unstaged vs untracked kinds, a
//! path that is dirty in both the index and the working tree at once, and
//! rename detection (cheap to enable via `StatusOptions::renames_*`).

use std::path::Path;

use sid_core::git::StatusKind;
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
fn clean_repo_reports_clean() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    commit_all(&repo, "init");

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    assert!(s.is_clean);
    assert!(s.entries.is_empty());
}

#[test]
fn untracked_file_is_untracked_and_unstaged() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    commit_all(&repo, "init");
    std::fs::write(dir.path().join("new.txt"), "new").unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let e = s.entries.iter().find(|e| e.path == "new.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Untracked);
    assert!(!e.staged);
}

#[test]
fn staged_new_file_is_added_and_staged() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    commit_all(&repo, "init");
    std::fs::write(dir.path().join("staged.txt"), "x").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("staged.txt")).unwrap();
    idx.write().unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let e = s.entries.iter().find(|e| e.path == "staged.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Added);
    assert!(e.staged);
}

#[test]
fn unstaged_modification_is_modified_and_unstaged() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    commit_all(&repo, "init");
    std::fs::write(dir.path().join("a.txt"), "v2").unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let e = s.entries.iter().find(|e| e.path == "a.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Modified);
    assert!(!e.staged);
}

#[test]
fn same_path_can_carry_both_a_staged_and_an_unstaged_entry() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    commit_all(&repo, "init");
    // Stage one change...
    std::fs::write(dir.path().join("a.txt"), "v2").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    // ...then make a further unstaged edit on top of the staged one.
    std::fs::write(dir.path().join("a.txt"), "v3").unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let matching: Vec<_> = s.entries.iter().filter(|e| e.path == "a.txt").collect();
    assert_eq!(matching.len(), 2);
    assert!(matching.iter().any(|e| e.staged));
    assert!(matching.iter().any(|e| !e.staged));
}

#[test]
fn deleted_tracked_file_shows_as_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("d.txt"), "bye").unwrap();
    commit_all(&repo, "init");
    std::fs::remove_file(dir.path().join("d.txt")).unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let e = s.entries.iter().find(|e| e.path == "d.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Deleted);
    assert!(!e.staged);
}

#[test]
fn staged_rename_is_detected_with_old_path() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    // A larger body gives libgit2's similarity heuristic something to match on.
    let body = "line of content\n".repeat(50);
    std::fs::write(dir.path().join("old.txt"), &body).unwrap();
    commit_all(&repo, "init");
    std::fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
    let mut idx = repo.index().unwrap();
    idx.remove_path(Path::new("old.txt")).unwrap();
    idx.add_path(Path::new("new.txt")).unwrap();
    idx.write().unwrap();

    let provider = Git2Provider::factory().open(dir.path()).unwrap();
    let s = provider.status().unwrap();
    let e = s.entries.iter().find(|e| e.path == "new.txt").unwrap();
    assert_eq!(e.kind, StatusKind::Renamed);
    assert!(e.staged);
    assert_eq!(e.old_path.as_deref(), Some("old.txt"));
}

#[test]
fn is_clean_is_consistent_with_entries_being_empty() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    commit_all(&repo, "init");

    let clean_provider = Git2Provider::factory().open(dir.path()).unwrap();
    let clean = clean_provider.status().unwrap();
    assert_eq!(clean.is_clean, clean.entries.is_empty());

    std::fs::write(dir.path().join("x.txt"), "x").unwrap();
    let dirty_provider = Git2Provider::factory().open(dir.path()).unwrap();
    let dirty = dirty_provider.status().unwrap();
    assert_eq!(dirty.is_clean, dirty.entries.is_empty());
    assert!(!dirty.is_clean);
}
