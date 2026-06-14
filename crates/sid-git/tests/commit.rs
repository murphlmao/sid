use std::{fs, path::Path};

use sid_core::adapters::git::{GitProvider, NewCommit};
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
fn commit_stages_all_when_requested_and_returns_oid() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("a.txt"), b"first\n").unwrap();
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let oid = provider
        .commit(NewCommit {
            message: "feat: add a.txt",
            author_name: Some("test author"),
            author_email: Some("test@example.com"),
            stage_all: true,
        })
        .unwrap();
    assert_eq!(oid.len(), 40);
    let log = provider.commit_log(1, None).unwrap();
    assert_eq!(log[0].summary, "feat: add a.txt");
    assert_eq!(log[0].author_name, "test author");
}

#[test]
fn commit_without_stage_all_uses_existing_index() {
    let dir = tempdir().unwrap();
    let repo = init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("b.txt"), b"two\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("b.txt")).unwrap();
    idx.write().unwrap();
    fs::write(dir.path().join("c.txt"), b"three\n").unwrap(); // unstaged
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let _oid = provider
        .commit(NewCommit {
            message: "just b",
            author_name: None,
            author_email: None,
            stage_all: false,
        })
        .unwrap();
    let log = provider.commit_log(1, None).unwrap();
    assert_eq!(log[0].summary, "just b");
    let s = provider.status().unwrap();
    // c.txt should still be untracked because stage_all was false
    assert!(s.entries.iter().any(|e| e.path == "c.txt"));
}

#[test]
fn commit_with_empty_message_succeeds_returning_valid_oid() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("a.txt"), b"x").unwrap();
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let oid = provider
        .commit(NewCommit {
            message: "",
            author_name: Some("t"),
            author_email: Some("t@t"),
            stage_all: true,
        })
        .unwrap();
    assert_eq!(oid.len(), 40);
}

// Adversarial: consecutive commits produce parent chain
#[test]
fn consecutive_commits_form_parent_chain() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    // First commit
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let oid1 = provider
        .commit(NewCommit {
            message: "first",
            author_name: Some("t"),
            author_email: Some("t@t"),
            stage_all: true,
        })
        .unwrap();
    // Second commit
    fs::write(dir.path().join("b.txt"), b"b").unwrap();
    let oid2 = provider
        .commit(NewCommit {
            message: "second",
            author_name: Some("t"),
            author_email: Some("t@t"),
            stage_all: true,
        })
        .unwrap();
    let log = provider.commit_log(3, None).unwrap();
    // log is newest-first: oid2 at index 0, oid1 at index 1
    assert_eq!(log[0].oid, oid2);
    assert_eq!(log[0].parents, vec![oid1.clone()]);
    assert_eq!(log[1].oid, oid1);
}

// Adversarial: commit with no files staged (empty index) doesn't panic
#[test]
fn commit_nothing_staged_does_not_panic() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    // Don't stage anything
    let result = provider.commit(NewCommit {
        message: "empty",
        author_name: Some("t"),
        author_email: Some("t@t"),
        stage_all: false,
    });
    // Either Ok (idempotent commit) or Err — no panic
    let _ = result;
}

// Adversarial: unicode in commit message round-trips
#[test]
fn commit_unicode_message_round_trips() {
    let dir = tempdir().unwrap();
    init_repo_with_initial_commit(dir.path());
    fs::write(dir.path().join("x.txt"), b"x").unwrap();
    let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
    let msg = "feat: 仕事 🐕 — Sid loves git";
    provider
        .commit(NewCommit {
            message: msg,
            author_name: Some("t"),
            author_email: Some("t@t"),
            stage_all: true,
        })
        .unwrap();
    let log = provider.commit_log(1, None).unwrap();
    assert_eq!(log[0].summary, msg);
}

use proptest::prelude::*;

proptest! {
    // Git trims leading/trailing whitespace from commit message summaries.
    // The invariant is: summary == message.trim() (git's own behavior).
    // We use a strategy that avoids leading/trailing spaces to keep the
    // round-trip exact, since git normalizes the message before storing.
    #[test]
    fn prop_commit_message_round_trips(
        // Strategy: must start and end with alphanumeric to guarantee trim() is identity
        msg in "[a-zA-Z0-9][a-zA-Z0-9 _.-]{0,78}[a-zA-Z0-9]|[a-zA-Z0-9]"
    ) {
        let dir = tempdir().unwrap();
        init_repo_with_initial_commit(dir.path());
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let mut provider = Git2ProviderFactory::new().open(dir.path()).unwrap();
        let _ = provider
            .commit(NewCommit {
                message: &msg,
                author_name: Some("t"),
                author_email: Some("t@t"),
                stage_all: true,
            })
            .unwrap();
        let log = provider.commit_log(1, None).unwrap();
        prop_assert_eq!(log[0].summary.as_str(), msg.as_str());
    }
}
