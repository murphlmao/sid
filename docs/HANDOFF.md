# sid — Handoff / Start Here

**You are picking up an in-progress GPUI rebuild of `sid`.** This is the single
orientation doc: current state, tab by tab, what is open, where the landmines are. The
resume point for the last session is
[`design/2026-11-07-night-sprint-handoff.md`](design/2026-11-07-night-sprint-handoff.md).

**Last verified:** 2026-11-08, branch `night/wrap-2026-11-07` @ `b948145` (this refresh
lives on `night/docs-refresh`). The wrap branch merges `night/sec-binding`, `night/perf` and
`night/db-polish` onto main without a combined gate; the **official combined gate is still
pending**. Last official gated state: `main @ 0fa6dbe: 75 suites / 1924 tests`, 0 failed
(fmt, clippy `-D warnings`, test; rustc 1.96, Linux). Incidental check: the docs-only
`night/docs-refresh` (wrap + docs) ran fmt, clippy and `cargo test --workspace` clean —
75 result lines, 1980 passed, 0 failed, 48 ignored.

## What sid is (30 seconds)

An integrated developer **ops-cockpit** — SSH/SFTP, Database, Network, Workspaces,
System, Settings — as a native **GPUI** desktop app, run locally. Built in **vertical
slices**; SSH/SFTP is the spearhead. The binding rules (adapter pattern, attributive
layered scope, secrets-never-committed) live in [`../CLAUDE.md`](../CLAUDE.md) — they are
invariants, not suggestions. Mac/Windows are being added behind cfg-gated platform
adapters; the MCP server, `.claude` plugin and CI are un-deferred and built.

## Read order

1. [`../CLAUDE.md`](../CLAUDE.md) — the binding ruleset.
2. [`ARCHITECTURE.md`](ARCHITECTURE.md) — crate graph, seam table, runtime model, recipes.
3. [`../.interface-design/system.md`](../.interface-design/system.md) — design law (v2); spec and per-tab rollout guide in [`design/2026-11-07-friendly-ui-v2.md`](design/2026-11-07-friendly-ui-v2.md).
4. [`design/2026-11-07-night-sprint-handoff.md`](design/2026-11-07-night-sprint-handoff.md) — what landed, what is ungated, next steps, pending decisions.
5. [`design/2026-11-07-perf-pass.md`](design/2026-11-07-perf-pass.md) — frame-cost method and what is left.
6. `crates/<crate>/CLAUDE.md` — per-crate invariants and how to test.
7. History only: `design/2026-09-09-ui-cohesion-loop.md` (annotated), `design/2026-06-27-gpui-rebuild-design.md` (North Star).

## Where things stand

Renderer is `gpui-pre` 0.3.11 + `gpui-component` 0.9 (root `Cargo.toml`). Design system v2 is
in `sid-ui`; every tab was rolled onto it on 2026-11-07.

