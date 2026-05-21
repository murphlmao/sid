# Architecture

`sid` is a multi-crate Cargo workspace. The shape of the workspace is the
architecture: each external dependency lives in exactly one crate, hides
behind a trait defined in `sid-core`, and is wired into the binary at
startup. Widget code never names an external library; it names traits.

This document explains how the pieces fit together, why they are split the
way they are, and the conventions the code follows. For the original design
intent see [`docs/superpowers/specs/2026-05-20-sid-foundation-design.md`].

## Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  sid (binary)                                                   │
│   ├─ wire.rs: builds App, picks concrete impls                  │
│   ├─ runtime.rs: Tokio runtime + crossterm event source         │
│   └─ main.rs: CLI args, panic hook, tracing init                │
├─────────────────────────────────────────────────────────────────┤
│  sid-widgets (one module per tab)                               │
│   workspaces · ssh · database · network · system · settings     │
├─────────────────────────────────────────────────────────────────┤
│  sid-ui            sid-job             sid-store                │
│  themes,           JobQueue,           Store trait,             │
│  block helpers     JobHandle           RedbStore (redb)         │
├─────────────────────────────────────────────────────────────────┤
│  sid-core (abstractions — no Ratatui, no Tokio, no redb)        │
│   Widget · Layout · App · Tab · Action · Keybind · Palette      │
│   StatePersister · adapters::{git, ssh, pty, db_client, ...}    │
├─────────────────────────────────────────────────────────────────┤
│  Adapter impls — one external dep per crate                     │
│   sid-git (git2) · sid-ssh (russh) · sid-sys (sysinfo) · ...    │
└─────────────────────────────────────────────────────────────────┘
```

The thirteen crates planned for v1 are:

| Crate | Status | Purpose |
|:---|:---:|:---|
| `sid-core` | live | Abstractions, App, traits |
| `sid-ui` | live | Theme + Ratatui helpers |
| `sid-store` | live | `Store` trait, `RedbStore` |
| `sid-job` | live | `JobQueue` for async work |
| `sid-widgets` | live | The six v1 tabs |
| `sid-git` | live | `GitProvider` impl over `git2` |
| `sid` | live | Binary entry point + wiring |
| `sid-ssh` | planned | `SshClient` impl over `russh` (Plan 3) |
| `sid-pty` | planned | `PtyProvider` over `portable-pty` (Plan 3) |
| `sid-db-clients` | planned | `DbClient` impls (Plan 4) |
| `sid-sys` | planned | `SysProvider` over `sysinfo` + `netstat2` (Plan 5) |
| `sid-secrets` | planned | `SecretStore` impls (Plan 6/7) |
| `sid-ipc` | planned | Unix socket protocol for detach (Plan 8) |

## The cockpit metaphor

A cockpit is a single workspace with one focused panel at a time and
muscle-memory controls. That maps onto `sid` directly:

- **One tab on screen at a time.** No splits in v1. When you change tab,
  the previous one is gone — fresh slate for the new context. The
  cognitive load of "what was I looking at?" is zero.
- **Tabs are conceptually pinned panels.** The set is fixed in v1
  (Workspaces, SSH, Database, Network, System, Settings); each one is the
  long-form home for that concern.
- **Adapter pattern under everything.** A cockpit dial is just a face over
  whatever is actually measuring the world; you can swap the sensor
  without re-laminating the dashboard. `GitProvider`, `SshClient`,
  `Store`, `DbClient`, and the rest are dials. `Git2Provider`,
  `RedbStore`, etc., are sensors.

The `Widget` trait is the only contract a tab body has to satisfy. The
`Layout` enum is the shape a tab body can take — `Single` today, `Split`
tomorrow without changing widget code.

## Crate-by-crate

### `sid-core`

The abstractions crate. Defines `Widget`, `Layout`, `App`, `Tab`,
`TabManager`, `Action`, `ActionRegistry`, `KeyBinding`, `KeybindMap`,
`CommandPalette`, `StatePersister`, the `Event` type that wraps
crossterm's events, and the adapter traits.

Depends only on `crossterm` (for the `Event` type), `serde` (for
serializable IDs), `thiserror`, and `tracing`. Does **not** depend on
Ratatui, Tokio, or redb. Other crates are free to.

Key types:

- `Widget` — the trait every tab body implements. Methods: `id`,
  `title`, `render`, `handle_event`, `save_state`, `load_state`.
- `Layout` — `Single(Box<dyn Widget>)` or
  `Split { dir, ratio, a, b }`; v1 always constructs `Single`.
- `App` — owns the `TabManager`, `KeybindMap`, `ActionRegistry`, and
  `CommandPalette`. Drives event dispatch.
- `WidgetCtx` — the small context handed to a widget's `handle_event`;
  carries an action-emission channel and a redraw flag.
- `RenderTarget` — opaque render handle so widgets aren't tied to any
  specific TUI library at this layer.

### `sid-ui`

The Ratatui-aware layer. Defines `Theme` (palette + glyphs), the built-in
theme constants (`cosmos`, `void`, `dusk`, `cosmos-light`), and small
helpers like `styled_block`, `glyph_for`, paragraph builders.

Widgets use `sid-ui` for theme tokens and helpers but never construct
Ratatui frames or backends directly above the helpers layer.

### `sid-store`

The persistence layer. Defines the `Store` trait — the domain interface
for all persistent state — and `RedbStore`, the only v1 implementation.
This is the **only** crate that depends on `redb`. Also home to the
versioned-blob codec (`encode_versioned` / `decode_versioned`) used to
make per-table values forward-compatible.

Schema tables (logical): `settings`, `workspaces`, `ssh_hosts`,
`db_connections`, `query_history`, `quick_actions`, `pinned_configs`,
`themes`, `keybinds`, `widget_state`, `sessions`, `secrets`, `processes`.

### `sid-job`

A tiny async job queue. `JobQueue<T>` spawns Tokio tasks and exposes
two ways to collect results: a per-job `JobHandle::await_result` oneshot,
and `JobQueue::drain_completed` for the App's render loop to pick up
completions each frame.

Concurrency is the surface that matters here, so this crate is the home
of the loom tests (`#[cfg(loom)]`) for `Arc<Mutex<...>>` and
oneshot-handoff scenarios.

