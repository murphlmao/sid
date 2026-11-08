//! Shared fixture helpers for the v2 integration tests (each test file `mod common;`s
//! this). Every repo gets a LOCAL identity so tests never depend on the host's config.
#![allow(dead_code)]

use std::path::Path;

use sid_core::git::GitProvider;
use sid_git::Git2Provider;

pub fn init_repo(path: &Path) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(path, &opts).unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Test").unwrap();
    cfg.set_str("user.email", "test@example.com").unwrap();
    // Pin line endings: Windows hosts default `core.autocrlf=true` (global config), which
    // would turn checked-out "\n" into "\r\n" and break byte-exact content assertions.
    cfg.set_bool("core.autocrlf", false).unwrap();
    drop(cfg);
    repo
}

pub fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

pub fn commit_all(repo: &git2::Repository, message: &str) -> git2::Oid {
    let mut idx = repo.index().unwrap();
    idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    idx.update_all(["*"], None).unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("Test", "test@example.com").unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .unwrap()
}

/// A repo with one commit `init` containing `a.txt` ("v1\n").
pub fn seeded() -> (tempfile::TempDir, git2::Repository) {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    write(dir.path(), "a.txt", "v1\n");
    commit_all(&repo, "init");
    (dir, repo)
}

pub fn open(path: &Path) -> Box<dyn GitProvider> {
    Git2Provider::factory().open(path).unwrap()
}
