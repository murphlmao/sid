# sid — v1 foundation design

**Status:** Draft for review
**Date:** 2026-05-20
**Author:** Murphy (with Claude brainstorming)

## What sid is

`sid` is a fast, focused TUI cockpit for personal developer workflow. It's named after my dog.

It is **tabs-first**: one focused module fills the screen at a time, with `Ctrl+←/→` (or `Ctrl+1..6`) to switch. Each tab is one of six things: your code workspaces, your SSH/SFTP hosts, your databases, your machine's network/process state, your machine's system configuration, or sid's own settings.

It is **minimal-footprint**: a single binary, a single pure-Rust embedded DB file, and one tiny optional `sid.toml` for overriding the DB path. No litter of dotfiles. Themes, keybinds, actions, sessions — all in the DB, edited in-app.

It is **resilient**: every meaningful state mutation persists continuously. Crashing or accidentally closing the terminal does not lose your work.

It is **detachable**: any tab can be popped out into another terminal — like `claude --resume`, but for a focused tab.

It is **galaxy-dusky**: the default `cosmos` theme is near-black with red accents and pale starlight cyan, occasional `✦` `·` `★` glyphs as decorative cues. Not loud. Not over-the-top like btop. Just enough atmosphere.

## Design goals

1. **Cognitive cleanliness** — one thing on screen, one thing in head. Fresh slate when you change context.
2. **Stay out of the way** — sid should never block your existing workflow. No global daemons, no required setup, no metadata pollution.
3. **Blazingly fast** — render loop is sync and never blocks; sub-ms DB reads; lazy widget construction; debounced persistence.
4. **Keyboard ergonomics** — intuitive defaults (`Ctrl+arrows` for tabs, `Ctrl+F` for command palette, `Tab` for in-pane focus cycling). Fully customizable.
5. **Beautiful** — the cosmos theme is the default identity. Other themes ship and can be authored.
6. **Built to grow** — the v1 code is structured so v2 features (widget composition, plugins, agent manager) are non-breaking additions, not rewrites.
7. **Adapter pattern everywhere** — every external library hides behind a trait. We aren't married to any one dependency; swaps and mock-friendly tests fall out for free.

## Non-goals (v1)

These features are valuable and explicitly planned for later, but **out of v1 scope**. See `2026-05-20-sid-future-features.md`.

- Multi-widget composition / Hyprland-style splits within a tab
- Agent manager (Claude Code session observer/supervisor)
- Plugin loading (WASM/dylib)
- Workspace-tree actions ("do X across all child repos")
- Network packet sniffing
- System log graphs / metrics dashboards

## Tech stack (locked)

| Concern | Choice | Why |
|---|---|---|
| Language | Rust (edition 2024) | Speed, type safety, single binary |
| TUI framework | **Ratatui** (crossterm backend) | Modern Rust TUI default; immediate-mode |
| Async runtime | **Tokio** (multi-thread runtime) | Needed for PTY, DB, SSH, file watching |
| Embedded DB | **redb** | Pure Rust, ACID, MVCC, multi-process readers (enables detach), small binary, stable file format |
| Domain serialization | **postcard** | Compact, schema-evolution-friendly via versioned prefixes |
| Git | **git2** (libgit2 bindings) | Standard; richer than pure-Rust alternatives |
| SSH client | **russh** | Pure Rust, supports modern algorithms |
| SFTP | **russh-sftp** | Companion to russh |
| Embedded PTY | **portable-pty** + **vt100** | Cross-platform; vt100 for proper ANSI rendering |
| DB client (Postgres) | **tokio-postgres** | Standard async Postgres |
| DB client (SQLite) | **rusqlite** (bundled) | Embedded SQLite for the DB tab |
| System info | **sysinfo**, **netstat2** | Ports, processes, interfaces |
| OS keyring (stubbed) | **keyring** | Trait for v2; v1 stores secrets in DB |
| Logging | **tracing** + **tracing-appender** | Structured logging, file rotation |
| Config TOML | **toml** | For the one tiny path-override file |

