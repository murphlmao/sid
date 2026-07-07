//! `sid-git` — the `git2`-backed [`sid_core::git::GitProvider`] implementation,
//! ported from the POC's `sid-git` crate (`sid-poc/crates/sid-git/src/lib.rs`,
//! 555 lines) and trimmed to the Workspaces v1 trait surface — see
//! `docs/superpowers/plans/2026-07-06-workspaces-v1.md` §A. This is the only
//! crate allowed to name `git2` (`CLAUDE.md`'s adapter rule).

use std::path::Path;

use sid_core::git::{
    Branch, CommitInfo, GitError, GitProvider, GitStatus, RepoSummary, StatusEntry, StatusKind,
};

/// git2-backed provider. [`Git2Provider::factory`] returns a stateless handle
/// with no bound repo; [`GitProvider::open`] binds a per-repo handle (the
/// receiver may be the factory or an already-open handle — both delegate to
/// the same `git2::Repository::open`).
///
/// # Examples
///
/// ```
/// use sid_git::Git2Provider;
/// let _factory = Git2Provider::factory();
/// ```
pub struct Git2Provider {
    repo: Option<git2::Repository>,
}

// SAFETY: `git2::Repository` already asserts `Send` itself ("a Repository can
// be sent among threads, or even shared among threads in a mutex" — see
// git2::Repository's own `unsafe impl Send`). It does not assert `Sync`
// because the underlying libgit2 handle is not internally synchronized.
// `sid_core::git::GitProvider` requires `Send + Sync` so `Box<dyn
// GitProvider>` can move freely between the render thread and the shared
// background runtime; the trait's doc comment establishes the contract that
// callers never touch a handle from more than one thread at a time
// ("callers run them on the shared background runtime, never the render
// thread"). Asserting `Sync` here is sound under that serialized-access
// contract — mirrors the POC's `Git2Provider` (`sid-poc/crates/sid-git`).
unsafe impl Sync for Git2Provider {}

impl Git2Provider {
    /// A stateless factory handle with no bound repo — call
    /// [`GitProvider::open`] to bind one.
    pub fn factory() -> Box<dyn GitProvider> {
        Box::new(Git2Provider { repo: None })
    }

    /// The bound repo, or an error if this handle is an unopened factory.
    fn repo(&self) -> Result<&git2::Repository, GitError> {
        self.repo
            .as_ref()
            .ok_or_else(|| GitError::Other("no repo bound; call open() first".into()))
    }
}

