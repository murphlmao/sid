//! Mutating operations: stage/unstage/discard, commit, stash, branch management.

use std::path::{Component, Path};

use sid_core::git::{Branch, CommitInfo, GitError, StashEntry};

use crate::status::collect_status_entries;
use crate::{branch_info, commit_info, is_unborn, map_git2_error};

// ─── Paths ──────────────────────────────────────────────────────────────────

/// Reject anything that could address outside the repo: absolute paths, `..`, empty.
fn check_path(p: &str) -> Result<&Path, GitError> {
    let path = Path::new(p);
    let bad = p.is_empty()
        || path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        });
    if bad {
        return Err(GitError::InvalidRef(format!(
            "path outside the repository: {p:?}"
        )));
    }
    Ok(path)
}

fn workdir(repo: &git2::Repository) -> Result<&Path, GitError> {
    repo.workdir()
        .ok_or_else(|| GitError::Other("bare repository has no working tree".into()))
}

/// Whether `p` (file or directory prefix) has tracked entries in the index.
fn is_tracked(index: &git2::Index, p: &str) -> bool {
    if index.get_path(Path::new(p), 0).is_some() {
        return true;
    }
    let prefix = format!("{}/", p.trim_end_matches('/'));
    index
        .iter()
        .any(|e| String::from_utf8_lossy(&e.path).starts_with(&prefix))
}

// ─── Stage / unstage / discard ──────────────────────────────────────────────

pub(crate) fn stage_paths(repo: &git2::Repository, paths: &[String]) -> Result<(), GitError> {
    let mut index = repo.index().map_err(map_git2_error)?;
    let wd = workdir(repo)?;
    for p in paths {
        let rel = check_path(p)?;
        let full = wd.join(rel);
        match std::fs::symlink_metadata(&full) {
            Ok(m) if m.is_dir() => {
                let spec = format!("{}/", p.trim_end_matches('/'));
                index
                    .add_all([spec.as_str()], git2::IndexAddOption::DEFAULT, None)
                    .map_err(map_git2_error)?;
                // Deletions inside the directory.
                index
                    .update_all([spec.as_str()], None)
                    .map_err(map_git2_error)?;
            }
            Ok(_) => index.add_path(rel).map_err(map_git2_error)?,
            Err(_) => {
                // Gone from disk: stage the deletion (a no-op if it was never tracked).
                if is_tracked(&index, p) {
                    let _ = index.remove_path(rel);
                    let prefix = format!("{}/", p.trim_end_matches('/'));
                    let _ = index.remove_dir(Path::new(prefix.trim_end_matches('/')), 0);
                } else {
                    return Err(GitError::InvalidRef(format!("no such path: {p}")));
                }
            }
        }
    }
    index.write().map_err(map_git2_error)
}

pub(crate) fn stage_all(repo: &git2::Repository) -> Result<(), GitError> {
    let mut index = repo.index().map_err(map_git2_error)?;
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .map_err(map_git2_error)?;
    index.update_all(["*"], None).map_err(map_git2_error)?;
    index.write().map_err(map_git2_error)
}

pub(crate) fn unstage_paths(repo: &git2::Repository, paths: &[String]) -> Result<(), GitError> {
    for p in paths {
        check_path(p)?;
    }
    match repo.head() {
        Ok(head) => {
            let obj = head
                .peel(git2::ObjectType::Commit)
                .map_err(map_git2_error)?;
            repo.reset_default(Some(&obj), paths.iter().map(String::as_str))
                .map_err(map_git2_error)
        }
        Err(e) if is_unborn(&e) => {
            // Nothing to reset to: staged-new files just leave the index.
            let mut index = repo.index().map_err(map_git2_error)?;
            for p in paths {
                let rel = check_path(p)?;
                if index.remove_path(rel).is_err() {
                    let _ = index.remove_dir(rel, 0);
                }
            }
            index.write().map_err(map_git2_error)
        }
        Err(e) => Err(map_git2_error(e)),
    }
}

pub(crate) fn unstage_all(repo: &git2::Repository) -> Result<(), GitError> {
    match repo.head() {
        Ok(head) => {
            let obj = head
                .peel(git2::ObjectType::Commit)
                .map_err(map_git2_error)?;
            repo.reset(&obj, git2::ResetType::Mixed, None)
                .map_err(map_git2_error)
        }
        Err(e) if is_unborn(&e) => {
            let mut index = repo.index().map_err(map_git2_error)?;
            index.clear().map_err(map_git2_error)?;
            index.write().map_err(map_git2_error)
        }
        Err(e) => Err(map_git2_error(e)),
    }
}

pub(crate) fn discard_paths(repo: &git2::Repository, paths: &[String]) -> Result<(), GitError> {
    let index = repo.index().map_err(map_git2_error)?;
    for p in paths {
        check_path(p)?;
        if !is_tracked(&index, p) {
            return Err(GitError::InvalidRef(format!(
                "{p}: not tracked (use remove_untracked)"
            )));
        }
    }
    let mut cb = git2::build::CheckoutBuilder::new();
    // Literal paths: a tracked name like `a*` must not match other files.
    cb.force()
        .remove_untracked(false)
        .disable_pathspec_match(true);
    for p in paths {
        cb.path(p.as_str());
    }
    repo.checkout_index(None, Some(&mut cb))
        .map_err(map_git2_error)
}

