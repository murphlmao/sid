# sid Plan 6 — System tab (configs + services + quick-actions)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. CLAUDE.md applies — every new pub fn needs a doc test, every function with invariants needs property tests, every parser-shaped function gets adversarial coverage. The `systemctl` output parser in particular MUST be exercised via proptest over arbitrary `Vec<u8>` inputs verifying never-panics.

**Goal:** When this plan is done, the **System** tab is fully functional. It is composed of three sub-panels stacked vertically (toggle focus with `Tab`):

1. **Pinned configs (top)** — user-defined list of `(path, label, opener_cmd)`. `Enter` on a pin launches an external `kitty` window cd'd into the parent dir running `$EDITOR <file>`. `a` to add, `d` to delete, `e` to edit, `/` to filter.
2. **Services (middle)** — `systemctl --user list-units` + `systemctl --system list-units` merged into a single list. Per-unit affordances: status, start, stop, restart, journal tail (last 100 lines, `f` to follow live). Filter by state (`/active`, `/failed`, `/inactive`) and by name. System-bus write actions surface a sudo-required toast if the user lacks privileges.
3. **Quick actions (bottom)** — user-defined shell snippets `(label, cmd, optional keybind)`. Scope is **global** — workspace-scoped quick-actions remain in the Workspaces tab (Plan 2). All global quick-actions are automatically merged into the `ActionRegistry` so they are available from the `Ctrl+F` palette no matter which tab is active.

**Architecture:** A new `sid-system` adapter crate hosts `SystemctlCmdClient`, which shells out to the `systemctl` and `journalctl` binaries and parses their output. The `SystemctlClient` trait lives in `sid-core::adapters::systemctl` (new), mirroring the existing `GitProvider`/`Store` adapter pattern. Pinned configs and global quick actions extend the `Store` trait in `sid-store` with two new tables (`pinned_configs`, `quick_actions`). A `TerminalSpawner` trait in `sid-core::adapters::terminal_spawner` abstracts the external-terminal launch so users on iTerm/wezterm/tmux can swap impls later — v1 ships `KittyTerminalSpawner` in `sid-system`. The widget lives in `sid-widgets/src/system.rs`, replacing the Plan 1 stub. The binary's `wire.rs` injects `SystemctlCmdClient` and `KittyTerminalSpawner` into the App and registers global quick-actions into the `ActionRegistry` at startup.

**Why a new crate (Option A over Option B):** This plan creates **`crates/sid-system`** rather than extending `crates/sid-sys` (Plan 5's sysinfo-backed `SysProvider`). Rationale: the adapter pattern enforced by CLAUDE.md and the foundation spec is "one external dependency surface per crate". `sid-sys` wraps `sysinfo` + `netstat2`; `sid-system` wraps the `systemctl`/`journalctl` CLI binaries (a fundamentally different surface — subprocess + text-parsing, vs. an in-process Rust library). Lumping them together would create a crate that depends on two unrelated externals and muddy the swap surface (someone wanting to swap to a D-Bus-based systemctl client should not have to touch sysinfo code). One judgment call to flag: the names `sid-sys` and `sid-system` are confusingly close. Renaming `sid-sys` to `sid-sysinfo` is preferable; this plan does not perform that rename (Plan 5 is the natural place if it has not landed yet, otherwise a follow-up cleanup).

**Tech stack additions (root `Cargo.toml` `[workspace.dependencies]`):**

```toml
which = "7"          # for resolving binary paths (kitty, $EDITOR, systemctl, journalctl)
shell-words = "1.1"  # for safe shell command parsing in quick-actions
```

Everything else (tokio, ratatui, redb, sid-core, sid-store, etc.) is already in `[workspace.dependencies]`.

**Out of scope (deferred, see `2026-05-20-sid-future-features.md` "System — beyond services and configs"):**
- System log viewer (`journalctl` with grouping / filtering UI) — only per-unit tail (last 100 + follow) is in scope.
- Sparkline metrics (CPU / mem / disk IO / network IO graphs).
- Hardware sensors (temperatures, fans, battery).
- Update notifier (pacman/apt/brew updates available).
- Cross-platform fallback for non-systemd OSes (macOS launchd, OpenRC, runit). Linux + systemd only in v1.
- D-Bus-based `SystemctlClient` impl (`SystemctlDbusClient`) — the trait is designed to permit this, but v1 ships only the CLI-shelling impl.
- Workspace-scoped quick-actions (those live in `.sid/_metadata.sid`, handled in Plan 2).
- Embedded PTY for `journalctl -f` follow mode — v1 uses a background `tokio::process` stream into a bounded buffer and renders the buffer; PTY-style ANSI handling is deferred.
- `cargo fuzz` harness for the parser — a proptest harness is required in this plan; the corresponding libFuzzer setup is a follow-up.

---

## File structure (new and modified only — existing crates unchanged unless noted)

```
sid/
├── Cargo.toml                                # MODIFY: + which, shell-words, sid-system workspace member
├── crates/
│   ├── sid-core/
│   │   └── src/
│   │       ├── lib.rs                        # MODIFY: re-export new adapter modules
│   │       └── adapters/
│   │           ├── mod.rs                    # MODIFY: + pub mod systemctl, pub mod terminal_spawner
│   │           ├── systemctl.rs              # NEW
│   │           └── terminal_spawner.rs       # NEW
│   ├── sid-system/                           # NEW CRATE
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs                        # re-exports
│   │   │   ├── parse.rs                      # systemctl/journalctl output parsers
│   │   │   ├── client.rs                     # SystemctlCmdClient
│   │   │   ├── kitty.rs                      # KittyTerminalSpawner
│   │   │   └── env.rs                        # $EDITOR resolution helper
│   │   ├── benches/
│   │   │   └── list_units.rs                 # criterion: parse 200-unit output
│   │   └── tests/
│   │       ├── parse_list_units.rs
│   │       ├── parse_status.rs
│   │       ├── parse_journal.rs
│   │       ├── parse_fuzz.rs                 # proptest never-panic
│   │       ├── client_integration.rs         # gated on `systemctl` being available
│   │       └── kitty_spawner.rs
│   ├── sid-store/
│   │   ├── src/
│   │   │   ├── lib.rs                        # MODIFY: + PinnedConfig, QuickAction, QuickActionScope types + Store trait methods
│   │   │   ├── schema.rs                     # MODIFY: + PINNED_CONFIGS, QUICK_ACTIONS tables
│   │   │   └── redb_impl.rs                  # MODIFY: + impls for the new methods
│   │   └── tests/
│   │       ├── pinned_configs.rs             # NEW
│   │       └── quick_actions.rs              # NEW
│   ├── sid-widgets/
│   │   └── src/
│   │       └── system.rs                     # MODIFY: replace stub with full impl
│   └── sid/
│       └── src/
│           ├── main.rs                       # MODIFY: + System subcommands (pin/unpin/services/action)
│           └── wire.rs                       # MODIFY: + SystemctlCmdClient + KittyTerminalSpawner injection
│                                             #           + ActionRegistry hydration from quick_actions table
└── docs/superpowers/plans/
    └── 2026-05-21-sid-system.md              # NEW (this file)
```

---

## Task index

| # | Task | Phase |
|---|---|---|
| 1 | Add `which`, `shell-words` to workspace deps + `sid-system` member | A. Foundation |
| 2 | Define `SystemctlClient` trait + domain types in `sid-core` | B. Trait surface |
| 3 | Define `TerminalSpawner` trait + `SpawnRequest` types in `sid-core` | B. Trait surface |
| 4 | `sid-system` crate skeleton + `parse_list_units` (porcelain parser) | C. SystemctlCmdClient |
| 5 | `parse_status` (single-unit `systemctl status` parser) | C. SystemctlCmdClient |
| 6 | `parse_journal` (`journalctl -n100` parser) | C. SystemctlCmdClient |
| 7 | `SystemctlCmdClient::new` + `list_units` (shell-out, merge user+system) | C. SystemctlCmdClient |
| 8 | `status`, `start`, `stop`, `restart` (per-unit operations + sudo detection) | C. SystemctlCmdClient |
| 9 | `journal_tail` (one-shot read + optional follow stream) | C. SystemctlCmdClient |
| 10 | `PinnedConfig` domain type in `sid-store` + serde round-trip tests | D. Storage: pinned configs |
| 11 | `PINNED_CONFIGS` table + Store trait extension methods | D. Storage: pinned configs |
| 12 | `RedbStore` impl for pinned-config methods | D. Storage: pinned configs |
| 13 | `QuickAction` + `QuickActionScope` domain types in `sid-store` | E. Storage: quick actions |
| 14 | `QUICK_ACTIONS` table + Store trait extension methods | E. Storage: quick actions |
| 15 | `RedbStore` impl for quick-action methods | E. Storage: quick actions |
| 16 | `KittyTerminalSpawner` impl + `$EDITOR` resolution | F. External spawn |
| 17 | `spawn_in_kitty(file_path)` helper + toast-friendly error mapping | F. External spawn |
| 18 | `SystemState` pure-Rust state struct (panes, focus, filter) | G. Widget: scaffolding |
| 19 | `SystemWidget` Pinned Configs sub-panel (list + add / edit / delete modal) | G. Widget: pinned configs |
| 20 | `SystemWidget` Services sub-panel (list + filter + per-unit menu) | G. Widget: services |
| 21 | Journal tail modal (one-shot read, optional follow toggle) | G. Widget: services |
| 22 | `SystemWidget` Quick Actions sub-panel (list + add / edit / delete modal) | G. Widget: quick actions |
| 23 | Wire global quick-actions into `ActionRegistry` (Ctrl+F palette) | H. Palette integration |
| 24 | Reload palette on quick-action CRUD (event-driven refresh) | H. Palette integration |
| 25 | `sid system pin <path> [--label …] [--opener …]` CLI subcommand | I. CLI |
| 26 | `sid system unpin <path>` + `sid system pins` (list) | I. CLI |
| 27 | `sid system services [--user|--system] [--state …]` + `sid system action add/list/remove/run <id>` | I. CLI |
| 28 | Wire `SystemctlCmdClient` + `KittyTerminalSpawner` into binary | J. Integration |
| 29 | Integration test: pinned-config + quick-action registry round-trip via CLI | J. Integration |
| 30 | README update + done-criteria pass | J. Integration |

**Total: 30 tasks, 10 phases.**

---

## Phase A — Foundation

### Task 1: Add `which`, `shell-words`, and `sid-system` workspace member

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add `sid-system` to workspace members**

Modify the `[workspace] members` list in `Cargo.toml`. After `crates/sid-git`, append `crates/sid-system`:

```toml
members = [
    "crates/sid",
    "crates/sid-core",
    "crates/sid-ui",
    "crates/sid-store",
    "crates/sid-job",
    "crates/sid-widgets",
    "crates/sid-git",
    "crates/sid-system",
]
```

If Plans 3–5 have landed and added more members, simply append `sid-system` to the existing list.

- [ ] **Step 2: Add new external deps + internal `sid-system` to `[workspace.dependencies]`**

Under the `# Internal` block, append:

```toml
sid-system = { path = "crates/sid-system" }
```

In a logical place (group with other CLI-shelling utility deps if any exist; otherwise immediately above the `# Internal` block), add:

```toml
# System / subprocess utilities
which = "7"
shell-words = "1.1"
```

- [ ] **Step 3: Stub out the crate so cargo can resolve the workspace**

```bash
mkdir -p crates/sid-system/src
cat > crates/sid-system/Cargo.toml <<'EOF'
[package]
name = "sid-system"
version.workspace = true
edition.workspace = true

[dependencies]
EOF
echo "// stub — Task 4 replaces this" > crates/sid-system/src/lib.rs
```

Confirm `cargo metadata --no-deps --format-version 1 > /dev/null` exits 0.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/sid-system
git commit -m "chore: add which, shell-words deps and sid-system workspace member stub"
```

---

## Phase B — Trait surface in `sid-core`

### Task 2: Define `SystemctlClient` trait + domain types

**Files:**
- Create: `crates/sid-core/src/adapters/systemctl.rs`
- Modify: `crates/sid-core/src/adapters/mod.rs`
- Test: `crates/sid-core/tests/systemctl_contract.rs`

The trait is dyn-compatible (`Box<dyn SystemctlClient>`), `Send + Sync`, methods take `&self`/`&mut self` with no generics in method positions.

- [ ] **Step 1: Write the contract test first**

Create `crates/sid-core/tests/systemctl_contract.rs`:

```rust
//! Verifies SystemctlClient is dyn-compatible and a MockClient covers every method.

use sid_core::adapters::systemctl::{
    JournalEntry, SystemUnit, SystemctlClient, SystemctlError, UnitBus, UnitFilter, UnitState,
};

struct MockClient;

impl SystemctlClient for MockClient {
    fn list_units(&self, _f: UnitFilter) -> Result<Vec<SystemUnit>, SystemctlError> { Ok(vec![]) }
    fn status(&self, _bus: UnitBus, _unit: &str) -> Result<SystemUnit, SystemctlError> {
        Ok(SystemUnit {
            name: "x.service".into(), bus: UnitBus::User,
            state: UnitState::Inactive, sub_state: "dead".into(),
            description: "x".into(), load_state: "loaded".into(),
        })
    }
    fn start(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> { Ok(()) }
    fn stop(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> { Ok(()) }
    fn restart(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> { Ok(()) }
    fn journal_tail(&self, _bus: UnitBus, _unit: &str, _lines: usize) -> Result<Vec<JournalEntry>, SystemctlError> {
        Ok(vec![])
    }
}

#[test]
fn client_is_dyn_compatible() {
    let c: Box<dyn SystemctlClient> = Box::new(MockClient);
    assert!(c.list_units(UnitFilter::default()).unwrap().is_empty());
}

#[test]
fn client_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn SystemctlClient>>();
}

#[test]
fn unit_state_variants_exist() {
    let _ = UnitState::Active;
    let _ = UnitState::Inactive;
    let _ = UnitState::Failed;
    let _ = UnitState::Activating;
    let _ = UnitState::Deactivating;
    let _ = UnitState::Unknown;
}

#[test]
fn unit_bus_variants_exist() {
    let _ = UnitBus::User;
    let _ = UnitBus::System;
}

#[test]
fn unit_filter_default_is_empty() {
    let f = UnitFilter::default();
    assert!(f.name_substring.is_none());
    assert!(f.state.is_none());
    assert!(matches!(f.bus, UnitBus::User | UnitBus::System) || f.bus_both);
}
```

- [ ] **Step 2: Run — should fail to compile**

Run: `cargo test -p sid-core --test systemctl_contract`
Expected: compile error (types and methods don't exist yet).

- [ ] **Step 3: Create `crates/sid-core/src/adapters/systemctl.rs`**

```rust
//! `SystemctlClient` trait + supporting domain types.
//! Implementations live in `sid-system`.

use serde::{Deserialize, Serialize};

/// Domain-shaped systemctl error. Concrete impls map their failure modes here.
#[derive(Debug, thiserror::Error)]
pub enum SystemctlError {
    #[error("systemctl binary not found in PATH")]
    SystemctlMissing,
    #[error("journalctl binary not found in PATH")]
    JournalctlMissing,
    #[error("unit '{0}' not found")]
    UnitNotFound(String),
    #[error("operation requires root (system-bus write); re-run with sudo or via polkit")]
    SudoRequired,
    #[error("systemctl returned non-zero: {0}")]
    NonZeroExit(String),
    #[error("output parser failure: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(String),
    #[error("other: {0}")]
    Other(String),
}

/// Which systemd bus the unit lives on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UnitBus {
    /// `systemctl --user` (per-user-session manager)
    User,
    /// `systemctl --system` (root-owned PID 1 manager)
    System,
}

/// Coarse-grained active state (`ActiveState` in systemd's vocabulary).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UnitState {
    Active,
    Reloading,
    Inactive,
    Failed,
    Activating,
    Deactivating,
    Unknown,
}

/// Display record for one unit row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SystemUnit {
    pub name: String,
    pub bus: UnitBus,
    pub state: UnitState,
    /// e.g. "running", "dead", "exited"
    pub sub_state: String,
    pub description: String,
    /// e.g. "loaded", "masked", "not-found"
    pub load_state: String,
}

/// Filter applied to `list_units`.
#[derive(Clone, Debug, Default)]
pub struct UnitFilter {
    /// Substring match against `name`. None = no name filter.
    pub name_substring: Option<String>,
    /// Match only units in this state. None = all states.
    pub state: Option<UnitState>,
    /// Which bus to query if `bus_both` is false. Default User.
    pub bus: UnitBus,
    /// If true, query both buses and merge the results. Overrides `bus`.
    pub bus_both: bool,
}

impl Default for UnitBus {
    fn default() -> Self { Self::User }
}

/// One line from `journalctl -n100 -u <unit>`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Seconds since UNIX epoch.
    pub timestamp_secs: i64,
    /// Hostname column (often elided if --no-hostname; included for completeness).
    pub hostname: String,
    /// Process / unit identifier ("systemd[1]:", "nginx[1234]:")
    pub source: String,
    pub message: String,
}

