//! Rich status: per-side rows, folded per-path rows, and bucket counts.

use std::collections::BTreeMap;

use sid_core::git::{
    FileStatus, GitError, GitStatus, StatusCounts, StatusEntry, StatusKind, StatusOptions,
};

use crate::map_git2_error;

/// The per-side rows only (what summaries / the dirty-tree guard need).
pub(crate) fn collect_status_entries(
    repo: &git2::Repository,
) -> Result<Vec<StatusEntry>, GitError> {
    Ok(collect(repo, &StatusOptions::default())?.entries)
}

/// Full [`GitStatus`] honouring `opts`.
pub(crate) fn full_status(
    repo: &git2::Repository,
    opts: &StatusOptions,
) -> Result<GitStatus, GitError> {
    collect(repo, opts)
}

fn collect(repo: &git2::Repository, opts: &StatusOptions) -> Result<GitStatus, GitError> {
    let mut so = git2::StatusOptions::new();
    so.include_untracked(opts.include_untracked)
        .recurse_untracked_dirs(opts.include_untracked)
        .include_ignored(opts.include_ignored)
        .recurse_ignored_dirs(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);
    let statuses = repo.statuses(Some(&mut so)).map_err(map_git2_error)?;

    let mut entries = Vec::new();
    let mut files: BTreeMap<String, FileStatus> = BTreeMap::new();
    for entry in statuses.iter() {
        let s = entry.status();
        let lone = entry.path().unwrap_or("").to_string();
        let row = files.entry(lone.clone()).or_insert_with(|| FileStatus {
            path: lone.clone(),
            old_path: None,
            index: None,
            worktree: None,
        });

        if let Some(kind) = index_kind(s) {
            if let Some(delta) = entry.head_to_index() {
                let old_path = s.is_index_renamed().then(|| delta_path(&delta.old_file()));
                let path = delta_path(&delta.new_file());
                row.path = path.clone();
                row.index = Some(kind);
                if old_path.is_some() {
                    row.old_path = old_path.clone();
                }
                entries.push(StatusEntry {
                    path,
                    kind,
                    staged: true,
                    old_path,
                });
            }
        }
        if let Some(kind) = worktree_kind(s) {
            let delta = entry.index_to_workdir();
            // Ignored rows carry no diff delta; fall back to the entry's own path.
            if delta.is_some() || kind == StatusKind::Ignored {
                let old_path = delta
                    .as_ref()
                    .filter(|_| s.is_wt_renamed())
                    .map(|d| delta_path(&d.old_file()));
                let path = delta
                    .as_ref()
                    .map(|d| delta_path(&d.new_file()))
                    .unwrap_or_else(|| lone.clone());
                row.worktree = Some(kind);
                if old_path.is_some() {
                    row.old_path = old_path.clone();
                }
                entries.push(StatusEntry {
                    path,
                    kind,
                    staged: false,
                    old_path,
                });
            }
        }
        if s.is_conflicted() {
            row.worktree = Some(StatusKind::Conflicted);
            entries.push(StatusEntry {
                path: lone,
                kind: StatusKind::Conflicted,
                staged: false,
                old_path: None,
            });
        }
    }

    // Drop empty rows (entries libgit2 listed that carry no state, e.g. CURRENT).
    let files: Vec<FileStatus> = files
        .into_values()
        .filter(|f| f.index.is_some() || f.worktree.is_some())
        .collect();
    let mut counts = StatusCounts::default();
    for f in &files {
        counts.staged += usize::from(f.is_staged());
        counts.unstaged += usize::from(f.is_unstaged());
        counts.untracked += usize::from(f.is_untracked());
        counts.conflicted += usize::from(f.is_conflicted());
        counts.ignored += usize::from(f.worktree == Some(StatusKind::Ignored));
    }
    Ok(GitStatus {
        // Ignored rows are opt-in noise: they never make a tree "dirty".
        is_clean: entries.iter().all(|e| e.kind == StatusKind::Ignored),
        entries,
        files,
        counts,
    })
}

fn index_kind(s: git2::Status) -> Option<StatusKind> {
    if s.is_index_new() {
        Some(StatusKind::Added)
    } else if s.is_index_deleted() {
        Some(StatusKind::Deleted)
    } else if s.is_index_renamed() {
        Some(StatusKind::Renamed)
    } else if s.is_index_typechange() {
        Some(StatusKind::Typechange)
    } else if s.is_index_modified() {
        Some(StatusKind::Modified)
    } else {
        None
    }
}

fn worktree_kind(s: git2::Status) -> Option<StatusKind> {
    if s.is_wt_new() {
        Some(StatusKind::Untracked)
    } else if s.is_wt_deleted() {
        Some(StatusKind::Deleted)
    } else if s.is_wt_renamed() {
        Some(StatusKind::Renamed)
    } else if s.is_wt_typechange() {
        Some(StatusKind::Typechange)
    } else if s.is_wt_modified() {
        Some(StatusKind::Modified)
    } else if s.is_ignored() {
        Some(StatusKind::Ignored)
    } else {
        None
    }
}

/// A `DiffFile`'s path ("" when the side carries none).
fn delta_path(f: &git2::DiffFile<'_>) -> String {
    f.path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}
