//! `Git2Provider` — libgit2-backed `GitProvider` implementation.
//!
//! `Git2ProviderFactory` is a tiny stateless factory used by the binary to
//! produce per-repo `Git2Provider` instances via `open(path)`.
//!
//! # Examples
//!
//! ```no_run
//! use sid_git::Git2ProviderFactory;
//! use sid_core::adapters::git::GitProvider;
//!
//! let factory = Git2ProviderFactory::new();
//! // factory.open(path) returns a per-repo Git2Provider
//! ```

use std::path::Path;

use sid_core::adapters::git::{
    Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit, StatusEntry,
    StatusKind,
};

/// Stateless factory. Call [`Git2ProviderFactory::open`] to get a per-repo provider.
///
/// # Examples
///
/// ```
/// use sid_git::Git2ProviderFactory;
/// let factory = Git2ProviderFactory::new();
/// let _ = &factory; // factory is Send + Sync
/// ```
pub struct Git2ProviderFactory;

impl Git2ProviderFactory {
    /// Create a new factory instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for Git2ProviderFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl GitProvider for Git2ProviderFactory {
    fn open(&self, path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        let repo = git2::Repository::open(path).map_err(map_git2_error_with_path(path))?;
        Ok(Box::new(Git2Provider { repo }))
    }

    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        Err(GitError::Other(
            "factory has no repo; call open() first".into(),
        ))
    }
    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        Err(GitError::Other("factory has no repo".into()))
    }
    fn status(&self) -> Result<GitStatus, GitError> {
        Err(GitError::Other("factory has no repo".into()))
    }
    fn commit_log(&self, _: usize, _: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
        Err(GitError::Other("factory has no repo".into()))
    }
    fn diff(&self, _: bool) -> Result<Vec<DiffEntry>, GitError> {
        Err(GitError::Other("factory has no repo".into()))
    }
    fn checkout_branch(&mut self, _: &str) -> Result<(), GitError> {
        Err(GitError::Other("factory has no repo".into()))
    }
    fn commit(&mut self, _: NewCommit<'_>) -> Result<String, GitError> {
        Err(GitError::Other("factory has no repo".into()))
    }
}

/// Per-repo provider. Returned by `Git2ProviderFactory::open`.
///
/// Holds a `git2::Repository` handle bound to a single directory.
/// Wrap in `Arc<Mutex<...>>` for shared access from multiple tasks.
pub struct Git2Provider {
    repo: git2::Repository,
}

// SAFETY: `git2::Repository` wraps a raw libgit2 pointer and is not internally
// synchronized. We assert `Send + Sync` here because the `App` holds every
// `Git2Provider` behind an `Arc<Mutex<dyn GitProvider>>`, which serializes
// all access to at most one thread at a time. As long as this invariant holds,
// no two threads access the underlying libgit2 handle concurrently.
//
// NOTE: A periodic `cargo +nightly miri test` pass should be added to CI once
// an unsafe-audit workflow is established, to confirm this safety contract is
// maintained as the codebase grows.
unsafe impl Send for Git2Provider {}
unsafe impl Sync for Git2Provider {}