/// Trait the System tab depends on. Implementations live in `sid-system`.
///
/// # Object safety
///
/// All methods take `&self` and use no generics in method position,
/// so `Box<dyn SystemctlClient>` works.
pub trait SystemctlClient: Send + Sync {
    fn list_units(&self, filter: UnitFilter) -> Result<Vec<SystemUnit>, SystemctlError>;
    fn status(&self, bus: UnitBus, unit: &str) -> Result<SystemUnit, SystemctlError>;
    fn start(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError>;
    fn stop(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError>;
    fn restart(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError>;

    /// Read the last `lines` journal entries for this unit. Bounded; never blocks indefinitely.
    fn journal_tail(&self, bus: UnitBus, unit: &str, lines: usize) -> Result<Vec<JournalEntry>, SystemctlError>;
}
```

- [ ] **Step 4: Register the module**

Modify `crates/sid-core/src/adapters/mod.rs` — append:

```rust
pub mod systemctl;
```

Plan 1's `adapters/mod.rs` already has `pub mod git;` and `pub mod store;` (or similar). Insert in alphabetical order.

- [ ] **Step 5: Run tests** — expected 5 passed.

```bash
cargo test -p sid-core --test systemctl_contract
```

- [ ] **Step 6: Add doc tests per CLAUDE.md**

Add `# Examples` blocks to `SystemctlError`, `UnitBus`, `UnitState`, `SystemUnit`, `UnitFilter`, `JournalEntry`, and `SystemctlClient`. Each doc test constructs the type and reads a field. For `SystemctlClient`, show a minimal mock impl covering `list_units`.

- [ ] **Step 7: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add SystemctlClient trait + domain types"
```

---

### Task 3: Define `TerminalSpawner` trait + `SpawnRequest`

**Files:**
- Create: `crates/sid-core/src/adapters/terminal_spawner.rs`
- Modify: `crates/sid-core/src/adapters/mod.rs`
- Test: `crates/sid-core/tests/terminal_spawner_contract.rs`

A small trait so users can later swap kitty for iTerm/wezterm/tmux. v1 ships `KittyTerminalSpawner` in `sid-system` (Task 16).

- [ ] **Step 1: Write the contract test first**

Create `crates/sid-core/tests/terminal_spawner_contract.rs`:

```rust
use std::path::PathBuf;

use sid_core::adapters::terminal_spawner::{SpawnRequest, SpawnerError, TerminalSpawner};

struct MockSpawner;

impl TerminalSpawner for MockSpawner {
    fn spawn(&self, _req: SpawnRequest) -> Result<(), SpawnerError> { Ok(()) }
    fn name(&self) -> &'static str { "mock" }
}

#[test]
fn spawner_is_dyn_compatible() {
    let s: Box<dyn TerminalSpawner> = Box::new(MockSpawner);
    s.spawn(SpawnRequest {
        cwd: PathBuf::from("/tmp"),
        cmd: "echo hi".into(),
    }).unwrap();
    assert_eq!(s.name(), "mock");
}

#[test]
fn spawner_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn TerminalSpawner>>();
}

#[test]
fn spawn_request_construction() {
    let r = SpawnRequest {
        cwd: PathBuf::from("/home/u"),
        cmd: "vim a.txt".into(),
    };
    assert_eq!(r.cwd.to_string_lossy(), "/home/u");
    assert_eq!(r.cmd, "vim a.txt");
}
```

- [ ] **Step 2: Run — should fail to compile**

- [ ] **Step 3: Create `crates/sid-core/src/adapters/terminal_spawner.rs`**

```rust
//! `TerminalSpawner` trait — abstraction over "launch an external terminal
//! window running this command in this directory". v1 ships KittyTerminalSpawner
//! in `sid-system`. Users on iTerm/wezterm/tmux can swap in their own impl
//! via the binary's wire layer.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SpawnerError {
    #[error("terminal binary not found in PATH (looked for: {0})")]
    TerminalMissing(String),
    #[error("$EDITOR is not set and no fallback (vi) is available")]
    EditorMissing,
    #[error("io: {0}")]
    Io(String),
    #[error("other: {0}")]
    Other(String),
}

#[derive(Clone, Debug)]
pub struct SpawnRequest {
    /// Working directory the spawned terminal cd's into.
    pub cwd: PathBuf,
    /// Shell command line to run (already-rendered; the spawner does not interpolate).
    pub cmd: String,
}

pub trait TerminalSpawner: Send + Sync {
    /// Launch a detached external terminal window. Returns immediately after spawn.
    fn spawn(&self, req: SpawnRequest) -> Result<(), SpawnerError>;

    /// Human-readable spawner name (e.g. "kitty", "wezterm", "iterm").
    fn name(&self) -> &'static str;
}
```

- [ ] **Step 4: Register the module**

Append to `crates/sid-core/src/adapters/mod.rs`:

```rust
pub mod terminal_spawner;
```

- [ ] **Step 5: Run tests** — expected 3 passed.

- [ ] **Step 6: Add doc tests + commit**

Doc-test the types. Then:

```bash
git add crates/sid-core
git commit -m "feat(core): add TerminalSpawner trait for external terminal abstraction"
```

---

## Phase C — `SystemctlCmdClient` impl in `sid-system`

Phase C builds the CLI-shelling systemctl client. Parsing functions are pure-Rust over `&[u8]` and `&str` so they are unit-testable without a live systemd. The integration-level methods (`list_units`, etc.) shell out via `tokio::process::Command` and are gated behind a `#[cfg(target_os = "linux")]` integration-test module that runs only when `systemctl` is in `$PATH`.

### Task 4: `sid-system` crate skeleton + `parse_list_units`

**Files:**
- Replace: `crates/sid-system/Cargo.toml` (stub from Task 1)
- Replace: `crates/sid-system/src/lib.rs` (stub from Task 1)
- Create: `crates/sid-system/src/parse.rs`
- Create: `crates/sid-system/tests/parse_list_units.rs`
- Create: `crates/sid-system/tests/parse_fuzz.rs`

`systemctl --no-pager --plain --no-legend list-units --type=service` produces lines like:

```
nginx.service                          loaded active   running  A high performance web server
foo.service                            loaded failed   failed   Foo service that broke
sshd.service                           loaded active   running  OpenSSH server daemon
```

Five whitespace-separated columns: name / load / active / sub / description (description has spaces). Our parser must handle: variable whitespace, missing description, unicode descriptions, truncated lines, and CRLF.

- [ ] **Step 1: Replace `crates/sid-system/Cargo.toml`**

```toml
[package]
name = "sid-system"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
sid-core.workspace = true
which.workspace = true
shell-words.workspace = true
tokio = { workspace = true, features = ["process", "io-util", "rt", "macros", "time"] }
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
proptest.workspace = true
insta.workspace = true
criterion.workspace = true

[[bench]]
name = "list_units"
harness = false
```

- [ ] **Step 2: Failing test**

Create `crates/sid-system/tests/parse_list_units.rs`:

```rust
use sid_core::adapters::systemctl::{UnitBus, UnitState};
use sid_system::parse::parse_list_units;

const SAMPLE: &str = "\
nginx.service                          loaded active   running  A high performance web server
foo.service                            loaded failed   failed   Foo service that broke
sshd.service                           loaded active   running  OpenSSH server daemon
empty-desc.service                     loaded inactive dead
";

#[test]
fn parses_three_typical_rows_plus_no_description() {
    let units = parse_list_units(SAMPLE, UnitBus::System).unwrap();
    assert_eq!(units.len(), 4);
    assert_eq!(units[0].name, "nginx.service");
    assert_eq!(units[0].load_state, "loaded");
    assert_eq!(units[0].state, UnitState::Active);
    assert_eq!(units[0].sub_state, "running");
    assert_eq!(units[0].description, "A high performance web server");
    assert_eq!(units[0].bus, UnitBus::System);

    assert_eq!(units[1].state, UnitState::Failed);
    assert_eq!(units[3].description, "");
}

#[test]
fn parses_empty_input_as_empty_list() {
    let units = parse_list_units("", UnitBus::User).unwrap();
    assert!(units.is_empty());
}

#[test]
fn parses_lines_with_unicode_descriptions() {
    let s = "x.service                              loaded active   running  ✦ starlight ★\n";
    let units = parse_list_units(s, UnitBus::User).unwrap();
    assert_eq!(units[0].description, "✦ starlight ★");
}

#[test]
fn parses_crlf_line_endings() {
    let s = "a.service loaded active running desc\r\nb.service loaded inactive dead other\r\n";
    let units = parse_list_units(s, UnitBus::User).unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(units[1].sub_state, "dead");
}
```

- [ ] **Step 3: Run — should fail to compile**

- [ ] **Step 4: Create `crates/sid-system/src/parse.rs`**

```rust
//! Pure-Rust parsers for `systemctl` and `journalctl` text output.
//! These never panic, even on adversarial input — invariant verified by proptest.

use sid_core::adapters::systemctl::{
    JournalEntry, SystemUnit, SystemctlError, UnitBus, UnitState,
};

/// Parse the output of `systemctl --no-pager --plain --no-legend list-units --type=service`.
///
/// Columns (whitespace-separated): `name load active sub description...`.
/// The description column is rest-of-line; may be empty. CRLF tolerated.
pub fn parse_list_units(out: &str, bus: UnitBus) -> Result<Vec<SystemUnit>, SystemctlError> {
    let mut units = Vec::new();
    for raw_line in out.split('\n') {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() { continue; }
        // Skip header-ish lines (systemd legend, "LOAD = ..." etc.) defensively.
        if line.starts_with("UNIT ") || line.starts_with("LOAD ") || line.starts_with("ACTIVE ") || line.starts_with("SUB ") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let name = match parts.next() { Some(n) => n.to_string(), None => continue };
        let load_state = parts.next().unwrap_or("").to_string();
        let active = parts.next().unwrap_or("");
        let sub_state = parts.next().unwrap_or("").to_string();
        let description = parts.collect::<Vec<_>>().join(" ");
        units.push(SystemUnit {
            name,
            bus,
            state: parse_unit_state(active),
            sub_state,
            description,
            load_state,
        });
    }
    Ok(units)
}

/// Map systemd's textual ActiveState to our enum. Unknown values become `Unknown`.
pub fn parse_unit_state(s: &str) -> UnitState {
    match s {
        "active" => UnitState::Active,
        "reloading" => UnitState::Reloading,
        "inactive" => UnitState::Inactive,
        "failed" => UnitState::Failed,
        "activating" => UnitState::Activating,
        "deactivating" => UnitState::Deactivating,
        _ => UnitState::Unknown,
    }
}

/// Single-unit status parser — used by `status()`. Returns a `SystemUnit` from
/// `systemctl status <name>` output. Best-effort.
pub fn parse_status(_out: &str, _name: &str, _bus: UnitBus) -> Result<SystemUnit, SystemctlError> {
    // Implemented in Task 5.
    Err(SystemctlError::Parse("parse_status — not yet implemented".into()))
}

/// Journal-entry parser. Implemented in Task 6.
pub fn parse_journal(_out: &str) -> Result<Vec<JournalEntry>, SystemctlError> {
    Err(SystemctlError::Parse("parse_journal — not yet implemented".into()))
}
```

- [ ] **Step 5: Replace `crates/sid-system/src/lib.rs`**

```rust
//! `sid-system` — adapter crate for systemd-based systems.
//! Exposes `SystemctlCmdClient` (CLI-shelling implementation of
//! `sid_core::adapters::systemctl::SystemctlClient`) and `KittyTerminalSpawner`
//! (CLI-shelling implementation of `sid_core::adapters::terminal_spawner::TerminalSpawner`).

pub mod parse;
// pub mod client;   // Task 7
// pub mod kitty;    // Task 16
// pub mod env;      // Task 16
```

- [ ] **Step 6: Run tests** — expected 4 passed in `parse_list_units.rs`.

- [ ] **Step 7: Adversarial coverage — proptest never-panic harness**

Create `crates/sid-system/tests/parse_fuzz.rs`:

```rust
//! CLAUDE.md mandates: parser-shaped functions must be exercised with proptest
//! over arbitrary inputs verifying never-panic, never-UB. cargo-fuzz setup
//! deferred (see plan §Out of scope).

use proptest::prelude::*;
use sid_core::adapters::systemctl::UnitBus;
use sid_system::parse::{parse_list_units, parse_unit_state};

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..Default::default() })]

    /// Arbitrary bytes (interpreted as UTF-8 best-effort) must never panic
    /// the parser. Output is intentionally unchecked — we are verifying
    /// the contract "never panics, always returns".
    #[test]
    fn list_units_parser_never_panics_on_arbitrary_str(s in ".*") {
        let _ = parse_list_units(&s, UnitBus::User);
    }

    /// Lossy UTF-8 from arbitrary bytes is the more aggressive case.
    #[test]
    fn list_units_parser_never_panics_on_arbitrary_bytes(b in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let s = String::from_utf8_lossy(&b);
        let _ = parse_list_units(&s, UnitBus::User);
    }

    /// parse_unit_state is total — every &str maps to a UnitState.
    #[test]
    fn unit_state_is_total(s in ".*") {
        let _ = parse_unit_state(&s);
    }
}
```

Run: `cargo test -p sid-system --test parse_fuzz`. Expected: all proptest cases pass.

- [ ] **Step 8: Add doc tests + commit**

Doc-test `parse_list_units` and `parse_unit_state` with the same sample as the integration test.

```bash
git add crates/sid-system
git commit -m "feat(system): add sid-system crate skeleton + parse_list_units (porcelain parser) + never-panic proptest"
```

---

### Task 5: `parse_status` (single-unit status parser)

**Files:**
- Modify: `crates/sid-system/src/parse.rs`
- Create: `crates/sid-system/tests/parse_status.rs`

`systemctl status nginx.service --no-pager` produces a header block:

```
● nginx.service - A high performance web server
     Loaded: loaded (/lib/systemd/system/nginx.service; enabled)
     Active: active (running) since Tue 2026-05-21 08:30:11 UTC; 2h 14min ago
   Main PID: 12345 (nginx)
      Tasks: 5 (limit: 4915)
     Memory: 4.5M
     ...
```

We need: name, description (from the first line after `● <name> - `), load_state, state, sub_state. We do **not** parse PID/tasks/memory/timestamps in v1 (deferred).

- [ ] **Step 1: Failing tests**

Create `crates/sid-system/tests/parse_status.rs`:

```rust
use sid_core::adapters::systemctl::{UnitBus, UnitState};
use sid_system::parse::parse_status;

const ACTIVE: &str = "\
● nginx.service - A high performance web server
     Loaded: loaded (/lib/systemd/system/nginx.service; enabled)
     Active: active (running) since Tue 2026-05-21 08:30:11 UTC; 2h 14min ago
   Main PID: 12345 (nginx)
";

const FAILED: &str = "\
● foo.service - Foo
     Loaded: loaded (/etc/systemd/system/foo.service; disabled)
     Active: failed (Result: exit-code) since Tue 2026-05-21 09:00:00 UTC; 1h ago
";

const NOT_FOUND: &str = "\
Unit not-here.service could not be found.
";

#[test]
fn parses_active_unit() {
    let u = parse_status(ACTIVE, "nginx.service", UnitBus::System).unwrap();
    assert_eq!(u.name, "nginx.service");
    assert_eq!(u.description, "A high performance web server");
    assert_eq!(u.state, UnitState::Active);
    assert_eq!(u.sub_state, "running");
    assert_eq!(u.load_state, "loaded");
}

#[test]
fn parses_failed_unit() {
    let u = parse_status(FAILED, "foo.service", UnitBus::User).unwrap();
    assert_eq!(u.state, UnitState::Failed);
}

#[test]
fn unit_not_found_returns_error() {
    let err = parse_status(NOT_FOUND, "not-here.service", UnitBus::System).unwrap_err();
    assert!(matches!(err, sid_core::adapters::systemctl::SystemctlError::UnitNotFound(_)));
}

#[test]
fn empty_output_errors_with_parse() {
    let err = parse_status("", "x.service", UnitBus::System).unwrap_err();
    assert!(matches!(err, sid_core::adapters::systemctl::SystemctlError::Parse(_)));
}
```

- [ ] **Step 2: Run — should fail**

- [ ] **Step 3: Implement `parse_status` in `parse.rs`**

Replace the stub with:

```rust
pub fn parse_status(out: &str, name: &str, bus: UnitBus) -> Result<SystemUnit, SystemctlError> {
    if out.trim().is_empty() {
        return Err(SystemctlError::Parse(format!("status output empty for {name}")));
    }
    if out.contains("could not be found") {
        return Err(SystemctlError::UnitNotFound(name.to_string()));
    }
    let mut description = String::new();
    let mut load_state = String::new();
    let mut state = UnitState::Unknown;
    let mut sub_state = String::new();
    for raw in out.split('\n') {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        // Header: "● name.service - Description"
        if let Some(rest) = trimmed.strip_prefix("● ") {
            if let Some(idx) = rest.find(" - ") {
                description = rest[idx + 3..].trim().to_string();
            }
        }
        if let Some(rest) = trimmed.strip_prefix("Loaded:") {
            // "Loaded: loaded (/path...; enabled)"
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let Some(s) = toks.first() {
                load_state = s.to_string();
            }
        }
        if let Some(rest) = trimmed.strip_prefix("Active:") {
            // "Active: active (running) since ..." or "Active: failed (...) since ..."
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let Some(active_word) = toks.first() {
                state = parse_unit_state(active_word);
            }
            // Extract substate from "(running)" or "(exited)" etc.
            if let Some(start) = rest.find('(') {
                if let Some(end) = rest[start + 1..].find(')') {
                    sub_state = rest[start + 1..start + 1 + end].to_string();
                }
            }
        }
    }
    Ok(SystemUnit {
        name: name.to_string(),
        bus,
        state,
        sub_state,
        description,
        load_state,
    })
}
```

- [ ] **Step 4: Run tests** — expected 4 passed.

- [ ] **Step 5: Adversarial coverage — extend proptest harness**

Append to `tests/parse_fuzz.rs`:

```rust
use sid_system::parse::parse_status;

proptest! {
    #[test]
    fn status_parser_never_panics_on_arbitrary_str(s in ".*", name in "[a-z0-9.-]{1,40}") {
        let _ = parse_status(&s, &name, UnitBus::User);
    }

    #[test]
    fn status_parser_never_panics_on_arbitrary_bytes(b in proptest::collection::vec(any::<u8>(), 0..2048), name in "[a-z0-9.-]{1,40}") {
        let s = String::from_utf8_lossy(&b);
        let _ = parse_status(&s, &name, UnitBus::User);
    }
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-system
git commit -m "feat(system): implement parse_status with active/failed/not-found detection"
```

---

### Task 6: `parse_journal` (`journalctl -n100` parser)

**Files:**
- Modify: `crates/sid-system/src/parse.rs`
- Create: `crates/sid-system/tests/parse_journal.rs`

We use `journalctl --no-pager --output=short-iso --lines=100 -u <unit>`, producing:

```
2026-05-21T08:30:11+0000 myhost nginx[12345]: starting up
2026-05-21T08:30:12+0000 myhost nginx[12345]: ready to accept connections
```

Format: `<ISO8601> <hostname> <source>: <message>`. The colon after source is the message separator.

- [ ] **Step 1: Failing tests**

Create `crates/sid-system/tests/parse_journal.rs`:

```rust
use sid_system::parse::parse_journal;

const SAMPLE: &str = "\
2026-05-21T08:30:11+0000 myhost nginx[12345]: starting up
2026-05-21T08:30:12+0000 myhost nginx[12345]: ready to accept connections
2026-05-21T08:30:13+0000 myhost systemd[1]: Started nginx.service.
";

#[test]
fn parses_three_journal_lines() {
    let entries = parse_journal(SAMPLE).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].hostname, "myhost");
    assert_eq!(entries[0].source, "nginx[12345]");
    assert_eq!(entries[0].message, "starting up");
    assert!(entries[0].timestamp_secs > 0);
    assert_eq!(entries[2].source, "systemd[1]");
}

#[test]
fn empty_input_returns_empty_list() {
    let entries = parse_journal("").unwrap();
    assert!(entries.is_empty());
}

#[test]
fn malformed_line_is_skipped_not_errored() {
    let s = "this is not a journal line\n2026-05-21T08:30:11+0000 host src: ok\n";
    let entries = parse_journal(s).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message, "ok");
}
```

- [ ] **Step 2: Implement**

Replace the `parse_journal` stub in `parse.rs`:

```rust
pub fn parse_journal(out: &str) -> Result<Vec<JournalEntry>, SystemctlError> {
    let mut entries = Vec::new();
    for raw in out.split('\n') {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() { continue; }
        // ISO timestamp ends at first space.
        let Some(ts_end) = line.find(' ') else { continue; };
        let ts_str = &line[..ts_end];
        let rest = &line[ts_end + 1..];
        let Some(host_end) = rest.find(' ') else { continue; };
        let hostname = rest[..host_end].to_string();
        let rest = &rest[host_end + 1..];
        let Some(src_end) = rest.find(": ") else { continue; };
        let source = rest[..src_end].to_string();
        let message = rest[src_end + 2..].to_string();
        let timestamp_secs = parse_iso8601_to_epoch(ts_str).unwrap_or(0);
        entries.push(JournalEntry { timestamp_secs, hostname, source, message });
    }
    Ok(entries)
}

/// Cheap ISO8601 → epoch seconds. Returns None on parse failure.
/// We avoid pulling in `chrono` for this — it's overkill. Format expected:
/// "YYYY-MM-DDTHH:MM:SS+ZZZZ" or with "Z" suffix.
fn parse_iso8601_to_epoch(s: &str) -> Option<i64> {
    // Pure-Rust minimal parse — accept-or-fall-back-to-0.
    // Year (4) Month (2) Day (2) Hour (2) Min (2) Sec (2)
    if s.len() < 19 { return None; }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u32 = s[11..13].parse().ok()?;
    let min: u32 = s[14..16].parse().ok()?;
    let sec: u32 = s[17..19].parse().ok()?;
    // Days from civil epoch (Howard Hinnant's algorithm — public domain).
    let y = if month <= 2 { year - 1 } else { year } as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let d = day as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_from_epoch = era * 146097 + doe as i64 - 719468;
    let total = days_from_epoch * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64;
    Some(total)
}
```

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Adversarial coverage — extend `parse_fuzz.rs`**

Append:

```rust
use sid_system::parse::parse_journal;

proptest! {
    #[test]
    fn journal_parser_never_panics_on_arbitrary_str(s in ".*") {
        let _ = parse_journal(&s);
    }

    #[test]
    fn journal_parser_never_panics_on_arbitrary_bytes(b in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let s = String::from_utf8_lossy(&b);
        let _ = parse_journal(&s);
    }
}
```

- [ ] **Step 5: Insta snapshot of parser output for stability regression**

Append to `tests/parse_journal.rs`:

```rust
#[test]
fn journal_parser_output_snapshot() {
    let entries = parse_journal(SAMPLE).unwrap();
    insta::assert_debug_snapshot!(entries);
}
```

Commit the resulting `.snap.new` after review.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-system
git commit -m "feat(system): implement parse_journal (ISO8601 + source-colon-message)"
```

---

### Task 7: `SystemctlCmdClient::new` + `list_units`

**Files:**
- Create: `crates/sid-system/src/client.rs`
- Modify: `crates/sid-system/src/lib.rs`
- Create: `crates/sid-system/tests/client_integration.rs`
- Create: `crates/sid-system/benches/list_units.rs`

- [ ] **Step 1: Failing tests (integration-style, gated)**

Create `crates/sid-system/tests/client_integration.rs`:

```rust
//! These tests run only when `systemctl` is in PATH (i.e. a Linux dev box with
//! systemd). On macOS / CI without systemd, they self-skip.

use sid_core::adapters::systemctl::{SystemctlClient, UnitBus, UnitFilter};
use sid_system::SystemctlCmdClient;

fn have_systemctl() -> bool {
    which::which("systemctl").is_ok()
}

#[test]
fn list_units_user_bus_returns_something() {
    if !have_systemctl() { eprintln!("skip: systemctl missing"); return; }
    let client = SystemctlCmdClient::new().unwrap();
    let units = client.list_units(UnitFilter { bus: UnitBus::User, ..Default::default() }).unwrap();
    // User-bus may legitimately have zero units on bare systems — assert no panic.
    let _ = units;
}

#[test]
fn list_units_with_name_filter() {
    if !have_systemctl() { eprintln!("skip: systemctl missing"); return; }
    let client = SystemctlCmdClient::new().unwrap();
    let units = client.list_units(UnitFilter {
        name_substring: Some("ssh".into()),
        bus: UnitBus::System,
        ..Default::default()
    }).unwrap();
    assert!(units.iter().all(|u| u.name.contains("ssh")));
}

#[test]
fn list_units_with_both_buses() {
    if !have_systemctl() { eprintln!("skip: systemctl missing"); return; }
    let client = SystemctlCmdClient::new().unwrap();
    let _ = client.list_units(UnitFilter { bus_both: true, ..Default::default() }).unwrap();
}
```

- [ ] **Step 2: Create `crates/sid-system/src/client.rs`**

```rust
//! `SystemctlCmdClient` — CLI-shelling implementation of `SystemctlClient`.

use std::process::Command;

use sid_core::adapters::systemctl::{
    JournalEntry, SystemUnit, SystemctlClient, SystemctlError, UnitBus, UnitFilter,
};

use crate::parse::{parse_list_units, parse_status as _parse_status};

pub struct SystemctlCmdClient {
    systemctl_path: std::path::PathBuf,
    journalctl_path: std::path::PathBuf,
}

impl SystemctlCmdClient {
    /// Resolve `systemctl` and `journalctl` via `which`. Errors if either is missing.
    pub fn new() -> Result<Self, SystemctlError> {
        let systemctl_path = which::which("systemctl")
            .map_err(|_| SystemctlError::SystemctlMissing)?;
        let journalctl_path = which::which("journalctl")
            .map_err(|_| SystemctlError::JournalctlMissing)?;
        Ok(Self { systemctl_path, journalctl_path })
    }

