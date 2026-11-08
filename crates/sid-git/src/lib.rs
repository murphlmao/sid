//! `sid-git` — the `git2`-backed [`sid_core::git::GitProvider`] implementation,
//! ported from the POC's `sid-git` crate (`sid-poc/crates/sid-git/src/lib.rs`,
//! 555 lines) and trimmed to the Workspaces v1 trait surface — see
//! `docs/superpowers/plans/2026-07-06-workspaces-v1.md` §A. This is the only
//! crate allowed to name `git2` (`CLAUDE.md`'s adapter rule).

use std::path::Path;

use sid_core::git::{
    Branch, CommitInfo, Diff, DiffLimits, DiffTarget, FetchReport, GitError, GitProvider,
    GitStatus, LogPage, LogQuery, PushReport, RepoSummary, StashEntry, StatusKind, StatusOptions,
    WorktreeInfo,
};

pub mod cli;
mod diff;
mod history;
mod remote;
mod status;
mod write;

use status::collect_status_entries;

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
    /// The bound repo behind a mutex: `git2::Repository` is `Send` but not `Sync`
    /// (libgit2 handles are not internally synchronized), while the trait requires
    /// `Sync`. Locking per method makes concurrent `&self` calls serialize instead of
    /// race — `Sync` then holds by construction, with no `unsafe` and no reliance on
    /// callers honoring a single-thread convention. An uncontended lock costs
    /// nanoseconds against git I/O that costs milliseconds.
    repo: Option<std::sync::Mutex<git2::Repository>>,
}

impl Git2Provider {
    /// A stateless factory handle with no bound repo — call
    /// [`GitProvider::open`] to bind one.
    pub fn factory() -> Box<dyn GitProvider> {
        Box::new(Git2Provider { repo: None })
    }

    /// The bound repo (locked), or an error if this handle is an unopened factory.
    fn repo(&self) -> Result<std::sync::MutexGuard<'_, git2::Repository>, GitError> {
        self.repo
            .as_ref()
            .ok_or_else(|| GitError::Other("no repo bound; call open() first".into()))?
            .lock()
            .map_err(|_| GitError::Other("git handle poisoned by a panicked thread".into()))
    }
}

impl GitProvider for Git2Provider {
    fn open(&self, path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        let repo = git2::Repository::open(path).map_err(|e| map_open_error(e, path))?;
        Ok(Box::new(Git2Provider {
            repo: Some(std::sync::Mutex::new(repo)),
        }))
    }

    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        let repo = self.repo()?;
        let mut out = Vec::new();
        let iter = repo
            .branches(Some(git2::BranchType::Local))
            .map_err(map_git2_error)?;
        for entry in iter {
            let (b, _branch_type) = entry.map_err(map_git2_error)?;
            out.push(branch_info(&b));
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
        self.status_with(&StatusOptions::default())
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
                let name = h.shorthand().ok().map(String::from);
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

        let entries = collect_status_entries(&repo)?;
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
            (Some(name), false) => ahead_behind(&repo, name)?,
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
        let tracked_changes = write::tracked_change_count(&repo)?;
        if tracked_changes > 0 {
            return Err(GitError::DirtyWorkingTree(tracked_changes));
        }
        let branch = repo
            .find_branch(name, git2::BranchType::Local)
            .map_err(|_| GitError::BranchNotFound(name.to_string()))?;
        let refname = branch
            .get()
            .name()
            .map_err(|_| GitError::InvalidRef(name.to_string()))?
            .to_string();
        let obj = repo.revparse_single(&refname).map_err(map_git2_error)?;
        repo.checkout_tree(&obj, None).map_err(map_git2_error)?;
        repo.set_head(&refname).map_err(map_git2_error)?;
        Ok(())
    }

    fn status_with(&self, opts: &StatusOptions) -> Result<GitStatus, GitError> {
        let repo = self.repo()?;
        status::full_status(&repo, opts)
    }

    fn stage_paths(&mut self, paths: &[String]) -> Result<(), GitError> {
        write::stage_paths(&*self.repo()?, paths)
    }

    fn stage_all(&mut self) -> Result<(), GitError> {
        write::stage_all(&*self.repo()?)
    }

    fn unstage_paths(&mut self, paths: &[String]) -> Result<(), GitError> {
        write::unstage_paths(&*self.repo()?, paths)
    }

    fn unstage_all(&mut self) -> Result<(), GitError> {
        write::unstage_all(&*self.repo()?)
    }

    fn discard_paths(&mut self, paths: &[String]) -> Result<(), GitError> {
        write::discard_paths(&*self.repo()?, paths)
    }

    fn remove_untracked(&mut self, paths: &[String]) -> Result<(), GitError> {
        write::remove_untracked(&*self.repo()?, paths)
    }

    fn diff(&self, target: &DiffTarget, limits: &DiffLimits) -> Result<Diff, GitError> {
        diff::diff(&*self.repo()?, target, limits)
    }

