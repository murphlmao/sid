//! Workspace metadata — parsed from `<workspace>/.sid/_metadata.sid` (JSON with a
//! custom extension) or sniffed from common manifest files (CLAUDE.md, Procfile,
//! `package.json#workspaces`, `Cargo.toml#workspace.members`).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SidError;

/// Classifies a workspace directory.
///
/// # Examples
///
/// ```
/// use sid_core::workspace_metadata::WorkspaceKind;
///
/// let kind = WorkspaceKind::Repo;
/// assert_eq!(kind, WorkspaceKind::Repo);
///
/// let umbrella = WorkspaceKind::Umbrella;
/// assert_ne!(kind, umbrella);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkspaceKind {
    /// A single git repository.
    Repo,
    /// A directory containing multiple sub-repos (e.g. a stack with symlinks or a Cargo workspace).
    Umbrella,
}

/// A user-defined quick-action associated with a workspace.
///
/// # Examples
///
/// ```
/// use sid_core::workspace_metadata::WorkspaceAction;
///
/// let action = WorkspaceAction {
///     label: "Build".into(),
///     cmd: "cargo build".into(),
///     key: Some('b'),
/// };
/// assert_eq!(action.key, Some('b'));
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceAction {
    /// Human-readable label shown in the action menu.
    pub label: String,
    /// Shell command to run (relative to the workspace root).
    pub cmd: String,
    /// Optional single-character keybind for the action.
    pub key: Option<char>,
}

/// Metadata describing a workspace.
///
/// Populated either from `.sid/_metadata.sid` (highest priority) or inferred
/// by sniffing common manifest files.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_core::workspace_metadata::{WorkspaceAction, WorkspaceKind, WorkspaceMetadata};
///
/// let m = WorkspaceMetadata {
///     name: "my-project".into(),
///     kind: WorkspaceKind::Repo,
///     actions: vec![],
///     children: vec![],
/// };
/// assert_eq!(m.name, "my-project");
/// assert_eq!(m.kind, WorkspaceKind::Repo);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceMetadata {
    /// Display name for this workspace.
    pub name: String,
    /// Structural classification.
    pub kind: WorkspaceKind,
    /// Quick-actions runnable from the Workspaces tab.
    #[serde(default)]
    pub actions: Vec<WorkspaceAction>,
    /// Relative paths (relative to the workspace root) of child workspaces.
    #[serde(default)]
    pub children: Vec<PathBuf>,
}

impl WorkspaceMetadata {
    /// Build a minimal `WorkspaceMetadata` inferred from a path's basename.
    ///
    /// Use when no explicit metadata is available. The `kind` is caller-supplied.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use sid_core::workspace_metadata::{WorkspaceKind, WorkspaceMetadata};
    ///
    /// let m = WorkspaceMetadata::from_basename(Path::new("/home/user/vcs/sid"), WorkspaceKind::Repo);
    /// assert_eq!(m.name, "sid");
    /// assert_eq!(m.kind, WorkspaceKind::Repo);
    /// assert!(m.actions.is_empty());
    /// ```
    pub fn from_basename(path: &Path, kind: WorkspaceKind) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();
        Self { name, kind, actions: Vec::new(), children: Vec::new() }
    }
}

/// Errors that can occur during metadata parsing and sniffing.
///
/// # Examples
///
/// ```
/// use sid_core::workspace_metadata::MetadataError;
///
/// let e = MetadataError::BadJson("invalid JSON at field 'name'".into());
/// let msg = format!("{e}");
/// assert!(msg.contains("malformed"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// An I/O error reading a metadata file.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The file was present but contained invalid JSON or TOML.
    #[error("malformed _metadata.sid: {0}")]
    BadJson(String),
}

impl From<MetadataError> for SidError {
    fn from(e: MetadataError) -> Self {
        SidError::Other(format!("{e}"))
    }
}

/// Parse `<path>/.sid/_metadata.sid` if it exists.
///
/// Returns:
/// - `Ok(Some(meta))` — file present and valid JSON
/// - `Ok(None)` — file absent
/// - `Err(MetadataError)` — file present but malformed
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_metadata::parse_metadata_file;
///
/// let result = parse_metadata_file(Path::new("/home/user/vcs/my-project")).unwrap();
/// // Returns None when .sid/_metadata.sid is absent
/// ```
pub fn parse_metadata_file(path: &Path) -> Result<Option<WorkspaceMetadata>, MetadataError> {
    let f = path.join(".sid").join("_metadata.sid");
    if !f.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&f)?;
    let meta: WorkspaceMetadata = serde_json::from_slice(&bytes)
        .map_err(|e| MetadataError::BadJson(format!("{f:?}: {e}")))?;
    Ok(Some(meta))
}

/// A summary of useful structured data sniffed from a project's CLAUDE.md.
///
/// # Examples
///
/// ```
/// use sid_core::workspace_metadata::ClaudeMdSnippet;
///
/// let snippet = ClaudeMdSnippet { ssh_aliases: vec!["dev-box".into()] };
/// assert!(snippet.ssh_aliases.contains(&"dev-box".to_string()));
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaudeMdSnippet {
    /// SSH host aliases extracted from a "Devices" markdown table.
    pub ssh_aliases: Vec<String>,
}