### Why redb specifically

Researched and benchmarked against fjall, sled, CozoDB, persy, SurrealDB embedded, bonsaidb, LMDB/heed, and SQLite. redb wins because:

1. **Pure Rust** (no C/C++ FFI) — portability, clean binary
2. **Multi-process readers** via `ReadOnlyDatabase` and shared file locks — enables detach
3. **ACID + MVCC** (single writer, unlimited concurrent readers, no blocking)
4. **Small** (~200–400 KB binary impact after LTO)
5. **Stable file format** (v3+, with declared upgrade path)
6. **Actively maintained** (v4.1 released April 2026)
7. **Modern but conservative** — copy-on-write B+ tree (LMDB-inspired), zero-copy reads via `AccessGuard`

fjall is the runner-up but **disqualifies** because it forbids multi-process opens (breaks detach). CozoDB's Datalog is overkill for our query patterns (PK lookups + time-range scans). sled is effectively abandoned. bonsaidb is alpha. SurrealDB embedded pulls in too much machinery.

## Architecture

### Core abstractions

```rust
trait Widget: Send {
    fn id(&self) -> WidgetId;
    fn title(&self) -> &str;
    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme);
    fn handle_event(&mut self, ev: Event, ctx: &mut WidgetCtx) -> EventOutcome;
    fn poll(&mut self) -> Option<Action>;
    fn save_state(&self) -> Bytes;
    fn load_state(&mut self, state: Bytes);
    fn launch_spec(&self) -> Option<LaunchSpec>;  // identity for detach
}

enum Layout {
    Single(Box<dyn Widget>),                                          // v1 always this
    Split { dir: Dir, ratio: f32, a: Box<Layout>, b: Box<Layout> },   // v2+
}

struct Tab { id: TabId, title: String, layout: Layout, hotkey: Option<char> }

struct LaunchSpec {
    kind: WidgetKind,           // e.g., "ssh", "db", "git-status"
    instance_id: InstanceId,
    config: serde_json::Value,  // e.g., { "host": "jp46-dev" }
}
```

The `Layout` enum is the future-proofing seed: v1 only constructs `Layout::Single`, but v2 splits widgets into the `Split` variant **without changing widget code**.

### Application layers

```
┌───────────────────────────────────────────────┐
│  Terminal (crossterm events, signals)         │
├───────────────────────────────────────────────┤
│  App (Ratatui draw loop)                      │
│   ├─ TabManager (active tab, switch, hotkeys) │
│   ├─ CommandPalette (Ctrl+F)                  │
│   └─ Toast/StatusBar                          │
├───────────────────────────────────────────────┤
│  Widgets (one per tab in v1)                  │
│  ├─ WorkspacesWidget                          │
│  ├─ SshWidget (incl. SFTP sub-panel)          │
│  ├─ DatabaseWidget                            │
│  ├─ NetworkWidget                             │
│  ├─ SystemWidget                              │
│  └─ SettingsWidget                            │
├───────────────────────────────────────────────┤
│  Engines / Registries (internal services)     │
│  ├─ WorkspaceStore     ThemeRegistry          │
│  ├─ KeybindMap         ActionRegistry         │
│  ├─ SshPool            DbPool                 │
│  ├─ SysProbe           JobQueue               │
│  ├─ StatePersister     SecretStore (trait)    │
│  └─ SessionManager     CommandPalette         │
├───────────────────────────────────────────────┤
│  Store trait (domain interface)               │
│      ↓ implementation: RedbStore              │
│  redb file at ~/.local/share/sid/sid.redb     │
└───────────────────────────────────────────────┘
```

Widgets are views; engines own canonical state. This separation is what makes v2's "same widget instance in two tabs" or "agent panel observing externally-started sessions" work cleanly.

### Data flow

