# sid — future features (post-v1 catalogue)

**Status:** Tracking doc for everything intentionally **not** in v1, with notes on how the v1 architecture supports each addition.
**Date:** 2026-05-20

This is a living document. Items move to their own design doc once promoted to a planned version.

## How to read this

Each feature has:
- **What it does** — the user-facing capability
- **Why deferred** — what makes it not v1
- **v1 hook** — the architectural seam the v1 spec leaves so this is an addition, not a rewrite

Items are loosely grouped by likely version (v2, v3, "someday"). Order within a group is informal.

---

## v2 — composition & supervision

### Multi-widget composition (Hyprland-style splits within a tab)

**What it does.** A tab can hold multiple widgets in a tiled tree, not just one. `Mod+H` / `Mod+V` to split, `Mod+arrows` to focus, `Mod+Shift+arrows` to swap widgets, `Mod+R` resize mode, `Mod+A` add widget by kind. Per-tab saved layouts.

For example, the **Workspaces** tab in v2 could contain `[git-branches | git-status | commit-drafter]` tiled side-by-side, instead of v1's single composed widget.

**Why deferred.** v1 intentionally optimizes for "fresh slate when I change context" — one focused thing per tab. Composition is power-user territory; it's better introduced once the v1 widget surface is stable.

**v1 hook.** `Layout::Split { dir, ratio, a, b }` already exists on the enum; v1 just never constructs it. Adding split support is widget-code-untouched.

---

### ~~Workspace open list (workspaces as tabs)~~ — **promoted to v1** (2026-05-22)

