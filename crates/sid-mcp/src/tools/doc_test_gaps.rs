//! `pub_items_without_doc_tests` tool — list pub items lacking a doc test.

use std::path::Path;

use serde::Serialize;
use walkdir::WalkDir;

use crate::error::SidMcpError;

/// One public item missing a doc test.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocTestGap {
    /// Workspace-relative file path.
    pub file: String,
    /// Line number (1-indexed) of the declaration.
    pub line: usize,
    /// Item kind: `"fn"`, `"struct"`, `"trait"`, `"enum"`.
    pub kind: String,
    /// Identifier name (e.g., `"TabManager"`).
    pub name: String,
}

/// Implementation entry point.
pub async fn run(
    workspace_root: &Path,
    crate_name: Option<&str>,
) -> Result<Vec<DocTestGap>, SidMcpError> {
    let root = match crate_name {
        Some(c) => {
            let dir = workspace_root.join("crates").join(c);
            if !dir.join("Cargo.toml").exists() {
                return Err(SidMcpError::UnknownCrate(c.to_string()));
            }
            dir
        }
        None => workspace_root.join("crates"),
    };

    let mut gaps = Vec::new();
    let kinds: &[(&str, &str)] = &[
        ("pub async fn ", "fn"),
        ("pub fn ", "fn"),
        ("pub struct ", "struct"),
        ("pub trait ", "trait"),
        ("pub enum ", "enum"),
    ];

    for entry in WalkDir::new(&root).into_iter().flatten() {
        if !entry.path().is_file() {
            continue;
        }
        if entry.path().extension().map(|e| e != "rs").unwrap_or(true) {
            continue;
        }
        // Skip tests and benches dirs — we only care about pub items in src/.
        if entry
            .path()
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some("tests") | Some("benches") | Some("target")))
        {
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
                    // Walk back to find the doc-block above. A doc test is
                    // a fenced code block (```) inside `///` comments.
                    if has_doc_test_above(&lines, idx) {
                        continue;
                    }
                    if let Some(name) = extract_ident(rest) {
                        let rel = entry
                            .path()
                            .strip_prefix(workspace_root)
                            .unwrap_or(entry.path())
                            .to_string_lossy()
                            .to_string();
                        gaps.push(DocTestGap {
                            file: rel,
                            line: idx + 1,
                            kind: kind.to_string(),
                            name,
                        });
                    }
                }
            }
        }
    }

    Ok(gaps)
}

/// True iff lines `[0..decl_line)` contain a doc-comment block that includes
/// a fenced code block (heuristic for a doc test).
fn has_doc_test_above(lines: &[&str], decl_line: usize) -> bool {
    let mut i = decl_line;
    let mut saw_doc = false;
    let mut saw_fence = false;
    while i > 0 {
        i -= 1;
        let t = lines[i].trim_start();
        if t.starts_with("///") || t.starts_with("//!") {
            saw_doc = true;
            // The fence inside a doc comment is "/// ```" or "/// ```rust".
            if t.trim_start_matches('/').trim_start().starts_with("```") {
                saw_fence = true;
            }
            continue;
        }
        if t.starts_with("#[") || t.is_empty() {
            // Attribute lines or blank lines between doc and decl are fine.
            if saw_doc {
                continue;
            }
        }
        break;
    }
    saw_doc && saw_fence
}

/// Extract the identifier name from text right after `pub fn `/etc.
fn extract_ident(rest: &str) -> Option<String> {
    let trimmed = rest.trim_start();
    let end = trimmed
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(trimmed.len());
    if end == 0 {
        None
    } else {
        Some(trimmed[..end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_ident_finds_name() {
        assert_eq!(extract_ident("TabManager {"), Some("TabManager".into()));
        assert_eq!(extract_ident("foo()"), Some("foo".into()));
        assert_eq!(extract_ident("  bar<T>()"), Some("bar".into()));
        // boundary: empty/none.
        assert_eq!(extract_ident(""), None);
        assert_eq!(extract_ident("()"), None);
    }

    #[test]
    fn has_doc_test_above_detects_fenced_block() {
        let lines = [
            "/// Returns a thing.",
            "///",
            "/// ```",
            "/// let x = thing();",
            "/// ```",
            "pub fn thing() {}",
        ];
        assert!(has_doc_test_above(&lines, 5));
    }

    #[test]
    fn has_doc_test_above_rejects_doc_without_fence() {
        // Adversarial: a doc comment without a code block isn't a doc test.
        let lines = ["/// Returns a thing.", "pub fn thing() {}"];
        assert!(!has_doc_test_above(&lines, 1));
    }

    #[test]
    fn has_doc_test_above_rejects_no_doc() {
        let lines = ["pub fn thing() {}"];
        assert!(!has_doc_test_above(&lines, 0));
    }

    #[tokio::test]
    async fn finds_gap_for_undocumented_pub_fn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cdir = root.join("crates/example");
        std::fs::create_dir_all(cdir.join("src")).unwrap();
        std::fs::write(cdir.join("Cargo.toml"), "[package]\nname=\"example\"\nversion=\"0.0.1\"\nedition=\"2024\"\n").unwrap();
        std::fs::write(
            cdir.join("src/lib.rs"),
            "/// A documented thing.\n/// ```\n/// let x = 1;\n/// ```\npub fn documented() {}\n\npub fn undocumented() {}\n",
        ).unwrap();

        let gaps = run(root, Some("example")).await.unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].name, "undocumented");
    }
}