    fn commit(&mut self, message: &str, amend: bool) -> Result<CommitInfo, GitError> {
        write::commit(&*self.repo()?, message, amend)
    }

    fn stash_list(&self) -> Result<Vec<StashEntry>, GitError> {
        write::stash_list(&mut *self.repo()?)
    }

    fn stash_push(
        &mut self,
        message: Option<&str>,
        include_untracked: bool,
    ) -> Result<StashEntry, GitError> {
        write::stash_push(&mut *self.repo()?, message, include_untracked)
    }

    fn stash_apply(&mut self, index: usize) -> Result<(), GitError> {
        write::stash_apply(&mut *self.repo()?, index)
    }

    fn stash_pop(&mut self, index: usize) -> Result<(), GitError> {
        write::stash_pop(&mut *self.repo()?, index)
    }

    fn stash_drop(&mut self, index: usize) -> Result<(), GitError> {
        write::stash_drop(&mut *self.repo()?, index)
    }

    fn create_branch(&mut self, name: &str, from: Option<&str>) -> Result<Branch, GitError> {
        write::create_branch(&*self.repo()?, name, from)
    }

    fn delete_branch(&mut self, name: &str, force: bool) -> Result<(), GitError> {
        write::delete_branch(&*self.repo()?, name, force)
    }

    fn rename_branch(&mut self, old: &str, new: &str) -> Result<(), GitError> {
        write::rename_branch(&*self.repo()?, old, new)
    }

    fn remotes(&self) -> Result<Vec<String>, GitError> {
        remote::remotes(&*self.repo()?)
    }

    fn fetch(&self, remote_name: &str) -> Result<FetchReport, GitError> {
        // Lock only for validation; the network work must not block other calls.
        let gitdir = remote::precheck(&*self.repo()?, remote_name)?;
        // Network remotes (ssh/https/git) go through the `git` CLI; libgit2 here has
        // no such transports. Local/file remotes stay on libgit2.
        let (url, workdir) = {
            let repo = self.repo()?;
            let url = repo
                .find_remote(remote_name)
                .ok()
                .and_then(|r| r.url().map(String::from))
                .unwrap_or_default();
            (url, repo.workdir().map(Path::to_path_buf))
        };
        if let (true, Some(workdir)) = (cli::is_network_url(&url), workdir) {
            return cli::fetch(&workdir, remote_name, cli::CLI_TIMEOUT);
        }
        remote::fetch(gitdir, remote_name, remote::FETCH_TIMEOUT)
    }

    fn push(&self) -> Result<PushReport, GitError> {
        let workdir = self
            .repo()?
            .workdir()
            .map(Path::to_path_buf)
            .ok_or_else(|| GitError::Other("bare repository has no working tree".into()))?;
        cli::push(&workdir, cli::CLI_TIMEOUT)
    }

    fn worktrees(&self) -> Result<Vec<WorktreeInfo>, GitError> {
        history::worktrees(&*self.repo()?)
    }

    fn log_page(&self, query: &LogQuery) -> Result<LogPage, GitError> {
        history::log_page(&*self.repo()?, query)
    }
}

// ─── Private helpers ────────────────────────────────────────────────────────

pub(crate) fn commit_info(c: &git2::Commit<'_>) -> CommitInfo {
    CommitInfo {
        oid: c.id().to_string(),
        summary: c.summary().ok().flatten().unwrap_or("").to_string(),
        author_name: c.author().name().unwrap_or("").to_string(),
        author_email: c.author().email().unwrap_or("").to_string(),
        timestamp_secs: c.time().seconds(),
    }
}

/// Ahead/behind counts for `branch_name` against its upstream. `(None, None)`
/// whenever there is no upstream configured — that is not an error case.
pub(crate) fn ahead_behind(
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

pub(crate) fn is_unborn(e: &git2::Error) -> bool {
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

pub(crate) fn map_git2_error(e: git2::Error) -> GitError {
    GitError::Other(e.message().to_string())
}

pub(crate) fn branch_info(b: &git2::Branch<'_>) -> Branch {
    Branch {
        name: b.name().ok().flatten().unwrap_or("").to_string(),
        head_oid: b.get().target().map(|o| o.to_string()).unwrap_or_default(),
        upstream: b
            .upstream()
            .ok()
            .and_then(|u| u.name().ok().flatten().map(String::from)),
        is_current: b.is_head(),
    }
}

/// Resolve any rev to its tree (for ref-to-ref diffs).
pub(crate) fn resolve_tree<'r>(
    repo: &'r git2::Repository,
    rev: &str,
) -> Result<git2::Tree<'r>, GitError> {
    repo.revparse_single(rev)
        .and_then(|o| o.peel_to_tree())
        .map_err(|_| GitError::InvalidRef(rev.to_string()))
}