### `sid-widgets`

One module per tab: `workspaces`, `ssh`, `database`, `network`, `system`,
`settings`. Each module exposes a public `XxxWidget` struct implementing
`Widget`. Widgets call into provider traits (`GitProvider`, `SshClient`,
`SysProvider`, ...) injected from above; they do not `use git2::...` or
`use russh::...` directly. The Cargo dependency graph enforces this:
`sid-widgets`'s `Cargo.toml` cannot list `git2` and pass review.

In the foundation build, only the `workspaces` widget is real;
`ssh`/`database`/`network`/`system`/`settings` are the "coming soon" stub
backed by `sid_widgets::stub` until their plan lands.

### `sid-git`

The first adapter impl crate to land. Provides `Git2Provider`, which
implements the `GitProvider` trait from `sid-core::adapters::git` using
`git2` (libgit2 with the `vendored-libgit2` feature). Wired into the App
in `crates/sid/src/wire.rs`.

### `sid` (binary)

The entry point. Three files:

- `main.rs` — parses CLI args via `clap`, installs `color-eyre`, sets up
  `tracing`, opens the `RedbStore`, sets up the terminal (raw mode +
  alternate screen + crossterm backend), spawns the event source, and
  runs the event loop.
- `runtime.rs` — Tokio multi-threaded runtime, crossterm event-stream
  pump, periodic `Tick` events for the StatePersister and friends.
- `wire.rs` — builds the `App` with the six tabs pre-registered, picks
  concrete adapter impls (`Git2Provider`, `RedbStore`, ...), and
  contains the Ratatui draw loop.

This is the **only** crate that names every adapter implementation. If
another crate ends up naming a concrete impl, that's a design bug.