1. **Input**: crossterm event arrives at `App::handle_event`
2. **Dispatch**: `TabManager` routes to active widget, or to `CommandPalette` if open, or to a global keybind via `KeybindMap → ActionRegistry → action handler`
3. **Widget side effects**: widget calls `ctx.jobs.spawn(...)` for long async work (git operations, queries) or `ctx.store.put_setting(...)` for state changes
4. **Async results**: `JobQueue` runs work on Tokio; results arrive via channel; widget's `poll()` picks them up next frame
5. **Persistence**: `StatePersister` debounces (~250 ms) state-change notifications and flushes them as a batch transaction to redb
6. **Render**: every frame, `App::draw` calls `widget.render(frame, area, theme)`

The render loop is synchronous; nothing in `render()` blocks on IO. Anything slow happens via channels.

### Error handling

- **Recoverable errors** (failed query, dropped SSH connection, file not found): surfaced via a **toast** (transient status-bar notification) and logged via `tracing`. Widget remains usable.
- **Persistent errors** (DB corruption, bad config): surfaced via a **modal** with the error and a "copy details" shortcut. App enters degraded mode where possible.
- **Panics**: caught at the App level via `std::panic::set_hook`, logged with backtrace to `~/.local/state/sid/crash-<timestamp>.log`, screen is restored cleanly, exit code 1.
- All errors use `thiserror` for domain types and `anyhow` only at the top-level entry points.

### Multi-process & detach

Detach is the killer ergonomic feature. The design:

1. **In main sid**: `Ctrl+D` on a focused tab/widget calls `widget.launch_spec()`. sid renders a small overlay:
   ```
   ┌─ Detach ────────────────────────────────────┐
   │ Run this in another terminal:               │
   │                                             │
   │   sid widget --kind ssh --instance abc-123  │
   │                                             │
   │ [c] Copy to clipboard   [Esc] Cancel         │
   └─────────────────────────────────────────────┘
   ```
2. **Detached process** (`sid widget ...`): opens redb as `ReadOnlyDatabase` (multi-process safe), loads the widget's state by instance ID, renders only that widget. No tab bar, no other tabs — focused.
3. **Writes from detached**: detached processes send small upsert requests over a Unix socket at `~/.local/state/sid/main.sock`. Main process performs the actual write. Socket protocol is a tiny line-delimited JSON request/response; this is the only IPC.
4. **Reattach**: `Ctrl+A` in main app, type the instance ID (or pick from a "detached widgets" list). Main app re-renders the widget inline; signals the detached process to exit. If no main app is running, detached process becomes "host" until you launch a main app.
5. **Heartbeat**: every running sid process writes its presence (pid + start_time + role + instance_ids) to the `processes` table every 5 s. Stale entries cleared at startup.

### Session restore

The "I crashed / accidentally closed the terminal" problem.

**The trick**: persistence is the default. There is no special "save" step; the DB *is* the live state.

- **Continuous persistence**: every widget signals "state changed"; `StatePersister` debounces (~250 ms) and writes.
- **Heartbeat**: `SessionManager` writes session heartbeat (`last_active`, `active_tab_id`, etc.) every 5 s.
- **Clean shutdown**: marks `ended_at` on the session.
- **On launch**:
  - If a session exists with `ended_at` unset and `last_active` < N minutes ago (default 60), show:
    ```
    ┌─ Resume session ────────────────────────────┐
    │ Last activity 14 min ago.                   │
    │ Tabs: workspaces · ssh · database           │
    │                                             │
    │ [Enter] Resume   [L] List sessions          │
    │ [N]     New session   [Esc] Quit            │
    └─────────────────────────────────────────────┘
    ```
  - Otherwise: normal startup (most recent clean session is still listed in Ctrl+F → "Sessions").
- A **Session** record contains: open tab IDs, focused tab, per-widget state blob, selected workspace/host/conn, timestamps. **It does not contain credentials.**
- **Sessions** are first-class: listable via `Ctrl+F → "sessions"`; user can load any prior session.

## Adapter layers (swappability)

Every external dependency hides behind a domain-shaped trait. This is enforced by crate boundaries: trait definitions live in `crates/sid-core/`; concrete implementations live in their own crates (`crates/sid-store/`, `crates/sid-git/`, etc.) and never appear in widget code.