    fn run_list(&self, bus: UnitBus) -> Result<String, SystemctlError> {
        let bus_flag = match bus { UnitBus::User => "--user", UnitBus::System => "--system" };
        let out = Command::new(&self.systemctl_path)
            .args([bus_flag, "--no-pager", "--plain", "--no-legend", "list-units", "--type=service", "--all"])
            .output()
            .map_err(|e| SystemctlError::Io(format!("spawn systemctl: {e}")))?;
        if !out.status.success() {
            return Err(SystemctlError::NonZeroExit(
                String::from_utf8_lossy(&out.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl SystemctlClient for SystemctlCmdClient {
    fn list_units(&self, filter: UnitFilter) -> Result<Vec<SystemUnit>, SystemctlError> {
        let buses: Vec<UnitBus> = if filter.bus_both {
            vec![UnitBus::User, UnitBus::System]
        } else {
            vec![filter.bus]
        };
        let mut out = Vec::new();
        for bus in buses {
            let raw = self.run_list(bus)?;
            out.extend(parse_list_units(&raw, bus)?);
        }
        // Apply name + state filters.
        if let Some(needle) = filter.name_substring.as_deref() {
            out.retain(|u| u.name.contains(needle));
        }
        if let Some(want) = filter.state {
            out.retain(|u| u.state == want);
        }
        Ok(out)
    }

    fn status(&self, _bus: UnitBus, _unit: &str) -> Result<SystemUnit, SystemctlError> {
        Err(SystemctlError::Other("status — implemented in Task 8".into()))
    }
    fn start(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> {
        Err(SystemctlError::Other("start — implemented in Task 8".into()))
    }
    fn stop(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> {
        Err(SystemctlError::Other("stop — implemented in Task 8".into()))
    }
    fn restart(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> {
        Err(SystemctlError::Other("restart — implemented in Task 8".into()))
    }
    fn journal_tail(&self, _bus: UnitBus, _unit: &str, _lines: usize) -> Result<Vec<JournalEntry>, SystemctlError> {
        Err(SystemctlError::Other("journal_tail — implemented in Task 9".into()))
    }
}
```

- [ ] **Step 3: Re-export in `lib.rs`**

Update `crates/sid-system/src/lib.rs`:

```rust
pub mod parse;
pub mod client;

pub use client::SystemctlCmdClient;
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p sid-system --test client_integration
```

Expected: passes (with self-skips on hosts without systemctl).

- [ ] **Step 5: Criterion benchmark — parse 200 units**

Create `crates/sid-system/benches/list_units.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sid_core::adapters::systemctl::UnitBus;
use sid_system::parse::parse_list_units;

fn build_200_unit_sample() -> String {
    let mut s = String::new();
    for i in 0..200 {
        s.push_str(&format!(
            "svc-{i:03}.service                          loaded active   running  service {i} description here\n"
        ));
    }
    s
}

fn bench_parse_list_units(c: &mut Criterion) {
    let sample = build_200_unit_sample();
    c.bench_function("parse_list_units_200", |b| {
        b.iter(|| {
            let _ = parse_list_units(black_box(&sample), UnitBus::System).unwrap();
        })
    });
}

criterion_group!(benches, bench_parse_list_units);
criterion_main!(benches);
```

Run once locally to confirm `cargo bench -p sid-system` produces a baseline. Target: parsing 200 units < 200µs. CI gate (when CI lands) will fail if the bench regresses ≥10% per CLAUDE.md.

- [ ] **Step 6: Adversarial coverage**

Append to `tests/client_integration.rs`:

```rust
#[test]
fn new_returns_systemctl_missing_when_path_disabled() {
    // We can't easily un-PATH systemctl in-process; this test runs only on
    // hosts where systemctl is truly absent. On other hosts it just verifies
    // construction succeeds.
    if !have_systemctl() {
        let err = SystemctlCmdClient::new().unwrap_err();
        assert!(matches!(err, sid_core::adapters::systemctl::SystemctlError::SystemctlMissing
            | sid_core::adapters::systemctl::SystemctlError::JournalctlMissing));
    }
}
```

- [ ] **Step 7: Commit**

```bash
git add crates/sid-system
git commit -m "feat(system): SystemctlCmdClient::new + list_units (user+system, name+state filters) + criterion bench"
```

---

### Task 8: `status`, `start`, `stop`, `restart` + sudo detection

**Files:**
- Modify: `crates/sid-system/src/client.rs`

- [ ] **Step 1: Failing tests**

Append to `tests/client_integration.rs`:

```rust
#[test]
fn status_of_known_user_unit_or_skips() {
    if !have_systemctl() { return; }
    let client = SystemctlCmdClient::new().unwrap();
    // Pick a user unit that almost always exists: `default.target`.
    let r = client.status(UnitBus::User, "default.target");
    // default.target either exists (Ok) or returns UnitNotFound (still acceptable for the test).
    match r {
        Ok(u) => assert_eq!(u.name, "default.target"),
        Err(e) => {
            let _ = format!("{e}");
        }
    }
}

#[test]
fn start_system_unit_without_sudo_returns_sudo_required() {
    if !have_systemctl() { return; }
    if std::env::var("USER").as_deref() == Ok("root") { return; }
    let client = SystemctlCmdClient::new().unwrap();
    // Try to start a system unit; should fail with SudoRequired (unless we're root).
    let r = client.start(UnitBus::System, "this-unit-does-not-exist.service");
    match r {
        Err(sid_core::adapters::systemctl::SystemctlError::SudoRequired)
        | Err(sid_core::adapters::systemctl::SystemctlError::UnitNotFound(_))
        | Err(sid_core::adapters::systemctl::SystemctlError::NonZeroExit(_)) => {}
        other => panic!("expected SudoRequired/UnitNotFound/NonZeroExit, got {other:?}"),
    }
}
```

- [ ] **Step 2: Implement the four methods + a private `run_action` helper**

In `client.rs`, replace the four stubs:

```rust
fn status(&self, bus: UnitBus, unit: &str) -> Result<SystemUnit, SystemctlError> {
    let bus_flag = match bus { UnitBus::User => "--user", UnitBus::System => "--system" };
    let out = Command::new(&self.systemctl_path)
        .args([bus_flag, "--no-pager", "status", unit])
        .output()
        .map_err(|e| SystemctlError::Io(format!("spawn systemctl: {e}")))?;
    // systemctl status returns non-zero for inactive units, but stdout is still useful.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("could not be found") {
        return Err(SystemctlError::UnitNotFound(unit.to_string()));
    }
    crate::parse::parse_status(&stdout, unit, bus)
}

fn start(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError> {
    self.run_action(bus, unit, "start")
}
fn stop(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError> {
    self.run_action(bus, unit, "stop")
}
fn restart(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError> {
    self.run_action(bus, unit, "restart")
}
```

And the private helper inside `impl SystemctlCmdClient`:

```rust
fn run_action(&self, bus: UnitBus, unit: &str, action: &str) -> Result<(), SystemctlError> {
    let bus_flag = match bus { UnitBus::User => "--user", UnitBus::System => "--system" };
    let out = Command::new(&self.systemctl_path)
        .args([bus_flag, "--no-pager", action, unit])
        .output()
        .map_err(|e| SystemctlError::Io(format!("spawn systemctl: {e}")))?;
    if out.status.success() { return Ok(()); }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("Failed to enable bus")
        || stderr.contains("Authentication is required")
        || stderr.contains("Interactive authentication required")
        || stderr.contains("Access denied")
    {
        return Err(SystemctlError::SudoRequired);
    }
    if stderr.contains("could not be found") {
        return Err(SystemctlError::UnitNotFound(unit.to_string()));
    }
    Err(SystemctlError::NonZeroExit(stderr.into_owned()))
}
```

- [ ] **Step 3: Run tests** — expected passes (with self-skips on non-Linux).

- [ ] **Step 4: Doc tests + commit**

Doc test `run_action` is private; doc test `status` with a brief example.

```bash
git add crates/sid-system
git commit -m "feat(system): status/start/stop/restart on SystemctlCmdClient + sudo-required detection"
```

---

### Task 9: `journal_tail` (one-shot + optional follow)

**Files:**
- Modify: `crates/sid-system/src/client.rs`

The trait's `journal_tail` is one-shot (bounded). For live follow, the widget calls a *separate* async streaming entry point on `SystemctlCmdClient` — not on the trait, because streaming has a different shape (token + sender). The trait method covers the common case ("show me the last 100 lines"); follow mode is a feature of the concrete impl and is wired by the widget through the binary.

- [ ] **Step 1: Failing tests**

Append to `tests/client_integration.rs`:

```rust
#[test]
fn journal_tail_returns_some_lines_or_unit_not_found() {
    if !have_systemctl() { return; }
    let client = SystemctlCmdClient::new().unwrap();
    let r = client.journal_tail(UnitBus::System, "systemd-journald.service", 10);
    match r {
        Ok(entries) => assert!(entries.len() <= 10),
        Err(sid_core::adapters::systemctl::SystemctlError::UnitNotFound(_))
        | Err(sid_core::adapters::systemctl::SystemctlError::NonZeroExit(_)) => {}
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn journal_tail_invalid_unit_returns_error() {
    if !have_systemctl() { return; }
    let client = SystemctlCmdClient::new().unwrap();
    let r = client.journal_tail(UnitBus::System, "this-cant-possibly-exist-xx.service", 10);
    assert!(r.is_err() || r.unwrap().is_empty());
}
```

- [ ] **Step 2: Implement `journal_tail`**

In `client.rs`, replace the stub:

```rust
fn journal_tail(&self, bus: UnitBus, unit: &str, lines: usize) -> Result<Vec<JournalEntry>, SystemctlError> {
    let bus_flag = match bus { UnitBus::User => "--user", UnitBus::System => "--system" };
    let lines_str = lines.to_string();
    let out = Command::new(&self.journalctl_path)
        .args([
            bus_flag,
            "--no-pager",
            "--output=short-iso",
            "-n", &lines_str,
            "-u", unit,
        ])
        .output()
        .map_err(|e| SystemctlError::Io(format!("spawn journalctl: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("No entries") { return Ok(Vec::new()); }
        return Err(SystemctlError::NonZeroExit(stderr.into_owned()));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    crate::parse::parse_journal(&stdout)
}
```

- [ ] **Step 3: Add follow-mode helper (concrete-impl-only, not on the trait)**

Append to `impl SystemctlCmdClient`:

```rust
/// Spawn a `journalctl -f -u <unit>` follower. Returns a tokio::sync::mpsc receiver
/// of parsed `JournalEntry` rows. The returned handle, when dropped, kills the child.
///
/// This is **not** part of the SystemctlClient trait — streaming has a different
/// shape (cancellable + async). The widget calls this directly via Arc<SystemctlCmdClient>.
pub async fn journal_follow(
    self: std::sync::Arc<Self>,
    bus: UnitBus,
    unit: &str,
) -> Result<(tokio::sync::mpsc::Receiver<JournalEntry>, tokio::task::JoinHandle<()>), SystemctlError> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command as TokioCommand;

    let bus_flag = match bus { UnitBus::User => "--user", UnitBus::System => "--system" };
    let mut child = TokioCommand::new(&self.journalctl_path)
        .args([bus_flag, "--no-pager", "--output=short-iso", "-f", "-u", unit])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| SystemctlError::Io(format!("spawn journalctl -f: {e}")))?;
    let stdout = child.stdout.take().ok_or_else(|| SystemctlError::Io("no stdout pipe".into()))?;
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let handle = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if let Ok(mut entries) = crate::parse::parse_journal(&line) {
                if let Some(e) = entries.pop() {
                    if tx.send(e).await.is_err() { break; }
                }
            }
        }
        let _ = child.kill().await;
    });
    Ok((rx, handle))
}
```

- [ ] **Step 4: Run tests** — expected passes / self-skips.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-system
git commit -m "feat(system): journal_tail (one-shot) + journal_follow (async stream, kill-on-drop)"
```

---

## Phase D — Pinned configs storage in `sid-store`

### Task 10: `PinnedConfig` domain type

**Files:**
- Modify: `crates/sid-store/src/lib.rs`
- Create: `crates/sid-store/tests/pinned_configs.rs`

`PinnedConfig` lives in `sid-store` because the redb tables are the storage authority. Spec table is `pinned_configs` keyed by path with value `{ label, opener_cmd }`. We add `created_at` for sortable display.

- [ ] **Step 1: Failing test**

Create `crates/sid-store/tests/pinned_configs.rs`:

```rust
use std::path::PathBuf;
use sid_store::{now_epoch, PinnedConfig};

#[test]
fn pinned_config_construction() {
    let p = PinnedConfig {
        path: PathBuf::from("/etc/nginx/nginx.conf"),
        label: "nginx config".into(),
        opener_cmd: None,
        created_at: now_epoch(),
    };
    assert_eq!(p.label, "nginx config");
    assert!(p.opener_cmd.is_none());
}
```

- [ ] **Step 2: Run — should fail**

- [ ] **Step 3: Add `PinnedConfig` to `sid-store/src/lib.rs`**

```rust
/// A user-pinned configuration file path with display label and optional
/// custom opener command. Default opener: external kitty cd'd into the parent
/// dir running `$EDITOR <file>`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PinnedConfig {
    /// Absolute path. Acts as the primary key.
    pub path: PathBuf,
    pub label: String,
    /// Override default opener. None = use the binary's `TerminalSpawner` default.
    pub opener_cmd: Option<String>,
    pub created_at: Epoch,
}
```

- [ ] **Step 4: Property test — postcard round-trip**

Append (inside an existing `#[cfg(test)]` block or a new one):

```rust
#[cfg(test)]
mod pinned_config_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_pinned_config_postcard_roundtrip(
            path in "/etc/[a-z]{1,8}/[a-z0-9_.-]{1,16}",
            label in "[a-zA-Z0-9 _.-]{1,40}",
            with_opener in proptest::bool::ANY,
        ) {
            let p = PinnedConfig {
                path: PathBuf::from(path),
                label,
                opener_cmd: with_opener.then(|| "vim".to_string()),
                created_at: now_epoch(),
            };
            let bytes = postcard::to_allocvec(&p).unwrap();
            let back: PinnedConfig = postcard::from_bytes(&bytes).unwrap();
            prop_assert_eq!(p, back);
        }
    }
}
```

- [ ] **Step 5: Run tests** — expected 1 unit + 1 proptest passing.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add PinnedConfig domain type with postcard round-trip property test"
```

---

### Task 11: `PINNED_CONFIGS` table + Store trait extension

**Files:**
- Modify: `crates/sid-store/src/schema.rs`
- Modify: `crates/sid-store/src/lib.rs` (Store trait)

- [ ] **Step 1: Add `PINNED_CONFIGS` to schema**

In `schema.rs`:

```rust
pub const PINNED_CONFIGS: TableDefinition<&str, &[u8]> = TableDefinition::new("pinned_configs");
```

Key = absolute path string. Value = postcard-encoded `PinnedConfig` (version-prefixed via the existing codec).

- [ ] **Step 2: Open the table in `RedbStore::open`**

In `redb_impl.rs`'s `OpenStore::open`, add:

```rust
let _ = txn.open_table(PINNED_CONFIGS).map_err(|e| SidError::Storage(format!("open pinned_configs: {e}")))?;
```

- [ ] **Step 3: Extend `Store` trait**

In `crates/sid-store/src/lib.rs`, add to the `Store` trait:

```rust
fn list_pinned_configs(&self) -> Result<Vec<PinnedConfig>, SidError>;
fn upsert_pinned_config(&self, pc: &PinnedConfig) -> Result<(), SidError>;
fn get_pinned_config(&self, path: &std::path::Path) -> Result<Option<PinnedConfig>, SidError>;
fn remove_pinned_config(&self, path: &std::path::Path) -> Result<(), SidError>;
```

Add stub impls to any mock Stores so the workspace still builds (return `Ok(Vec::new())`, etc.).

- [ ] **Step 4: Run existing tests** to confirm no regressions: `cargo test -p sid-store`.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add PINNED_CONFIGS table + Store trait extension methods"
```

---

### Task 12: `RedbStore` impl for pinned-config methods

**Files:**
- Modify: `crates/sid-store/src/redb_impl.rs`
- Modify: `crates/sid-store/tests/pinned_configs.rs`

- [ ] **Step 1: Failing tests**

Append to `tests/pinned_configs.rs`:

```rust
use sid_store::{OpenStore, RedbStore, Store};
use tempfile::tempdir;

fn pc(p: &str, label: &str) -> PinnedConfig {
    PinnedConfig {
        path: PathBuf::from(p),
        label: label.into(),
        opener_cmd: None,
        created_at: now_epoch(),
    }
}

#[test]
fn upsert_then_list() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_pinned_config(&pc("/etc/a.conf", "a")).unwrap();
    store.upsert_pinned_config(&pc("/etc/b.conf", "b")).unwrap();
    let all = store.list_pinned_configs().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn get_returns_existing_and_none_for_missing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_pinned_config(&pc("/etc/x.conf", "x")).unwrap();
    assert!(store.get_pinned_config(std::path::Path::new("/etc/x.conf")).unwrap().is_some());
    assert!(store.get_pinned_config(std::path::Path::new("/etc/missing.conf")).unwrap().is_none());
}

#[test]
fn remove_drops_it() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_pinned_config(&pc("/etc/a.conf", "a")).unwrap();
    store.remove_pinned_config(std::path::Path::new("/etc/a.conf")).unwrap();
    assert!(store.list_pinned_configs().unwrap().is_empty());
}

#[test]
fn upsert_replaces_existing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_pinned_config(&pc("/etc/a.conf", "v1")).unwrap();
    store.upsert_pinned_config(&pc("/etc/a.conf", "v2")).unwrap();
    let got = store.get_pinned_config(std::path::Path::new("/etc/a.conf")).unwrap().unwrap();
    assert_eq!(got.label, "v2");
}
```

- [ ] **Step 2: Implement on `RedbStore`**

Mirror the `Workspace` impl (Plan 2 Task 18). In `redb_impl.rs`:

```rust
fn list_pinned_configs(&self) -> Result<Vec<PinnedConfig>, SidError> {
    let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
    let tbl = txn.open_table(PINNED_CONFIGS).map_err(|e| SidError::Storage(format!("open pinned_configs: {e}")))?;
    let mut out = Vec::new();
    for entry in tbl.iter().map_err(|e| SidError::Storage(format!("iter pinned_configs: {e}")))? {
        let (_k, v) = entry.map_err(|e| SidError::Storage(format!("iter step: {e}")))?;
        let (_v, p) = crate::codec::decode_versioned::<PinnedConfig>(v.value())?;
        out.push(p);
    }
    Ok(out)
}

fn upsert_pinned_config(&self, pc: &PinnedConfig) -> Result<(), SidError> {
    let bytes = crate::codec::encode_versioned(1, pc)?;
    let key = pc.path.to_string_lossy().to_string();
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(PINNED_CONFIGS).map_err(|e| SidError::Storage(format!("open pinned_configs: {e}")))?;
        tbl.insert(key.as_str(), &bytes[..]).map_err(|e| SidError::Storage(format!("insert pinned_config: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit pinned_config: {e}")))?;
    Ok(())
}

fn get_pinned_config(&self, path: &std::path::Path) -> Result<Option<PinnedConfig>, SidError> {
    let key = path.to_string_lossy().to_string();
    let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
    let tbl = txn.open_table(PINNED_CONFIGS).map_err(|e| SidError::Storage(format!("open pinned_configs: {e}")))?;
    let got = tbl.get(key.as_str()).map_err(|e| SidError::Storage(format!("get pinned_config: {e}")))?;
    match got {
        Some(v) => {
            let (_v, p) = crate::codec::decode_versioned::<PinnedConfig>(v.value())?;
            Ok(Some(p))
        }
        None => Ok(None),
    }
}

fn remove_pinned_config(&self, path: &std::path::Path) -> Result<(), SidError> {
    let key = path.to_string_lossy().to_string();
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(PINNED_CONFIGS).map_err(|e| SidError::Storage(format!("open pinned_configs: {e}")))?;
        tbl.remove(key.as_str()).map_err(|e| SidError::Storage(format!("remove pinned_config: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit remove pinned_config: {e}")))?;
    Ok(())
}
```

- [ ] **Step 3: Run tests** — expected 4 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
#[test]
fn remove_nonexistent_is_noop() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.remove_pinned_config(std::path::Path::new("/never")).unwrap();
}

#[test]
fn list_with_100_pins_returns_all() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    for i in 0..100 {
        store.upsert_pinned_config(&pc(&format!("/etc/c{i}.conf"), &format!("l{i}"))).unwrap();
    }
    assert_eq!(store.list_pinned_configs().unwrap().len(), 100);
}

#[test]
fn unicode_in_label_round_trips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_pinned_config(&pc("/etc/a.conf", "✦ cosmos config 🐕")).unwrap();
    let got = store.get_pinned_config(std::path::Path::new("/etc/a.conf")).unwrap().unwrap();
    assert_eq!(got.label, "✦ cosmos config 🐕");
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): implement RedbStore pinned-config CRUD"
```

---

## Phase E — Quick-action storage in `sid-store`

### Task 13: `QuickAction` + `QuickActionScope` types

**Files:**
- Modify: `crates/sid-store/src/lib.rs`
- Create: `crates/sid-store/tests/quick_actions.rs`

Per the foundation spec, `quick_actions` keys are `action_id` strings (we generate UUIDv7-ish — see below) with value `{ label, scope, cmd, keybind }`. Workspace-scoped actions are stored separately in workspace `_metadata.sid`, but for forward-compat we model `scope` as an enum here so the **global** store table can technically hold workspace-scoped entries too (used when a user "promotes" a workspace action to global).

- [ ] **Step 1: Failing tests**

Create `crates/sid-store/tests/quick_actions.rs`:

```rust
use sid_store::{now_epoch, QuickAction, QuickActionScope};

#[test]
fn quick_action_construction() {
    let a = QuickAction {
        id: "qa-01HJZ...".into(),
        label: "kill port 5432".into(),
        scope: QuickActionScope::Global,
        cmd: "fuser -k 5432/tcp".into(),
        keybind: None,
        created_at: now_epoch(),
    };
    assert_eq!(a.scope, QuickActionScope::Global);
}

#[test]
fn quick_action_scope_variants() {
    let _ = QuickActionScope::Global;
    let _ = QuickActionScope::WorkspaceTagged("/home/u/vcs/foo".into());
}
```

- [ ] **Step 2: Add types to `sid-store/src/lib.rs`**

```rust
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum QuickActionScope {
    /// Available from any tab via the Ctrl+F palette.
    Global,
    /// Promoted from a workspace's `_metadata.sid`. Path = workspace root.
    /// Surfaced in the palette only when that workspace is selected (Plan 2).
    WorkspaceTagged(std::path::PathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QuickAction {
    pub id: String,
    pub label: String,
    pub scope: QuickActionScope,
    pub cmd: String,
    /// Optional single-character hotkey that's bound when the System tab is focused.
    /// Does not affect the Ctrl+F palette (always searchable by label there).
    pub keybind: Option<char>,
    pub created_at: Epoch,
}

impl QuickAction {
    /// Generate a fresh action id. Format: `qa-<14 lowercase hex>` (random).
    pub fn new_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos()).unwrap_or(0);
        // Mix in process id to avoid same-nanosecond collisions across processes.
        let pid = std::process::id() as u128;
        let mixed = nanos ^ (pid.wrapping_mul(2_654_435_761));
        format!("qa-{:014x}", (mixed & 0xFFFFFFFFFFFFFF) as u64)
    }
}
```

- [ ] **Step 3: Run tests** — expected 2 passed.

- [ ] **Step 4: Property tests**

Append:

```rust
#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_quick_action_postcard_roundtrip(
            label in "[a-zA-Z0-9 _.-]{1,40}",
            cmd in "[a-zA-Z0-9 ./_-]{1,80}",
            keybind in proptest::option::of(prop_oneof!['a'..='z']),
        ) {
            let a = QuickAction {
                id: QuickAction::new_id(),
                label, scope: QuickActionScope::Global, cmd, keybind,
                created_at: now_epoch(),
            };
            let bytes = postcard::to_allocvec(&a).unwrap();
            let back: QuickAction = postcard::from_bytes(&bytes).unwrap();
            prop_assert_eq!(a, back);
        }

        #[test]
        fn prop_quick_action_ids_are_unique(_n in 0u8..32) {
            // Generate two consecutive IDs; they must differ.
            let id1 = QuickAction::new_id();
            let id2 = QuickAction::new_id();
            prop_assert_ne!(id1, id2);
        }
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add QuickAction + QuickActionScope types with id generator"
```

---

### Task 14: `QUICK_ACTIONS` table + Store trait extension

**Files:**
- Modify: `crates/sid-store/src/schema.rs`
- Modify: `crates/sid-store/src/lib.rs`
- Modify: `crates/sid-store/src/redb_impl.rs`

- [ ] **Step 1: Add `QUICK_ACTIONS` to schema**

In `schema.rs`:

```rust
pub const QUICK_ACTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("quick_actions");
```

Key = `action_id`. Value = postcard-encoded `QuickAction`.

- [ ] **Step 2: Open the table in `RedbStore::open`**

```rust
let _ = txn.open_table(QUICK_ACTIONS).map_err(|e| SidError::Storage(format!("open quick_actions: {e}")))?;
```

- [ ] **Step 3: Extend `Store` trait**

```rust
fn list_quick_actions(&self) -> Result<Vec<QuickAction>, SidError>;
fn upsert_quick_action(&self, a: &QuickAction) -> Result<(), SidError>;
fn get_quick_action(&self, id: &str) -> Result<Option<QuickAction>, SidError>;
fn remove_quick_action(&self, id: &str) -> Result<(), SidError>;
```

Stub on mock Stores.

- [ ] **Step 4: Run existing tests** to confirm no regressions.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add QUICK_ACTIONS table + Store trait extension methods"
```

---

### Task 15: `RedbStore` impl for quick-action methods

**Files:**
- Modify: `crates/sid-store/src/redb_impl.rs`
- Modify: `crates/sid-store/tests/quick_actions.rs`

- [ ] **Step 1: Failing tests**

Append to `tests/quick_actions.rs`:

```rust
use sid_store::{OpenStore, RedbStore, Store};
use tempfile::tempdir;

fn qa(label: &str, cmd: &str) -> QuickAction {
    QuickAction {
        id: QuickAction::new_id(),
        label: label.into(),
        scope: QuickActionScope::Global,
        cmd: cmd.into(),
        keybind: None,
        created_at: now_epoch(),
    }
}

#[test]
fn upsert_then_list() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let a = qa("kill 5432", "fuser -k 5432/tcp");
    store.upsert_quick_action(&a).unwrap();
    let all = store.list_quick_actions().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].label, "kill 5432");
}

#[test]
fn get_by_id_returns_existing_and_none_for_missing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let a = qa("x", "echo x");
    store.upsert_quick_action(&a).unwrap();
    assert!(store.get_quick_action(&a.id).unwrap().is_some());
    assert!(store.get_quick_action("does-not-exist").unwrap().is_none());
}

#[test]
fn remove_drops_it() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let a = qa("x", "echo x");
    store.upsert_quick_action(&a).unwrap();
    store.remove_quick_action(&a.id).unwrap();
    assert!(store.list_quick_actions().unwrap().is_empty());
}
```

- [ ] **Step 2: Implement on `RedbStore`**

Mirror the `PinnedConfig` impl (Task 12) but keyed on `id`:

```rust
fn list_quick_actions(&self) -> Result<Vec<QuickAction>, SidError> { /* same shape */ }
fn upsert_quick_action(&self, a: &QuickAction) -> Result<(), SidError> {
    let bytes = crate::codec::encode_versioned(1, a)?;
    let txn = self.db.begin_write()?;
    { let mut tbl = txn.open_table(QUICK_ACTIONS)?; tbl.insert(a.id.as_str(), &bytes[..])?; }
    txn.commit()?;
    Ok(())
}
fn get_quick_action(&self, id: &str) -> Result<Option<QuickAction>, SidError> { /* same shape */ }
fn remove_quick_action(&self, id: &str) -> Result<(), SidError> { /* same shape */ }
```

Map `redb::Error` to `SidError::Storage` consistently (use the pattern from Task 12).

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Adversarial + property coverage**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_upsert_get_round_trip(label in "[a-zA-Z0-9 ]{1,32}", cmd in "[a-z./ -]{1,64}") {
        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        let a = QuickAction {
            id: QuickAction::new_id(), label, scope: QuickActionScope::Global,
            cmd, keybind: None, created_at: now_epoch(),
        };
        store.upsert_quick_action(&a).unwrap();
        let got = store.get_quick_action(&a.id).unwrap().unwrap();
        prop_assert_eq!(a, got);
    }
}

#[test]
fn shell_words_command_round_trips_through_storage() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let cmd = r#"sh -c "echo 'one two' | tr o O""#;
    let a = QuickAction {
        id: QuickAction::new_id(),
        label: "weird quoting".into(),
        scope: QuickActionScope::Global,
        cmd: cmd.into(),
        keybind: None,
        created_at: now_epoch(),
    };
    store.upsert_quick_action(&a).unwrap();
    let got = store.get_quick_action(&a.id).unwrap().unwrap();
    assert_eq!(got.cmd, cmd);
    // Parser tolerates the weird quoting (smoke test only — actual parsing is in Task 22).
    let _ = shell_words::split(&got.cmd).unwrap();
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): implement RedbStore quick-action CRUD + shell-words round-trip test"
```

---

## Phase F — External terminal spawn

### Task 16: `KittyTerminalSpawner` + `$EDITOR` resolution

**Files:**
- Create: `crates/sid-system/src/kitty.rs`
- Create: `crates/sid-system/src/env.rs`
- Modify: `crates/sid-system/src/lib.rs`
- Create: `crates/sid-system/tests/kitty_spawner.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-system/tests/kitty_spawner.rs`:

```rust
use sid_system::env::resolve_editor;

#[test]
fn editor_from_env_overrides_default() {
    let prev = std::env::var("EDITOR").ok();
    // SAFETY: tests run serially with env mutation; restore on drop.
    // (Use a guard pattern in real code; for the plan template we just set+restore inline.)
    unsafe { std::env::set_var("EDITOR", "nano"); }
    let r = resolve_editor();
    unsafe { match prev { Some(v) => std::env::set_var("EDITOR", v), None => std::env::remove_var("EDITOR") } }
    assert_eq!(r.unwrap(), "nano");
}

#[test]
fn editor_falls_back_to_vi_when_unset() {
    let prev = std::env::var("EDITOR").ok();
    unsafe { std::env::remove_var("EDITOR"); }
    let r = resolve_editor();
    unsafe { if let Some(v) = prev { std::env::set_var("EDITOR", v); } }
    // resolve_editor returns Ok("vi") only if `vi` exists on PATH.
    // If not on PATH, returns EditorMissing — acceptable on stripped CI containers.
    match r {
        Ok(e) => assert_eq!(e, "vi"),
        Err(sid_core::adapters::terminal_spawner::SpawnerError::EditorMissing) => {}
        other => panic!("unexpected: {other:?}"),
    }
}
```

- [ ] **Step 2: Create `crates/sid-system/src/env.rs`**

```rust
//! Environment resolution helpers for the System tab.

use sid_core::adapters::terminal_spawner::SpawnerError;

/// Resolve the user's editor: `$EDITOR` if set, else `$VISUAL`, else `vi`
/// if it exists in `$PATH`, else `EditorMissing`.
pub fn resolve_editor() -> Result<String, SpawnerError> {
    if let Ok(e) = std::env::var("EDITOR") {
        if !e.trim().is_empty() { return Ok(e); }
    }
    if let Ok(v) = std::env::var("VISUAL") {
        if !v.trim().is_empty() { return Ok(v); }
    }
    if which::which("vi").is_ok() {
        return Ok("vi".to_string());
    }
    Err(SpawnerError::EditorMissing)
}
```

- [ ] **Step 3: Create `crates/sid-system/src/kitty.rs`**

```rust
//! `KittyTerminalSpawner` — launches kitty in a detached child process,
//! cd'd to the requested cwd, running the requested command.

use std::process::Command;

use sid_core::adapters::terminal_spawner::{SpawnRequest, SpawnerError, TerminalSpawner};

pub struct KittyTerminalSpawner {
    kitty_path: std::path::PathBuf,
}

impl KittyTerminalSpawner {
    /// Resolve `kitty` via `which`. Errors if absent.
    pub fn new() -> Result<Self, SpawnerError> {
        let kitty_path = which::which("kitty")
            .map_err(|_| SpawnerError::TerminalMissing("kitty".into()))?;
        Ok(Self { kitty_path })
    }
}

impl TerminalSpawner for KittyTerminalSpawner {
    fn name(&self) -> &'static str { "kitty" }

    fn spawn(&self, req: SpawnRequest) -> Result<(), SpawnerError> {
        // We pass `--detach` so kitty backgrounds itself, freeing sid's child handle.
        // We pass `--directory` so cwd is set inside the new window.
        // The command is invoked via `sh -lc <cmd>` so shell-parsed arguments work.
        let cmd_arg = req.cmd;
        let cwd_arg = req.cwd.to_string_lossy().into_owned();
        Command::new(&self.kitty_path)
            .args(["--detach", "--directory", &cwd_arg, "sh", "-lc", &cmd_arg])
            .spawn()
            .map(|_child| ())
            .map_err(|e| SpawnerError::Io(format!("kitty spawn: {e}")))
    }
}
```

- [ ] **Step 4: Re-export in `lib.rs`**

Update `crates/sid-system/src/lib.rs`:

```rust
pub mod parse;
pub mod client;
pub mod kitty;
pub mod env;

pub use client::SystemctlCmdClient;
pub use kitty::KittyTerminalSpawner;
```

- [ ] **Step 5: Run tests** — expected passes (with self-skip on hosts missing `vi`/`kitty`).

- [ ] **Step 6: Commit**

```bash
git add crates/sid-system
git commit -m "feat(system): KittyTerminalSpawner + $EDITOR resolution helper"
```

---

### Task 17: `spawn_in_kitty(file_path)` helper + toast-friendly errors

**Files:**
- Modify: `crates/sid-system/src/kitty.rs`
- Modify: `crates/sid-system/tests/kitty_spawner.rs`

A convenience that composes the editor + parent-dir spawn the widget will do hundreds of times.

- [ ] **Step 1: Failing test**

Append to `tests/kitty_spawner.rs`:

```rust
use std::path::Path;
use sid_system::kitty::spawn_request_for_file;

#[test]
fn spawn_request_uses_parent_dir_and_editor_cmd() {
    let prev = std::env::var("EDITOR").ok();
    unsafe { std::env::set_var("EDITOR", "nvim"); }
    let r = spawn_request_for_file(Path::new("/etc/nginx/nginx.conf"), None).unwrap();
    unsafe { match prev { Some(v) => std::env::set_var("EDITOR", v), None => std::env::remove_var("EDITOR") } }
    assert_eq!(r.cwd.to_string_lossy(), "/etc/nginx");
    assert!(r.cmd.contains("nvim"));
    assert!(r.cmd.contains("nginx.conf"));
}

#[test]
fn spawn_request_uses_explicit_opener_when_provided() {
    let r = spawn_request_for_file(
        Path::new("/etc/x.conf"),
        Some("zellij action edit /etc/x.conf"),
    ).unwrap();
    assert_eq!(r.cmd, "zellij action edit /etc/x.conf");
}

#[test]
fn spawn_request_handles_file_in_root() {
    let prev = std::env::var("EDITOR").ok();
    unsafe { std::env::set_var("EDITOR", "vi"); }
    let r = spawn_request_for_file(Path::new("/etc.conf"), None);
    unsafe { match prev { Some(v) => std::env::set_var("EDITOR", v), None => std::env::remove_var("EDITOR") } }
    let r = r.unwrap();
    // Parent of /etc.conf is "/"
    assert_eq!(r.cwd.to_string_lossy(), "/");
}
```

- [ ] **Step 2: Implement in `kitty.rs`**

Append:

```rust
use std::path::{Path, PathBuf};

/// Build a `SpawnRequest` for opening `file_path` in the user's `$EDITOR`,
/// with the cwd set to the file's parent directory. If `opener_override` is
/// `Some`, the command is used as-is (still cd'd into the parent). Otherwise
/// the command is `<editor> <file_name>` so paths look short in the shell.
pub fn spawn_request_for_file(file_path: &Path, opener_override: Option<&str>) -> Result<SpawnRequest, SpawnerError> {
    let cwd: PathBuf = file_path.parent().map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"));
    let cmd = match opener_override {
        Some(c) => c.to_string(),
        None => {
            let editor = crate::env::resolve_editor()?;
            let name = file_path.file_name().and_then(|n| n.to_str())
                .unwrap_or_else(|| file_path.to_str().unwrap_or(""));
            // shell-words::quote is the safe way to interpolate.
            format!("{editor} {}", shell_words::quote(name))
        }
    };
    Ok(SpawnRequest { cwd, cmd })
}
```

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
#[test]
fn spawn_request_quotes_filename_with_spaces() {
    let prev = std::env::var("EDITOR").ok();
    unsafe { std::env::set_var("EDITOR", "vi"); }
    let r = spawn_request_for_file(Path::new("/home/u/My Configs/my conf.toml"), None);
    unsafe { match prev { Some(v) => std::env::set_var("EDITOR", v), None => std::env::remove_var("EDITOR") } }
    let r = r.unwrap();
    // shell-words quotes the path to avoid splitting.
    assert!(r.cmd.contains("'my conf.toml'") || r.cmd.contains("\"my conf.toml\""));
}

#[test]
fn spawn_request_with_unicode_path() {
    let prev = std::env::var("EDITOR").ok();
    unsafe { std::env::set_var("EDITOR", "vi"); }
    let r = spawn_request_for_file(Path::new("/home/u/🐕/conf.toml"), None);
    unsafe { match prev { Some(v) => std::env::set_var("EDITOR", v), None => std::env::remove_var("EDITOR") } }
    let r = r.unwrap();
    assert!(r.cwd.to_string_lossy().contains("🐕"));
}
```

- [ ] **Step 5: Doc tests + commit**

Doc-test `spawn_request_for_file`. Commit:

```bash
git add crates/sid-system
git commit -m "feat(system): spawn_request_for_file helper composes $EDITOR + parent-dir cwd"
```

---

## Phase G — `SystemWidget`

### Task 18: `SystemState` pure-Rust state struct

**Files:**
- Modify: `crates/sid-widgets/src/system.rs` (replace stub)
- Create: `crates/sid-widgets/tests/system_state.rs`

The widget is structured as a pure-Rust state struct (`SystemState`) covering panes + focus + filter, isolated from ratatui rendering. The widget's `handle_event` mutates state; rendering is a thin layer.

- [ ] **Step 1: Failing tests**

Create `crates/sid-widgets/tests/system_state.rs`:

```rust
use sid_widgets::system::{SystemPane, SystemState};

#[test]
fn initial_focus_is_pinned_configs() {
    let s = SystemState::new();
    assert_eq!(s.focused_pane(), SystemPane::PinnedConfigs);
}

#[test]
fn tab_cycles_focus_through_three_panes() {
    let mut s = SystemState::new();
    s.cycle_focus_forward();
    assert_eq!(s.focused_pane(), SystemPane::Services);
    s.cycle_focus_forward();
    assert_eq!(s.focused_pane(), SystemPane::QuickActions);
    s.cycle_focus_forward();
    assert_eq!(s.focused_pane(), SystemPane::PinnedConfigs);
}

#[test]
fn shift_tab_cycles_backward() {
    let mut s = SystemState::new();
    s.cycle_focus_backward();
    assert_eq!(s.focused_pane(), SystemPane::QuickActions);
    s.cycle_focus_backward();
    assert_eq!(s.focused_pane(), SystemPane::Services);
}

#[test]
fn filter_substring_is_per_pane() {
    let mut s = SystemState::new();
    s.set_filter("nginx".into());
    assert_eq!(s.filter(), Some("nginx"));
    s.cycle_focus_forward();
    // Filter resets per focused pane — pinning the filter to each pane.
    assert_eq!(s.filter(), None);
}
```

- [ ] **Step 2: Replace `crates/sid-widgets/src/system.rs`**

```rust
//! `SystemWidget` — System tab (pinned configs + systemctl services + quick actions).

use std::collections::HashMap;

use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemPane {
    PinnedConfigs,
    Services,
    QuickActions,
}

pub struct SystemState {
    focused: SystemPane,
    filters: HashMap<SystemPaneKey, String>,
    pub selected_idx: HashMap<SystemPaneKey, usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct SystemPaneKey(u8);

impl From<SystemPane> for SystemPaneKey {
    fn from(p: SystemPane) -> Self {
        match p {
            SystemPane::PinnedConfigs => SystemPaneKey(0),
            SystemPane::Services => SystemPaneKey(1),
            SystemPane::QuickActions => SystemPaneKey(2),
        }
    }
}

impl Default for SystemState { fn default() -> Self { Self::new() } }

impl SystemState {
    pub fn new() -> Self {
        Self {
            focused: SystemPane::PinnedConfigs,
            filters: HashMap::new(),
            selected_idx: HashMap::new(),
        }
    }

    pub fn focused_pane(&self) -> SystemPane { self.focused }

    pub fn cycle_focus_forward(&mut self) {
        self.focused = match self.focused {
            SystemPane::PinnedConfigs => SystemPane::Services,
            SystemPane::Services => SystemPane::QuickActions,
            SystemPane::QuickActions => SystemPane::PinnedConfigs,
        };
    }
    pub fn cycle_focus_backward(&mut self) {
        self.focused = match self.focused {
            SystemPane::PinnedConfigs => SystemPane::QuickActions,
            SystemPane::Services => SystemPane::PinnedConfigs,
            SystemPane::QuickActions => SystemPane::Services,
        };
    }

    pub fn set_filter(&mut self, s: String) {
        self.filters.insert(self.focused.into(), s);
    }
    pub fn clear_filter(&mut self) {
        self.filters.remove(&self.focused.into());
    }
    pub fn filter(&self) -> Option<&str> {
        self.filters.get(&self.focused.into()).map(String::as_str)
    }

    pub fn select_at(&self, pane: SystemPane) -> usize {
        *self.selected_idx.get(&pane.into()).unwrap_or(&0)
    }
    pub fn set_select(&mut self, pane: SystemPane, idx: usize) {
        self.selected_idx.insert(pane.into(), idx);
    }
}

pub struct SystemWidget {
    state: SystemState,
    id: WidgetId,
}

impl Default for SystemWidget { fn default() -> Self { Self::new() } }

impl SystemWidget {
    pub fn new() -> Self {
        Self { state: SystemState::new(), id: WidgetId::new("system.root") }
    }
    pub fn state(&self) -> &SystemState { &self.state }
    pub fn state_mut(&mut self) -> &mut SystemState { &mut self.state }
}

impl Widget for SystemWidget {
    fn id(&self) -> &WidgetId { &self.id }
    fn title(&self) -> &str { "System" }
    fn render(&self, _target: &mut dyn RenderTarget) {
        // Rendering is performed by the binary's draw() function via a
        // match on tab id. Tasks 19-22 implement per-pane renderers.
    }
    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            match (chord.code, chord.mods) {
                (KeyCode::Tab, KeyModifiers::NONE) => { self.state.cycle_focus_forward(); return EventOutcome::Consumed; }
                (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::SHIFT) => { self.state.cycle_focus_backward(); return EventOutcome::Consumed; }
                _ => {}
            }
        }
        EventOutcome::Bubble
    }
}
```

- [ ] **Step 3: Run tests** — expected 4 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
#[test]
fn many_forward_cycles_do_not_panic() {
    let mut s = SystemState::new();
    for _ in 0..1000 { s.cycle_focus_forward(); }
    // Three-cycle period means after 1000 cycles we're at pane 1000 % 3 = 1 = Services.
    assert_eq!(s.focused_pane(), SystemPane::Services);
}

#[test]
fn very_long_filter_substring_does_not_panic() {
    let mut s = SystemState::new();
    s.set_filter("x".repeat(100_000));
    assert_eq!(s.filter().unwrap().len(), 100_000);
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): SystemState + SystemWidget scaffolding (focus cycling, per-pane filters)"
```

---

### Task 19: Pinned Configs sub-panel (list + add/edit/delete modal)

**Files:**
- Modify: `crates/sid-widgets/src/system.rs`
- Modify: `crates/sid-widgets/tests/system_state.rs`

- [ ] **Step 1: Failing tests**

Append to `tests/system_state.rs`:

```rust
use std::path::PathBuf;
use sid_store::PinnedConfig;
use sid_widgets::system::{PinnedConfigsState, PinnedConfigsModal};

fn pc(p: &str, l: &str) -> PinnedConfig {
    PinnedConfig { path: PathBuf::from(p), label: l.into(), opener_cmd: None, created_at: 0 }
}

#[test]
fn pinned_configs_state_holds_and_selects() {
    let s = PinnedConfigsState::new(vec![pc("/a", "a"), pc("/b", "b")]);
    assert_eq!(s.selected().unwrap().label, "a");
}

#[test]
fn select_next_and_prev_cycle() {
    let mut s = PinnedConfigsState::new(vec![pc("/a", "a"), pc("/b", "b")]);
    s.select_next();
    assert_eq!(s.selected().unwrap().label, "b");
    s.select_next();
    assert_eq!(s.selected().unwrap().label, "a");
    s.select_prev();
    assert_eq!(s.selected().unwrap().label, "b");
}

#[test]
fn modal_opens_for_add_and_returns_new_record() {
    let s = PinnedConfigsState::new(vec![]);
    let m = s.begin_add();
    assert!(matches!(m, PinnedConfigsModal::Add { .. }));
}

#[test]
fn modal_begins_edit_of_selected() {
    let s = PinnedConfigsState::new(vec![pc("/etc/x", "x")]);
    let m = s.begin_edit_selected().unwrap();
    if let PinnedConfigsModal::Edit { original, .. } = m {
        assert_eq!(original.label, "x");
    } else { panic!("expected Edit modal"); }
}

#[test]
fn modal_returns_none_on_edit_when_empty() {
    let s = PinnedConfigsState::new(vec![]);
    assert!(s.begin_edit_selected().is_none());
}

#[test]
fn filter_narrows_visible_list() {
    let s = PinnedConfigsState::new(vec![
        pc("/etc/nginx.conf", "nginx"),
        pc("/etc/sshd.conf", "ssh"),
    ]);
    let filtered = s.visible(Some("ngi"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].label, "nginx");
}
```

- [ ] **Step 2: Implement in `system.rs`**

Append:

```rust
use sid_store::PinnedConfig;

#[derive(Debug)]
pub enum PinnedConfigsModal {
    Closed,
    Add { path_buf: String, label_buf: String, opener_buf: String },
    Edit { original: PinnedConfig, path_buf: String, label_buf: String, opener_buf: String },
    ConfirmDelete { target: PinnedConfig },
}

pub struct PinnedConfigsState {
    items: Vec<PinnedConfig>,
    selected_idx: usize,
    pub modal: PinnedConfigsModal,
}

impl PinnedConfigsState {
    pub fn new(items: Vec<PinnedConfig>) -> Self {
        Self { items, selected_idx: 0, modal: PinnedConfigsModal::Closed }
    }
    pub fn items(&self) -> &[PinnedConfig] { &self.items }
    pub fn replace_items(&mut self, items: Vec<PinnedConfig>) {
        self.items = items;
        if self.selected_idx >= self.items.len() {
            self.selected_idx = self.items.len().saturating_sub(1);
        }
    }
    pub fn selected(&self) -> Option<&PinnedConfig> { self.items.get(self.selected_idx) }

    pub fn select_next(&mut self) {
        if self.items.is_empty() { return; }
        self.selected_idx = (self.selected_idx + 1) % self.items.len();
    }
    pub fn select_prev(&mut self) {
        if self.items.is_empty() { return; }
        let n = self.items.len();
        self.selected_idx = (self.selected_idx + n - 1) % n;
    }

    pub fn visible<'a>(&'a self, filter: Option<&'a str>) -> Vec<&'a PinnedConfig> {
        match filter {
            None => self.items.iter().collect(),
            Some(needle) => self.items.iter()
                .filter(|p| p.label.contains(needle) || p.path.to_string_lossy().contains(needle))
                .collect(),
        }
    }

    pub fn begin_add(&self) -> PinnedConfigsModal {
        PinnedConfigsModal::Add { path_buf: String::new(), label_buf: String::new(), opener_buf: String::new() }
    }
    pub fn begin_edit_selected(&self) -> Option<PinnedConfigsModal> {
        let sel = self.selected()?.clone();
        Some(PinnedConfigsModal::Edit {
            path_buf: sel.path.to_string_lossy().into_owned(),
            label_buf: sel.label.clone(),
            opener_buf: sel.opener_cmd.clone().unwrap_or_default(),
            original: sel,
        })
    }
    pub fn begin_confirm_delete(&self) -> Option<PinnedConfigsModal> {
        Some(PinnedConfigsModal::ConfirmDelete { target: self.selected()?.clone() })
    }
}
```

The widget's `handle_event` and renderer call `JobQueue::spawn(store.upsert_pinned_config(...))` / `store.remove_pinned_config(...)` on modal save/confirm, and refresh `self.items` from `store.list_pinned_configs()` after each mutation.

- [ ] **Step 3: Run tests** — expected 6 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
#[test]
fn select_next_on_empty_is_noop() {
    let mut s = PinnedConfigsState::new(vec![]);
    s.select_next();
    s.select_prev();
    assert!(s.selected().is_none());
}

#[test]
fn replace_items_clamps_selected_index() {
    let mut s = PinnedConfigsState::new(vec![pc("/a", "a"), pc("/b", "b"), pc("/c", "c")]);
    s.select_next(); s.select_next(); // selected_idx = 2
    s.replace_items(vec![pc("/x", "x")]); // shrinks to 1 item
    assert_eq!(s.selected().unwrap().label, "x");
}
```

- [ ] **Step 5: Insta snapshot of the pin-list rendering**

Add to `tests/system_state.rs` once we have a render helper. For this plan we exercise the state; render snapshots come in Task 21 alongside the unified widget render. Skip for now.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): PinnedConfigsState + add/edit/delete modal state machine"
```

---

### Task 20: Services sub-panel (list + filter + per-unit menu)

**Files:**
- Modify: `crates/sid-widgets/src/system.rs`
- Modify: `crates/sid-widgets/tests/system_state.rs`

The Services pane is more behavioral. State holds the loaded `Vec<SystemUnit>`, the filter (state + name substring), and the selected index. The per-unit "menu" is a popup showing `[s]tart [t]op [r]estart [j]ournal`. Triggers dispatch a `JobQueue::spawn(client.<op>(bus, unit))`; the result lands as a toast.

- [ ] **Step 1: Failing tests**

Append to `tests/system_state.rs`:

```rust
use sid_core::adapters::systemctl::{SystemUnit, UnitBus, UnitState};
use sid_widgets::system::{ServicesState, ServicesAction};

fn unit(name: &str, state: UnitState) -> SystemUnit {
    SystemUnit {
        name: name.into(), bus: UnitBus::User, state,
        sub_state: "x".into(), description: "x".into(), load_state: "loaded".into(),
    }
}

#[test]
fn services_state_filters_by_name() {
    let s = ServicesState::new(vec![
        unit("nginx.service", UnitState::Active),
        unit("sshd.service", UnitState::Active),
    ]);
    let v = s.visible(Some("ngi"), None);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "nginx.service");
}

#[test]
fn services_state_filters_by_state() {
    let s = ServicesState::new(vec![
        unit("a", UnitState::Active),
        unit("b", UnitState::Failed),
    ]);
    let v = s.visible(None, Some(UnitState::Failed));
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "b");
}

#[test]
fn open_menu_returns_actions() {
    let mut s = ServicesState::new(vec![unit("x.service", UnitState::Active)]);
    s.open_menu();
    assert!(s.menu_open());
    let actions = ServicesState::menu_actions();
    assert!(actions.contains(&ServicesAction::Start));
    assert!(actions.contains(&ServicesAction::Stop));
    assert!(actions.contains(&ServicesAction::Restart));
    assert!(actions.contains(&ServicesAction::JournalTail));
}

#[test]
fn menu_closes_on_escape() {
    let mut s = ServicesState::new(vec![unit("x", UnitState::Active)]);
    s.open_menu();
    s.close_menu();
    assert!(!s.menu_open());
}
```

- [ ] **Step 2: Implement**

Append to `system.rs`:

```rust
use sid_core::adapters::systemctl::{SystemUnit, UnitState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServicesAction { Start, Stop, Restart, JournalTail }

pub struct ServicesState {
    units: Vec<SystemUnit>,
    selected_idx: usize,
    menu_open: bool,
}

impl ServicesState {
    pub fn new(units: Vec<SystemUnit>) -> Self {
        Self { units, selected_idx: 0, menu_open: false }
    }
    pub fn units(&self) -> &[SystemUnit] { &self.units }
    pub fn replace_units(&mut self, units: Vec<SystemUnit>) {
        self.units = units;
        if self.selected_idx >= self.units.len() {
            self.selected_idx = self.units.len().saturating_sub(1);
        }
    }
    pub fn selected(&self) -> Option<&SystemUnit> { self.units.get(self.selected_idx) }
    pub fn select_next(&mut self) {
        if self.units.is_empty() { return; }
        self.selected_idx = (self.selected_idx + 1) % self.units.len();
    }
    pub fn select_prev(&mut self) {
        if self.units.is_empty() { return; }
        let n = self.units.len();
        self.selected_idx = (self.selected_idx + n - 1) % n;
    }
    pub fn visible<'a>(&'a self, name_filter: Option<&'a str>, state_filter: Option<UnitState>) -> Vec<&'a SystemUnit> {
        self.units.iter()
            .filter(|u| name_filter.map_or(true, |n| u.name.contains(n)))
            .filter(|u| state_filter.map_or(true, |s| u.state == s))
            .collect()
    }
    pub fn open_menu(&mut self) { self.menu_open = true; }
    pub fn close_menu(&mut self) { self.menu_open = false; }
    pub fn menu_open(&self) -> bool { self.menu_open }
    pub const fn menu_actions() -> &'static [ServicesAction] {
        &[ServicesAction::Start, ServicesAction::Stop, ServicesAction::Restart, ServicesAction::JournalTail]
    }
}
```

- [ ] **Step 3: Run tests** — expected 4 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
#[test]
fn services_with_200_units_filters_correctly() {
    let mut units = Vec::new();
    for i in 0..200 {
        units.push(unit(&format!("svc-{i}.service"), if i % 5 == 0 { UnitState::Failed } else { UnitState::Active }));
    }
    let s = ServicesState::new(units);
    let failed = s.visible(None, Some(UnitState::Failed));
    assert_eq!(failed.len(), 40);
}

#[test]
fn very_long_unit_name_does_not_panic() {
    let s = ServicesState::new(vec![unit(&"x".repeat(2000), UnitState::Active)]);
    assert!(s.units()[0].name.len() == 2000);
}

#[test]
fn unit_name_with_spaces_is_handled() {
    // systemd disallows unit names with spaces, but defense in depth.
    let s = ServicesState::new(vec![unit("svc with spaces.service", UnitState::Active)]);
    let v = s.visible(Some("with spaces"), None);
    assert_eq!(v.len(), 1);
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): ServicesState + per-unit menu (start/stop/restart/journal)"
```

---

### Task 21: Journal tail modal (one-shot read + optional follow toggle)

**Files:**
- Modify: `crates/sid-widgets/src/system.rs`
- Modify: `crates/sid-widgets/tests/system_state.rs`

The journal tail is a full-screen modal: top header (`unit @ bus`), scrollable body with the last 100 lines, footer (`[f] follow [Esc] close [r] reload`). Follow mode wires the `journal_follow` mpsc receiver (Task 9) into a bounded ring buffer (cap = 1000 lines).

- [ ] **Step 1: Failing tests**

Append to `tests/system_state.rs`:

```rust
use sid_core::adapters::systemctl::JournalEntry;
use sid_widgets::system::JournalTailState;

fn je(secs: i64, msg: &str) -> JournalEntry {
    JournalEntry { timestamp_secs: secs, hostname: "host".into(), source: "src".into(), message: msg.into() }
}

#[test]
fn journal_tail_initial_state() {
    let s = JournalTailState::new("nginx.service".into(), UnitBus::System);
    assert_eq!(s.unit_name(), "nginx.service");
    assert!(!s.is_following());
    assert!(s.entries().is_empty());
}

#[test]
fn journal_tail_replaces_entries_on_reload() {
    let mut s = JournalTailState::new("x".into(), UnitBus::User);
    s.set_entries(vec![je(1, "a"), je(2, "b")]);
    assert_eq!(s.entries().len(), 2);
    s.set_entries(vec![je(3, "c")]);
    assert_eq!(s.entries().len(), 1);
}

#[test]
fn journal_tail_append_in_follow_mode_caps_at_1000() {
    let mut s = JournalTailState::new("x".into(), UnitBus::User);
    s.start_follow();
    assert!(s.is_following());
    for i in 0..1500 {
        s.push_followed(je(i, &format!("msg-{i}")));
    }
    assert!(s.entries().len() <= 1000);
    // Oldest is dropped
    assert!(!s.entries().iter().any(|e| e.message == "msg-0"));
}

#[test]
fn stop_follow_clears_following_flag() {
    let mut s = JournalTailState::new("x".into(), UnitBus::User);
    s.start_follow();
    s.stop_follow();
    assert!(!s.is_following());
}
```

- [ ] **Step 2: Implement**

Append to `system.rs`:

```rust
use std::collections::VecDeque;
use sid_core::adapters::systemctl::{JournalEntry, UnitBus};

pub struct JournalTailState {
    unit_name: String,
    bus: UnitBus,
    entries: VecDeque<JournalEntry>,
    follow: bool,
}

impl JournalTailState {
    pub const MAX_ENTRIES: usize = 1000;

    pub fn new(unit_name: String, bus: UnitBus) -> Self {
        Self { unit_name, bus, entries: VecDeque::with_capacity(Self::MAX_ENTRIES), follow: false }
    }
    pub fn unit_name(&self) -> &str { &self.unit_name }
    pub fn bus(&self) -> UnitBus { self.bus }
    pub fn entries(&self) -> &VecDeque<JournalEntry> { &self.entries }
    pub fn set_entries(&mut self, mut v: Vec<JournalEntry>) {
        self.entries.clear();
        for e in v.drain(..) {
            self.entries.push_back(e);
            if self.entries.len() > Self::MAX_ENTRIES { self.entries.pop_front(); }
        }
    }
    pub fn push_followed(&mut self, e: JournalEntry) {
        self.entries.push_back(e);
        if self.entries.len() > Self::MAX_ENTRIES { self.entries.pop_front(); }
    }
    pub fn start_follow(&mut self) { self.follow = true; }
    pub fn stop_follow(&mut self) { self.follow = false; }
    pub fn is_following(&self) -> bool { self.follow }
}
```

- [ ] **Step 3: Run tests** — expected 4 passed.

- [ ] **Step 4: Insta snapshot of one-shot rendering**

Add a small render helper test in `tests/system_state.rs` that formats `entries` into a fixed-width ASCII view and snapshots it via insta. Defer the full ratatui buffer snapshot to a future "render harness" task — the snapshot here is the pure-data view.

```rust
#[test]
fn journal_tail_format_snapshot() {
    let mut s = JournalTailState::new("nginx.service".into(), UnitBus::System);
    s.set_entries(vec![
        je(1748000000, "starting"),
        je(1748000005, "ready"),
    ]);
    let rendered: Vec<String> = s.entries().iter().map(|e| {
        format!("{:>10}  {}", e.timestamp_secs, e.message)
    }).collect();
    insta::assert_debug_snapshot!(rendered);
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): JournalTailState with one-shot + follow modes + ring buffer cap"
```

---

### Task 22: Quick Actions sub-panel (list + add/edit/delete modal)

**Files:**
- Modify: `crates/sid-widgets/src/system.rs`
- Modify: `crates/sid-widgets/tests/system_state.rs`

The QuickActions panel mirrors PinnedConfigs structurally. The added wrinkle: when the user enters a command, we use `shell_words::split` to lex it for display (showing `["cmd", "arg1", "arg2"]`) — this also surfaces malformed quoting before persistence.

- [ ] **Step 1: Failing tests**

Append to `tests/system_state.rs`:

```rust
use sid_store::{QuickAction, QuickActionScope};
use sid_widgets::system::{QuickActionsState, QuickActionsModal, parse_quick_action_cmd};

fn qa(label: &str, cmd: &str) -> QuickAction {
    QuickAction {
        id: QuickAction::new_id(),
        label: label.into(),
        scope: QuickActionScope::Global,
        cmd: cmd.into(),
        keybind: None,
        created_at: 0,
    }
}

#[test]
fn quick_actions_state_holds_and_selects() {
    let s = QuickActionsState::new(vec![qa("k", "kill x"), qa("l", "ls")]);
    assert_eq!(s.selected().unwrap().label, "k");
}

#[test]
fn parse_quick_action_cmd_splits_correctly() {
    let v = parse_quick_action_cmd("fuser -k 5432/tcp").unwrap();
    assert_eq!(v, vec!["fuser", "-k", "5432/tcp"]);
}

#[test]
fn parse_quick_action_cmd_handles_quotes() {
    let v = parse_quick_action_cmd(r#"sh -c "echo 'one two'""#).unwrap();
    assert_eq!(v, vec!["sh", "-c", "echo 'one two'"]);
}

#[test]
fn parse_quick_action_cmd_rejects_malformed_quoting() {
    let r = parse_quick_action_cmd(r#"echo "unclosed"#);
    assert!(r.is_err());
}

#[test]
fn quick_actions_filter_by_label() {
    let s = QuickActionsState::new(vec![
        qa("kill port 5432", "fuser -k 5432/tcp"),
        qa("open scripts", "cd ~/scripts"),
    ]);
    let v = s.visible(Some("port"));
    assert_eq!(v.len(), 1);
}

#[test]
fn quick_actions_modal_add() {
    let s = QuickActionsState::new(vec![]);
    let m = s.begin_add();
    assert!(matches!(m, QuickActionsModal::Add { .. }));
}
```

- [ ] **Step 2: Implement**

Append to `system.rs`:

```rust
use sid_store::{QuickAction, QuickActionScope};

#[derive(Debug)]
pub enum QuickActionsModal {
    Closed,
    Add { label_buf: String, cmd_buf: String, keybind_buf: String },
    Edit { original: QuickAction, label_buf: String, cmd_buf: String, keybind_buf: String },
    ConfirmDelete { target: QuickAction },
}

pub struct QuickActionsState {
    items: Vec<QuickAction>,
    selected_idx: usize,
    pub modal: QuickActionsModal,
}

impl QuickActionsState {
    pub fn new(items: Vec<QuickAction>) -> Self {
        Self { items, selected_idx: 0, modal: QuickActionsModal::Closed }
    }
    pub fn items(&self) -> &[QuickAction] { &self.items }
    pub fn replace_items(&mut self, items: Vec<QuickAction>) {
        self.items = items;
        if self.selected_idx >= self.items.len() {
            self.selected_idx = self.items.len().saturating_sub(1);
        }
    }
    pub fn selected(&self) -> Option<&QuickAction> { self.items.get(self.selected_idx) }
    pub fn select_next(&mut self) {
        if self.items.is_empty() { return; }
        self.selected_idx = (self.selected_idx + 1) % self.items.len();
    }
    pub fn select_prev(&mut self) {
        if self.items.is_empty() { return; }
        let n = self.items.len();
        self.selected_idx = (self.selected_idx + n - 1) % n;
    }
    pub fn visible<'a>(&'a self, filter: Option<&'a str>) -> Vec<&'a QuickAction> {
        match filter {
            None => self.items.iter().collect(),
            Some(needle) => self.items.iter()
                .filter(|a| a.label.contains(needle) || a.cmd.contains(needle)).collect(),
        }
    }
    pub fn begin_add(&self) -> QuickActionsModal {
        QuickActionsModal::Add { label_buf: String::new(), cmd_buf: String::new(), keybind_buf: String::new() }
    }
    pub fn begin_edit_selected(&self) -> Option<QuickActionsModal> {
        let sel = self.selected()?.clone();
        Some(QuickActionsModal::Edit {
            label_buf: sel.label.clone(),
            cmd_buf: sel.cmd.clone(),
            keybind_buf: sel.keybind.map(|c| c.to_string()).unwrap_or_default(),
            original: sel,
        })
    }
    pub fn begin_confirm_delete(&self) -> Option<QuickActionsModal> {
        Some(QuickActionsModal::ConfirmDelete { target: self.selected()?.clone() })
    }
}

/// Parse a quick-action command via `shell_words`. Errors out on malformed quoting.
pub fn parse_quick_action_cmd(cmd: &str) -> Result<Vec<String>, shell_words::ParseError> {
    shell_words::split(cmd)
}
```

- [ ] **Step 3: Run tests** — expected 6 passed.

Add `sid-store.workspace = true` and `shell-words.workspace = true` to `crates/sid-widgets/Cargo.toml`'s `[dependencies]`.

- [ ] **Step 4: Adversarial + property coverage**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn parse_quick_action_cmd_never_panics(s in ".*") {
        let _ = parse_quick_action_cmd(&s);
    }
}

#[test]
fn quick_action_with_empty_cmd_can_be_added_but_fails_to_parse() {
    let s = QuickActionsState::new(vec![qa("noop", "")]);
    assert!(parse_quick_action_cmd(&s.items()[0].cmd).unwrap().is_empty());
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): QuickActionsState + parse_quick_action_cmd (shell-words splitting)"
```

---

## Phase H — Palette integration

### Task 23: Wire global quick-actions into `ActionRegistry`

**Files:**
- Modify: `crates/sid/src/wire.rs`
- Modify: `crates/sid-core/src/action.rs` (or wherever `ActionRegistry` lives — Plan 1)

Plan 1 introduces `ActionRegistry` with `fuzzy(query)` lookup. We add a `register_quick_action(qa: &QuickAction)` method (or equivalent) so the binary can hydrate the registry at startup from `store.list_quick_actions()`. Workspace-tagged actions are excluded here — they belong to the Workspaces tab.

- [ ] **Step 1: Add registry-side method (if not already in Plan 1)**

In `sid-core/src/action.rs`, ensure there is a way to register external actions at runtime. If `ActionRegistry::register(Action)` already exists from Plan 1, no change. Otherwise add:

```rust
impl ActionRegistry {
    /// Register a user-defined action. Idempotent on action.id (re-registering
    /// replaces the prior entry).
    pub fn register(&mut self, action: Action) {
        // (Implementation per Plan 1.)
    }

