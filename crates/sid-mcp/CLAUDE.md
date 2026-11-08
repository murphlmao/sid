# CLAUDE.md - sid-mcp

MCP server (stdio, `rmcp` 3.9) exposing tools for working ON the sid codebase. Binary
`sid-mcp`; lib holds the server. **No gpui, no concrete adapter crates** beyond
`sid-store` (used read-only by `store_inspect`) - keep it building in seconds.

## Layout
- `src/lib.rs` - `SidMcp`, argument structs (`JsonSchema`), `#[tool]` methods (thin).
- `src/tools/<name>.rs` - one module per tool: pure, unit-tested logic + a small `run`.
- `src/util.rs` - root discovery, `run()` with timeout (kills child), `clip`.
- `tests/stdio.rs` - spawns the real binary: `initialize`, `tools/list`, `tools/call`.

## Tools
crate_map, find_symbol, adapter_check, gate, capture_ui, backlog, docs_index,
recent_commits, store_inspect. Details in `README.md`.

## Adding a tool
1. New `src/tools/foo.rs` (register in `tools/mod.rs`); put logic in pure functions that
   take a root `&Path` or plain strings, so tests can use a `tempfile` fixture.
2. Add an args struct (`Deserialize + JsonSchema`, doc-comment every field - they become
   the schema) and a `#[tool(description = ...)]` method in `lib.rs`. Return
   `reply(Result<String, String>)`; errors are `isError` results, never panics.
3. Compact text output (agents pay per token); clip long lines.
4. Add the name to `EXPECTED_TOOLS` in `tests/stdio.rs`; update README table.

## Rules
- Subprocesses go through `util::run` (timeout + kill_on_drop). Never `cargo` directly in
  tools except `gate`, which honours `SID_CARGO`.
- Never print secret values (`store_inspect` shows `secret_ref` only).
- Keep `adapter_check` consistent with `crates/sid-ui/tests/hygiene.rs`.

## Tests
`cargo test -p sid-mcp` (unit + stdio integration; sway/gh/gate are not exercised live).
