//! Verifies the GitProvider trait is dyn-compatible (Box<dyn GitProvider> works)
//! and that a no-op MockProvider can implement every method.

use std::path::Path;

use sid_core::adapters::git::{
    Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit, StatusEntry,
    StatusKind,
};

struct MockProvider;

impl GitProvider for MockProvider {
    fn open(&self, _path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        Ok(Box::new(MockProvider))
    }
    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        Ok(vec![])
    }
    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        Ok(None)
    }
    fn status(&self) -> Result<GitStatus, GitError> {
        Ok(GitStatus {
            entries: vec![],
            is_clean: true,
        })
    }
    fn commit_log(
        &self,
        _max: usize,
        _from_oid: Option<&str>,
    ) -> Result<Vec<CommitInfo>, GitError> {
        Ok(vec![])
    }
    fn diff(&self, _staged: bool) -> Result<Vec<DiffEntry>, GitError> {
        Ok(vec![])
    }
    fn checkout_branch(&mut self, _name: &str) -> Result<(), GitError> {
        Ok(())
    }
    fn commit(&mut self, _new: NewCommit<'_>) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

#[test]
fn provider_is_dyn_compatible() {
    let p: Box<dyn GitProvider> = Box::new(MockProvider);
    assert!(p.list_branches().unwrap().is_empty());
    assert!(p.current_branch().unwrap().is_none());
    assert!(p.status().unwrap().is_clean);
    assert!(p.commit_log(10, None).unwrap().is_empty());
    assert!(p.diff(false).unwrap().is_empty());
}

#[test]
fn provider_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn GitProvider>>();
}

#[test]
fn status_kind_variants_exist() {
    let _ = StatusKind::Modified;
    let _ = StatusKind::Added;
    let _ = StatusKind::Deleted;
    let _ = StatusKind::Renamed;
    let _ = StatusKind::Untracked;
    let _ = StatusKind::Conflicted;
}

#[test]
fn status_entry_construction() {
    let e = StatusEntry {
        path: "src/main.rs".into(),
        kind: StatusKind::Modified,
        staged: true,
        old_path: None,
    };
    assert_eq!(e.path, "src/main.rs");
    assert!(e.staged);
}

// Adversarial: every GitError variant must produce a non-empty Display string.
#[test]
fn git_error_display_non_empty_for_all_variants() {
    let variants: &[GitError] = &[
        GitError::NotARepo("/some/path".into()),
        GitError::DirtyWorkingTree(3),
        GitError::BranchNotFound("main".into()),
        GitError::InvalidRef("refs/bad".into()),
        GitError::Conflict(2),
        GitError::Other("something broke".into()),
    ];
    for v in variants {
        let msg = format!("{v}");
        assert!(
            !msg.is_empty(),
            "GitError variant produced empty Display: {v:?}"
        );
    }
}
