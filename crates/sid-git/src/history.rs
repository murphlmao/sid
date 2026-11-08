//! Paginated decorated log and read-only worktree listing.

use std::collections::HashMap;

use sid_core::git::{GitError, LogEntry, LogPage, LogQuery, RefDecoration, RefKind, WorktreeInfo};

use crate::{commit_info, is_unborn, map_git2_error};

/// oid -> decorations, built once per page. Order within a commit: HEAD, local
/// branches, remote branches, tags (each alphabetical).
fn decorations(
    repo: &git2::Repository,
) -> Result<HashMap<git2::Oid, Vec<RefDecoration>>, GitError> {
    let mut map: HashMap<git2::Oid, Vec<RefDecoration>> = HashMap::new();
    let refs = repo.references().map_err(map_git2_error)?;
    for r in refs.flatten() {
        let Some(name) = r.name() else { continue };
        let (short, kind) = if let Some(n) = name.strip_prefix("refs/heads/") {
            (n, RefKind::Branch)
        } else if let Some(n) = name.strip_prefix("refs/remotes/") {
            if n.ends_with("/HEAD") {
                continue;
            }
            (n, RefKind::RemoteBranch)
        } else if let Some(n) = name.strip_prefix("refs/tags/") {
            (n, RefKind::Tag)
        } else {
            continue;
        };
        // Annotated tags peel to the commit they name.
        let Ok(commit) = r.peel_to_commit() else {
            continue;
        };
        map.entry(commit.id()).or_default().push(RefDecoration {
            name: short.to_string(),
            kind,
        });
    }
    if let Some(oid) = repo.head().ok().and_then(|h| h.target()) {
        map.entry(oid).or_default().push(RefDecoration {
            name: "HEAD".into(),
            kind: RefKind::Head,
        });
    }
    let rank = |k: RefKind| match k {
        RefKind::Head => 0,
        RefKind::Branch => 1,
        RefKind::RemoteBranch => 2,
        RefKind::Tag => 3,
    };
    for v in map.values_mut() {
        v.sort_by(|a, b| (rank(a.kind), &a.name).cmp(&(rank(b.kind), &b.name)));
    }
    Ok(map)
}

pub(crate) fn log_page(repo: &git2::Repository, q: &LogQuery) -> Result<LogPage, GitError> {
    if q.limit == 0 {
        return Ok(LogPage::default());
    }
    let mut walk = repo.revwalk().map_err(map_git2_error)?;
    walk.set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)
        .map_err(map_git2_error)?;
    if q.first_parent {
        walk.simplify_first_parent().map_err(map_git2_error)?;
    }
    match &q.rev {
        Some(rev) => {
            let oid = repo
                .revparse_single(rev)
                .and_then(|o| o.peel_to_commit())
                .map_err(|_| GitError::InvalidRef(rev.clone()))?
                .id();
            walk.push(oid).map_err(map_git2_error)?;
        }
        None => match repo.head() {
            Ok(_) => walk.push_head().map_err(map_git2_error)?,
            Err(e) if is_unborn(&e) => return Ok(LogPage::default()),
            Err(e) => return Err(map_git2_error(e)),
        },
    }
    let decor = decorations(repo)?;
    let mut entries = Vec::with_capacity(q.limit.min(256));
    let mut has_more = false;
    for (i, oid) in walk.skip(q.skip).enumerate() {
        if i == q.limit {
            has_more = true;
            break;
        }
        let oid = oid.map_err(map_git2_error)?;
        let c = repo.find_commit(oid).map_err(map_git2_error)?;
        // With first-parent the page still reports every parent (the graph is
        // simplified, the commit is not).
        entries.push(LogEntry {
            commit: commit_info(&c),
            parents: c.parent_ids().map(|p| p.to_string()).collect(),
            refs: decor.get(&oid).cloned().unwrap_or_default(),
        });
    }
    Ok(LogPage { entries, has_more })
}

fn head_branch(repo: &git2::Repository) -> Option<String> {
    let h = repo.head().ok()?;
    h.is_branch().then(|| h.shorthand().map(String::from))?
}

pub(crate) fn worktrees(repo: &git2::Repository) -> Result<Vec<WorktreeInfo>, GitError> {
    // The main worktree is whatever owns the common dir.
    let main_repo;
    let main = if repo.is_worktree() {
        main_repo = git2::Repository::open(repo.commondir()).map_err(map_git2_error)?;
        &main_repo
    } else {
        repo
    };
    let mut out = vec![WorktreeInfo {
        name: None,
        path: main
            .workdir()
            .unwrap_or_else(|| main.path())
            .display()
            .to_string(),
        branch: head_branch(main),
        is_main: true,
        locked: false,
        prunable: false,
    }];
    let names = main.worktrees().map_err(map_git2_error)?;
    for name in names.iter().flatten() {
        let Ok(wt) = main.find_worktree(name) else {
            continue;
        };
        let locked = matches!(wt.is_locked(), Ok(git2::WorktreeLockStatus::Locked(_)));
        let prunable = wt.is_prunable(None).unwrap_or(false);
        let branch = git2::Repository::open_from_worktree(&wt)
            .ok()
            .and_then(|r| head_branch(&r));
        out.push(WorktreeInfo {
            name: Some(name.to_string()),
            path: wt.path().display().to_string(),
            branch,
            is_main: false,
            locked,
            prunable,
        });
    }
    Ok(out)
}
