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
