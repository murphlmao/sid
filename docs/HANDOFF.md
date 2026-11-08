# sid — Handoff / Start Here

**You are picking up an in-progress GPUI rebuild of `sid`.** This is the single
orientation doc: current state, tab by tab, what is open, where the landmines are. The
resume point for the last session is
[`design/2026-11-07-night-sprint-handoff.md`](design/2026-11-07-night-sprint-handoff.md).

**Last verified:** `main @ 9dd6c42 — 76 suites / 2071 tests / 0 failed (fmt, clippy -D warnings clean)`
(rustc 1.96, Linux). Everything from the 2026-11-07 night sprint, the 2026-11-08 integration,
security review #2 and the QA-sweep fixes is merged; there are no pending branches.

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
4. [`design/2026-11-07-night-sprint-handoff.md`](design/2026-11-07-night-sprint-handoff.md) — what landed in the sprint (closed 2026-11-08; see the note at its top).
5. [`design/2026-11-07-perf-pass.md`](design/2026-11-07-perf-pass.md) — frame-cost method and what is left.
6. `crates/<crate>/CLAUDE.md` — per-crate invariants and how to test.
7. History only: `design/2026-09-09-ui-cohesion-loop.md` (annotated), `design/2026-06-27-gpui-rebuild-design.md` (North Star).

## Where things stand

Renderer is `gpui-pre` 0.3.11 + `gpui-component` 0.9 (root `Cargo.toml`). Design system v2 is
in `sid-ui`; every tab was rolled onto it on 2026-11-07.

| Tab | Shipped | Known gaps |
|:--|:--|:--|
| **SSH/SFTP** | Multi-tab shell (home + per-connection sessions, session v2 in `ui/session.rs` + `ui/session/`); host groups as a `Tree`; import from `~/.ssh/config` (parsed off the UI thread); add form whose save-to is always preselected (focused workspace, else global); few-hosts home with a second row of cards; SFTP browser (bounded view/download, no-overwrite downloads, display-safe remote names); **SFTP edit-in-place** (`ui/session/edit.rs` over `sid-ssh::remote_edit`: SHA-256 conflict check with a queued-conflict UI, opt-in atomic tmp-rename upload, unsaved edits preserved as backups under the data dir on every exit path, `LinuxEditorLauncher` wired); secret bindings (`secret_binding.rs`) and the unknown-host-key fingerprint prompt "Trust and connect" (`ui/host_key_prompt.rs`); friendly connect errors (`ui/connect_error.rs`: host:port in the title, plain-language hint, raw error under Details); terminal fidelity A/B-matched to kitty; zoom incl. terminal reflow. | Downloads over 256 MiB are refused (no streaming read on `SftpSession`). Placeholders still use ASCII dots (QA #17). The third get-started card at 1024x700 is unverified after the home rework (QA #19). Tmp-rename upload is opt-in because hard links are invisible over SFTP v3. |
| **Database** | Query editor (Run/Explain), schema `Tree` (select vs activate, icons, folder headers), relationships diagram (button shows why it is disabled), CSV export, query history, empty states, error toasts; query errors map to the user's SQL (engine message only, offset shifted past the pagination wrapper, "check the SQL near" hint, cursor moves to the error); SQLite plans render as an indented detail tree under "Query plan"; add-connection form (engine first Tab stop, sentence case, save-to preselected); hardened Postgres/TimescaleDB decoding. | Export is still a one-item menu "CSV (current page)"; history rows are one-line truncations; connection rows: click or double-click opens, F2 or the context menu renames (tooltip names F2). The diagram pop-out window cannot be captured by the harness. |
| **Network** | Ports / Services / Interfaces / Docker / Kubernetes / **Connections** sub-tabs (Connections has kill); Interfaces as a responsive card grid; Docker summary cards and a wider Status column; row actions (Kill/Stop/Restart) show only on the hovered, selected or armed row; system services read "needs root" with a tooltip for a non-root user; shared page gutters; table text cells without a wrapper node (perf). | Kubernetes empty state has no link or action (QA #17). |
| **Workspaces** | v2: changes (Heading + count pill group headers), diff, commit, stash (by oid), branches, **log as a full-width table** (commit/message/author/date, ref badges), fetch/push via the CLI; **Overview is a card grid** (branch, last commit, working tree, remotes, tags, stash, scope Hosts/Connections); Umbrella fleet table; pin-a-directory. The CLI path is hardened against repo-configured commands (hooks skipped, `core.fsmonitor`/`askPass` off, `GIT_ALLOW_PROTOCOL=ssh:https`, scrubbed `GIT_*` env, literal discard, nested-repo guard); UI in `crates/sid/src/ui/workspaces/`. | Workspace rows truncate the path at the end rather than the middle (QA #16, unverified). Hooks are skipped on CLI-driven operations, so a repo's own hooks do not run for sid's fetch/push. |
| **System** | CPU/mem (cached memory shown)/swap with history sparklines that show real data after ~2.5 s (warmup tick, 3-sample minimum), sortable process table with kill, pinned config-file manager with sudo-elevated no-shell editor (`config_editor.rs::trap_owner`). **Hibernation:** when hidden or unfocused the probe does nothing unless the user sets Settings → Behaviour → background refresh (Off by default, Settings v8). | Narrower `ProcessRefreshKind` for the process table is not done. |
| **Settings** | Section rail (Appearance / Behaviour / Keyboard / Storage) with two-column cards on every section; four palettes shown as theme cards (cosmos, void, dusk, cosmos-light); violet accent (red is destructive only); text rendering (Auto / Subpixel / Grayscale); background refresh; keymap rebinding UI; zoom, reduce motion, desktop notifications, last-tab restore, palette recents (Settings v8), keyring status. | The keyring probe is synchronous at startup (`sid_secrets::probe_keyring`), so the status bar cannot show "checking…"; making it async needs a design decision. Settings gutter versus the other tabs was not re-measured (QA #9). |

Chrome: app-wide `StatusBar`; command-palette hub (current-tab actions and recents ranked first, recents keyed by origin + alias, no self-entry); `?` or Shift+/ opens the shortcut modal; the top-bar tabs collapse to icons (with a Ctrl+N tooltip) below 1100px; action labels are sentence case (keymap test).

## Open

1. Waiting on Murphy: an async keyring probe (the status bar can't show "checking…" because the
   probe runs synchronously before the window opens), and the Workspaces umbrella tree (member repos
   would need to be stored as data). Decided 2026-11-08: CI `gate` stays advisory; fetch/push stay
   on the hardened git CLI; SFTP "keep server copy" backs local edits up; violet accent; background
   refresh Off by default.
2. QA sweep (`design/2026-11-08-qa-sweep.md`): every finding is fixed (each annotated with its commit).
   Still worth doing: a full visual re-sweep (every tab x 4 themes x 1920/1280, live SFTP
   edit-in-place incl. a server stop mid-edit, keyboard-only pass) since the fixes landed.
3. zbus 6 is held back (keyring store still on 5).
4. Perf: the System process probe is already minimal (cpu + memory, user/cmd once per pid);
   the remaining per-tick cost is sysinfo's `/proc/<pid>/task` walk — see `design/2026-11-07-perf-pass.md`.
   Table cells still pay gpui-component's per-cell id + two nodes (needs vendoring the table).
5. Security leftovers from `night/sec-binding`: global secrets saved before bindings existed
   bind on first use (a global record tampered with before that first connect still sends
   once); workspace secrets saved before per-workspace keys re-prompt once and leave a bare
   keyring entry behind; `GlobalStore::record_host_command` must never be wired from raw keystrokes.

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