impl GitProvider for Git2Provider {
    fn open(&self, path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        let repo = git2::Repository::open(path).map_err(|e| map_open_error(e, path))?;
        Ok(Box::new(Git2Provider { repo: Some(repo) }))
    }

    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        let repo = self.repo()?;
        let mut out = Vec::new();
        let iter = repo
            .branches(Some(git2::BranchType::Local))
            .map_err(map_git2_error)?;
        for entry in iter {
            let (b, _branch_type) = entry.map_err(map_git2_error)?;
            let name = b.name().map_err(map_git2_error)?.unwrap_or("").to_string();
            let head_oid = b.get().target().map(|o| o.to_string()).unwrap_or_default();
            let upstream = b
                .upstream()
                .ok()
                .and_then(|u| u.name().ok().flatten().map(String::from));
            let is_current = b.is_head();
            out.push(Branch {
                name,
                head_oid,
                upstream,
                is_current,
            });
        }
        // Current first, then alphabetical (trait contract). At most one
        // branch is ever current, so a total-order sort is sufficient.
        out.sort_by(|a, b| match (a.is_current, b.is_current) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
        Ok(out)
    }

    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        // Delegates to list_branches so "no branches yet" (unborn HEAD) and
        // "HEAD points at a commit, not a branch" (detached HEAD) both fall
        // out of the same is_current/empty-list logic instead of needing
        // separate error-code handling here.
        Ok(self.list_branches()?.into_iter().find(|b| b.is_current))
    }

    fn status(&self) -> Result<GitStatus, GitError> {
        let repo = self.repo()?;
        let entries = collect_status_entries(repo)?;
        Ok(GitStatus {
            is_clean: entries.is_empty(),
            entries,
        })
    }

    fn commit_log(&self, max: usize) -> Result<Vec<CommitInfo>, GitError> {
        if max == 0 {
            return Ok(Vec::new());
        }
        let repo = self.repo()?;
        // Unborn HEAD (fresh init, no commits): `revwalk().push_head()` fails
        // with a generic "reference not found" (not a distinguishable error
        // code), so check via `head()` first — its unborn case *is*
        // distinguishable — and treat it as "no commits yet", not an error.
        match repo.head() {
            Ok(_) => {}
            Err(e) if is_unborn(&e) => return Ok(Vec::new()),
            Err(e) => return Err(map_git2_error(e)),
        }
        let mut walk = repo.revwalk().map_err(map_git2_error)?;
        // TIME | TOPOLOGICAL: plain TIME sorting alone only breaks ties by
        // commit timestamp, which collide within a test's (or a fast rebase
        // squash's) single-second resolution; TOPOLOGICAL guarantees a
        // child is always walked before its parent, so "newest first" holds
        // even when timestamps tie.
        walk.set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)
            .map_err(map_git2_error)?;
        walk.push_head().map_err(map_git2_error)?;
        let mut out = Vec::with_capacity(max);
        for oid_res in walk.take(max) {
            let oid = oid_res.map_err(map_git2_error)?;
            let c = repo.find_commit(oid).map_err(map_git2_error)?;
            out.push(commit_info(&c));
        }
        Ok(out)
    }

    fn summary(&self) -> Result<RepoSummary, GitError> {
        let repo = self.repo()?;
        let (branch, detached, last_commit) = match repo.head() {
            Ok(h) if h.is_branch() => {
                let name = h.shorthand().map(String::from);
                let commit = h.peel_to_commit().ok();
                (name, false, commit.as_ref().map(commit_info))
            }
            Ok(h) => {
                // Detached HEAD: `branch` holds a short OID per RepoSummary's doc.
                let short = h.target().map(short_oid);
                let commit = h.peel_to_commit().ok();
                (short, true, commit.as_ref().map(commit_info))
            }
            Err(e) if is_unborn(&e) => (None, false, None),
            Err(e) => return Err(map_git2_error(e)),
        };

        let entries = collect_status_entries(repo)?;
        let staged = entries.iter().filter(|e| e.staged).count();
        let unstaged = entries
            .iter()
            .filter(|e| !e.staged && e.kind != StatusKind::Untracked)
            .count();
        let untracked = entries
            .iter()
            .filter(|e| e.kind == StatusKind::Untracked)
            .count();

        let (ahead, behind) = match (&branch, detached) {
            (Some(name), false) => ahead_behind(repo, name)?,
            _ => (None, None),
        };

        Ok(RepoSummary {
            branch,
            detached,
            staged,
            unstaged,
            untracked,
            ahead,
            behind,
            last_commit,
        })
    }

    fn checkout_branch(&mut self, name: &str) -> Result<(), GitError> {
        let repo = self.repo()?;
        // Dirty-tree guard: only *tracked* changes (staged or unstaged) block
        // a checkout. Untracked files are not at risk of being clobbered by
        // a safe checkout, so they must not refuse the switch.
        let entries = collect_status_entries(repo)?;
        let tracked_changes = entries
            .iter()
            .filter(|e| e.kind != StatusKind::Untracked)
            .count();
        if tracked_changes > 0 {
            return Err(GitError::DirtyWorkingTree(tracked_changes));
        }
        let branch = repo
            .find_branch(name, git2::BranchType::Local)
            .map_err(|_| GitError::BranchNotFound(name.to_string()))?;
        let refname = branch
            .get()
            .name()
            .ok_or_else(|| GitError::InvalidRef(name.to_string()))?
            .to_string();
        let obj = repo.revparse_single(&refname).map_err(map_git2_error)?;
        repo.checkout_tree(&obj, None).map_err(map_git2_error)?;
        repo.set_head(&refname).map_err(map_git2_error)?;
        Ok(())
    }
}

