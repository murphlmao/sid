# sid-mcp

An MCP server (stdio) that makes AI agents effective at working on the sid codebase.
No GPUI dependency, so it builds in seconds. Built on `rmcp` 3.9.

## Registering

The repo-root `.mcp.json` registers it for Claude Code:

```json
{ "mcpServers": { "sid": { "command": "cargo", "args": ["run", "-q", "-p", "sid-mcp", "--"] } } }
```

Claude Code launches it from the repo root, which is how it finds the workspace
(otherwise set `SID_ROOT`). `cargo run` re-checks the build on every start; a prebuilt
`target/debug/sid-mcp` starts instantly, so for daily use point `command` at that path.

Why the committed config still uses `cargo run`: `.mcp.json` takes a single `command` with
no "binary if present, else build" fallback, and a relative binary path would silently break
on a fresh clone. The implication is that **opening the repo in Claude Code compiles and runs
workspace code (`sid-mcp` and its build scripts/dependencies) on server start**, so only
approve the project MCP server in a checkout you trust. Prefer a local override (for example
`claude mcp add` at user/local scope) pointing at your own built binary.

## Tools

| tool | what it does |
|---|---|
| `crate_map` | crates, one-line purpose, workspace dependency edges, GPUI allowance |
| `find_symbol` | pub items by name regex via `syn`: `path:line` + signature |
| `adapter_check` | CLAUDE.md rule-1 violations (mirrors `sid-ui/tests/hygiene.rs`) |
| `gate` | fmt / clippy / test with `nice -n 10`, `-j 3`; compact pass/fail summary |
| `capture_ui` | `scripts/sid-cap.sh` screenshot; returns the path and the PNG image |
| `backlog` | unchecked `- [ ]` items, HANDOFF "Open", open GitHub issues (via `gh`) |
| `docs_index` | docs with title, date, first paragraph |
| `recent_commits` | compact `git log`, optional path filter |
| `store_inspect` | read-only origin-tagged JSON dump of a sid store; never secret values |

Tool failures are returned as `isError` tool results, not protocol errors.

## Environment

- `SID_ROOT` - workspace root override (default: nearest ancestor of cwd with `crates/sid`).
- `SID_CARGO` - cargo executable used by `gate` (e.g. a throttling wrapper); default `cargo`.

## Notes

- `capture_ui` needs `sway`, `grim` (and `wtype` for `type` actions); a missing tool is
  reported by name. A cold build can exceed the default timeout: build `-p sid` first and
  pass `no_build`. Captures land in `target/caps/`.
- `store_inspect` never opens the redb file in place: it takes an advisory lock (fails
  clearly if a running sid holds it), copies the file, and reads the copy.
- `gate` can run for many minutes; scope with `package` while iterating.