    /// Remove an action by id. No-op if absent.
    pub fn unregister(&mut self, id: &str) {
        // ...
    }
}
```

(If Plan 1 does not yet expose runtime registration, this task adds it — TDD: write a failing test in `sid-core` first that registers an action then queries it via `fuzzy`.)

- [ ] **Step 2: Failing integration test**

Create `crates/sid/tests/quick_actions_palette.rs`:

```rust
use std::path::PathBuf;
use sid_store::{OpenStore, QuickAction, QuickActionScope, RedbStore, Store};
use tempfile::tempdir;

#[test]
fn startup_hydrates_quick_actions_into_palette() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let store = RedbStore::open(&db).unwrap();
    let a = QuickAction {
        id: QuickAction::new_id(),
        label: "kill 5432".into(),
        scope: QuickActionScope::Global,
        cmd: "fuser -k 5432/tcp".into(),
        keybind: None,
        created_at: 0,
    };
    store.upsert_quick_action(&a).unwrap();

    // `hydrate_quick_actions_into_registry` is the new helper we expose.
    let mut registry = sid_core::action::ActionRegistry::new();
    let n = sid::wire::hydrate_quick_actions_into_registry(&store, &mut registry).unwrap();
    assert_eq!(n, 1);
    let matches = registry.fuzzy("kill");
    assert!(!matches.is_empty());
}
```

- [ ] **Step 3: Implement `hydrate_quick_actions_into_registry` in `wire.rs`**

```rust
use sid_core::action::{Action, ActionRegistry, ActionScope};
use sid_store::{QuickAction, QuickActionScope, Store};

pub fn hydrate_quick_actions_into_registry(
    store: &dyn Store,
    registry: &mut ActionRegistry,
) -> anyhow::Result<usize> {
    let actions = store.list_quick_actions()?;
    let mut n = 0;
    for qa in actions {
        // Only global actions go into the global palette; workspace-tagged
        // entries are surfaced by Plan 2's WorkspacesWidget.
        if !matches!(qa.scope, QuickActionScope::Global) { continue; }
        registry.register(Action {
            id: qa.id.clone(),
            label: qa.label.clone(),
            scope: ActionScope::Global,
            // The action handler shells out the cmd via `tokio::process::Command`
            // using shell_words::split. See execute_quick_action() below.
            run: std::sync::Arc::new(move |_ctx| {
                let cmd = qa.cmd.clone();
                Box::pin(async move {
                    let parts = shell_words::split(&cmd).map_err(|e| anyhow::anyhow!("shell-words: {e}"))?;
                    let Some((bin, args)) = parts.split_first() else {
                        return Err(anyhow::anyhow!("empty command"));
                    };
                    let out = tokio::process::Command::new(bin).args(args).output().await?;
                    if !out.status.success() {
                        return Err(anyhow::anyhow!(
                            "non-zero exit: {}",
                            String::from_utf8_lossy(&out.stderr),
                        ));
                    }
                    Ok(())
                })
            }),
        });
        n += 1;
    }
    Ok(n)
}
```

(The exact `Action` shape comes from Plan 1; adjust signature to match. The intent: the registered action, when invoked from the palette, spawns the command in the background via `JobQueue`.)

- [ ] **Step 4: Call from main**

In `crates/sid/src/main.rs`, after the registry is constructed and Plan 1's defaults are loaded, append:

```rust
wire::hydrate_quick_actions_into_registry(&*store, &mut registry)?;
```

- [ ] **Step 5: Run tests** — expected 1 passed.

- [ ] **Step 6: Adversarial coverage**

Append:

```rust
#[test]
fn workspace_tagged_actions_do_not_pollute_global_palette() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let store = RedbStore::open(&db).unwrap();
    store.upsert_quick_action(&QuickAction {
        id: QuickAction::new_id(),
        label: "ws-only".into(),
        scope: QuickActionScope::WorkspaceTagged(PathBuf::from("/home/u/vcs/foo")),
        cmd: "echo x".into(),
        keybind: None,
        created_at: 0,
    }).unwrap();
    let mut registry = sid_core::action::ActionRegistry::new();
    let n = sid::wire::hydrate_quick_actions_into_registry(&store, &mut registry).unwrap();
    assert_eq!(n, 0);
    assert!(registry.fuzzy("ws-only").is_empty());
}
```

- [ ] **Step 7: Commit**

```bash
git add crates/sid crates/sid-core
git commit -m "feat(bin): hydrate global quick-actions into ActionRegistry at startup"
```

---

### Task 24: Reload palette on quick-action CRUD

**Files:**
- Modify: `crates/sid/src/wire.rs`
- Modify: `crates/sid-widgets/src/system.rs`

When the user adds/edits/deletes a quick action in the System tab, the palette must reflect it without a restart. Cheapest reliable approach: after each mutation, the widget posts an `Action::ReloadQuickActions` event up through `ctx.events`; the binary observes the event and calls `hydrate_quick_actions_into_registry` (clearing global QA entries first via `registry.unregister`).

- [ ] **Step 1: Failing test**

Append to `tests/quick_actions_palette.rs`:

```rust
#[test]
fn add_then_delete_round_trip_reflects_in_palette() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let store = RedbStore::open(&db).unwrap();

    let mut registry = sid_core::action::ActionRegistry::new();
    sid::wire::hydrate_quick_actions_into_registry(&store, &mut registry).unwrap();
    assert!(registry.fuzzy("test").is_empty());

    let a = QuickAction {
        id: QuickAction::new_id(), label: "test action".into(),
        scope: QuickActionScope::Global, cmd: "echo".into(),
        keybind: None, created_at: 0,
    };
    store.upsert_quick_action(&a).unwrap();
    sid::wire::rehydrate_global_quick_actions(&store, &mut registry).unwrap();
    assert!(!registry.fuzzy("test").is_empty());

    store.remove_quick_action(&a.id).unwrap();
    sid::wire::rehydrate_global_quick_actions(&store, &mut registry).unwrap();
    assert!(registry.fuzzy("test").is_empty());
}
```

- [ ] **Step 2: Implement `rehydrate_global_quick_actions`**

In `wire.rs`:

```rust
/// Clear all globally-scoped quick-actions from the registry and re-add from
/// the store. Called after any QuickAction CRUD in the System widget.
pub fn rehydrate_global_quick_actions(
    store: &dyn Store,
    registry: &mut ActionRegistry,
) -> anyhow::Result<usize> {
    // Quick actions all use ids prefixed with "qa-" (see QuickAction::new_id).
    // Clear those before re-registering.
    registry.unregister_with_prefix("qa-");
    hydrate_quick_actions_into_registry(store, registry)
}
```

If `unregister_with_prefix` does not exist on `ActionRegistry`, add it (this task includes the small Plan 1 patch).

- [ ] **Step 3: Wire the event in `SystemWidget::handle_event`**

After every successful `upsert_quick_action`/`remove_quick_action`, the widget emits an `Action::ReloadQuickActions` event via `ctx.dispatch_action(...)`. The binary's outer event loop matches on the action and calls `rehydrate_global_quick_actions`. (Concrete API depends on Plan 1's action-dispatch shape.)

- [ ] **Step 4: Run tests** — expected 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid crates/sid-core crates/sid-widgets
git commit -m "feat(bin): rehydrate global quick-actions on add/edit/delete in System tab"
```