### Planned adapter crates

Each owns exactly one external dependency it wraps:

- `sid-ssh` — `russh` + `russh-sftp` for the SSH tab
- `sid-pty` — `portable-pty` + `vt100` for embedded PTYs
- `sid-db-clients` — `tokio-postgres` + `rusqlite` for the Database tab
- `sid-sys` — `sysinfo` + `netstat2` for the Network and System tabs
- `sid-secrets` — `keyring` (v2) or plain-in-DB (v1) for credentials
- `sid-ipc` — Unix socket protocol for detach (Plan 8)

Until those plans land, the trait shells live in
`sid-core/src/adapters/` so widget code can already program against the
right interface.

## Adapter pattern enforcement

Every external dependency hides behind a domain-shaped trait. This is not
a code-review convention — it is a Cargo dependency-graph constraint:

| Surface | Trait | v1 impl | Future impls |
|:---|:---|:---|:---|
| App-state storage | `Store` | `RedbStore` | `SqliteStore`, user plugins |
| Secret storage | `SecretStore` | `PlainStore` | `KeyringStore`, `EnvStore` |
| Git provider | `GitProvider` | `Git2Provider` | `GitoxideProvider`, `CliGitProvider` |
| SSH client | `SshClient` | `RussshClient` (planned) | `OpenSshForkClient`, mocked |
| PTY provider | `PtyProvider` | `PortablePtyProvider` (planned) | platform-specific |
| DB client | `DbClient` | `PostgresClient`, `SqliteClient` (planned) | MySQL, DuckDB, ClickHouse |
| System probe | `SysProvider` | `SysinfoProvider` (planned) | platform-specific, mocked |
| Notifier | `Notifier` | `ToastNotifier` | `OsNotifier`, `WebhookNotifier` |
| Clipboard | `Clipboard` | `ArboardClipboard` | `Osc52Clipboard` |

The structural rules:

1. **Widget code never names an external crate.** `crates/sid-widgets/`
   may `use sid_core::adapters::git::GitProvider`, never `use git2::*`.
2. **`sid-core` depends on no external runtime.** Specifically: no
   `ratatui`, no `tokio`, no `redb`. It owns `crossterm` because the
   `Event` type wraps `crossterm::event::Event`; that's the one exception.
3. **`sid-store` is the only crate that depends on `redb`.** Same shape
   for the other adapter crates: one external dep each.
4. **Concrete impls live in their own crate.** `Git2Provider` in
   `sid-git`, `RussshClient` in `sid-ssh`, and so on. The binary crate
   `sid` is the only place that ties a concrete impl to a trait slot.

Any PR that violates these rules fails review. No exceptions.

## Data flow

The path of a keystroke through `sid`:

```
crossterm Event
    │
    ▼ (in runtime.rs, on a Tokio task)
runtime::EventPump → Event::Key(chord) → mpsc::Sender
    │
    ▼ (received in wire::run_event_loop on the main task)
SidEvent::Key(chord)
    │
    ├──> CommandPalette open?
    │      └─ yes → palette handles input directly
    │
    ▼ no
KeybindMap::lookup(chord) → Some(ActionId) or None
    │
    ├──> Some(action_id)
    │      └─ App::dispatch(action_id) → action handler
    │            └─ side effects: TabManager::next/prev/jump,
    │                              CommandPalette::open,
    │                              JobQueue::spawn, ...
    │
    ▼ None (no global keybind matched)
ActiveTab::handle_event(ev, &mut ctx)
    │
    ├─ ctx.emit_action("...") → action channel → dispatched next loop
    ├─ ctx.request_redraw()    → mark dirty
    └─ EventOutcome::{Consumed, Bubble}
    │
    ▼
StatePersister::mark_dirty()  ← any widget state change requests this
    │  (debounced ~250 ms)
    ▼
Store::put_widget_state(blob)  → redb transaction
    │
    ▼
App::draw → Ratatui frame, every relevant tick or after a dirty event
```