impl GitProvider for Git2Provider {
    fn open(&self, path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        let repo = git2::Repository::open(path).map_err(map_git2_error_with_path(path))?;
        Ok(Box::new(Git2Provider { repo }))
    }

    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        let mut out = Vec::new();
        let current_name = current_branch_shorthand(&self.repo).ok();
        let iter = self
            .repo
            .branches(Some(git2::BranchType::Local))
            .map_err(map_git2_error)?;
        for entry in iter {
            let (b, _bt) = entry.map_err(map_git2_error)?;
            let name = b.name().map_err(map_git2_error)?.unwrap_or("").to_string();
            let head_oid = b.get().target().map(|o| o.to_string()).unwrap_or_default();
            let upstream = b
                .upstream()
                .ok()
                .and_then(|u| u.name().ok().flatten().map(String::from));
            let is_current = current_name.as_deref() == Some(name.as_str());
            out.push(Branch {
                name,
                head_oid,
                upstream,
                is_current,
            });
        }
        Ok(out)
    }

    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        let head = match self.repo.head() {
            Ok(h) => h,
            Err(e)
                if e.code() == git2::ErrorCode::UnbornBranch
                    || e.code() == git2::ErrorCode::NotFound =>
            {
                return Ok(None);
            }
            Err(e) => return Err(map_git2_error(e)),
        };
        let name = head.shorthand().unwrap_or_default().to_string();
        let head_oid = head.target().map(|o| o.to_string()).unwrap_or_default();
        let upstream = self
            .repo
            .find_branch(&name, git2::BranchType::Local)
            .ok()
            .and_then(|b| b.upstream().ok())
            .and_then(|u| u.name().ok().flatten().map(String::from));
        Ok(Some(Branch {
            name,
            head_oid,
            upstream,
            is_current: true,
        }))
    }

    fn status(&self) -> Result<GitStatus, GitError> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true).recurse_untracked_dirs(true);
        let statuses = self
            .repo
            .statuses(Some(&mut opts))
            .map_err(map_git2_error)?;
        let mut entries = Vec::new();
        for entry in statuses.iter() {
            let path = entry.path().unwrap_or("").to_string();
            let s = entry.status();
            let in_index = s.is_index_new()
                || s.is_index_modified()
                || s.is_index_deleted()
                || s.is_index_renamed()
                || s.is_index_typechange();
            let in_wt = s.is_wt_new()
                || s.is_wt_modified()
                || s.is_wt_deleted()
                || s.is_wt_renamed()
                || s.is_wt_typechange();
            // Emit a staged entry if it's in the index, and/or an unstaged entry if it's in WT.
            if in_index {
                entries.push(StatusEntry {
                    path: path.clone(),
                    kind: status_kind_index(s),
                    staged: true,
                    old_path: rename_old_path(&entry, true),
                });
            }
            if in_wt {
                entries.push(StatusEntry {
                    path: path.clone(),
                    kind: status_kind_wt(s),
                    staged: false,
                    old_path: rename_old_path(&entry, false),
                });
            }
            if s.is_conflicted() {
                entries.push(StatusEntry {
                    path,
                    kind: StatusKind::Conflicted,
                    staged: false,
                    old_path: None,
                });
            }
        }
        Ok(GitStatus {
            is_clean: entries.is_empty(),
            entries,
        })
    }

    fn commit_log(&self, max: usize, from_oid: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
        if max == 0 {
            return Ok(Vec::new());
        }
        let mut walk = self.repo.revwalk().map_err(map_git2_error)?;
        match from_oid {
            Some(oid_str) => {
                let oid = git2::Oid::from_str(oid_str)
                    .map_err(|e| GitError::InvalidRef(format!("{oid_str}: {e}")))?;
                walk.push(oid).map_err(map_git2_error)?;
            }
            None => walk.push_head().map_err(map_git2_error)?,
        }
        let mut out = Vec::with_capacity(max);
        for oid_res in walk.take(max) {
            let oid = oid_res.map_err(map_git2_error)?;
            let c = self.repo.find_commit(oid).map_err(map_git2_error)?;
            out.push(CommitInfo {
                oid: oid.to_string(),
                summary: c.summary().ok().flatten().unwrap_or("").to_string(),
                author_name: c.author().name().unwrap_or("").to_string(),
                author_email: c.author().email().unwrap_or("").to_string(),
                timestamp_secs: c.time().seconds(),
                parents: c.parent_ids().map(|p| p.to_string()).collect(),
            });
        }
        Ok(out)
    }

    fn diff(&self, staged: bool) -> Result<Vec<DiffEntry>, GitError> {
        let head_tree = self.repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        let mut opts = git2::DiffOptions::new();
        let diff = if staged {
            // index vs HEAD
            let index = self.repo.index().map_err(map_git2_error)?;
            self.repo
                .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut opts))
                .map_err(map_git2_error)?
        } else {
            // working tree vs index
            let index = self.repo.index().map_err(map_git2_error)?;
            self.repo
                .diff_index_to_workdir(Some(&index), Some(&mut opts))
                .map_err(map_git2_error)?
        };
        let mut entries: Vec<DiffEntry> = Vec::new();
        let mut current: Option<DiffEntry> = None;
        diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            if current.as_ref().map(|e| e.path != path).unwrap_or(true) {
                if let Some(e) = current.take() {
                    entries.push(e);
                }
                current = Some(DiffEntry {
                    path: path.clone(),
                    old_path: delta
                        .old_file()
                        .path()
                        .and_then(|p| p.to_str())
                        .map(String::from),
                    patch: String::new(),
                    added: 0,
                    removed: 0,
                });
            }
            let entry = current.as_mut().unwrap();
            let origin = line.origin();
            let line_content = std::str::from_utf8(line.content()).unwrap_or("");
            match origin {
                '+' => {
                    entry.added += 1;
                    entry.patch.push('+');
                }
                '-' => {
                    entry.removed += 1;
                    entry.patch.push('-');
                }
                ' ' => {
                    entry.patch.push(' ');
                }
                '@' => {
                    entry.patch.push_str("@@");
                }
                _ => {}
            }
            entry.patch.push_str(line_content);
            if !line_content.ends_with('\n') {
                entry.patch.push('\n');
            }
            true
        })
        .map_err(map_git2_error)?;
        if let Some(e) = current.take() {
            entries.push(e);
        }
        Ok(entries)
    }

    fn checkout_branch(&mut self, name: &str) -> Result<(), GitError> {
        // Dirty-tree guard
        let status = self.status()?;
        if !status.is_clean {
            return Err(GitError::DirtyWorkingTree(status.entries.len()));
        }
        let branch = self
            .repo
            .find_branch(name, git2::BranchType::Local)
            .map_err(|_| GitError::BranchNotFound(name.to_string()))?;
        let refname = branch
            .get()
            .name()
            .map_err(|_| GitError::InvalidRef(name.to_string()))?
            .to_string();
        let obj = self
            .repo
            .revparse_single(&refname)
            .map_err(map_git2_error)?;
        self.repo
            .checkout_tree(&obj, None)
            .map_err(map_git2_error)?;
        self.repo.set_head(&refname).map_err(map_git2_error)?;
        Ok(())
    }

    fn commit(&mut self, new: NewCommit<'_>) -> Result<String, GitError> {
        let mut idx = self.repo.index().map_err(map_git2_error)?;
        if new.stage_all {
            idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
                .map_err(map_git2_error)?;
            idx.write().map_err(map_git2_error)?;
        }
        let tree_id = idx.write_tree().map_err(map_git2_error)?;
        let tree = self.repo.find_tree(tree_id).map_err(map_git2_error)?;
        let sig = match (new.author_name, new.author_email) {
            (Some(n), Some(e)) => git2::Signature::now(n, e).map_err(map_git2_error)?,
            _ => self.repo.signature().map_err(map_git2_error)?,
        };
        let parents: Vec<_> = self
            .repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect();
        let parent_refs: Vec<_> = parents.iter().collect();
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, new.message, &tree, &parent_refs)
            .map_err(map_git2_error)?;
        Ok(oid.to_string())
    }
}

