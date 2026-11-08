//! A missing git identity must be a clear, typed error. This file holds ONE test
//! because it points libgit2's process-global config search paths at an empty
//! directory (so the host's `~/.gitconfig` can't supply a name) — it must not share a
//! process with tests that rely on config.

use sid_core::git::GitError;
use sid_git::Git2Provider;

#[test]
fn commit_and_stash_without_identity_fail_clearly() {
    let empty = tempfile::tempdir().unwrap();
    // SAFETY: single test in this binary; no other thread touches libgit2 config yet.
    unsafe {
        for level in [
            git2::ConfigLevel::System,
            git2::ConfigLevel::XDG,
            git2::ConfigLevel::Global,
        ] {
            git2::opts::set_search_path(level, empty.path()).unwrap();
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let mut o = git2::RepositoryInitOptions::new();
    o.initial_head("main");
    let _repo = git2::Repository::init_opts(dir.path(), &o).unwrap();
    std::fs::write(dir.path().join("a.txt"), "x").unwrap();

    let mut p = Git2Provider::factory().open(dir.path()).unwrap();
    p.stage_all().unwrap();
    let err = p.commit("msg", false).unwrap_err();
    assert!(matches!(err, GitError::MissingIdentity(_)), "{err:?}");
    assert!(err.to_string().contains("user.name"));
    let err = p.stash_push(None, false).unwrap_err();
    assert!(matches!(err, GitError::MissingIdentity(_)), "{err:?}");
}