The render loop is synchronous; nothing in `render()` blocks on I/O.
Anything slow goes through `JobQueue`. Job completions are picked up via
`JobQueue::drain_completed` at the start of each frame, before
`render()`, so the widget that spawned the job can pull in its result.

## State persistence

State lives in a single redb file. Default location is
`$XDG_DATA_HOME/sid/sid.redb`, which on Linux is
`~/.local/share/sid/sid.redb`. The path is overridable via `--db`.

The `Store` trait is the only surface widget and engine code talks to.
Concretely, `RedbStore` owns the redb `Database` handle and exposes:

- `current_session() / start_session() / heartbeat_session() / end_session()`
- `put_setting(key, value) / get_setting(key)`
- `put_widget_state(tab_id, widget_id, blob) / get_widget_state(...)`
- workspace methods landed in Plan 2: `add_workspace`, `list_workspaces`, `remove_workspace`
- Plan 3+ extensions: `ssh_hosts`, `db_connections`, `query_history`, ...

### Schema tables

The redb schema is a set of `TableDefinition` constants in
`sid-store::schema`. Each one maps an opaque key type to a postcard-
encoded value type.

| Table | Key | Value |
|:---|:---|:---|
| `settings` | `&str` | postcard `SettingValue` |
| `sessions` | `&str` | postcard `SessionRecord` |
| `workspaces` | `&str` (canonical path) | postcard `Workspace` |
| `widget_state` | `(&str, &str)` (tab_id, widget_id) | postcard blob, version-prefixed |
| `query_history` | `(u64, u64)` (ts_ns, seq) | postcard `QueryRecord` |
| `keybinds` | `&str` (profile) | postcard `KeybindProfile` |
| `themes` | `&str` (theme name) | postcard `ThemeSpec` |
| `processes` | `u32` (pid) | postcard `RunningProcess` |
| `secrets` | `&str` (secret_id) | postcard `PlainSecret` (v2: `KeyringRef`) |