This means:

- We aren't married to any library. If a better choice emerges, swapping it is a single-crate change.
- Tests use in-memory or mock impls instead of spinning up real services.
- v2 can offer **user-configurable backends** (e.g., "use SQLite instead of redb", "use OS keyring instead of plaintext secrets") without touching widget code.

| Surface | Trait | v1 impl(s) | Future impl candidates |
|---|---|---|---|
| App-state storage | `Store` | `RedbStore` | `SqliteStore`, `FjallStore`, user plugins |
| Secret storage | `SecretStore` | `PlainStore` (in DB) | `KeyringStore`, `EnvStore`, `OnePasswordStore` |
| Git provider | `GitProvider` | `Git2Provider` (libgit2) | `GitoxideProvider` (pure-Rust, maturing), `CliGitProvider` |
| SSH client | `SshClient` | `RussshClient` | `OpenSshForkClient`, mocked |
| PTY provider | `PtyProvider` | `PortablePtyProvider` | platform-specific impls |
| DB client (Database tab) | `DbClient` | `PostgresClient`, `SqliteClient` | MySQL, DuckDB, ClickHouse, … |
| System probe | `SysProvider` | `SysinfoProvider` (sysinfo+netstat2) | platform-specific impls, mocked |
| Notifier | `Notifier` | `ToastNotifier` (in-app) | `OsNotifier` (libnotify/AppleScript), `WebhookNotifier` |
| Clipboard | `Clipboard` | `ArboardClipboard` | `Osc52Clipboard` (headless terminals) |

**Principle**: no widget or engine code names an external crate. They name a trait. The binary crate (`crates/sid/`) picks the concrete impl at startup from user config. Mock impls live in test modules of `sid-core/` for fast unit tests across the codebase.

We picked `git2` over `gitoxide` for v1 because gitoxide's write operations (commit/push) are still maturing; the `GitProvider` trait lets us swap once gitoxide is fully production-ready.

## Tabs (v1)

### 1. Workspaces

**v1 scope: git operations only.**

- **Tree** of registered workspaces in the left pane (parents expandable for the eggsight-stack pattern of "umbrella dir + sub-repos")
- **Right pane** (for selected workspace):
  - Branch list (current marked, click/Enter to checkout — confirms first)
  - Status: working tree (`modified`, `staged`, `untracked`)
  - Commit log (paginated, `Enter` for details)
  - Diff viewer (staged + unstaged toggle)
  - Commit drafter: opens `$EDITOR` (`EDITOR` env or `vi` fallback) for message; on save, commits via git2
  - **Actions**: lists workspace-defined quick-actions (e.g., `clone-repos.sh`, `switch-branches.sh`) from `.sid/_metadata.sid`

**Discovery:** scan configured roots (default `~/vcs/`) at startup; manual via `sid workspace add /path`. Workspaces persist in the redb `workspaces` table.

**Workspace metadata** (`<workspace>/.sid/_metadata.sid` — optional, JSON with custom extension):
```json
{
  "name": "eggsight-stack",
  "type": "umbrella",
  "actions": [
    { "label": "Clone all repos", "cmd": "./clone-repos.sh", "key": "c" },
    { "label": "Switch branches", "cmd": "./switch-branches.sh", "key": "s" }
  ],
  "children": ["../eggsight-core", "../eggsight-frontend"]
}
```

If absent, sid sniffs for `CLAUDE.md`, `*.code-workspace`, `Procfile`, `package.json#workspaces`, `Cargo.toml` workspaces, etc. — but only uses them to populate display metadata, never to mutate the workspace.

### 2. SSH

- **Hosts** (left pane): read from `~/.ssh/config` + manually-added entries (stored in DB)
- **Connection pane** (right): when a host is selected, shows last-connect status + a "Connect" affordance
- On connect: opens an embedded PTY (`portable-pty` + `russh`) with `vt100` rendering for proper colors and cursor handling
- **SFTP sub-panel** (toggleable with `Tab` while in SSH): remote file tree; up/download; edit-in-place by streaming file → local temp → spawn `$EDITOR` → write back on save
- Per-host metadata persisted: last connected, command history, custom welcome command