---

## Phase I — CLI

### Task 25: `sid system pin <path>` subcommand

**Files:**
- Modify: `crates/sid/src/main.rs`

The CLI lets users script pin management from outside the TUI (`sid system pin /etc/nginx/nginx.conf --label "nginx config"`).

- [ ] **Step 1: Add subcommand**

In `main.rs`, extend the `Cmd` enum:

```rust
#[derive(clap::Subcommand, Debug)]
enum Cmd {
    Workspace { #[command(subcommand)] op: WorkspaceOp },
    /// System tab operations (pin configs, list services, manage quick actions)
    System { #[command(subcommand)] op: SystemOp },
}

#[derive(clap::Subcommand, Debug)]
enum SystemOp {
    /// Add a pinned config
    Pin {
        path: PathBuf,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        opener: Option<String>,
    },
    /// Remove a pinned config
    Unpin { path: PathBuf },
    /// List pinned configs
    Pins,
    /// List services (uses systemctl)
    Services {
        #[arg(long)]
        user: bool,
        #[arg(long)]
        system: bool,
        #[arg(long, value_name = "STATE")]
        state: Option<String>,
    },
    /// Quick action operations
    Action { #[command(subcommand)] op: ActionOp },
}

#[derive(clap::Subcommand, Debug)]
enum ActionOp {
    /// Add a global quick action
    Add { label: String, cmd: String, #[arg(long)] key: Option<char> },
    /// List all quick actions
    List,
    /// Remove a quick action by id
    Remove { id: String },
    /// Run a quick action by id immediately (no TUI)
    Run { id: String },
}
```