/// Parse `<path>/CLAUDE.md` if it exists, extracting structured signals.
///
/// Returns `Ok(None)` if absent. Never errors on malformed content — best-effort.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_metadata::sniff_claude_md;
///
/// let snippet = sniff_claude_md(Path::new("/home/user/vcs/sid")).unwrap();
/// ```
pub fn sniff_claude_md(path: &Path) -> Result<Option<ClaudeMdSnippet>, MetadataError> {
    let f = path.join("CLAUDE.md");
    if !f.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&f)?;
    let mut snippet = ClaudeMdSnippet::default();
    // Heuristic: look for markdown table rows where the first column is a backtick-wrapped identifier.
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        if let Some(end_first) = trimmed[1..].find('|') {
            let first_col = trimmed[1..1 + end_first].trim();
            if first_col.starts_with('`') && first_col.ends_with('`') && first_col.len() > 2 {
                let alias = first_col.trim_matches('`').to_string();
                // Filter obvious non-alias values (e.g., header dividers, numbers).
                if !alias.chars().all(|c| c == '-' || c == ':' || c.is_whitespace())
                    && alias.parse::<f64>().is_err()
                {
                    snippet.ssh_aliases.push(alias);
                }
            }
        }
    }
    Ok(Some(snippet))
}

/// Parse Cargo.toml's `[workspace] members` array.
///
/// Returns `Ok(None)` if no Cargo.toml or if it has no `[workspace]` section.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_metadata::sniff_cargo_workspace;
///
/// let members = sniff_cargo_workspace(Path::new("/home/user/vcs/sid")).unwrap();
/// ```
pub fn sniff_cargo_workspace(path: &Path) -> Result<Option<Vec<String>>, MetadataError> {
    let f = path.join("Cargo.toml");
    if !f.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&f)?;
    let doc: toml::Value = text
        .parse()
        .map_err(|e| MetadataError::BadJson(format!("Cargo.toml: {e}")))?;
    let members = doc
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array());
    let Some(arr) = members else {
        return Ok(None);
    };
    let out: Vec<String> =
        arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    Ok(Some(out))
}

/// Parse package.json's `workspaces` array.
///
/// Returns `Ok(None)` if no package.json or if `workspaces` field is absent.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_metadata::sniff_package_json_workspaces;
///
/// let ws = sniff_package_json_workspaces(Path::new("/home/user/vcs/my-app")).unwrap();
/// ```
pub fn sniff_package_json_workspaces(
    path: &Path,
) -> Result<Option<Vec<String>>, MetadataError> {
    let f = path.join("package.json");
    if !f.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&f)?;
    let doc: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| MetadataError::BadJson(format!("package.json: {e}")))?;
    let ws = doc.get("workspaces");
    let arr = match ws {
        Some(serde_json::Value::Array(a)) => a,
        Some(serde_json::Value::Object(o)) => match o.get("packages") {
            Some(serde_json::Value::Array(a)) => a,
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    let out = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    Ok(Some(out))
}

/// Parse Procfile process names (the left side of `name: cmd` lines).
///
/// Checks both `Procfile` and `Procfile.dev`. Returns `Ok(None)` if neither exists
/// or if both are empty.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_metadata::sniff_procfile;
///
/// let procs = sniff_procfile(Path::new("/home/user/vcs/my-app")).unwrap();
/// ```
pub fn sniff_procfile(path: &Path) -> Result<Option<Vec<String>>, MetadataError> {
    let candidates = ["Procfile", "Procfile.dev"];
    for c in candidates {
        let f = path.join(c);
        if !f.exists() {
            continue;
        }
        let text = fs::read_to_string(&f)?;
        let names: Vec<String> = text
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
            .filter_map(|l| l.split_once(':').map(|(n, _)| n.trim().to_string()))
            .collect();
        if !names.is_empty() {
            return Ok(Some(names));
        }
    }
    Ok(None)
}

/// Combined read: prefers `.sid/_metadata.sid`; falls back to sniffing.
///
/// Always succeeds with a `WorkspaceMetadata` — uses the directory basename if nothing
/// is found. Never panics.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_metadata::read_workspace_metadata;
///
/// let meta = read_workspace_metadata(Path::new("/home/user/vcs/sid")).unwrap();
/// assert!(!meta.name.is_empty());
/// ```
pub fn read_workspace_metadata(path: &Path) -> Result<WorkspaceMetadata, MetadataError> {
    // Highest priority: explicit .sid/_metadata.sid
    if let Some(m) = parse_metadata_file(path)? {
        return Ok(m);
    }
    // Sniff for umbrella indicators
    let cargo_members = sniff_cargo_workspace(path)?;
    let pkg_workspaces = sniff_package_json_workspaces(path)?;
    let children: Vec<PathBuf> = cargo_members
        .as_ref()
        .or(pkg_workspaces.as_ref())
        .map(|v| v.iter().map(PathBuf::from).collect())
        .unwrap_or_default();
    let kind = if !children.is_empty() {
        WorkspaceKind::Umbrella
    } else {
        WorkspaceKind::Repo
    };
    Ok(WorkspaceMetadata {
        name: path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string(),
        kind,
        actions: Vec::new(),
        children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_metadata_json_roundtrip(name in "[a-zA-Z0-9 _-]{1,40}", n_actions in 0usize..5) {
            let m = WorkspaceMetadata {
                name: name.clone(),
                kind: WorkspaceKind::Repo,
                actions: (0..n_actions).map(|i| WorkspaceAction {
                    label: format!("act-{i}"),
                    cmd: format!("./run-{i}.sh"),
                    key: None,
                }).collect(),
                children: Vec::new(),
            };
            let j = serde_json::to_string(&m).unwrap();
            let back: WorkspaceMetadata = serde_json::from_str(&j).unwrap();
            prop_assert_eq!(m, back);
        }

        #[test]
        fn prop_read_workspace_metadata_is_total(name in "[a-z]{1,8}") {
            let dir = tempfile::tempdir().unwrap();
            let sub = dir.path().join(&name);
            std::fs::create_dir(&sub).unwrap();
            // Should always succeed regardless of contents
            let _ = read_workspace_metadata(&sub).unwrap();
        }
    }
}
