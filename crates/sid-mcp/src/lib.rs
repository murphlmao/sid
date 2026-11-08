//! `sid-mcp` — an MCP server (stdio) that makes AI agents effective at working ON the
//! sid codebase: crate map, symbol search, adapter-rule check, the gate, UI capture,
//! backlog, docs index, recent commits and a read-only store dump.
//!
//! No GPUI dependency, so it builds in seconds. See `README.md` and `CLAUDE.md`.

pub mod tools;
pub mod util;

use std::path::PathBuf;

use base64::Engine;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use tools::capture_ui::{Action, Request};
use tools::gate::Step;

/// The MCP server, rooted at the sid workspace.
#[derive(Clone)]
pub struct SidMcp {
    root: PathBuf,
    tool_router: ToolRouter<Self>,
}

/// No arguments.
#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct NoArgs {}

/// Arguments for `find_symbol`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindSymbolArgs {
    /// Regex matched against item names (use `^Name$` for exact). Methods match on the
    /// bare name or `Type::name`; trait impls match on the trait or implementing type.
    pub query: String,
    /// Restrict to one crate directory name, e.g. `sid-store`.
    pub krate: Option<String>,
    /// Restrict to kinds: fn, method, struct, enum, trait, type, const, static, union,
    /// mod, impl-trait.
    pub kinds: Option<Vec<String>>,
    /// Include non-`pub` items (default false).
    pub include_private: Option<bool>,
    /// Max hits to show (default 50).
    pub limit: Option<usize>,
}

/// Arguments for `gate`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GateArgs {
    /// Cargo package (e.g. `sid-ui`); omit for the whole workspace.
    pub package: Option<String>,
    /// Subset of `fmt`, `clippy`, `test` (default all three, in that order).
    pub steps: Option<Vec<String>>,
    /// Overall time budget in seconds (default 1800, max 3600).
    pub timeout_secs: Option<u64>,
}

/// One scripted input action for `capture_ui`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ActionArg {
    /// click | dclick | rclick | key | type | scroll | drag | sleep
    pub kind: String,
    /// `X,Y` for click/dclick/rclick; `X,Y,N` for scroll (negative N scrolls up);
    /// `X1,Y1,X2,Y2[,STEPS]` for drag (must be last); key chord (`ctrl+k`) for key;
    /// literal text for type; seconds for sleep.
    pub value: String,
}

/// Arguments for `capture_ui`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CaptureArgs {
    /// ssh | database | network | workspaces | system | settings (default ssh).
    pub tab: Option<String>,
    /// cosmos | void | dusk | cosmos-light.
    pub theme: Option<String>,
    /// Virtual output size, e.g. `1920x1080` (default).
    pub size: Option<String>,
    /// UI zoom (`SID_UI_SCALE`), e.g. `1.25`.
    pub scale: Option<String>,
    /// Ordered input actions applied before the screenshot.
    pub actions: Option<Vec<ActionArg>>,
    /// Skip the `cargo build -p sid` (faster, but may photograph a stale binary).
    pub no_build: Option<bool>,
    /// Extra settle seconds before the screenshot (default 3, max 60).
    pub wait: Option<u32>,
    /// Hard timeout in seconds (default 600; a cold build is slow).
    pub timeout_secs: Option<u64>,
}

/// Arguments for `backlog`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BacklogArgs {
    /// Also query open GitHub issues via `gh` (default true; skipped gracefully).
    pub issues: Option<bool>,
}

/// Arguments for `docs_index`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocsArgs {
    /// Case-insensitive substring filter over path/title/summary.
    pub filter: Option<String>,
}

/// Arguments for `recent_commits`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommitsArgs {
    /// Number of commits (default 15, max 200).
    pub n: Option<usize>,
    /// Only commits touching this path.
    pub path: Option<String>,
}

/// Arguments for `store_inspect`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StoreArgs {
    /// XDG data dir holding `sid/store.redb` (or `store.redb` directly). Default:
    /// `$XDG_DATA_HOME` or `~/.local/share`.
    pub data_dir: Option<String>,
    /// A workspace root whose `.sid/config.toml` is layered in, origin-tagged.
    pub workspace_root: Option<String>,
}

fn text(s: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(s)]))
}