Every postcard blob carries a `u8` version byte as its first byte; see
[Storage migrations](DEVELOPMENT.md#storage-migrations) for the convention.

### Session restore

Every active widget can call `ctx.persister.mark_dirty()` when its
state changes. `StatePersister` batches these on a ~250 ms debounce and
flushes them as a single transaction to redb.

`SessionManager` writes a session heartbeat (`last_active`,
`active_tab_id`, ...) every 5 s. On clean shutdown the session is marked
`ended_at`.

On launch, the binary looks at `Store::current_session()`. If a session
exists with `ended_at` unset and `last_active` within the configured
window (default 60 minutes), the user sees a "Resume session?" prompt
(`sid_core::restore::decide`). Pressing `Enter` reuses the session,
`N` starts fresh.

## The `Widget` trait + `Layout` enum

The v1 widget surface is intentionally minimal:

```rust
pub trait Widget: Send {
    fn id(&self) -> WidgetId;
    fn title(&self) -> &str;
    fn render(&self, target: &mut dyn RenderTarget);
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome;
    fn save_state(&self) -> Vec<u8> { Vec::new() }
    fn load_state(&mut self, _bytes: &[u8]) {}
}
```

`save_state` and `load_state` are how widgets participate in session
restore. `render` takes an opaque `RenderTarget` rather than a Ratatui
`Frame` directly — the Ratatui-aware impl lives in `sid-ui` and `wire.rs`.

`Layout` is the future-proofing seed:

```rust
pub enum Layout {
    Single(Box<dyn Widget>),
    Split { dir: Dir, ratio: f32, a: Box<Layout>, b: Box<Layout> },
}
```

v1 only constructs `Single`. The `Split` variant is on the enum so that
v2's Hyprland-style composition is a `wire.rs` change — not a widget
rewrite. Iterators (`iter_widgets`, `iter_widgets_mut`) already walk the
tree in order, so layout-aware code (focus traversal, persistence)
doesn't need rewriting either.

## Cosmos theme + theming surface

The default theme is `cosmos`. It's near-black with a faint blue-purple
tint, soft starlight white text, deep red accents, and `✦` / `·` / `★`
glyph accents in borders. The palette tokens are:

| Token | Hex | Use |
|:---|:---|:---|
| Background | `#0b0b14` | Frame fill |
| Surface | `#13131f` | Lifted panels |
| Foreground | `#e6e6f0` | Body text |
| Muted | `#4a4a60` | Secondary text |
| Accent primary | `#d44141` | Active tab, headings |
| Accent success | `#a8d8e8` | Status OK |
| Accent warning | `#e8b04a` | Status warn |
| Accent error | `#ff5570` | Status error |
| Border | `#1f1f2e` | Block borders |

Other built-in themes:

- `void` — pure black background, near-monochrome with red accent
- `dusk` — warmer dark with amber accents
- `cosmos-light` — light variant for daytime work

Custom themes live in the `themes` redb table. The schema is just a
color map plus a few decorative glyphs. See
[DEVELOPMENT.md → Theming](DEVELOPMENT.md#theming) for how to add one.

## Concurrency model

`sid` uses one Tokio multi-thread runtime with four worker threads
(`tokio::main(flavor = "multi_thread", worker_threads = 4)`):

- **Main task** runs the Ratatui render + event loop. This task is
  synchronous between awaits — it doesn't `.await` on anything that can
  block. The Ratatui `Frame` is drawn here.
- **Event source task** (`runtime::spawn_event_pump`) pumps
  crossterm events into a `tokio::sync::mpsc` channel and also sends
  periodic `Tick` events on a configurable interval (default 250 ms).
- **Job worker tasks** are spawned by `JobQueue::spawn` for any work
  that can't be synchronous — git operations, file I/O over large
  trees, future SSH/DB work. Results land in a shared completion buffer
  via `Arc<Mutex<Vec<...>>>` and on a per-handle oneshot.
- **StatePersister** debounces dirty marks on a Tokio interval and
  flushes them to redb via `tokio::task::spawn_blocking`, because redb's
  API is sync.

Bounded channels with explicit capacity are used everywhere a producer
could outpace a consumer — backpressure protects the App from memory
blowup if a widget pumps events faster than the loop drains them.

Loom tests cover the shared-state primitives (`Arc<Mutex<...>>`,
oneshot completion). They are gated behind `#[cfg(loom)]` and run with
`RUSTFLAGS="--cfg loom" cargo test --test loom_concurrency -p sid-job`.

## What this codebase deliberately doesn't do

The v1 architecture supports each of these as a non-breaking addition,
not a rewrite. See
[`docs/superpowers/specs/2026-05-20-sid-future-features.md`] for the
full catalogue.

- **Multi-widget composition within a tab.** v2. The `Layout::Split`
  variant is already on the enum.
- **Plugin loading.** v2 or v3. `Widget` is a trait, not a closed enum;
  a plugin loader can register more kinds at startup.
- **Agent manager (Claude Code session observer).** v2. New
  `AgentService` registers as an engine; new widget kind implements
  the same `Widget` trait.
- **Hyprland-style spaces above tabs.** v3. `TabManager` generalizes to
  a list-of-spaces without changing widgets.
- **Keyring secrets.** v2. The `SecretStore` trait already abstracts.
- **User-configurable storage backend.** v2. The `Store` trait already
  abstracts.
- **Detach via Unix socket.** Designed for v1 (Plan 8) but not yet wired.
- **Windows as first-class.** v3+. Linux is the primary target; macOS
  works; Windows is best-effort until then.

Anything that would require breaking the `Widget` trait, the `Store`
trait, or the adapter pattern is itself a design bug. The architecture
is the contract.

[`docs/superpowers/specs/2026-05-20-sid-foundation-design.md`]: superpowers/specs/2026-05-20-sid-foundation-design.md
[`docs/superpowers/specs/2026-05-20-sid-future-features.md`]: superpowers/specs/2026-05-20-sid-future-features.md