| Tab | Shipped | Known gaps |
|:--|:--|:--|
| **SSH/SFTP** | Multi-tab shell (home + per-connection sessions); host groups as a `Tree`; import from `~/.ssh/config` (`ui/ssh_import.rs`, from the home header, empty state and a first-run banner); quick-connect add; SFTP browser (bounded view/download, no-overwrite downloads, display-safe remote names); secret bindings (`secret_binding.rs`) and the unknown-host-key "Trust and connect" prompt (`ui/host_key_prompt.rs`); terminal fidelity A/B-matched to kitty; zoom incl. terminal reflow. | Session v2, SFTP edit-in-place UI, keep-server-copy backup and Esc on modals sit on `night/ssh-session` (conflicts with sec-binding in `ui/session.rs`; its tip does not compile, last green `18175d4`). Downloads over 256 MiB are refused (no streaming read on `SftpSession`). `EditorLauncher` (`sid-os`) is not yet wired into `crates/sid`. |
| **Database** | Query editor (Run/Explain), schema `Tree` (select vs activate fixed, table/column icons, folder headers), relationships diagram, CSV export, query history, empty states, error toasts; hardened Postgres/TimescaleDB decoding. | Only gated as part of the pending wrap gate. Table frame cost (per-cell wrapper div ~11% of a frame, perf-pass doc). The diagram pop-out window cannot be captured by the harness. |
| **Network** | Ports / Services / Interfaces / Docker / Kubernetes / **Connections** sub-tabs (Connections has kill), CPU/mem columns, friendlier empty states. | Table frame cost (shared with Database). |
| **Workspaces** | v2: changes, diff, commit, stash, branches, paged log, fetch/push via the git CLI (`crates/sid-git/src/cli.rs`); Umbrella fleet table; pin-a-directory. UI in `crates/sid/src/ui/workspaces/` (`model.rs`, `ops.rs`, `views.rs`). | Log badges live on `night/polish-settings-ws`. The git CLI honours repo config and runs hooks, unlike libgit2 — hardening review not yet done (see Open). |
| **System** | CPU/mem/swap with history sparklines, sortable process table with kill, pinned config-file manager with sudo-elevated no-shell editor (focus-trap sudo-password leak is **fixed**: `config_editor.rs::trap_owner`); unfocused probe runs ~5x slower (perf). | No "cached" memory in the overview; no "collecting…" state for empty sparklines at launch; narrower `ProcessRefreshKind` not done; unfocused interval (10 s) awaits Murphy's call. |
| **Settings** | Section rail (Appearance / Behaviour / Keyboard / Storage), four palettes (cosmos, void, dusk, cosmos-light), keymap rebinding UI, zoom, reduce motion, desktop notifications, last-tab restore (Settings v6), keyring status. | "Text rendering" option (Auto / Subpixel / Grayscale), Settings v7, theme swatch cards, palette recents are on `night/polish-settings-ws` (conflicts with `secret_bindings` in `crates/sid-store/src/{global.rs,lib.rs}`; keep both, re-check the v6 to v7 migration test). |

Chrome (sprint handoff): app-wide `StatusBar`, command-palette hub, `?` shortcut modal.

## Open

Order and detail are in the sprint handoff; in short:

1. Gate `night/wrap-2026-11-07`; merge `night/ssh-session`, `night/polish-settings-ws`,
   `night/ci-fix` one at a time (type-check after every textual merge); gate; fast-forward `main`.
2. Second adversarial security review (planned, not run): `crates/sid-git/src/cli.rs`
   (consider `-c core.hooksPath=/dev/null`, `--no-verify`, `protocol.ext.allow=never`, a
   `core.sshCommand` override), the remote-edit UI, the import dialog, the committed hook.
3. sysinfo 0.40 (fallible constructors) — held back, as is zbus 6 (keyring store still on 5).
4. QA sweep: every tab x 4 themes x 1920/1280, keyboard-only pass, live docker sshd/postgres.
5. Perf leftovers: per-cell wrapper div in tables, narrower `ProcessRefreshKind`.
6. Security leftovers from `night/sec-binding`: global secrets saved before bindings existed
   bind on first use (a global record tampered with before that first connect still sends
   once); workspace secrets saved before per-workspace keys re-prompt once and leave a bare
   keyring entry behind; `GlobalStore::record_host_command` must never be wired from raw keystrokes.
7. Waiting on Murphy: D5 accent (cosmos red primary reads as danger), unfocused System
   refresh interval, SFTP keep-server-copy semantics, making the CI `gate` job a required
   check, keeping fetch/push on the git CLI.

## Crates and key files

16 workspace members (root `Cargo.toml`). Only `sid` and `sid-ui` may name GPUI.

| Crate | What it is |
|:--|:--|
| `sid` | GPUI frontend — app shell, six tabs, the single `AppState` entity. |
| `sid-ui` | The design system, compiled — tokens, type roles, metrics, motion, components, gallery, headless interaction tests. |
| `sid-store` | Layered attributive store: global redb + per-workspace `.sid/config.toml`; includes the `secret_bindings` table. |
| `sid-secrets` | Secret-storage seam — keyring, in-memory fallback, dormant encrypted-file backend. |
| `sid-core` | Pure trait seam (ssh/term/db/git/sys/gpu/svc/containers/privfs/editor/notify) — no concrete deps. |
| `sid-ssh` | `russh` `SshClient` (fail-closed known-hosts) and the `~/.ssh/config` parser. |
| `sid-term` | `vt100`-backed terminal screen. |
| `sid-db` | `DbClient` impls: Postgres, SQLite, redb-browse. |
| `sid-git` | `git2` `GitProvider` plus the `git` CLI path for fetch/push (`cli.rs`). |
| `sid-gpu` | GPU pre-flight in a subprocess. |
| `sid-sysinfo` | `SysProvider`: sysinfo + netstat2 + nix. |
| `sid-svcctl` | `systemctl` `ServiceProvider`. |
| `sid-containers` | `docker` / `kubectl` providers. |
| `sid-privfs` | `sudo`-backed `PrivilegedFs`. |
| `sid-os` | OS-integration impls: `LinuxEditorLauncher`, `DbusNotifier`; other platforms are empty slots. |
| `sid-mcp` | MCP server (stdio, `rmcp`) with tools for working on this repo; configured by `.mcp.json`. |

