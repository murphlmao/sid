//! Workspace discovery — scan a configured root path for git repos and umbrella
//! patterns. Pure walk-the-filesystem logic; persistence is the caller's job.

use std::path::{Path, PathBuf};

use crate::workspace_metadata::{WorkspaceKind, WorkspaceMetadata, read_workspace_metadata};

/// A workspace found during a filesystem scan.
///
/// # Examples
///
/// ```no_run
/// use std::path::PathBuf;
/// use sid_core::workspace_discovery::{DiscoveredWorkspace};
/// use sid_core::workspace_metadata::{WorkspaceKind, WorkspaceMetadata};
///
/// let d = DiscoveredWorkspace {
///     path: PathBuf::from("/home/user/vcs/sid"),
///     kind: WorkspaceKind::Repo,
///     metadata: WorkspaceMetadata::from_basename(
///         std::path::Path::new("/home/user/vcs/sid"),
///         WorkspaceKind::Repo,
///     ),
/// };
/// assert_eq!(d.kind, WorkspaceKind::Repo);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredWorkspace {
    /// Absolute path to the workspace root.
    pub path: PathBuf,
    /// Structural classification (Repo or Umbrella).
    pub kind: WorkspaceKind,
    /// Metadata populated from `.sid/_metadata.sid` or inferred by sniffing.
    pub metadata: WorkspaceMetadata,
}

/// Directory names that are never meaningful workspaces and should be skipped.
const SKIP_DIRS: &[&str] = &["target", "node_modules", "vendor", "build", "dist", ".git"];

/// Scan `root` for git repositories up to `max_depth` levels deep.
///
/// Hidden directories (starting with `.`) and build artifact directories
/// (`target`, `node_modules`, `vendor`, `build`, `dist`) are skipped.
///
/// After collecting repositories, the function runs umbrella detection:
/// any directory that contains a CLAUDE.md, `.code-workspace` file, or
/// `workspace.deps.yaml` AND has git sub-repos under it is emitted as an
/// `Umbrella` workspace alongside its child repos.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_discovery::scan_workspace_root;
///
/// let found = scan_workspace_root(Path::new("/home/user/vcs"), 2).unwrap();
/// for w in &found {
///     println!("{}: {:?}", w.path.display(), w.kind);
/// }
/// ```
pub fn scan_workspace_root(
    root: &Path,
    max_depth: usize,
) -> std::io::Result<Vec<DiscoveredWorkspace>> {
    let mut repos: Vec<DiscoveredWorkspace> = Vec::new();
    let mut umbrella_candidates: Vec<PathBuf> = Vec::new();

    let walker = walkdir::WalkDir::new(root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            // Skip hidden dirs (except the root itself which has depth 0)
            let is_hidden = name.starts_with('.') && e.depth() > 0;
            let is_skip = SKIP_DIRS.contains(&name.as_ref());
            !is_hidden && !is_skip
        });

    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // Detect git repo: .git directory or .git file (for worktrees)
        let git_marker = path.join(".git");
        if git_marker.exists() {
            let metadata = read_workspace_metadata(path)
                .unwrap_or_else(|_| WorkspaceMetadata::from_basename(path, WorkspaceKind::Repo));
            repos.push(DiscoveredWorkspace {
                path: path.to_path_buf(),
                kind: WorkspaceKind::Repo,
                metadata,
            });
        }

        // Detect umbrella signals at any depth
        if is_umbrella_signal(path) {
            umbrella_candidates.push(path.to_path_buf());
        }
    }

    // Post-process: reclassify umbrella candidates that have sub-repos
    let mut out = repos.clone();
    for umbrella_path in &umbrella_candidates {
        let children: Vec<PathBuf> = repos
            .iter()
            .filter(|r| r.path.starts_with(umbrella_path) && r.path != *umbrella_path)
            .map(|r| r.path.clone())
            .collect();
        if !children.is_empty() {
            let mut meta = read_workspace_metadata(umbrella_path).unwrap_or_else(|_| {
                WorkspaceMetadata::from_basename(umbrella_path, WorkspaceKind::Umbrella)
            });
            meta.kind = WorkspaceKind::Umbrella;
            meta.children = children;
            out.push(DiscoveredWorkspace {
                path: umbrella_path.clone(),
                kind: WorkspaceKind::Umbrella,
                metadata: meta,
            });
        }
    }

    Ok(out)
}

/// One adoptable repository found directly under an umbrella root.
///
/// Unlike [`DiscoveredWorkspace`], this carries only the data the adopt-existing
/// wizard needs (display name + absolute path) and is produced by a one-level
/// scan that *does* resolve symlinks — gen4-style umbrellas register satellites
/// as symlinks, which [`scan_workspace_root`] deliberately skips.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_core::workspace_discovery::AdoptableRepo;
///
/// let r = AdoptableRepo { name: "api".into(), path: PathBuf::from("/stack/api") };
/// assert_eq!(r.name, "api");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptableRepo {
    /// Directory basename (the satellite's display name).
    pub name: String,
    /// Absolute path to the repo (symlink resolved to a real dir).
    pub path: PathBuf,
}

