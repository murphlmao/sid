# CLAUDE.md — sid (GPUI rebuild)

The lean, load-bearing ruleset for the rebuilt `sid`. The TUI proof-of-concept is archived
at `murphlmao/sid-poc` (locally `~/vcs/sid-poc`) — crib adapters and view logic from it,
but do not carry its scaffolding over wholesale. Design source of truth: `docs/design/`.

## What sid is

An integrated developer **ops-cockpit** (SSH/SFTP, Database, Network, Workspaces, System,
Settings), a native **GPUI** desktop app, run locally to reach out at everything else.
Built in **vertical slices**: one tab is taken to daily-use quality before the next.
The **SSH/SFTP** tab is the spearhead.

## Start here (read in this order, stop when you have enough)

1. `docs/HANDOFF.md` — current state per tab, crate map, **Landmines** (read these).
2. `docs/ARCHITECTURE.md` — crate graph, seam table, runtime model, recipes (add a tab /
   adapter / stored field / keybinding / component).
3. `.interface-design/system.md` — design law (tokens, depth, type roles).
4. `crates/<crate>/CLAUDE.md` — per-crate purpose, invariants, how to test it.
5. `docs/design/` — dated design docs; `docs/design/2026-09-09-ui-cohesion-loop.md` is the last checklist.

Project plugin: `.claude/plugins/sid-dev/` (skills `sid-gate`, `sid-capture`,
`sid-new-component`, `sid-new-adapter`, `sid-store-migration`, `sid-worktree`; agents
`rust-test-writer`, `ui-reviewer`, `adapter-auditor`). `just --list` shows dev commands.

## Architecture rules (binding)

1. **Adapter pattern everywhere.** Every external library and every OS integration hides
   behind a trait owned by `sid-core`. Concrete impls live in their own crates.
   - GPUI (`gpui`, `gpui_component`, `gpui_platform`, `gpui_base`, `gpui_kit_assets`) may be
     named **only** in frontend crates: `crates/sid` and `crates/sid-ui`. Inside `crates/sid`,
     `gpui_component::` is further confined to `main.rs` (hygiene test in
     `crates/sid-ui/tests/hygiene.rs`); elsewhere go through `sid_ui`.
   - OS-integration points — keyring, PTY, editor-launch, clipboard, notifications — are
     traits with a Linux/Wayland impl (`sid-os` holds editor-launch + notifications); other platforms are cfg-gated slots.
   - The point is swappability. If a concrete crate name (`redb`, `russh`, `git2`,
     `rusqlite`, `tokio_postgres`, `sysinfo`, `vt100`, `nix`, ...) leaks into `sid-core`,
     that's a bug. A PreToolUse hook (`.claude/hooks/adapter-violation.sh`) blocks it.
2. **Layered scope is the core data invariant.**
   - A **global** store (always loaded; "nothing lost"; redb).
   - A **per-workspace** committed `.sid/config.toml` (git-diffable — never redb),
     attributed to that workspace.
   - **Composition is attributive — never override.** A read is the union of global +
     workspace, each tagged by origin; nothing is shadowed or lost. The default view
     collapses true duplicates (workspace wins) with an opt-in "hide global" filter —
     view checkboxes over a lossless store. redb values are `postcard`; the file is TOML.
   - postcard is positional: `#[serde(default)]` only, never `skip_serializing_if`. A new
     stored field means bumping the entity version + a migration test.
   - **Secrets are never committed** — they live in the OS keyring, referenced by an
     opaque id from the committed config.
   - **Single process.** Focusing a workspace swaps the active scope; it never launches a
     second instance.
   - New items prompt for `save to: workspace | global`, with a configurable
     `default_scope`. New write paths get a `StoreError::Conflict` guard + a test.
3. **Cross-platform is accommodated.** Wayland/Linux is the primary target; macOS/Windows
   are now being added behind cfg-gated platform adapters (never in `sid-core`/domain).
4. **Introspection is idle unless the user opts in.** Hidden or unfocused tabs do no background work (no timers, probes or repaints) unless the user sets it in Settings → Behaviour (`background_refresh_secs`, Off by default). Live external connections (SSH sessions, DB connections) are exempt.

## Testing (pragmatic mode)

Targeted tests per feature, red-first for bug fixes; one gate review at the end of a
slice — not per-commit rigor. Critical paths (the layered store, scope composition,
secret handling) still get real tests. UI changes get before/after captures. A rendering
spike is gated by observation, not unit tests.

**The gate** (all three, check real exit codes — `| tail` hides them; zsh `${pipestatus[1]}`):

```
nice -n 10 cargo fmt --all --check
nice -n 10 cargo clippy --workspace --all-targets -j 3 -- -D warnings
nice -n 10 cargo test --workspace -j 3
```

Or `just gate` / the `sid-gate` skill. One `sid-privfs` sudo test can flake under parallel
threads — rerun it alone. Docker-backed `#[ignore]`d tests: `just test-ssh`, `just test-db`.
Headless UI interaction tests (gpui-kit, focus/keys/clicks, no compositor): `just ui-test`.

## Visual verification

`scripts/sid-cap.sh --tab ssh --theme cosmos --out target/caps/x.png` runs the app in a
private headless sway (nothing hits the real screen; parallel-safe), then Read the PNG.
Never use `scripts/sid-shot.sh`; never `cargo run` the GUI on the live session. Details:
the `sid-capture` skill. `SID_GALLERY=1` mounts the component gallery instead of the app.

## Workflow conventions

- **Worktrees:** every branch of work lives in its own `git worktree` — `~/vcs/sid-wt/<branch>`
  or the harness-provided one. Never edit `main`'s checkout from an agent; never put
  worktrees or cargo targets under `/tmp` (16GB RAM tmpfs).
- **Machine etiquette:** `nice -n 10 cargo <cmd> -j 3`; iterate with `-p <crate>`, run the
  full gate once near the end.
- **Commits:** conventional style matching `git log --oneline` (`feat(sid-ui): …`,
  `fix(ssh): …`); bodies explain *why*. **No trailers** — no `Co-Authored-By`, nothing.
- **Push gate-green units** after every commit (origin is the only durable place); never
  force-push; `main` is merged by Murphy/the orchestrator. Merge `origin/main` in, don't rebase.

## Previously deferred — now un-deferred (2026-11-07)

Murphy un-deferred these; they are being built and are no longer off-limits:
the **MCP server** (`crates/sid-mcp`, wired by `.mcp.json` as the `sid` server: crate_map, find_symbol,
adapter_check, gate, capture_ui, backlog, docs_index, ... — prefer it for orientation), the **`.claude` plugin** (`.claude/`, this repo's
`sid-dev`), **CI** (`.github/workflows/ci.yml`; the old draft is `docs/ci/`), and
**Mac/Windows support** (cfg-gated platform adapters behind the existing traits). Each
still gets its own design pass — keep them lean and behind the adapter seams.
