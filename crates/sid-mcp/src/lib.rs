//! `sid-mcp` — Model Context Protocol server exposing sid codebase
//! introspection as typed tools to AI clients (Claude Code, etc.).
//!
//! The server runs as `sid mcp` (a subcommand of the main binary) over
//! stdio. It speaks JSON-RPC 2.0 per the MCP spec and exposes the tool
//! surface described in [`manifest`].
//!
//! ## Design
//!
//! Tools are **data accessors**, not workflow orchestrators. They return
//! structured JSON about the codebase: crate info, public items, coverage
//! summaries, gate status, plan progress, criterion baselines. Workflows
//! that compose these tools live in skills (`sid-testing:sid-gate`,
//! `sid-testing:coverage-report`, `sid-testing:perf-check`) or agents
//! (`sid-testing:sid-store-reviewer`, `sid-testing:widget-render-reviewer`).
//!
//! ## Dependency manifest
//!
//! [`manifest::Manifest`] declares for every tool: its description, the
//! files/paths it depends on, its schema version, and the downstream
//! consumers (skills/agents) that call it. The `tool_manifest` MCP tool
//! exposes this manifest at runtime so a session can query "what breaks
//! if I change tool X?". Per the maintenance contract in `CLAUDE.md`,
//! when a tool's schema version changes, every consumer in this manifest
//! must be reviewed in the same commit.

#![warn(missing_docs)]

pub mod error;
pub mod manifest;
pub mod tools;

use std::path::PathBuf;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde::Deserialize;

pub use crate::error::SidMcpError;

/// Root MCP server for sid codebase introspection.
#[derive(Clone, Debug)]
pub struct SidMcp {
    /// Absolute path to the sid workspace root (the directory containing
    /// the workspace `Cargo.toml`).
    pub workspace_root: PathBuf,
    // Held so the `#[tool_handler]` macro can route incoming
    // `tools/call` requests through the generated dispatch table.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

/// Parameters for the `crate_info` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateInfoParams {
    /// Name of the crate (e.g., `"sid-core"`). Must be a member of the
    /// sid workspace.
    pub name: String,
}

/// Parameters for the `find_pub_item` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindPubItemParams {
    /// Name of the public item (function, struct, trait, enum) to locate.
    pub name: String,
    /// Optional crate to scope the search. If absent, scans every crate.
    #[serde(default)]
    pub crate_name: Option<String>,
}

/// Parameters for the `pub_items_without_doc_tests` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PubItemsWithoutDocTestsParams {
    /// Optional crate to scope the search. If absent, scans every crate.
    #[serde(default)]
    pub crate_name: Option<String>,
}

/// Parameters for the `coverage_summary` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CoverageSummaryParams {
    /// Optional crate to scope coverage. If absent, returns workspace-
    /// wide summary.
    #[serde(default)]
    pub crate_name: Option<String>,
    /// If true, force a fresh `cargo llvm-cov` run. Default false reads
    /// the cached `target/llvm-cov/sid-mcp-cache.json` if present.
    #[serde(default)]
    pub fresh: bool,
}

/// Parameters for the `gate_status` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GateStatusParams {
    /// Optional crate scope. If absent, returns last workspace-wide
    /// gate result.
    #[serde(default)]
    pub crate_name: Option<String>,
}

/// Parameters for the `recent_commits` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecentCommitsParams {
    /// Optional crate to scope commits to. If absent, returns workspace-
    /// wide commits.
    #[serde(default)]
    pub crate_name: Option<String>,
    /// Number of commits to return. Defaults to 10.
    #[serde(default = "default_commit_count")]
    pub count: u32,
}

fn default_commit_count() -> u32 {
    10
}

/// Parameters for the `criterion_compare` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CriterionCompareParams {
    /// Optional crate to scope. If absent, checks every crate with a
    /// `benches/` directory.
    #[serde(default)]
    pub crate_name: Option<String>,
    /// Regression threshold as a percentage. Defaults to 10% per
    /// CLAUDE.md.
    #[serde(default = "default_regression_threshold")]
    pub threshold_pct: f32,
}

