//! Pure logic behind the Workspaces v2 git views: bucketing status rows, the commit
//! box's enable rule, friendly error copy, and diff gutter formatting. Nothing here
//! touches gpui or git, so it is all unit-tested directly.

use sid_core::git::{DiffLine, DiffStatus, FileStatus, GitError, StatusKind};

/// Status rows split into the three lists the Changes view shows. A path changed in
/// both the index and the working tree appears in `staged` and `unstaged`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Buckets {
    pub staged: Vec<FileStatus>,
    pub unstaged: Vec<FileStatus>,
    pub untracked: Vec<FileStatus>,
    pub conflicted: Vec<FileStatus>,
}

pub(crate) fn bucket(files: &[FileStatus]) -> Buckets {
    let mut b = Buckets::default();
    for f in files {
        if f.is_conflicted() {
            b.conflicted.push(f.clone());
            continue;
        }
        if f.is_staged() {
            b.staged.push(f.clone());
        }
        if f.is_unstaged() {
            b.unstaged.push(f.clone());
        }
        if f.is_untracked() {
            b.untracked.push(f.clone());
        }
    }
    b
}

/// Which side of the index a selected file lives on — decides which diff to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Side {
    Staged,
    Unstaged,
    Untracked,
}

/// The Commit button's rule: something staged and a non-blank message — except an
/// amend, which may just reword (so the message alone suffices).
pub(crate) fn commit_enabled(staged: usize, message: &str, amend: bool) -> bool {
    !message.trim().is_empty() && (staged > 0 || amend)
}

/// One-letter status glyph for a file row.
pub(crate) fn kind_letter(kind: Option<StatusKind>) -> &'static str {
    match kind {
        Some(StatusKind::Added) => "A",
        Some(StatusKind::Modified) => "M",
        Some(StatusKind::Deleted) => "D",
        Some(StatusKind::Renamed) => "R",
        Some(StatusKind::Typechange) => "T",
        Some(StatusKind::Untracked) => "U",
        Some(StatusKind::Conflicted) => "!",
        Some(StatusKind::Ignored) => "I",
        None => " ",
    }
}

pub(crate) fn diff_status_label(s: DiffStatus) -> &'static str {
    match s {
        DiffStatus::Added => "added",
        DiffStatus::Modified => "modified",
        DiffStatus::Deleted => "deleted",
        DiffStatus::Renamed => "renamed",
        DiffStatus::Copied => "copied",
        DiffStatus::Typechange => "type changed",
        DiffStatus::Untracked => "new file",
        DiffStatus::Conflicted => "conflicted",
    }
}

/// Gutter text for a diff line: old and new line numbers, blank where absent.
pub(crate) fn gutter(line: &DiffLine) -> (String, String) {
    let f = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    (f(line.old_lineno), f(line.new_lineno))
}

/// Short (7 char) form of an object id.
pub(crate) fn short_oid(oid: &str) -> &str {
    oid.get(..7).unwrap_or(oid)
}

/// Friendly, sentence-case copy for a git failure: what happened, and what to try.
pub(crate) fn friendly_error(e: &GitError) -> String {
    match e {
        GitError::MissingIdentity(_) => "Git doesn't know who you are yet. Set your name and \
            email once with `git config --global user.name \"You\"` and `git config --global \
            user.email you@example.com`, then commit again."
            .into(),
        GitError::NothingToCommit => "Nothing is staged. Tick the files you want in this \
            commit first."
            .into(),
        GitError::EmptyMessage => "Write a commit message first.".into(),
        GitError::NothingToStash => "There are no local changes to stash.".into(),
        GitError::Conflicts(n) => format!(
            "{n} file{} ha{} conflicts. Resolve {} in your editor, then stage {}.",
            if *n == 1 { "" } else { "s" },
            if *n == 1 { "s" } else { "ve" },
            if *n == 1 { "it" } else { "them" },
            if *n == 1 { "it" } else { "them" },
        ),
        GitError::Conflict(m) => format!("Your local changes collide with this: {m}"),
        GitError::BranchExists(n) => format!("A branch named '{n}' already exists."),
        GitError::BranchIsCurrent(n) => {
            format!("'{n}' is checked out. Switch to another branch before deleting it.")
        }
        GitError::BranchNotMerged(n) => format!(
            "'{n}' has commits that aren't merged anywhere else. Deleting it would lose them."
        ),
        GitError::DirtyWorkingTree(n) => format!(
            "You have {n} uncommitted change{}. Commit or stash them before switching branches.",
            if *n == 1 { "" } else { "s" }
        ),
        GitError::AuthFailed(m) => format!(
            "The remote refused our credentials ({}). sid never asks for passwords — make sure \
             your ssh-agent or credential helper has a key for this remote.",
            m.lines().next().unwrap_or("authentication failed")
        ),
        GitError::TimedOut(m) => format!("Timed out {m}. Check your connection and try again."),
        GitError::RemoteNotFound(n) => format!("There is no remote called '{n}'."),
        other => other.to_string(),
    }
}