Key files: `crates/sid/src/app.rs` (AppState, `Tab`, render-from-cache), `keymap.rs`,
`secret_binding.rs`, `ssh_connect.rs`; `crates/sid-ui/src/{theme,typography,metrics,motion,bridge}.rs`;
`crates/sid-store/src/{store,composer,global}.rs`; `.interface-design/system.md`; `justfile`;
`.github/workflows/ci.yml` (linux/macos/windows); `.claude/plugins/sid-dev/` (skills + agents).

## Landmines (things that already bit us)

- gpui reports a text element's **min-content width as its full string width**: use
  `min_w(0)` + `.clamp_one_line()` + `flex_none()` on the sibling that must not grow.
- `tests/hygiene.rs` bans the literal `.truncate(`; use `sid_ui::StyledExt::clamp_one_line()`.
- `h_flex()` centres on the cross axis: a `flex_1` table beside a sized sibling resolves to
  zero height. Use `div().flex().flex_row().min_h(0)`.
- `FillTable` widths land on the next frame (`Window::on_next_frame`); `Column.width` is `Pixels`-only.
- `tab_group()` only renumbers tab stops; wrapping Tab needs gpui-component's `focus_trap`.
  Two traps registered on one surface race — keep a single owner (`config_editor.rs::trap_owner`).
- A gpui-component widget not threaded through `sid_ui::bridge` renders unthemed.
- Typography and colours only via sid-ui roles and tokens (hygiene test).
- **Parallel branches produce semantic conflicts** (git2 0.21 `Result` getters, duplicate fns):
  always type-check after a clean textual merge.
- ETXTBSY when tests write then exec scripts concurrently; write them once before spawning.
- `cargo ... | tail` hides exit codes (zsh `${pipestatus[1]}`).
- postcard is positional: `#[serde(default)]` only, never `skip_serializing_if`.
- `hyprctl keyword` is rejected by Murphy's Lua Hyprland config (silent no-op): use
  `scripts/sid-cap.sh`, not `sid-shot.sh`.
- Never put worktrees or cargo targets under `/tmp` (16GB tmpfs).
- Commits carry **no trailers**.

## Testing and workflow

- **Gate:** `just gate`, or `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` (CLAUDE.md has the niced forms). One `sid-privfs` sudo test can flake in parallel; rerun alone.
- **UI interaction tests:** `just ui-test` — headless gpui-kit harness, files in `crates/sid-ui/tests/`
  (focus trap, button activation, text input editing, tree keys, FillTable floor). Sees behaviour, not pixels.
- **Captures:** `scripts/sid-cap.sh --tab <tab> --theme <theme> --size WxH --out x.png` (private headless sway;
  needs `sway` + `wtype`; honours `SID_CARGO=<wrapper>`). `just cap`, `just gallery`; frame cost: `just perf`.
- **Docker fixtures:** `just test-ssh`, `just test-db` (`#[ignore]`d tests).
- **MCP:** `just mcp` runs `sid-mcp` (crate_map, find_symbol, adapter_check, gate, capture_ui, backlog, docs_index, recent_commits, store_inspect).
- **Worktrees** under `~/vcs/sid-wt/<branch>` or the harness one; conventional commits, no trailers; push gate-green
  units; never force-push; merge `origin/main` in, don't rebase.
- **Salvage source:** the archived TUI at `murphlmao/sid-poc` (read-only).
