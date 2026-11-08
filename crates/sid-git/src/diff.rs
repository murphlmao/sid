//! Structured, size-capped diffs.

use sid_core::git::{
    Diff, DiffHunk, DiffLimits, DiffLine, DiffLineKind, DiffStatus, DiffTarget, FileDiff, GitError,
};

use crate::{map_git2_error, resolve_tree};

pub(crate) fn diff(
    repo: &git2::Repository,
    target: &DiffTarget,
    limits: &DiffLimits,
) -> Result<Diff, GitError> {
    let mut opts = git2::DiffOptions::new();
    opts.context_lines(3);
    let mut d = match target {
        DiffTarget::WorkdirVsIndex => {
            opts.include_untracked(true)
                .recurse_untracked_dirs(true)
                .show_untracked_content(true);
            repo.diff_index_to_workdir(None, Some(&mut opts))
        }
        DiffTarget::IndexVsHead => {
            let head = match repo.head() {
                Ok(h) => Some(h.peel_to_tree().map_err(map_git2_error)?),
                Err(e) if crate::is_unborn(&e) => None,
                Err(e) => return Err(map_git2_error(e)),
            };
            repo.diff_tree_to_index(head.as_ref(), None, Some(&mut opts))
        }
        DiffTarget::Commit(rev) => {
            let commit = repo
                .revparse_single(rev)
                .and_then(|o| o.peel_to_commit())
                .map_err(|_| GitError::InvalidRef(rev.clone()))?;
            let new = commit.tree().map_err(map_git2_error)?;
            let old = match commit.parent(0) {
                Ok(p) => Some(p.tree().map_err(map_git2_error)?),
                Err(_) => None,
            };
            repo.diff_tree_to_tree(old.as_ref(), Some(&new), Some(&mut opts))
        }
        DiffTarget::Refs { from, to } => {
            let old = resolve_tree(repo, from)?;
            let new = resolve_tree(repo, to)?;
            repo.diff_tree_to_tree(Some(&old), Some(&new), Some(&mut opts))
        }
    }
    .map_err(map_git2_error)?;
    // Rename/copy detection across the whole delta set.
    let mut find = git2::DiffFindOptions::new();
    find.renames(true).copies(false);
    d.find_similar(Some(&mut find)).map_err(map_git2_error)?;

    let total = d.deltas().len();
    let mut out = Diff {
        files: Vec::new(),
        total_files: total,
        truncated: false,
    };
    let mut total_bytes = 0usize;
    for idx in 0..total {
        if out.files.len() >= limits.max_files {
            out.truncated = true;
            break;
        }
        let Some(delta) = d.get_delta(idx) else {
            continue;
        };
        let status = match delta.status() {
            git2::Delta::Added => DiffStatus::Added,
            git2::Delta::Deleted => DiffStatus::Deleted,
            git2::Delta::Renamed => DiffStatus::Renamed,
            git2::Delta::Copied => DiffStatus::Copied,
            git2::Delta::Typechange => DiffStatus::Typechange,
            git2::Delta::Untracked => DiffStatus::Untracked,
            git2::Delta::Conflicted => DiffStatus::Conflicted,
            _ => DiffStatus::Modified,
        };
        let path_of = |f: git2::DiffFile<'_>| f.path().map(|p| p.to_string_lossy().into_owned());
        let mut file = FileDiff {
            old_path: match status {
                DiffStatus::Added | DiffStatus::Untracked => None,
                _ => path_of(delta.old_file()),
            },
            new_path: match status {
                DiffStatus::Deleted => None,
                _ => path_of(delta.new_file()),
            },
            status,
            binary: delta.flags().is_binary(),
            truncated: false,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
        };

        if let Some(patch) = git2::Patch::from_diff(&d, idx).map_err(map_git2_error)? {
            // Binary-ness is only certain once the content has been loaded.
            file.binary = patch.delta().flags().is_binary();
            if !file.binary {
                let (_, adds, dels) = patch.line_stats().map_err(map_git2_error)?;
                file.additions = adds;
                file.deletions = dels;
                let mut file_bytes = 0usize;
                'hunks: for h in 0..patch.num_hunks() {
                    let (hunk, nlines) = patch.hunk(h).map_err(map_git2_error)?;
                    let mut out_hunk = DiffHunk {
                        header: String::from_utf8_lossy(hunk.header())
                            .trim_end()
                            .to_string(),
                        old_start: hunk.old_start(),
                        old_lines: hunk.old_lines(),
                        new_start: hunk.new_start(),
                        new_lines: hunk.new_lines(),
                        lines: Vec::new(),
                    };
                    for l in 0..nlines {
                        let line = patch.line_in_hunk(h, l).map_err(map_git2_error)?;
                        let kind = match line.origin() {
                            '+' => DiffLineKind::Added,
                            '-' => DiffLineKind::Removed,
                            ' ' => DiffLineKind::Context,
                            // "\ No newline at end of file" markers and the like.
                            _ => continue,
                        };
                        let text = String::from_utf8_lossy(line.content());
                        let text = text.trim_end_matches(['\n', '\r']).to_string();
                        let cost = text.len();
                        if file_bytes + cost > limits.max_bytes_per_file
                            || total_bytes + cost > limits.max_total_bytes
                        {
                            file.truncated = true;
                            out.truncated = true;
                            if !out_hunk.lines.is_empty() {
                                file.hunks.push(out_hunk);
                            }
                            break 'hunks;
                        }
                        file_bytes += cost;
                        total_bytes += cost;
                        out_hunk.lines.push(DiffLine {
                            kind,
                            old_lineno: line.old_lineno(),
                            new_lineno: line.new_lineno(),
                            text,
                        });
                    }
                    file.hunks.push(out_hunk);
                }
            }
        }
        out.files.push(file);
    }
    Ok(out)
}