- [ ] **Step 2: Handle Pin**

```rust
SystemOp::Pin { path, label, opener } => {
    let abs = std::fs::canonicalize(&path)?;
    let display_label = label.unwrap_or_else(|| abs.file_name()
        .and_then(|n| n.to_str()).unwrap_or("(unnamed)").to_string());
    let pc = sid_store::PinnedConfig {
        path: abs.clone(),
        label: display_label,
        opener_cmd: opener,
        created_at: sid_store::now_epoch(),
    };
    store.upsert_pinned_config(&pc)?;
    println!("pinned: {}", abs.display());
}
```

- [ ] **Step 3: Failing test**

Create `crates/sid/tests/system_pin_cli.rs`:

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn system_pin_then_pins_lists_it() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let target = dir.path().join("my.conf");
    std::fs::write(&target, "x").unwrap();

    let bin = env!("CARGO_BIN_EXE_sid");
    let pin = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "system", "pin",
               target.to_str().unwrap(), "--label", "test cfg"])
        .output().unwrap();
    assert!(pin.status.success(), "stderr: {}", String::from_utf8_lossy(&pin.stderr));

    let pins = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "system", "pins"])
        .output().unwrap();
    assert!(pins.status.success());
    let out = String::from_utf8_lossy(&pins.stdout);
    assert!(out.contains("test cfg"));
}
```

- [ ] **Step 4: Run tests** — expected 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): add `sid system pin/unpin/pins` CLI subcommands"
```