/// Whether `p` is inside, or contains at any depth, a repository's metadata directory.
/// Symlinks are not followed.
fn holds_repo(p: &Path) -> bool {
    const META: &str = ".git";
    if p.components().any(|c| c.as_os_str() == META) {
        return true;
    }
    if !std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(p) else {
        return false;
    };
    rd.flatten().any(|e| {
        e.file_name() == META || (e.file_type().is_ok_and(|t| t.is_dir()) && holds_repo(&e.path()))
    })
}

pub(crate) fn remove_untracked(repo: &git2::Repository, paths: &[String]) -> Result<(), GitError> {
    let index = repo.index().map_err(map_git2_error)?;
    let wd = workdir(repo)?;
    // Validate everything before deleting anything.
    for p in paths {
        check_path(p)?;
        if is_tracked(&index, p) {
            return Err(GitError::InvalidRef(format!(
                "{p}: tracked (use discard_paths)"
            )));
        }
    }
    // A directory holding a repository (at any depth) is someone's repo, not junk.
    for p in paths {
        if holds_repo(&wd.join(check_path(p)?)) {
            return Err(GitError::InvalidRef(format!(
                "{p}: contains a repository — remove it by hand"
            )));
        }
    }
    for p in paths {
        let full = wd.join(check_path(p)?);
        match std::fs::symlink_metadata(&full) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&full),
            Ok(_) => std::fs::remove_file(&full),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
        .map_err(|e| GitError::Other(format!("removing {p}: {e}")))?;
    }
    Ok(())
}

// ─── Commit ─────────────────────────────────────────────────────────────────

pub(crate) fn signature(repo: &git2::Repository) -> Result<git2::Signature<'static>, GitError> {
    repo.signature().map_err(|e| {
        if e.code() == git2::ErrorCode::NotFound {
            GitError::MissingIdentity(
                "set user.name and user.email (git config --global user.name \"…\"; \
                 git config --global user.email \"…\")"
                    .into(),
            )
        } else {
            map_git2_error(e)
        }
    })
}

pub(crate) fn commit(
    repo: &git2::Repository,
    message: &str,
    amend: bool,
) -> Result<CommitInfo, GitError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(GitError::EmptyMessage);
    }
    let message = format!("{message}\n");
    let sig = signature(repo)?;

    let mut index = repo.index().map_err(map_git2_error)?;
    if index.has_conflicts() {
        let n = index.conflicts().map_err(map_git2_error)?.count();
        return Err(GitError::Conflicts(n));
    }
    if repo.state() != git2::RepositoryState::Clean {
        return Err(GitError::Other(format!(
            "repository is mid-{:?}; finish or abort it first",
            repo.state()
        )));
    }
    let tree_id = index.write_tree().map_err(map_git2_error)?;
    let tree = repo.find_tree(tree_id).map_err(map_git2_error)?;

    let head = match repo.head() {
        Ok(h) => Some(h.peel_to_commit().map_err(map_git2_error)?),
        Err(e) if is_unborn(&e) => None,
        Err(e) => return Err(map_git2_error(e)),
    };

    let new_oid = if amend {
        let head =
            head.ok_or_else(|| GitError::Other("nothing to amend: no commits yet".into()))?;
        head.amend(
            Some("HEAD"),
            None,
            Some(&sig),
            None,
            Some(&message),
            Some(&tree),
        )
        .map_err(map_git2_error)?
    } else {
        if let Some(parent) = &head {
            if parent.tree_id() == tree_id {
                return Err(GitError::NothingToCommit);
            }
        } else if tree.is_empty() {
            return Err(GitError::NothingToCommit);
        }
        let parents: Vec<&git2::Commit<'_>> = head.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, &message, &tree, &parents)
            .map_err(map_git2_error)?
    };
    let c = repo.find_commit(new_oid).map_err(map_git2_error)?;
    Ok(commit_info(&c))
}

// ─── Stash ──────────────────────────────────────────────────────────────────

/// `"WIP on main: abc subject"` / `"On main: custom"` -> `main`.
fn stash_branch(message: &str) -> Option<String> {
    let rest = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))?;
    let (branch, _) = rest.split_once(':')?;
    Some(branch.to_string())
}

pub(crate) fn stash_list(repo: &mut git2::Repository) -> Result<Vec<StashEntry>, GitError> {
    let mut out = Vec::new();
    repo.stash_foreach(|index, message, oid| {
        out.push(StashEntry {
            index,
            message: message.to_string(),
            oid: oid.to_string(),
            branch: stash_branch(message),
        });
        true
    })
    .map_err(map_git2_error)?;
    Ok(out)
}