/// Find git repos exactly one level under `umbrella`, following symlinks.
///
/// Returns repos sorted by name for deterministic rendering. The umbrella root
/// itself is never included. A directory counts as a repo if it contains a
/// `.git` entry (directory or file — worktrees use a `.git` file). Best-effort:
/// an unreadable `umbrella` yields an empty vec rather than an error, because
/// the caller (a wizard pre-scan) treats "nothing found" and "couldn't read"
/// identically.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_discovery::scan_adoptable_repos;
///
/// let repos = scan_adoptable_repos(Path::new("/home/user/vcs/gen4-stack"));
/// for r in &repos {
///     println!("{} -> {}", r.name, r.path.display());
/// }
/// ```
pub fn scan_adoptable_repos(umbrella: &Path) -> Vec<AdoptableRepo> {
    let mut out: Vec<AdoptableRepo> = Vec::new();
    let Ok(read) = umbrella.read_dir() else {
        return out;
    };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        // Resolve symlinks: metadata() follows links; symlink targets that are
        // real dirs become candidates.
        let path = entry.path();
        let is_dir = std::fs::metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        if path.join(".git").exists() {
            // Canonicalize so a symlinked satellite stores its real path (the
            // primary key for a Workspace record). Fall back to the link path
            // if canonicalization fails (e.g. permission).
            let real = std::fs::canonicalize(&path).unwrap_or(path);
            out.push(AdoptableRepo { name, path: real });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Returns `true` if `path` shows signs of being an umbrella workspace
/// (a directory that groups multiple git repos).
fn is_umbrella_signal(path: &Path) -> bool {
    // Explicit CLAUDE.md signal
    if path.join("CLAUDE.md").exists() {
        return true;
    }
    // Explicit workspace.deps.yaml
    if path.join("workspace.deps.yaml").exists() {
        return true;
    }
    // Any .code-workspace file
    if let Ok(rd) = path.read_dir() {
        for entry in rd.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(".code-workspace") {
                return true;
            }
        }
    }
    false
}

/// Narrow trait used by the discovery service to write workspace records
/// without depending on `sid-store` directly.
///
/// The binary's wire layer adapts the concrete `Store` to this trait.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use sid_core::workspace_discovery::WorkspaceUpserter;
/// use sid_core::workspace_metadata::WorkspaceKind;
///
/// struct NoopStore;
/// impl WorkspaceUpserter for NoopStore {
///     fn upsert(&self, _path: &Path, _kind: WorkspaceKind, _name: &str) -> Result<(), String> {
///         Ok(())
///     }
/// }
///
/// let store = NoopStore;
/// assert!(store.upsert(Path::new("/tmp/x"), WorkspaceKind::Repo, "x").is_ok());
/// ```
pub trait WorkspaceUpserter {
    /// Persist or update a workspace record.
    ///
    /// Returns `Ok(())` on success or `Err(message)` on failure.
    fn upsert(&self, path: &Path, kind: WorkspaceKind, name: &str) -> Result<(), String>;
}

/// Merge a slice of `DiscoveredWorkspace`s into a `WorkspaceUpserter`.
///
/// Stops on the first error. Returns the number of successfully upserted workspaces.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_discovery::{merge_discoveries_into, WorkspaceUpserter};
/// use sid_core::workspace_metadata::WorkspaceKind;
///
/// struct NoopStore;
/// impl WorkspaceUpserter for NoopStore {
///     fn upsert(&self, _: &Path, _: WorkspaceKind, _: &str) -> Result<(), String> { Ok(()) }
/// }
///
/// let n = merge_discoveries_into(&NoopStore, &[]).unwrap();
/// assert_eq!(n, 0);
/// ```
pub fn merge_discoveries_into(
    upserter: &dyn WorkspaceUpserter,
    discoveries: &[DiscoveredWorkspace],
) -> Result<usize, String> {
    let mut count = 0;
    for d in discoveries {
        upserter.upsert(&d.path, d.kind.clone(), &d.metadata.name)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_repo(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn scan_adoptable_finds_direct_and_symlinked_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_repo(&root.join("api"));
        make_repo(&root.join("web"));
        // a symlinked satellite living outside the umbrella, linked in
        let external = tmp.path().join("external-lib");
        make_repo(&external);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&external, root.join("lib")).unwrap();
        // a non-repo dir must be ignored
        std::fs::create_dir_all(root.join("docs")).unwrap();

        let found = scan_adoptable_repos(root);
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"api"));
        assert!(names.contains(&"web"));
        #[cfg(unix)]
        assert!(names.contains(&"lib"));
        assert!(!names.contains(&"docs"));
        // sorted by name for deterministic rendering
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn scan_adoptable_skips_the_umbrella_root_itself() {
        let tmp = tempfile::tempdir().unwrap();
        make_repo(tmp.path()); // root is itself a repo
        make_repo(&tmp.path().join("sub"));
        let found = scan_adoptable_repos(tmp.path());
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["sub"]);
    }

    #[test]
    fn scan_adoptable_on_missing_dir_is_empty() {
        assert!(scan_adoptable_repos(std::path::Path::new("/nonexistent-xyz")).is_empty());
    }
}
