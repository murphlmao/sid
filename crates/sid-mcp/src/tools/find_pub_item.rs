//! `find_pub_item` tool — locate a public item by name across the workspace.

use std::path::Path;

use serde::Serialize;
use walkdir::WalkDir;

use crate::error::SidMcpError;

/// One hit for the `find_pub_item` search.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PubItemHit {
    /// Path relative to the workspace root, e.g., `"crates/sid-core/src/tab.rs"`.
    pub file: String,
    /// Line number (1-indexed) where the declaration was found.
    pub line: usize,
    /// One of `"fn"`, `"async fn"`, `"struct"`, `"trait"`, `"enum"`.
    pub kind: String,
    /// The matched declaration line (trimmed).
    pub declaration: String,
    /// Whether a `/// ` doc comment is present on the line above (heuristic).
    pub has_doc_comment: bool,
}

/// Implementation entry point.
pub async fn run(
    workspace_root: &Path,
    name: &str,
    crate_name: Option<&str>,
) -> Result<Vec<PubItemHit>, SidMcpError> {
    let search_root = match crate_name {
        Some(c) => {
            let dir = workspace_root.join("crates").join(c);
            if !dir.join("Cargo.toml").exists() {
                return Err(SidMcpError::UnknownCrate(c.to_string()));
            }
            dir
        }
        None => workspace_root.join("crates"),
    };

    let mut hits = Vec::new();
    let kinds: &[(&str, &str)] = &[
        ("pub async fn ", "async fn"),
        ("pub fn ", "fn"),
        ("pub struct ", "struct"),
        ("pub trait ", "trait"),
        ("pub enum ", "enum"),
    ];

    for entry in WalkDir::new(&search_root).into_iter().flatten() {
        if !entry.path().is_file() {
            continue;
        }
        if entry.path().extension().map(|e| e != "rs").unwrap_or(true) {
            continue;
        }
        if entry.path().components().any(|c| c.as_os_str() == "target") {
            continue;
        }

        let Ok(content) = tokio::fs::read_to_string(entry.path()).await else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            for (prefix, kind) in kinds {
                if let Some(rest) = trimmed.strip_prefix(prefix) {
                    if name_matches(rest, name) {
                        let rel = entry
                            .path()
                            .strip_prefix(workspace_root)
                            .unwrap_or(entry.path())
                            .to_string_lossy()
                            .to_string();
                        let has_doc_comment = idx
                            .checked_sub(1)
                            .and_then(|i| lines.get(i))
                            .map(|prev| prev.trim_start().starts_with("///"))
                            .unwrap_or(false);
                        hits.push(PubItemHit {
                            file: rel,
                            line: idx + 1,
                            kind: kind.to_string(),
                            declaration: trimmed.to_string(),
                            has_doc_comment,
                        });
                        break;
                    }
                }
            }
        }
    }

    Ok(hits)
}

/// Heuristic: does `rest` (the text after `pub fn `/`pub struct `/etc.)
/// declare an item with name `name`?
fn name_matches(rest: &str, name: &str) -> bool {
    // Skip whitespace, then match identifier characters.
    let trimmed = rest.trim_start();
    if !trimmed.starts_with(name) {
        return false;
    }
    let after = &trimmed[name.len()..];
    matches!(
        after.chars().next(),
        Some(c) if !(c.is_alphanumeric() || c == '_')
    ) || after.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matches_exact_token() {
        assert!(name_matches("TabManager {", "TabManager"));
        assert!(name_matches("TabManager<T>(", "TabManager"));
        assert!(name_matches("TabManager", "TabManager")); // end-of-line
        // boundary: prefix matches don't count.
        assert!(!name_matches("TabManagerExtra(", "TabManager"));
        // boundary: leading whitespace handled.
        assert!(name_matches("  TabManager(", "TabManager"));
    }

    #[tokio::test]
    async fn finds_pub_struct_in_synthetic_crate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cdir = root.join("crates/example");
        std::fs::create_dir_all(cdir.join("src")).unwrap();
        std::fs::write(cdir.join("Cargo.toml"), "[package]\nname=\"example\"\nversion=\"0.0.1\"\nedition=\"2024\"\n").unwrap();
        std::fs::write(
            cdir.join("src/lib.rs"),
            "/// A widget.\npub struct Widget {}\n\npub fn build_widget() {}\n",
        )
        .unwrap();

        let hits = run(root, "Widget", Some("example")).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "struct");
        assert!(hits[0].has_doc_comment);
    }

    #[tokio::test]
    async fn unknown_crate_scope_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run(tmp.path(), "Foo", Some("no-such")).await.unwrap_err();
        assert!(matches!(err, SidMcpError::UnknownCrate(_)));
    }

    #[tokio::test]
    async fn empty_when_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("crates")).unwrap();
        let hits = run(root, "Nonexistent", None).await.unwrap();
        assert!(hits.is_empty());
    }
}