pub(crate) fn stash_push(
    repo: &mut git2::Repository,
    message: Option<&str>,
    include_untracked: bool,
) -> Result<StashEntry, GitError> {
    let sig = signature(repo)?;
    let flags = if include_untracked {
        git2::StashFlags::INCLUDE_UNTRACKED
    } else {
        git2::StashFlags::DEFAULT
    };
    let msg = message.map(str::trim).filter(|m| !m.is_empty());
    match repo.stash_save2(&sig, msg, Some(flags)) {
        Ok(_) => {}
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Err(GitError::NothingToStash),
        Err(e) => return Err(map_git2_error(e)),
    }
    stash_list(repo)?
        .into_iter()
        .next()
        .ok_or_else(|| GitError::Other("stash vanished after push".into()))
}

fn check_stash_index(repo: &mut git2::Repository, index: usize) -> Result<(), GitError> {
    if stash_list(repo)?.iter().any(|s| s.index == index) {
        Ok(())
    } else {
        Err(GitError::StashNotFound(index))
    }
}

fn map_apply_error(e: git2::Error) -> GitError {
    match e.code() {
        git2::ErrorCode::Conflict | git2::ErrorCode::MergeConflict | git2::ErrorCode::Unmerged => {
            GitError::Conflict(e.message().to_string())
        }
        _ => map_git2_error(e),
    }
}

/// libgit2 reports a *content* conflict by applying with markers and leaving
/// unmerged index entries (no error). Surface that as [`GitError::Conflicts`].
fn conflicts_left(repo: &git2::Repository) -> Result<(), GitError> {
    let index = repo.index().map_err(map_git2_error)?;
    if index.has_conflicts() {
        let n = index.conflicts().map_err(map_git2_error)?.count();
        return Err(GitError::Conflicts(n));
    }
    Ok(())
}

pub(crate) fn stash_apply(repo: &mut git2::Repository, index: usize) -> Result<(), GitError> {
    check_stash_index(repo, index)?;
    repo.stash_apply(index, None).map_err(map_apply_error)?;
    conflicts_left(repo)
}

/// Apply then drop — but, like `git stash pop`, keep the entry when the apply left
/// conflicts behind.
pub(crate) fn stash_pop(repo: &mut git2::Repository, index: usize) -> Result<(), GitError> {
    check_stash_index(repo, index)?;
    repo.stash_apply(index, None).map_err(map_apply_error)?;
    conflicts_left(repo)?;
    repo.stash_drop(index).map_err(map_git2_error)
}

pub(crate) fn stash_drop(repo: &mut git2::Repository, index: usize) -> Result<(), GitError> {
    check_stash_index(repo, index)?;
    repo.stash_drop(index).map_err(map_git2_error)
}

// ─── Branches ───────────────────────────────────────────────────────────────

fn map_branch_error(e: git2::Error, name: &str) -> GitError {
    match e.code() {
        git2::ErrorCode::Exists => GitError::BranchExists(name.to_string()),
        git2::ErrorCode::NotFound => GitError::BranchNotFound(name.to_string()),
        git2::ErrorCode::InvalidSpec => GitError::InvalidRef(name.to_string()),
        _ => map_git2_error(e),
    }
}

pub(crate) fn create_branch(
    repo: &git2::Repository,
    name: &str,
    from: Option<&str>,
) -> Result<Branch, GitError> {
    let rev = from.unwrap_or("HEAD");
    let commit = repo
        .revparse_single(rev)
        .and_then(|o| o.peel_to_commit())
        .map_err(|_| GitError::InvalidRef(rev.to_string()))?;
    let b = repo
        .branch(name, &commit, false)
        .map_err(|e| map_branch_error(e, name))?;
    Ok(branch_info(&b))
}

pub(crate) fn delete_branch(
    repo: &git2::Repository,
    name: &str,
    force: bool,
) -> Result<(), GitError> {
    let mut b = repo
        .find_branch(name, git2::BranchType::Local)
        .map_err(|e| map_branch_error(e, name))?;
    if b.is_head() {
        return Err(GitError::BranchIsCurrent(name.to_string()));
    }
    if !force {
        let tip = b
            .get()
            .target()
            .ok_or_else(|| GitError::InvalidRef(name.to_string()))?;
        let merged_into = |target: Option<git2::Oid>| {
            target.is_some_and(|t| t == tip || repo.graph_descendant_of(t, tip).unwrap_or(false))
        };
        let in_head = merged_into(repo.head().ok().and_then(|h| h.target()));
        let in_upstream = merged_into(b.upstream().ok().and_then(|u| u.get().target()));
        if !in_head && !in_upstream {
            return Err(GitError::BranchNotMerged(name.to_string()));
        }
    }
    b.delete().map_err(|e| map_branch_error(e, name))
}

pub(crate) fn rename_branch(repo: &git2::Repository, old: &str, new: &str) -> Result<(), GitError> {
    let mut b = repo
        .find_branch(old, git2::BranchType::Local)
        .map_err(|e| map_branch_error(e, old))?;
    b.rename(new, false)
        .map(|_| ())
        .map_err(|e| map_branch_error(e, new))
}

/// Dirty-tree guard shared with `checkout_branch`.
pub(crate) fn tracked_change_count(repo: &git2::Repository) -> Result<usize, GitError> {
    Ok(collect_status_entries(repo)?
        .iter()
        .filter(|e| e.kind != sid_core::git::StatusKind::Untracked)
        .count())
}