fn default_regression_threshold() -> f32 {
    10.0
}

/// Empty params for tools that take no arguments.
#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct NoParams {}

/// Render a tool result. Tools return JSON-serializable structs; we
/// stringify them so the MCP layer ships them as text content. The MCP
/// client (Claude Code) sees structured JSON and parses it.
fn to_json_string<T: serde::Serialize>(value: &T) -> Result<String, McpError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))
}

#[tool_router]
impl SidMcp {
    /// Construct a new server rooted at `workspace_root`.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            tool_router: Self::tool_router(),
        }
    }

    /// Return a structured manifest of every tool: description, file
    /// dependencies, schema version, and downstream consumers.
    #[tool(
        description = "Return the dependency manifest: for each tool, list its description, the files it depends on, its schema_version, and the downstream skills/agents that consume it. Use this to assess blast radius before changing tool semantics."
    )]
    async fn tool_manifest(&self, _: Parameters<NoParams>) -> Result<String, McpError> {
        let m = manifest::load(&self.workspace_root)
            .map_err(|e| McpError::internal_error(format!("manifest: {e}"), None))?;
        to_json_string(&m)
    }

    /// Return Cargo metadata, LOC counts, test counts, and the public
    /// item list for a single workspace crate.
    #[tool(
        description = "Return Cargo metadata, line-of-code count, test count, and public-item list for a single workspace crate. Use when you need a quick crate-level overview before diving into a specific file."
    )]
    async fn crate_info(
        &self,
        Parameters(p): Parameters<CrateInfoParams>,
    ) -> Result<String, McpError> {
        let info = tools::crate_info::run(&self.workspace_root, &p.name)
            .await
            .map_err(|e| McpError::internal_error(format!("crate_info: {e}"), None))?;
        to_json_string(&info)
    }

    /// Locate a public item by name across the workspace.
    #[tool(
        description = "Locate a public item (function, struct, trait, enum) by name. Returns file:line, item kind, whether a doc test exists, and grep-discovered callers across the workspace. Use to navigate the 14-crate spread."
    )]
    async fn find_pub_item(
        &self,
        Parameters(p): Parameters<FindPubItemParams>,
    ) -> Result<String, McpError> {
        let hits = tools::find_pub_item::run(&self.workspace_root, &p.name, p.crate_name.as_deref())
            .await
            .map_err(|e| McpError::internal_error(format!("find_pub_item: {e}"), None))?;
        to_json_string(&hits)
    }

    /// List public items that lack a doc test.
    #[tool(
        description = "List every public item (pub fn / struct / trait / enum) that lacks a doc test. CLAUDE.md requires doc tests on all public items; this tool surfaces gaps in that contract."
    )]
    async fn pub_items_without_doc_tests(
        &self,
        Parameters(p): Parameters<PubItemsWithoutDocTestsParams>,
    ) -> Result<String, McpError> {
        let gaps = tools::doc_test_gaps::run(&self.workspace_root, p.crate_name.as_deref())
            .await
            .map_err(|e| McpError::internal_error(format!("doc_test_gaps: {e}"), None))?;
        to_json_string(&gaps)
    }

    /// Return llvm-cov coverage summary.
    #[tool(
        description = "Return cargo-llvm-cov coverage percentages per-crate or workspace-wide. Flags critical-path crates (sid-store, sid-core, sid-job, sid-secrets) below the 95% bar. May trigger a fresh llvm-cov run (slow, 30s-2min) if no cached result exists or `fresh: true` is passed."
    )]
    async fn coverage_summary(
        &self,
        Parameters(p): Parameters<CoverageSummaryParams>,
    ) -> Result<String, McpError> {
        let cov = tools::coverage::run(&self.workspace_root, p.crate_name.as_deref(), p.fresh)
            .await
            .map_err(|e| McpError::internal_error(format!("coverage: {e}"), None))?;
        to_json_string(&cov)
    }

    /// Return the last cached gate status from `target/gate-logs/`.
    #[tool(
        description = "Return the last cached gate status (test/clippy/fmt/deny) from target/gate-logs/. Does NOT run the gate; that is the /sid-gate skill's job. Returns null fields if the gate has never run for the requested scope."
    )]
    async fn gate_status(
        &self,
        Parameters(p): Parameters<GateStatusParams>,
    ) -> Result<String, McpError> {
        let s = tools::gate_status::run(&self.workspace_root, p.crate_name.as_deref())
            .await
            .map_err(|e| McpError::internal_error(format!("gate_status: {e}"), None))?;
        to_json_string(&s)
    }

    /// Return per-plan task completion.
    #[tool(
        description = "Parse docs/superpowers/plans/*.md and return per-plan task completion (counts of done / pending / total tasks per checkbox list). Use to know which plan tasks remain before claiming a plan complete."
    )]
    async fn plan_status(&self, _: Parameters<NoParams>) -> Result<String, McpError> {
        let p = tools::plan_status::run(&self.workspace_root)
            .await
            .map_err(|e| McpError::internal_error(format!("plan_status: {e}"), None))?;
        to_json_string(&p)
    }

    /// Return recent git commits.
    #[tool(
        description = "Return recent git commits. If `crate_name` is set, scope to commits touching that crate's directory. Useful for review context — what's changed lately in the area I'm reviewing."
    )]
    async fn recent_commits(
        &self,
        Parameters(p): Parameters<RecentCommitsParams>,
    ) -> Result<String, McpError> {
        let c = tools::recent_commits::run(&self.workspace_root, p.crate_name.as_deref(), p.count)
            .await
            .map_err(|e| McpError::internal_error(format!("recent_commits: {e}"), None))?;
        to_json_string(&c)
    }

    /// Compare criterion benchmark results vs baseline.
    #[tool(
        description = "Compare criterion benchmark results vs the saved baseline under target/criterion/<bench>/base/estimates.json. Flag any regression over `threshold_pct` (default 10% per CLAUDE.md). Returns per-bench mean-time delta with regression flag."
    )]
    async fn criterion_compare(
        &self,
        Parameters(p): Parameters<CriterionCompareParams>,
    ) -> Result<String, McpError> {
        let c = tools::criterion::run(&self.workspace_root, p.crate_name.as_deref(), p.threshold_pct)
            .await
            .map_err(|e| McpError::internal_error(format!("criterion_compare: {e}"), None))?;
        to_json_string(&c)
    }
}