**Status:** Now in design as part of the [TUI UX interaction overhaul](2026-05-22-tui-ux-interaction-design.md) (branch #3 — `feat/workspace-detail-as-tab`). The promotion was driven by user feedback that the single Workspaces tab did not feel "inhabit-able": you couldn't open a workspace and stay in it. The design landed in v1 differs from the original v2 sketch in two ways:

- v1 ships **one detail tab per open workspace**, not "every registered workspace as a tab in the strip". You explicitly open a workspace from the overview; it gets its own dynamic tab; closing it returns to the overview.
- v1's detail tab is a **multi-repo dashboard** (umbrella + sub-repos) by default, not a single-repo view. This matches the user's umbrella-heavy workflow (e.g., `eggsight-stack`).

---

### Agent manager (Claude Code session observer)

**What it does.** A new top-level tab — **Agents** — that lists active and historical Claude Code sessions across all your projects.

- **Passive mode (v2.0):**
  - Reads `~/.claude/projects/<dir-hash>/<session-id>.jsonl` transcripts
  - Lists sessions sortable by project, started_at, last_activity, model
  - Drill in to a session → render the full conversation history with role/tool markup
  - "Copy claude --resume command" → clipboard
  - "Copy transcript as markdown" → clipboard
- **Active mode (v2.5):**
  - sid spawns Claude Code sessions via PTY (using the same `portable-pty` infra as SSH)
  - Pause/resume/inject from within sid
  - Multi-session simultaneous viewing (relies on composition from v2.0)

**Why deferred.** Significant UX surface; requires careful handling of transcript schemas (which evolve); active supervision needs PTY pipeline maturity from v1.

**v1 hook.** New `AgentService` registers with the engine; new widget kind implements the same `Widget` trait. The transcript directory format is read-only, so it's safe to add without changing v1 behavior.

---

### Workspace-tree actions

**What it does.** Actions that operate across an umbrella workspace and all its children. Examples: "switch every sub-repo to branch X", "pull all sub-repos", "run command in each sub-repo in parallel" with a per-repo result aggregation pane.

**Why deferred.** Per-workspace actions in v1 are scoped to one workspace; the multi-target execution model + result aggregation UI is a separate feature.

**v1 hook.** `ActionRegistry` already supports a `scope: workspace-tree` field; v1 just doesn't surface a UI affordance for it.

---

### Keyring integration for secrets

**What it does.** DB connection passwords, SSH keys (if non-default), and other secrets stored in OS keyring (secret-service on Linux, Keychain on macOS, Credential Manager on Windows) instead of as plaintext in the DB.

**Why deferred.** User explicitly said it's fine to use plaintext-in-DB for v1; keyring integration adds OS-specific complexity and dependency surface.

**v1 hook.** `SecretStore` trait already abstracts secret access. v1 has a `PlainSecretStore` impl that reads from the redb `secrets` table. v2 adds a `KeyringStore` impl with the same interface, plus a one-time migration tool to move existing secrets.

---

### User-configurable storage backend

**What it does.** The Settings tab grows a "Storage backend" picker. Default `redb`; alternatives might include `SQLite` (for queryability via external tools), `Fjall` (LSM, write-heavy workloads), or future fast pure-Rust KV stores. A "Migrate" action moves data between backends with a progress bar.

**Why deferred.** v1 ships with one solid backend (redb). Adding backend-swapping to the UX prematurely is decision-paralysis-by-default and complicates testing.

**v1 hook.** The `Store` trait + `RedbStore` impl already abstract the access layer. Adding more impls is a localized addition; the migrate action and picker are a Settings-tab feature, not a core change. Plugins (see below) could ship third-party `Store` impls.

---

### Plugin loading

**What it does.** Third-party widgets installable without recompiling sid. Either:
- Rust dylib via `libloading` + a stable ABI (performant, riskier)
- WASM via `wasmtime` (safer, slightly slower, sandboxed)

Plugins declare which widget kinds they provide; sid loads them at startup from a `~/.local/share/sid/plugins/` directory.

**Why deferred.** Plugin systems are a significant design commitment (ABI stability, sandboxing, plugin marketplace if community grows). Premature for v1, where sid is mostly a personal tool. Easier to add once the internal Widget trait has stabilized through 2-3 versions of internal use.

**v1 hook.** `Widget` is a trait, not a closed enum. All v1 widgets register via a known list, but the architecture supports a "plugin loader" registering additional kinds at startup.

---

## v3+ — bigger structural changes

### Hyprland-style spaces (above tabs)

**What it does.** Spaces are a layer *above* tabs. `Mod+1..9` switches spaces; each space has its own set of tabs and per-space layouts. Example: a "system space" with utility tabs, an "eggsight-stack space" with workspace-related tabs, a "side-project space" with different tabs.

Combines with composition (v2): each space contains a tabbed cockpit, each tab can be a tiled widget tree.

**Why deferred.** Big mental-model addition; only valuable once you've outgrown single-cockpit usage. v1's "I prefer a fresh slate when I change context" is satisfied by tabs alone.

**v1 hook.** The current `TabManager` is conceptually "the one space"; generalizing to a list-of-spaces is a TabManager refactor that doesn't touch widgets.

---

### Multi-platform polish

**What it does.** Treat macOS and Windows as first-class. ConPTY quirks, AppleScript bridges (e.g., to spawn iTerm tabs instead of kitty), Linux-specific assumptions audited.

**Why deferred.** v1 targets Linux first; macOS and Windows work but with rough edges.

---

## "Someday" features

These are clearly desirable but unscheduled. Listed so they don't get forgotten.

### Workspace tab — beyond git

- **Dev process manager** (the Procfile/Procfile.dev concept renamed to "Dev processes"): per-workspace process list with run/stop/restart/log-tail. Auto-discovered from `Procfile`, `package.json#scripts`, `Cargo.toml`, or declared in `.sid/_metadata.sid`.
- **Workspace shell**: a per-workspace embedded terminal with the right cwd and env.
- **Agent observer panel** (workspace-scoped; depends on Agent manager v2.0)

### Database tab — more backends and features

- MySQL, Redis, MongoDB, DuckDB, ClickHouse
- ER diagram view (schema introspection)
- Saved query library (per connection or shared)
- Result row → "open related" foreign-key navigation
- Query plan visualization
- Notebook-style cells (mix queries + markdown notes)

### SFTP enhancements

- Multi-file selection, drag-equivalent move/copy keybinds
- Two-pane sync mode (local ↔ remote diff + sync)
- Background transfer queue with progress in status bar
- Resume interrupted transfers

### Network — beyond ports

- **Live packet capture** (tcpdump-style) per interface with filters
- **Bandwidth graphs** per interface
- **Connection geo-resolution** (IP → country/ASN)
- **iptables/nftables/firewalld viewer**

### System — beyond services and configs

- **System log viewer** (journalctl) with grouping/filtering UI
- **Sparkline metrics** (CPU, mem, disk IO, network IO) — small btop-like graphs without the visual noise
- **Hardware sensors** (temperatures, fans, battery)
- **Update notifier** (pacman/apt/brew updates available)

### SSH enhancements

- **Tunnel manager**: forward ports, reverse forwards, list active tunnels
- **Mosh** as an alternative to SSH for unreliable networks
- **Multiplexed sessions** (one connection, many shells)

### UI/UX polish

- **Theme editor in-app** with live preview
- **Workspace templates** ("create new workspace from template")
- **Quick switcher** (Cmd-K palette in addition to Ctrl+F command palette)
- **Themable star animations** (rare twinkles in cosmos theme — very subtle, off by default)
- **Vim-style modal keybind profile**
- **Notification center** (recent toasts, dismissable)
- **Status bar customization** (left/center/right segments)

### Workspace metadata enhancements

- **Auto-detect more manifest types**: `flake.nix`, `pyproject.toml`, `mix.exs`, `Gemfile`, `composer.json`
- **Workspace-tagged search**: full-text search scoped to a workspace
- **Workspace-bound clipboard** (kill-ring style; per-workspace)

### Sync and portability

- **Settings/sessions sync** across machines via a user-provided git repo or s3 bucket. Opt-in. Encrypted client-side.
- **Export/import** workspaces+settings as a portable archive

### Integrations

- **Linear / Jira / GitHub Issues** widget (read-only ticket lookup; no v1 because pulls in external API surfaces)
- **GitLab / GitHub PR widget** (status, comments, approve)
- **Discord / Slack notifier hooks** for long-running jobs

---

## Deferred from the 2026-05-22 interaction overhaul

These items came up during the [TUI UX interaction overhaul spec](2026-05-22-tui-ux-interaction-design.md) and were intentionally cut from its v1 scope so the spec stayed focused. Each has a v1 hook in place so the follow-up is an addition, not a rewrite.

### Network interface mutation (write path)

**What it does.** Edit DHCP/static IP, MTU, default-route preference; restart the interface; restart NetworkManager/networkd. Triggered by `E` from a focused interface in the Network tab.

**Why deferred.** Requires a new adapter (`NetworkAdmin`), a polkit/sudo elevation flow, and careful failure-mode coverage (what if `nmcli` succeeds but the connection drops?). Bigger surface than the rest of the overhaul.

**v1 hook.** v1 ships the `E` chord bound to a stub that toasts "Interface editing not yet supported — see backlog". The `InterfaceDetailModal` substrate is also in v1 — adding editable fields means swapping `Field::Display` for `Field::Text`/`Field::Choice` rows and wiring submit to the `NetworkAdmin` adapter.

---

### Real CI status fetcher for `WorkspaceDetailWidget`

**What it does.** Each sub-repo row in the workspace detail dashboard shows live CI status (passing/failing/in-progress). Probably shells out to `gh run list --json --limit 1` per repo, cached per-session.

**Why deferred.** Pulls a `gh` subprocess dependency into the hot path of the dashboard; cache invalidation needs care. v1 ships the column with `CiStatus::Unknown` everywhere so the layout is final.

**v1 hook.** The `CiStatus` enum and the column already exist; populating them is a job for a new `CiStatusProvider` adapter behind a feature flag.

---

### "Sync all sub-repos to branch X" actual implementation

**What it does.** From a workspace detail tab, press `b` to open a modal that takes a branch name and attempts `git checkout` on every sub-repo, reporting per-repo success/skip/conflict.

**Why deferred.** Need to think about partial-failure semantics: do we roll back the repos that succeeded if a later one fails? Probably not, but the user should see a clear per-repo result.

**v1 hook.** v1 ships the `b` modal that opens, accepts a branch name, and toasts "Not yet implemented". Wiring it up is implementation-only.

---

### Kitty-protocol auto-enable on startup

**What it does.** sid emits `CSI > 1 u` at startup to ask the terminal to deliver enhanced key chords. Terminals that support the protocol (kitty, ghostty, wezterm, foot, recent xterm/modifyOtherKeys=2) start sending `Ctrl+1`, `Ctrl+,`, etc. as distinct chords.

**Why deferred.** Alt fallbacks ship in v1 and work universally. Auto-enabling adds a startup side effect and a "leave the terminal in a clean state on exit" obligation that needs care.

**v1 hook.** Bindings under `KeyModifiers::CONTROL` already exist — they just don't fire on terminals that swallow the chord. Once the protocol is enabled the existing bindings work without further changes.

---

### Established connections / Unix sockets in the ports table

**What it does.** Network tab shows not just listening sockets but also active outbound connections, Unix-domain sockets, and sudo-required visibility into root-owned sockets.

**Why deferred.** Expands the sys-probe adapter surface and needs a privilege-elevation model. v1's listening-only view is honest about its scope.

**v1 hook.** The `ListeningPort` type can grow a `direction: PortDirection` discriminator without breaking callers; the adapter trait can add new methods alongside `list_listening_ports`.

---

### Workspace scan-on-demand command-palette action

**What it does.** A `workspaces.scan_now` action in the command palette runs the existing `scan_workspace_root` over user-defined roots and presents discovered repos as a "promote these?" picker.

**Why deferred.** The function is exported and tested in v1 (it's the same code that used to run at startup). Adding the palette affordance + the promotion picker is UI-only work for a follow-up.

**v1 hook.** `wire::startup_discover` is exported. A new `palette` action calls it on demand.

---

## Definitively out of scope

- **Not a text editor.** sid spawns your editor for commit messages, configs, etc. It does not implement editing.
- **Not a file manager.** SFTP is for remote files; local file management belongs in `ranger`, `yazi`, `lf`, etc.
- **Not a process supervisor.** Dev processes can be run/stopped but sid is not a replacement for `systemd`, `runit`, or `overmind`.
- **Not a chat client.** Even when integrated with agent sessions, sid is read/observe + send canned messages, not a general chat UI.
- **Not a window manager.** sid runs *in* one terminal window; it doesn't manage OS windows. Detach uses your terminal emulator's native window/tab creation.
