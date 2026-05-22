//! Dependency manifest for sid-mcp.
//!
//! Loads `crates/sid-mcp/tools.toml` and parses it into a typed
//! [`Manifest`] struct. The manifest is the source-of-truth for which
//! downstream skills/agents consume each MCP tool. When a tool's
//! `schema_version` bumps, every consumer listed here MUST be reviewed
//! in the same commit.
//!
//! The `tool_manifest` MCP tool exposes a `Manifest` JSON at runtime
//! so a session can query "what breaks if I change tool X?".

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use crate::error::SidMcpError;

/// Top-level manifest. Parsed from `crates/sid-mcp/tools.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Semver of the manifest schema itself. Bump when the *shape* of
    /// this struct changes, not when individual tools change.
    pub manifest_version: String,
    /// Per-tool metadata, keyed by tool name (e.g., `"crate_info"`).
    #[serde(default)]
    pub tools: BTreeMap<String, ToolEntry>,
    /// Per-consumer (skill/agent) usage, keyed by namespaced consumer
    /// name (e.g., `"sid-testing:sid-gate"`).
    #[serde(default)]
    pub consumers: BTreeMap<String, ConsumerEntry>,
}

/// Manifest entry for one MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolEntry {
    /// Short human-readable description (mirrors the `#[tool(description = ...)]` attribute).
    pub description: String,
    /// Tool's input/output schema version. Bump on breaking changes;
    /// every consumer in `[consumers]` calling this tool needs review.
    pub schema_version: u32,
    /// File globs whose meaningful changes invalidate this tool's
    /// output. Used for blast-radius analysis ("if I change this file,
    /// which tools may misbehave?").
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Manifest entry for one downstream consumer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsumerEntry {
    /// Names of the tools this consumer calls.
    pub tools: Vec<String>,
    /// One-line explanation of why this consumer uses these tools.
    pub purpose: String,
}

impl Manifest {
    /// Return the tools (by name) that a given consumer uses.
    pub fn tools_for_consumer(&self, consumer: &str) -> Vec<&str> {
        self.consumers
            .get(consumer)
            .map(|e| e.tools.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Return the consumers (by name) that use a given tool.
    pub fn consumers_of_tool(&self, tool: &str) -> Vec<&str> {
        self.consumers
            .iter()
            .filter_map(|(name, entry)| {
                if entry.tools.iter().any(|t| t == tool) {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Validate internal consistency. Returns the list of issues found
    /// (e.g., consumer references a tool that doesn't exist).
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let tool_names: std::collections::BTreeSet<&str> =
            self.tools.keys().map(String::as_str).collect();
        for (consumer, entry) in &self.consumers {
            for tool in &entry.tools {
                if !tool_names.contains(tool.as_str()) {
                    issues.push(format!(
                        "consumer `{consumer}` references unknown tool `{tool}`"
                    ));
                }
            }
        }
        issues
    }
}

/// Load the manifest from `<workspace_root>/crates/sid-mcp/tools.toml`.
///
/// # Errors
///
/// Returns [`SidMcpError::MissingPath`] if the file isn't found, or
/// [`SidMcpError::Toml`] if it fails to parse.
pub fn load(workspace_root: &Path) -> Result<Manifest, SidMcpError> {
    let path = workspace_root.join("crates/sid-mcp/tools.toml");
    if !path.exists() {
        return Err(SidMcpError::MissingPath(path));
    }
    let bytes = std::fs::read_to_string(&path)?;
    let manifest: Manifest = toml::from_str(&bytes)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_toml() -> &'static str {
        r#"
manifest_version = "1.0.0"

[tools.crate_info]
description = "Crate info."
schema_version = 1
depends_on = ["crates/*/Cargo.toml"]

[tools.find_pub_item]
description = "Find pub item."
schema_version = 1

[consumers."sid-testing:sid-gate"]
tools = ["crate_info"]
purpose = "Gate uses crate info."

[consumers."sid-testing:bad"]
tools = ["nonexistent_tool"]
purpose = "Tests validation."
"#
    }

    #[test]
    fn parses_minimal_manifest() {
        let m: Manifest = toml::from_str(sample_toml()).unwrap();
        assert_eq!(m.manifest_version, "1.0.0");
        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.consumers.len(), 2);
        let crate_info = &m.tools["crate_info"];
        assert_eq!(crate_info.schema_version, 1);
        assert_eq!(crate_info.depends_on, vec!["crates/*/Cargo.toml"]);
    }

    #[test]
    fn tools_for_consumer_returns_expected() {
        let m: Manifest = toml::from_str(sample_toml()).unwrap();
        let tools = m.tools_for_consumer("sid-testing:sid-gate");
        assert_eq!(tools, vec!["crate_info"]);
        // boundary: unknown consumer returns empty, doesn't panic.
        assert!(m.tools_for_consumer("nonexistent").is_empty());
    }

    #[test]
    fn consumers_of_tool_reverse_lookup() {
        let m: Manifest = toml::from_str(sample_toml()).unwrap();
        let consumers = m.consumers_of_tool("crate_info");
        assert_eq!(consumers, vec!["sid-testing:sid-gate"]);
        // boundary: tool with no consumers returns empty.
        assert!(m.consumers_of_tool("find_pub_item").is_empty());
    }

    #[test]
    fn validate_catches_dangling_tool_reference() {
        let m: Manifest = toml::from_str(sample_toml()).unwrap();
        let issues = m.validate();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("sid-testing:bad"));
        assert!(issues[0].contains("nonexistent_tool"));
    }

    #[test]
    fn validate_returns_empty_on_clean_manifest() {
        let toml = r#"
manifest_version = "1.0.0"
[tools.t1]
description = "T1."
schema_version = 1
[consumers.c1]
tools = ["t1"]
purpose = "."
"#;
        let m: Manifest = toml::from_str(toml).unwrap();
        assert!(m.validate().is_empty());
    }

    #[test]
    fn load_missing_returns_missing_path() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(tmp.path()).unwrap_err();
        assert!(matches!(err, SidMcpError::MissingPath(_)));
    }

    #[test]
    fn load_real_manifest_in_repo_parses_cleanly() {
        // Skip if not run from the workspace root (e.g., docs tests in
        // a built crate). The CARGO_MANIFEST_DIR points at this crate.
        let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = crate_dir.parent().unwrap().parent().unwrap().to_path_buf();
        let m = load(&workspace_root).expect("real manifest should parse");
        let issues = m.validate();
        assert!(
            issues.is_empty(),
            "real manifest has dangling references: {issues:?}"
        );
        // All 9 tools should be present.
        assert!(m.tools.contains_key("tool_manifest"));
        assert!(m.tools.contains_key("crate_info"));
        assert!(m.tools.contains_key("find_pub_item"));
    }
}