### 3. Database

- **Saved connections** (left pane): Postgres + SQLite in v1
- **Query editor** (top right): multi-line, syntax highlight via a simple SQL lexer (full tree-sitter integration deferred)
- **Result table** (bottom right): paginated, sortable, copy-cell, export-CSV
- **History** (collapsible bar): per-connection query history, sortable by recency / duration / row count
- Secrets: v1 stores them as plaintext in redb behind a `SecretStore` trait. v2 swaps in a `KeyringStore` impl with zero call-site changes.

### 4. Network

- **Listening ports table**: port, PID, command, protocol
- **Processes table** (sortable): PID, name, CPU%, mem, started, command
- **Interfaces sidebar**: summary per interface (ip, rx/tx)
- **Actions**: `k` to kill PID; `Enter` to drill into process; `/` filter
- Backed by `sysinfo` + `netstat2`, polled by `SysProbe` (default 2 s, configurable)

### 5. System

- **Pinned configs** (top): label + path + opener command (default: spawn external `kitty -e $EDITOR <file>` cd'd to parent dir). Add/remove/edit pins.
- **Services** (middle): systemctl list-units; filter by state/name; per-unit: status / start / stop / restart / journal tail
- **Quick actions** (bottom): user-defined shell snippets with labels and optional keybinds. Examples: "kill port", "open kitty here". Scope: global (workspace-scoped ones live in Workspaces).
- All actions also runnable via `Ctrl+F` palette regardless of which tab you're on.

### 6. Settings

In-app configuration editor. No more "where the hell is that config file".

- **Theme** picker with live preview pane on the right
- **Keybinds** editor: action → key; conflict detection; reset-to-defaults
- **Behavior toggles**: auto-restore session (yes/ask/no), auto-scan workspace roots, debounce intervals, default tab on launch
- **Workspace roots** editor (add/remove paths)
- **Quick actions** editor (label/cmd/keybind; global vs workspace scope)
- **DB path override** (writes the one-line `sid.toml`)
- **Reset to defaults** with confirmation

## Default keybinds (v1)

All editable in Settings → Keybinds; persisted as a profile in the DB.

| Key | Action |
|---|---|
| `Ctrl+←` / `Ctrl+→` | Previous / next tab |
| `Ctrl+1..6` | Jump directly to tab |
| `Ctrl+F` | Open global command palette |
| `Tab` / `Shift+Tab` | Cycle focus within current tab |
| `Enter` | Activate / drill into |
| `Esc` | Back / cancel / close palette |
| `/` | Inline filter in any list |
| `?` | Contextual help overlay |
| `Ctrl+D` | Detach focused tab/widget |
| `Ctrl+A` | Attach a previously detached widget |
| `Ctrl+R` | Reload current tab's data |
| `Ctrl+,` | Open Settings tab |
| `Ctrl+Q` | Quit |

## Theme system

**Built-in themes** (compiled into binary, customizable copies stored in DB):

1. **`cosmos`** (default): galaxy-dusky minimal
   - Background: `#0b0b14` (near-black with faint blue-purple tint)
   - Surface: `#13131f` (slightly lifted for panels)
   - Foreground: `#e6e6f0` (soft starlight white)
   - Muted: `#4a4a60` (dim blue-gray)
   - Accent primary: `#d44141` (deep red)
   - Accent success: `#a8d8e8` (pale cyan, like starlight)
   - Accent warning: `#e8b04a` (warm amber)
   - Accent error: `#ff5570`
   - Border: `#1f1f2e` with `✦` / `·` / `★` glyph accents in headers
2. **`void`**: pure black background, near-monochrome with red accent (for terminals where blue tints don't render well)
3. **`dusk`**: warmer dark with amber accents
4. **`cosmos-light`**: light variant for daytime work

**Custom themes**: created via Settings UI (saves to `themes` table) or imported from a palette file. The theme schema is just a color map + a few decorative glyphs.

## Storage schema (redb tables, logical)

All access goes through the `Store` trait — redb-specific code lives in `crates/sid-store/`. This means we could swap to a different KV store later without touching widget code.

| Table | Key | Value |
|---|---|---|
| `settings` | `&str` (setting key) | postcard-serialized `SettingValue` |
| `workspaces` | `&str` (absolute path) | `Workspace { name, type, manifest_hash, last_seen }` |
| `ssh_hosts` | `&str` (alias) | `SshHost { host, port, user, source }` |
| `db_connections` | `&str` (conn_id) | `DbConnection { kind, name, dsn, secret_ref }` |
| `query_history` | `(u64 ts_ns, u64 seq)` | `QueryRecord { conn_id, sql, duration_ms, row_count }` |
| `quick_actions` | `&str` (action_id) | `QuickAction { label, scope, cmd, keybind }` |
| `pinned_configs` | `&str` (path) | `PinnedConfig { label, opener_cmd }` |
| `themes` | `&str` (name) | `ThemeSpec { palette, glyphs }` |
| `keybinds` | `&str` (profile) | `KeybindProfile { bindings }` |
| `widget_state` | `(TabId, WidgetId)` | `Bytes` (postcard blob, version-prefixed) |
| `sessions` | `&str` (session_id) | `SessionRecord { started_at, ended_at, last_active, tabs, focus }` |
| `secrets` | `&str` (secret_id) | v1: `PlainSecret { value }`; v2: `KeyringRef { service, account }` |
| `processes` | `u32` (pid) | `RunningProcess { start_time, role, instance_ids[], last_heartbeat }` |

**Schema evolution**: postcard values are prefixed with a `u16` version byte. `Store` impl dispatches by version to the right deserializer. Migrations happen lazily on first-read.

`query_history` uses a `(ts_ns, seq)` composite key so reverse range scans give recent-first iteration without secondary indexes.

## File layout (intentionally tiny)

```
~/.config/sid/sid.toml          # OPTIONAL — only for DB path override
~/.local/share/sid/sid.redb     # everything else
~/.local/state/sid/main.sock    # IPC socket (detach)
~/.local/state/sid/crash-*.log  # panic logs (auto-rotated)
<workspace>/.sid/_metadata.sid  # OPTIONAL per-workspace overrides (JSON)
```

Honors XDG environment variables (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`).

## Performance principles

- **Sync render, async IO**: render loop never blocks. All slow work is on Tokio.
- **redb sync API + spawn_blocking**: redb itself is sync; wrap calls in `tokio::task::spawn_blocking` for the async runtime, then channel results back.
- **Service caching**: re-entering a tab is instant — widget state is in memory; data refreshes are background-triggered.
- **Debounced writes**: state changes batch into ~250ms windows.
- **Lazy widget construction**: a tab's widget is constructed on first activation, not at app startup.
- **Background polling rate-limited**: SysProbe runs at 2 s default; can be tuned in Settings.
- **Theme/palette resolution cached**: no per-cell hash lookups.
- **Bounded channels**: backpressure if a producer outpaces the consumer (prevents memory blowup on broken consumers).

## Cargo workspace layout

```
sid/
├── Cargo.toml                    # workspace manifest
├── crates/
│   ├── sid-core/                 # Widget trait, Layout, App, event loop, adapter trait defs
│   ├── sid-ui/                   # Ratatui helpers, Theme types, themed widgets
│   ├── sid-job/                  # JobQueue, async job lifecycle
│   ├── sid-ipc/                  # Unix socket protocol for detach
│   │
│   │ # ── adapter impls (each owns exactly one external dep) ───────
│   ├── sid-store/                # Store trait impls (RedbStore in v1)
│   ├── sid-secrets/              # SecretStore impls (PlainStore in v1; KeyringStore v2)
│   ├── sid-git/                  # GitProvider impls (Git2Provider in v1)
│   ├── sid-ssh/                  # SshClient + SFTP impls (russh-based)
│   ├── sid-pty/                  # PtyProvider impls (portable-pty + vt100)
│   ├── sid-db-clients/           # DbClient impls for the Database tab (Postgres, SQLite)
│   ├── sid-sys/                  # SysProvider impls (sysinfo + netstat2)
│   │
│   │ # ── widgets and binary ─────────────────────────────────────
│   ├── sid-widgets/              # All widgets (one module per tab)
│   │   ├── workspaces/
│   │   ├── ssh/                  # includes SFTP
│   │   ├── database/
│   │   ├── network/
│   │   ├── system/
│   │   └── settings/
│   └── sid/                      # binary entry point; `sid widget ...` subcommand
└── docs/superpowers/specs/
```

Each `sid-*` adapter crate has exactly one external dependency it wraps. Widgets in `sid-widgets/` never directly depend on `git2`, `russh`, `redb`, etc. — they depend on traits in `sid-core/`, and the binary crate `sid/` wires in concrete impls. This is the structural enforcement of the adapter pattern: it's not a convention, it's a Cargo.toml dependency graph.

## Testing strategy

- **Unit tests** in each crate (Store, services)
- **Snapshot tests** for widget rendering via `insta` (render a widget into a fixed buffer, snapshot it)
- **Integration tests** with a temp redb file (no FS or network)
- **No live SSH/DB in CI** — those use mocked transport
- **Property tests** for state serialization round-trips via `proptest`
- **Lint gate**: `cargo clippy -- -D warnings` in CI
- **Audit gate**: `cargo deny` in CI

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| redb file-format migration (v3 ⇒ v4 happened in 2025) | Pin to v4+; check for format on open; migration tool stub at startup |
| Multi-process write coordination via socket adds complexity | Keep socket interface narrow (single-key upserts only); no cross-process transactions |
| PTY + SSH on Windows quirky | russh + portable-pty support Windows but expect rough edges; v1 prioritizes Linux/macOS |
| Workspace auto-discovery scanning slow on large `~/vcs/` | Cap depth (default 2), parallelize via rayon, cache results between launches |
| Detach IPC protocol changes break running detached processes | Version the socket protocol; main rejects mismatched detached clients with a clear "please relaunch" message |

## Open items (resolve during planning)

- **Serialization**: confirm postcard (vs bincode) for state blobs — postcard's stronger schema-evolution story leans us toward it
- **SQL syntax highlight**: hand-rolled lexer vs `tree-sitter` for the query editor — recommend hand-rolled in v1; tree-sitter post-v1
- **Workspace discovery scan trigger**: at startup only, or also via file-watcher? — recommend startup-only + manual refresh in v1
- **Settings live-preview implementation**: re-render theme buffer on hover vs apply-temporarily — recommend re-render-on-hover
- **Crash log retention**: how many to keep, where to rotate — recommend last 10, auto-delete older

## Out of v1, but designed for

See `2026-05-20-sid-future-features.md` for the catalogue. The v1 architecture supports each non-trivially:

1. **Widget composition (v2)** — `Layout::Single` → `Layout::Split` (already on the enum)
2. **Detach** (already v1) — `launch_spec()` + `sid widget` subcommand + Unix socket
3. **Agent manager (v2)** — new `AgentService` reading `~/.claude/projects/*/`, plus an `Agents` tab; uses the same `Widget` trait
4. **Plugin loading (v2 or v3)** — `dyn-widgets/` dir, WASM or Rust dylib hosts; widgets implement the same trait
5. **Workspace-tree actions (v2)** — `ActionRegistry` already supports `scope: workspace-tree`; needs UI affordance only
6. **Keyring secrets (v2)** — `SecretStore` trait already abstracts; swap impl
7. **Hyprland-style spaces (v3?)** — current "tab list" generalizes to a "list of spaces, each holding a tab list" without ABI breakage