#[tool_handler]
impl ServerHandler for SidMcp {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo and Implementation are #[non_exhaustive], so build
        // from Default and mutate fields rather than using a struct
        // literal.
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info.name = "sid-mcp".to_string();
        info.server_info.version = env!("CARGO_PKG_VERSION").to_string();
        info.instructions = Some(
            "MCP server for the sid TUI cockpit. Exposes structured \
             codebase introspection (crate info, public items, coverage, \
             gate status, plan progress, recent commits, criterion \
             baselines, dependency manifest). All tools return JSON; \
             long-running operations like coverage_summary may take \
             30s-2min on first call (cached thereafter). The \
             tool_manifest tool returns the dependency map showing \
             which downstream skills/agents consume each tool."
                .to_string(),
        );
        info
    }
}

/// Convenience entry point for `sid mcp` subcommand: serve over stdio
/// until the client disconnects.
pub async fn run_stdio(workspace_root: PathBuf) -> Result<(), SidMcpError> {
    use rmcp::{transport::stdio, ServiceExt};
    let server = SidMcp::new(workspace_root);
    let service = server.serve(stdio()).await.map_err(SidMcpError::from_init)?;
    service.waiting().await.map_err(SidMcpError::from_run)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_constructs() {
        let s = SidMcp::new(PathBuf::from("/tmp/nonexistent"));
        assert_eq!(s.workspace_root, PathBuf::from("/tmp/nonexistent"));
    }

    #[test]
    fn get_info_advertises_tools() {
        let s = SidMcp::new(PathBuf::from("/tmp"));
        let info = s.get_info();
        assert_eq!(info.server_info.name, "sid-mcp");
        assert!(info.capabilities.tools.is_some());
    }
}