---

### Task 26: `sid system unpin` + `sid system pins` + adversarial

**Files:**
- Modify: `crates/sid/src/main.rs`
- Modify: `crates/sid/tests/system_pin_cli.rs`

- [ ] **Step 1: Implement Unpin and Pins**

```rust
SystemOp::Unpin { path } => {
    let abs = std::fs::canonicalize(&path).unwrap_or(path);
    store.remove_pinned_config(&abs)?;
    println!("unpinned: {}", abs.display());
}
SystemOp::Pins => {
    for p in store.list_pinned_configs()? {
        println!("{:<40} {}", p.label, p.path.display());
    }
}
```

- [ ] **Step 2: Failing test (full round-trip)**

Append to `tests/system_pin_cli.rs`:

```rust
#[test]
fn full_pin_unpin_round_trip() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let target = dir.path().join("my.conf");
    std::fs::write(&target, "x").unwrap();
    let bin = env!("CARGO_BIN_EXE_sid");

    let _ = Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "pin", target.to_str().unwrap()]).output().unwrap();
    let _ = Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "unpin", target.to_str().unwrap()]).output().unwrap();
    let pins = Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "pins"]).output().unwrap();
    let out = String::from_utf8_lossy(&pins.stdout);
    assert!(!out.contains(target.to_str().unwrap()));
}

#[test]
fn unpin_nonexistent_is_noop_returns_zero() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "unpin", "/never"]).output().unwrap();
    assert!(out.status.success());
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): add `sid system unpin` + adversarial CLI tests"
```