fn failure(s: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(s)]))
}

fn reply(r: Result<String, String>) -> Result<CallToolResult, McpError> {
    match r {
        Ok(s) => text(s),
        Err(e) => failure(e),
    }
}

#[tool_router]
impl SidMcp {
    /// Construct a server rooted at the sid workspace.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List the workspace crates: one-line purpose, intra-workspace dependency edges, and whether each may name GPUI (only sid and sid-ui). Start here to orient."
    )]
    async fn crate_map(&self, _: Parameters<NoArgs>) -> Result<CallToolResult, McpError> {
        reply(tools::crate_map::collect(&self.root).map(|c| tools::crate_map::render(&c)))
    }

    #[tool(
        description = "Find pub items (fn/method/struct/enum/trait/type/const/impl-of-trait) by name regex across crates/, via syn. Returns path:line plus the signature line. Does not see macro-generated items."
    )]
    async fn find_symbol(
        &self,
        Parameters(a): Parameters<FindSymbolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let re = match regex::Regex::new(&a.query) {
            Ok(r) => r,
            Err(e) => return failure(format!("bad regex: {e}")),
        };
        let kinds = a.kinds.unwrap_or_default();
        let include_private = a.include_private.unwrap_or(false);
        let root = self.root.clone();
        let krate = a.krate.clone();
        let limit = a.limit.unwrap_or(50).clamp(1, 500);
        // syn parsing of hundreds of files is CPU work; keep it off the reactor.
        let res = tokio::task::spawn_blocking(move || {
            let q = tools::find_symbol::Query {
                re: &re,
                kinds: &kinds,
                include_private,
            };
            tools::find_symbol::search_workspace(&root, krate.as_deref(), &q, limit)
                .map(|f| tools::find_symbol::render(&f))
        })
        .await;
        match res {
            Ok(r) => reply(r),
            Err(e) => failure(format!("search task failed: {e}")),
        }
    }

    #[tool(
        description = "Report CLAUDE.md rule-1 (adapter pattern) violations: GPUI-family crates named outside sid/sid-ui, concrete crates (redb, russh, git2, ...) named in sid-core, and gpui_component:: named directly in the app crate. Mirrors crates/sid-ui/tests/hygiene.rs."
    )]
    async fn adapter_check(&self, _: Parameters<NoArgs>) -> Result<CallToolResult, McpError> {
        let root = self.root.clone();
        match tokio::task::spawn_blocking(move || tools::adapter_check::check(&root)).await {
            Ok(r) => reply(r.map(|v| tools::adapter_check::render(&v))),
            Err(e) => failure(format!("check task failed: {e}")),
        }
    }

    #[tool(
        description = "Run the gate (cargo fmt --check, clippy -D warnings, test) with nice -n 10 and -j 3. Returns per-step pass/fail, suite/passed/failed counts and the first ~20 error lines. Can take many minutes; scope with `package` while iterating."
    )]
    async fn gate(&self, Parameters(a): Parameters<GateArgs>) -> Result<CallToolResult, McpError> {
        if let Some(p) = &a.package
            && !tools::gate::valid_package(p)
        {
            return failure(format!("invalid package name `{p}`"));
        }
        let mut steps = Vec::new();
        match &a.steps {
            Some(list) => {
                for s in list {
                    match Step::parse(s) {
                        Some(st) => steps.push(st),
                        None => return failure(format!("unknown step `{s}` (fmt|clippy|test)")),
                    }
                }
            }
            None => steps = vec![Step::Fmt, Step::Clippy, Step::Test],
        }
        let t = a.timeout_secs.unwrap_or(1800).clamp(10, 3600);
        match tools::gate::run(&self.root, a.package.as_deref(), &steps, t).await {
            Ok(rs) => {
                let s = tools::gate::render(&rs, a.package.as_deref());
                if rs.iter().all(|r| r.ok) {
                    text(s)
                } else {
                    failure(s)
                }
            }
            Err(e) => failure(e),
        }
    }

    #[tool(
        description = "Screenshot the sid GUI in a hermetic headless sway (scripts/sid-cap.sh) so you can SEE the UI. Pick tab/theme/size/scale and script clicks, keys, typing, scroll, drag. Returns the PNG path and the image. Needs sway+grim (+wtype for typing)."
    )]
    async fn capture_ui(
        &self,
        Parameters(a): Parameters<CaptureArgs>,
    ) -> Result<CallToolResult, McpError> {
        let req = Request {
            tab: a.tab,
            theme: a.theme,
            size: a.size,
            scale: a.scale,
            actions: a
                .actions
                .unwrap_or_default()
                .into_iter()
                .map(|x| Action {
                    kind: x.kind,
                    value: x.value,
                })
                .collect(),
            no_build: a.no_build.unwrap_or(false),
            wait: a.wait,
        };
        let t = a.timeout_secs.unwrap_or(600).clamp(30, 1800);
        match tools::capture_ui::run(&self.root, &req, t).await {
            Err(e) => failure(e),
            Ok(c) => {
                let bytes = match tokio::fs::read(&c.path).await {
                    Ok(b) => b,
                    Err(e) => return failure(format!("read {}: {e}", c.path.display())),
                };
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let mut msg = format!("captured {} ({} KiB)", c.path.display(), bytes.len() / 1024);
                if c.log.contains("WARN") || c.log.to_lowercase().contains("stale") {
                    msg.push_str(&format!("\nharness notes:\n{}", c.log));
                }
                Ok(CallToolResult::success(vec![
                    ContentBlock::text(msg),
                    ContentBlock::image(b64, "image/png"),
                ]))
            }
        }
    }

    #[tool(
        description = "Open work: unchecked `- [ ]` items (file:line + nearest heading) from docs/design/*.md, the Open section of docs/HANDOFF.md, and open GitHub issues via gh (skipped gracefully if gh is missing)."
    )]
    async fn backlog(
        &self,
        Parameters(a): Parameters<BacklogArgs>,
    ) -> Result<CallToolResult, McpError> {
        reply(tools::backlog::run(&self.root, a.issues.unwrap_or(true)).await)
    }

    #[tool(
        description = "Index the docs (docs/design, docs/superpowers/plans, HANDOFF, CLAUDE.md, ARCHITECTURE if present): path, title, date, first paragraph. Optional substring filter."
    )]
    async fn docs_index(
        &self,
        Parameters(a): Parameters<DocsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let root = self.root.clone();
        match tokio::task::spawn_blocking(move || tools::docs_index::collect(&root)).await {
            Ok(d) => text(tools::docs_index::render(&d, a.filter.as_deref())),
            Err(e) => failure(format!("index task failed: {e}")),
        }
    }

    #[tool(
        description = "Compact git log (hash, date, author, subject). Optional count and path filter."
    )]
    async fn recent_commits(
        &self,
        Parameters(a): Parameters<CommitsArgs>,
    ) -> Result<CallToolResult, McpError> {
        reply(tools::recent_commits::run(&self.root, a.n.unwrap_or(15), a.path.as_deref()).await)
    }

    #[tool(
        description = "Read-only JSON dump of a sid store: hosts, DB connections, workspaces, settings. Global layer from an XDG data dir plus an optional workspace root's .sid/config.toml, every item origin-tagged. Never outputs secret values (only secret_ref). Errors clearly if a running sid holds the redb lock."
    )]
    async fn store_inspect(
        &self,
        Parameters(a): Parameters<StoreArgs>,
    ) -> Result<CallToolResult, McpError> {
        let data = a
            .data_dir
            .map(PathBuf::from)
            .unwrap_or_else(tools::store_inspect::default_data_dir);
        let ws = a.workspace_root.map(PathBuf::from);
        let res = tokio::task::spawn_blocking(move || {
            tools::store_inspect::inspect(&data, ws.as_deref())
                .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
        })
        .await;
        match res {
            Ok(r) => reply(r),
            Err(e) => failure(format!("inspect task failed: {e}")),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SidMcp {
    fn get_info(&self) -> ServerConfig {
        let mut cfg = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Tools for working on the sid codebase (GPUI ops-cockpit). Start with crate_map, \
                 find_symbol and docs_index; use adapter_check and gate before finishing; \
                 capture_ui to look at the running UI.",
            );
        cfg.server_info = Implementation::new("sid-mcp", env!("CARGO_PKG_VERSION"));
        cfg
    }
}