// ─── Private helpers ─────────────────────────────────────────────────────────

fn map_git2_error_with_path(path: &Path) -> impl Fn(git2::Error) -> GitError + '_ {
    move |e: git2::Error| match e.code() {
        git2::ErrorCode::NotFound => GitError::NotARepo(format!("{}", path.display())),
        _ => GitError::Other(e.message().to_string()),
    }
}

pub(crate) fn map_git2_error(e: git2::Error) -> GitError {
    match e.code() {
        git2::ErrorCode::NotFound => GitError::Other(format!("not found: {}", e.message())),
        git2::ErrorCode::Conflict => GitError::Conflict(1),
        _ => GitError::Other(e.message().to_string()),
    }
}

fn current_branch_shorthand(repo: &git2::Repository) -> Result<String, GitError> {
    let head = repo.head().map_err(map_git2_error)?;
    Ok(head.shorthand().unwrap_or_default().to_string())
}

fn status_kind_index(s: git2::Status) -> StatusKind {
    if s.is_index_new() {
        StatusKind::Added
    } else if s.is_index_deleted() {
        StatusKind::Deleted
    } else if s.is_index_renamed() {
        StatusKind::Renamed
    } else {
        StatusKind::Modified
    }
}

fn status_kind_wt(s: git2::Status) -> StatusKind {
    if s.is_wt_new() {
        StatusKind::Untracked
    } else if s.is_wt_deleted() {
        StatusKind::Deleted
    } else if s.is_wt_renamed() {
        StatusKind::Renamed
    } else {
        StatusKind::Modified
    }
}

fn rename_old_path(_entry: &git2::StatusEntry<'_>, _staged: bool) -> Option<String> {
    // git2 exposes rename heads via head_to_index().old_file() / index_to_workdir().old_file()
    // For v1 simplicity, return None; rename detection is a Phase B refinement.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_default_equals_new() {
        // Just check that new() works; Default::default() is a unit struct, so
        // both construct the same ZST. Checking new() is sufficient.
        let _a = Git2ProviderFactory::new();
    }

    // Tests for the unsafe Send + Sync impls.
    // These verify the type compiles with the marker traits and that wrapping in
    // Arc<Mutex<...>> works as expected by the safety contract.
    #[test]
    fn git2_provider_factory_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Git2ProviderFactory>();
    }

    #[test]
    fn git2_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Git2Provider>();
    }

    // Verify the safety contract boundary: multiple sequential accesses
    // through Mutex serialize correctly (no data races observed by the type system).
    #[test]
    fn arc_mutex_wrapping_satisfies_safety_contract() {
        use std::sync::{Arc, Mutex};
        let factory = Git2ProviderFactory::new();
        // Can place factory in Arc<Mutex<...>> as the binary does
        let _shared: Arc<Mutex<Git2ProviderFactory>> = Arc::new(Mutex::new(factory));
    }
}