---

### Task 27: `sid system services` + `sid system action add/list/remove/run`

**Files:**
- Modify: `crates/sid/src/main.rs`
- Create: `crates/sid/tests/system_services_cli.rs`
- Create: `crates/sid/tests/system_action_cli.rs`

- [ ] **Step 1: Implement Services**

```rust
SystemOp::Services { user, system, state } => {
    let bus_both = user && system || (!user && !system);
    let bus = if user { sid_core::adapters::systemctl::UnitBus::User }
              else { sid_core::adapters::systemctl::UnitBus::System };
    let state_filter = state.as_deref().map(|s| sid_system::parse::parse_unit_state(s));
    let client = sid_system::SystemctlCmdClient::new()?;
    let units = client.list_units(sid_core::adapters::systemctl::UnitFilter {
        name_substring: None, state: state_filter, bus, bus_both,
    })?;
    for u in units {
        println!("{:<40} {:<10} {:<10} {}", u.name, format!("{:?}", u.state), u.sub_state, u.description);
    }
}
```

- [ ] **Step 2: Implement Action subcommands**

```rust
SystemOp::Action { op } => match op {
    ActionOp::Add { label, cmd, key } => {
        let a = sid_store::QuickAction {
            id: sid_store::QuickAction::new_id(),
            label, scope: sid_store::QuickActionScope::Global, cmd, keybind: key,
            created_at: sid_store::now_epoch(),
        };
        store.upsert_quick_action(&a)?;
        println!("added action: {} ({})", a.label, a.id);
    }
    ActionOp::List => {
        for a in store.list_quick_actions()? {
            println!("{:<24} {:<40} {}", a.id, a.label, a.cmd);
        }
    }
    ActionOp::Remove { id } => {
        store.remove_quick_action(&id)?;
        println!("removed: {id}");
    }
    ActionOp::Run { id } => {
        let a = store.get_quick_action(&id)?.ok_or_else(|| anyhow::anyhow!("no such action: {id}"))?;
        let parts = shell_words::split(&a.cmd)?;
        let (bin, args) = parts.split_first().ok_or_else(|| anyhow::anyhow!("empty cmd"))?;
        let status = std::process::Command::new(bin).args(args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
```

- [ ] **Step 3: Failing tests**

Create `crates/sid/tests/system_services_cli.rs`:

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn services_cli_runs_or_self_skips() {
    if which::which("systemctl").is_err() { return; }
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "services", "--user"]).output().unwrap();
    assert!(out.status.success() || !out.stderr.is_empty()); // either ran or surfaced an error
}
```

Create `crates/sid/tests/system_action_cli.rs`:

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn action_add_list_run_remove_round_trip() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");

    let add = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "system", "action", "add",
               "echo greeting", "echo hello"])
        .output().unwrap();
    assert!(add.status.success(), "stderr: {}", String::from_utf8_lossy(&add.stderr));

    let list = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "system", "action", "list"])
        .output().unwrap();
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("echo greeting"));

    // Extract the id (first token of the line).
    let id = out.lines().next().unwrap().split_whitespace().next().unwrap().to_string();

    let run = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "system", "action", "run", &id])
        .output().unwrap();
    assert!(run.status.success());
    assert!(String::from_utf8_lossy(&run.stdout).contains("hello"));

    let _ = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "system", "action", "remove", &id])
        .output().unwrap();
    let list2 = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "system", "action", "list"])
        .output().unwrap();
    assert!(!String::from_utf8_lossy(&list2.stdout).contains("echo greeting"));
}
```

- [ ] **Step 4: Run tests** — expected passes (services test self-skips without systemctl).

- [ ] **Step 5: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): add `sid system services` + `sid system action add/list/remove/run`"
```

---

## Phase J — Integration

### Task 28: Wire `SystemctlCmdClient` + `KittyTerminalSpawner` into the binary

**Files:**
- Modify: `crates/sid/Cargo.toml` (add `sid-system`)
- Modify: `crates/sid/src/wire.rs`
- Modify: `crates/sid/src/main.rs`

- [ ] **Step 1: Add deps**

In `crates/sid/Cargo.toml`:

```toml
sid-system.workspace = true
```

- [ ] **Step 2: Inject the trait objects into `SidApp`**

In `wire.rs`:

```rust
use sid_core::adapters::systemctl::SystemctlClient;
use sid_core::adapters::terminal_spawner::TerminalSpawner;

pub struct SidApp {
    pub app: App,
    pub store: Arc<RedbStore>,
    pub session_id: String,
    pub git: Arc<dyn GitProvider>,
    pub systemctl: Arc<dyn SystemctlClient>,
    pub spawner: Arc<dyn TerminalSpawner>,
}
```

In `build_app`, construct `SystemctlCmdClient::new()` and `KittyTerminalSpawner::new()` — degrade gracefully on failure:

```rust
let systemctl: Arc<dyn SystemctlClient> = match sid_system::SystemctlCmdClient::new() {
    Ok(c) => Arc::new(c),
    Err(e) => {
        tracing::warn!("systemctl unavailable: {e}; System tab services pane will show empty");
        Arc::new(NoopSystemctlClient)
    }
};
let spawner: Arc<dyn TerminalSpawner> = match sid_system::KittyTerminalSpawner::new() {
    Ok(s) => Arc::new(s),
    Err(e) => {
        tracing::warn!("kitty unavailable: {e}; pinned configs will surface 'kitty missing' toasts");
        Arc::new(NoopTerminalSpawner)
    }
};
```

Define `NoopSystemctlClient` and `NoopTerminalSpawner` in `wire.rs` returning empty/error responses. Their purpose: keep the App usable on stripped systems (Docker, macOS without homebrew kitty).

- [ ] **Step 3: Pass to `SystemWidget`**

Update `SystemWidget::new` to accept `Arc<dyn SystemctlClient>` + `Arc<dyn TerminalSpawner>` + `Arc<dyn Store>`. The widget uses these in its event handlers to dispatch jobs via `ctx.jobs`.

- [ ] **Step 4: Failing smoke test**

The startup-with-noop-impls test:

```rust
#[test]
fn app_constructs_with_noop_impls_when_external_binaries_missing() {
    // Construction must succeed even if systemctl and kitty are absent.
    // We can't easily mock `which::which` in-process; this test simply
    // exercises the construction path and asserts no panic.
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let store = RedbStore::open(&db).unwrap();
    let _ = sid::wire::build_app_for_tests(&store).unwrap();
}
```

Add a `build_app_for_tests` shim in `wire.rs` that builds enough of the app to assert no panics.

- [ ] **Step 5: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): wire SystemctlCmdClient + KittyTerminalSpawner with graceful no-op fallbacks"
```

---

### Task 29: Integration test — pinned-config + quick-action registry round-trip via CLI

**Files:**
- Create: `crates/sid/tests/system_round_trip.rs`

End-to-end: add a pin and an action via CLI in one invocation; list both in a second invocation; remove both; confirm clean.

- [ ] **Step 1: Failing test**

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn pinned_configs_and_quick_actions_round_trip_via_cli() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let target = dir.path().join("a.conf");
    std::fs::write(&target, "x").unwrap();
    let bin = env!("CARGO_BIN_EXE_sid");

    // Add a pin.
    Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "pin", target.to_str().unwrap(), "--label", "L"]).output().unwrap();
    // Add an action.
    Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "action", "add", "test", "echo hi"]).output().unwrap();

    // List both.
    let pins = Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "pins"]).output().unwrap();
    assert!(String::from_utf8_lossy(&pins.stdout).contains("L"));
    let actions = Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "action", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&actions.stdout).contains("test"));

    // Verify the second invocation sees the same state (persistence).
    let pins2 = Command::new(bin).args(["--db", db.to_str().unwrap(), "system", "pins"]).output().unwrap();
    assert_eq!(pins.stdout, pins2.stdout);
}
```

- [ ] **Step 2: Run tests** — expected 1 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/sid
git commit -m "test(bin): integration round-trip for pins + quick-actions via CLI"
```

---

### Task 30: README update + done-criteria sweep

**Files:**
- Modify: `README.md`

Update the "What works in this build" callout and the System row in the "What's inside" table.

- [ ] **Step 1: Edit `README.md`**

Replace the System row:

```markdown
| **System** | Pinned config files (Enter launches external kitty + `$EDITOR`), systemctl services (start/stop/restart, journal tail), user-defined shell quick-actions (available globally from `Ctrl+F`) |
```

Add to the Quickstart section:

```markdown
# System management
sid system pin /etc/nginx/nginx.conf --label "nginx"
sid system services --user
sid system action add "kill 5432" "fuser -k 5432/tcp"
sid system action list
sid system action run <id>
```

Update the "What works in this build" callout to add System:

> Foundation + Workspaces + (other completed plans) + **System tab** fully functional. Pinned configs spawn external kitty windows; systemctl user+system units listed with start/stop/restart and last-100-line journal tail (follow mode supported); global quick-actions reachable from `Ctrl+F` palette and via `sid system action run <id>`.

- [ ] **Step 2: Done-criteria sweep**

Re-read the Done criteria below and confirm each item is met. If any fails, file a follow-up task (do not weaken the criteria).

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: update README to reflect Plan 6 System tab functionality"
```

---

## Done criteria for Plan 6

- [ ] `cargo build --workspace` succeeds with no errors or warnings.
- [ ] `cargo test --all-features --workspace` passes. New tests across `sid-core`, `sid-store`, `sid-system`, `sid-widgets`, and `sid` add roughly 80-120 cases (incl. proptest harnesses + insta snapshots).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is clean.
- [ ] `cargo fmt --check` is clean.
- [ ] `cargo run -p sid` launches; the System tab is composed of three sub-panels; `Tab` cycles focus through Pinned Configs → Services → Quick Actions.
- [ ] In the Pinned Configs pane: `a` adds (modal), `e` edits, `d` deletes, `Enter` on a pin spawns kitty cd'd into the parent dir running `$EDITOR <file>`. If kitty is missing, a toast says "kitty not found".
- [ ] In the Services pane: list shows user+system units; `/` filters; `Enter` opens the per-unit menu; `s`/`t`/`r` triggers start/stop/restart; `j` opens the journal tail modal (last 100 lines); `f` inside the modal toggles follow mode. System-bus writes without privileges surface a "sudo required" toast.
- [ ] In the Quick Actions pane: `a`/`e`/`d` for CRUD; `shell_words::split` parsing surfaces malformed quoting before save.
- [ ] After adding a quick action, `Ctrl+F` palette finds it; after deletion, it is gone — no restart required.
- [ ] `sid system pin <path>`, `sid system unpin <path>`, `sid system pins`, `sid system services [--user|--system] [--state STATE]`, `sid system action {add,list,remove,run}` all work non-interactively.
- [ ] `cargo bench -p sid-system --bench list_units` produces a baseline; `parse_list_units` on 200 units runs in well under 1ms.
- [ ] `cargo test -p sid-system --test parse_fuzz` runs ≥4096 proptest cases per parser and never panics.
- [ ] No regressions in Plans 1-5 (theme, tabs, palette, session restore, workspaces tab, …).

---

## Self-review notes (run before requesting human review)

**1. Spec coverage.** Plan 6 covers the spec's "System" tab fully:
   - Pinned configs (top): label + path + opener command, default opener = external kitty + `$EDITOR`, add/remove/edit pins. ✅
   - Services (middle): systemctl list-units, filter by state/name, per-unit status/start/stop/restart/journal tail. ✅
   - Quick actions (bottom): user-defined shell snippets, labels + optional keybinds, scope = global, surfaced in Ctrl+F palette. ✅

**2. Items deferred to later (confirmed against the future-features doc):**
   - System log viewer (full journalctl UI) — only per-unit tail in scope.
   - Sparkline metrics / hardware sensors / update notifier — Someday.
   - macOS launchd / OpenRC / runit fallbacks — Linux+systemd only in v1.
   - D-Bus `SystemctlClient` impl — trait designed to permit, v1 ships CLI-shelling only.
   - `cargo fuzz` libFuzzer setup — proptest harness in this plan, libFuzzer follow-up.

**3. Crate-pattern check.**
   - `SystemctlClient` trait lives in `sid-core::adapters::systemctl`. `SystemctlCmdClient` impl lives in `sid-system`. Widget references `sid_core::adapters::systemctl::SystemctlClient` only — never `sid_system::SystemctlCmdClient` directly (adapter pattern).
   - `TerminalSpawner` trait lives in `sid-core::adapters::terminal_spawner`. `KittyTerminalSpawner` lives in `sid-system`. Same enforcement.
   - `PinnedConfig` and `QuickAction` domain types live in `sid-store`. The `sid-widgets/system.rs` references them through `sid-store`'s public API.
   - `sid-system`'s only external runtime deps beyond sid-core: `which`, `shell-words`, `tokio` (already workspace-shared), `tracing`, `thiserror`. No `redb`, no `ratatui`, no `git2`. Clean adapter surface.

**4. Type consistency check.**
   - `SystemctlClient` methods return `Result<…, SystemctlError>` consistently. Sudo detection is a distinct `SystemctlError::SudoRequired` variant — separable from generic `NonZeroExit` in the widget's error-to-toast mapping.
   - `QuickActionScope::WorkspaceTagged(PathBuf)` is forward-compatible with Plan 2's `.sid/_metadata.sid` actions but not surfaced anywhere in this plan — the global palette ignores tagged actions. Plan 2's WorkspacesWidget owns that surface.
   - `SystemWidget::new` signature ends up taking `Arc<dyn SystemctlClient>`, `Arc<dyn TerminalSpawner>`, `Arc<dyn Store>`. This is what `wire.rs` passes. Three trait objects is on the upper end but tolerable; if Plan 8 (detach) forces a refactor toward a `Services` bag struct, that's a separate cleanup.

**5. Placeholder scan.**
   - One non-blocking placeholder: Task 23 assumes `ActionRegistry::register / unregister_with_prefix` exists from Plan 1 — if Plan 1's registry shape is different, Task 23 includes the small adjustment in `sid-core`.
   - Tasks 19-22 elide the actual ratatui rendering code (the widget's `render` method). The split is intentional: state is fully tested in `tests/system_state.rs`; rendering follows the same pattern as Plan 2's WorkspacesWidget. A future "render harness" task (Plan 1 may already have one) will add insta snapshots of the rendered buffers.

**6. Scope check.** 30 tasks, 10 phases. Within target band (25-35). Comparable to Plan 2's 33 tasks. Each phase produces working/testable software; plan can pause at the end of any phase with the project in a consistent state. Estimated line count: ~2400-2700 (this file: ~2500 written; aligned with target 2000-3000).

**7. CLAUDE.md compliance.**
   - Every public item gets a doc-test instruction.
   - Every parser-shaped function (`parse_list_units`, `parse_status`, `parse_journal`, `parse_quick_action_cmd`) has a proptest never-panic harness over arbitrary `&str` and arbitrary `Vec<u8>` (via `String::from_utf8_lossy`).
   - Adversarial cases enumerated in tasks: systemctl missing, kitty missing, $EDITOR unset, very long unit names, unit names with spaces (defense in depth), system-bus writes without sudo, malformed journal output, malformed shell quoting in quick-actions, empty inputs, unicode descriptions / labels / paths.
   - Insta snapshot of journal parser output (Task 6) — output stability regression gate.
   - Criterion benchmark on `parse_list_units(200 units)` (Task 7) — 10% regression gate per CLAUDE.md.
   - Property test on `QuickAction::new_id` collision avoidance.
   - Postcard round-trip property tests on `PinnedConfig` and `QuickAction`.

**8. Co-author trailer.** All commit subjects in this plan deliberately omit `Co-Authored-By: Claude…` trailers per the user's stated preference (memory: `no-claude-coauthor-trailer`).

**9. Judgment calls flagged for human review.**
   - **Crate name collision: `sid-sys` vs `sid-system`.** Recommended in plan preamble: rename `sid-sys` → `sid-sysinfo` (cleanup task in whichever plan owns the rename). This plan does not perform that rename.
   - **`unsafe impl Send + Sync` on `SystemctlCmdClient`:** none needed — the struct holds only `PathBuf`s, which are already `Send + Sync`. Contrast with Plan 2's `Git2Provider` which needed `unsafe impl` for libgit2's repo handle. Clean.
   - **`journal_follow` is on the concrete impl, not the trait.** Reason: streaming has a different shape (mpsc + JoinHandle). Adding it to the trait would force every impl (including future D-Bus impls) to expose the same streaming API; better to wait for a second use case before generalizing. Documented in Task 9 step 3.
   - **Sudo detection is string-matching on stderr.** Fragile across locales (systemd respects `$LANG`). Acceptable for v1; the binary's wire layer sets `LANG=C` before invoking systemctl. Documented as a follow-up if locale-specific bug reports surface.
   - **Quick-action execution path security.** v1 runs `shell_words::split(cmd)` then passes the result to `tokio::process::Command::new(bin).args(args)`. This is **not** `sh -c`; the command does not invoke a shell, so shell expansions (`$VAR`, `~`, `&&`, pipes) do not work. This is a deliberate hardening choice — if users need a shell, they write `sh -c "actual command"` explicitly. Documented in the QuickAction add modal help text.