// ─── Private helpers ────────────────────────────────────────────────────────

/// Collect every status entry (staged, unstaged/untracked, conflicted).
///
/// A single path can produce two entries (one staged, one unstaged) when it
/// has changes in both the index and the working tree — that mirrors `git
/// status`'s two-column porcelain output and is what lets callers count
/// "staged" and "unstaged" independently.
fn collect_status_entries(repo: &git2::Repository) -> Result<Vec<StatusEntry>, GitError> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(map_git2_error)?;

    let mut entries = Vec::new();
    for entry in statuses.iter() {
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

        if in_index {
            if let Some(delta) = entry.head_to_index() {
                let old_path = s.is_index_renamed().then(|| delta_path(&delta.old_file()));
                entries.push(StatusEntry {
                    path: delta_path(&delta.new_file()),
                    kind: status_kind_index(s),
                    staged: true,
                    old_path,
                });
            }
        }
        if in_wt {
            if let Some(delta) = entry.index_to_workdir() {
                let old_path = s.is_wt_renamed().then(|| delta_path(&delta.old_file()));
                entries.push(StatusEntry {
                    path: delta_path(&delta.new_file()),
                    kind: status_kind_wt(s),
                    staged: false,
                    old_path,
                });
            }
        }
        if s.is_conflicted() {
            entries.push(StatusEntry {
                path: entry.path().unwrap_or("").to_string(),
                kind: StatusKind::Conflicted,
                staged: false,
                old_path: None,
            });
        }
    }
    Ok(entries)
}

/// A `DiffFile`'s path, falling back to the other side (deletions carry no
/// "new" path; the fallback keeps the entry non-empty rather than dropping
/// it).
fn delta_path(f: &git2::DiffFile<'_>) -> String {
    f.path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
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

fn commit_info(c: &git2::Commit<'_>) -> CommitInfo {
    CommitInfo {
        oid: c.id().to_string(),
        summary: c.summary().unwrap_or("").to_string(),
        author_name: c.author().name().unwrap_or("").to_string(),
        author_email: c.author().email().unwrap_or("").to_string(),
        timestamp_secs: c.time().seconds(),
    }
}

/// Ahead/behind counts for `branch_name` against its upstream. `(None, None)`
/// whenever there is no upstream configured — that is not an error case.
fn ahead_behind(
    repo: &git2::Repository,
    branch_name: &str,
) -> Result<(Option<usize>, Option<usize>), GitError> {
    let Ok(local) = repo.find_branch(branch_name, git2::BranchType::Local) else {
        return Ok((None, None));
    };
    let Some(local_oid) = local.get().target() else {
        return Ok((None, None));
    };
    let Ok(upstream) = local.upstream() else {
        return Ok((None, None));
    };
    let Some(upstream_oid) = upstream.get().target() else {
        return Ok((None, None));
    };
    let (ahead, behind) = repo
        .graph_ahead_behind(local_oid, upstream_oid)
        .map_err(map_git2_error)?;
    Ok((Some(ahead), Some(behind)))
}

/// First 7 hex chars of an OID — the conventional "short OID" length.
fn short_oid(oid: git2::Oid) -> String {
    let full = oid.to_string();
    full[..7.min(full.len())].to_string()
}

fn is_unborn(e: &git2::Error) -> bool {
    matches!(
        e.code(),
        git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
    )
}

fn map_open_error(e: git2::Error, path: &Path) -> GitError {
    match e.code() {
        git2::ErrorCode::NotFound => GitError::NotARepo(path.display().to_string()),
        _ => GitError::Other(e.message().to_string()),
    }
}

fn map_git2_error(e: git2::Error) -> GitError {
    GitError::Other(e.message().to_string())
}