/// Validate a branch name typed by the user before bothering git. `None` = fine.
pub(crate) fn branch_name_problem(name: &str) -> Option<&'static str> {
    let n = name.trim();
    if n.is_empty() {
        return Some("Give the branch a name.");
    }
    if n.starts_with('-') || n.starts_with('/') || n.ends_with('/') || n.ends_with(".lock") {
        return Some("That name can't start with '-' or '/', or end with '/' or '.lock'.");
    }
    if n.contains("..") || n.contains("//") || n.ends_with('.') {
        return Some("That name can't contain '..' or '//', or end with '.'.");
    }
    if n.chars()
        .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\@{".contains(c))
    {
        return Some("Branch names can't contain spaces or any of ~ ^ : ? * [ \\ @ {.");
    }
    None
}

/// A stable `Copy` key for a string, for `ConfirmArm` (which needs `K: Copy`).
pub(crate) fn key_of(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_core::git::DiffLineKind;

    fn fs(path: &str, index: Option<StatusKind>, wt: Option<StatusKind>) -> FileStatus {
        FileStatus {
            path: path.into(),
            old_path: None,
            index,
            worktree: wt,
        }
    }

    #[test]
    fn a_path_changed_on_both_sides_lands_in_both_lists() {
        let b = bucket(&[
            fs("a", Some(StatusKind::Added), Some(StatusKind::Modified)),
            fs("b", None, Some(StatusKind::Untracked)),
            fs("c", Some(StatusKind::Modified), None),
            fs("d", None, Some(StatusKind::Conflicted)),
        ]);
        assert_eq!(b.staged.len(), 2);
        assert_eq!(b.unstaged.len(), 1);
        assert_eq!(b.untracked.len(), 1);
        assert_eq!(b.conflicted.len(), 1);
        assert_eq!(b.unstaged[0].path, "a");
    }

    #[test]
    fn commit_needs_staged_content_and_a_message() {
        assert!(commit_enabled(1, "fix", false));
        assert!(!commit_enabled(0, "fix", false));
        assert!(!commit_enabled(1, "   ", false));
        assert!(commit_enabled(0, "reword", true));
        assert!(!commit_enabled(0, "", true));
    }

    #[test]
    fn gutter_blanks_the_absent_side() {
        let l = DiffLine {
            kind: DiffLineKind::Added,
            old_lineno: None,
            new_lineno: Some(7),
            text: String::new(),
        };
        assert_eq!(gutter(&l), (String::new(), "7".to_string()));
    }

    #[test]
    fn errors_say_what_to_do() {
        assert!(friendly_error(&GitError::MissingIdentity("x".into())).contains("user.email"));
        assert!(friendly_error(&GitError::NothingToCommit).contains("Tick"));
        assert!(friendly_error(&GitError::Conflicts(1)).contains("1 file has conflicts"));
        assert!(friendly_error(&GitError::Conflicts(3)).contains("3 files have conflicts"));
        assert!(friendly_error(&GitError::AuthFailed("denied\nmore".into())).contains("denied"));
    }

    #[test]
    fn branch_names_are_screened() {
        assert!(branch_name_problem("feature/x").is_none());
        for bad in [
            "", "  ", "-x", "a b", "a..b", "a~1", "x.lock", "a/", "a@{1}",
        ] {
            assert!(branch_name_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn short_oids() {
        assert_eq!(short_oid("0123456789"), "0123456");
        assert_eq!(short_oid("abc"), "abc");
        assert_eq!(key_of("a"), key_of("a"));
        assert_ne!(key_of("a"), key_of("b"));
    }
}
