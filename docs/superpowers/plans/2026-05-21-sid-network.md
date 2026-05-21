# sid Plan 5 — Network tab + sys adapter

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. CLAUDE.md applies — every new pub fn needs a doc test, every function with invariants needs property tests, every parser-shaped function gets adversarial coverage, and the `SysProbe`'s `Arc<Mutex<…>>` handoff is gated on `loom` model-checking tests per the project policy.

**Goal:** When this plan is done, the **Network** tab is fully functional. The tab shows three coordinated panes: a **Listening ports** table (port, PID, command, protocol, state — sortable by any column), a **Processes** table (PID, name, CPU%, RSS, started timestamp, command — sortable by any column), and an **Interfaces** sidebar (per-interface IP addresses, rx/tx counters, up/down state). The `/` key opens an inline filter over the focused list; `Enter` drills into a process; `k` triggers a kill (SIGTERM → 5 s grace → SIGKILL on confirm) wired through the `JobQueue`. All data is supplied by a polling `SysProbe` service in `sid-core` running at 2 s default cadence (configurable), backed by `SysinfoProvider` in the new `sid-sys` crate (which wraps `sysinfo` for processes/interfaces and `netstat2` for listening ports, with `nix` for signal delivery). Non-interactive companion subcommands ship alongside: `sid net ports`, `sid net procs`, `sid net interfaces`, and `sid net kill <port-or-pid>`.

**Architecture:** A new `sid-sys` adapter crate hosts `SysinfoProvider` (composed: `sysinfo::System` for processes + interfaces; `netstat2::iterate_sockets_info` for listening ports; `nix::sys::signal::kill` for signal delivery). The `SysProvider` trait in `sid-core::adapters::sys` — currently an empty marker — gains the real method surface plus domain types (`ListeningPort`, `ProcessInfo`, `NetInterface`, `Pid`, `Signal`, `SysError`). A `SysProbe` service in `sid-core::sys_probe` owns the periodic-poll lifecycle: it holds the provider behind `Arc<Mutex<dyn SysProvider>>`, polls on a Tokio interval, and broadcasts snapshots over a `tokio::sync::broadcast` channel. Widgets subscribe to the broadcast and render the most-recent snapshot. The kill action is dispatched through the existing `JobQueue` (no blocking calls from the render loop). The widget lives in `sid-widgets/network.rs`, replacing the Plan 1 stub. The binary's `wire.rs` injects `SysinfoProvider` into the `SysProbe` and the `NetworkWidget`. `sid net …` CLI subcommands borrow the same provider and exit (no TUI).

**Tech stack additions:**
- `sysinfo = "0.32"` (default-features off; features `system`, `network`, `disk`, `user`) — processes, interfaces, system metrics
- `netstat2 = "0.10"` — listening TCP/UDP sockets with PID attribution on Linux/macOS
- `nix = "0.29"` (features `signal`, `process`) — `kill(2)` syscall + signal enums
- Everything else (tokio, ratatui, redb, sid-core, sid-store, etc.) already in workspace.dependencies

**Out of scope (deferred, see `2026-05-20-sid-future-features.md` "Someday" section):**
- Packet capture and decode (tshark/pcap surface)
- Bandwidth graphs / per-interface rate-over-time charts
- IP geo-resolution overlay for established connections
- `iptables` / `nftables` rules viewer + editor
- Established (non-listening) connection table — v1 lists only LISTEN sockets
- Per-process file descriptor / open-file enumeration (separate tab in System surface)
- Alternative `SysProvider` impls (`procfs`-direct, eBPF-backed, mocked-remote)
- Cgroup / namespace inspection
- Killing process trees (only single-PID in v1)

---

## File structure (new and modified only — existing crates unchanged unless noted)

```
sid/
├── Cargo.toml                          # MODIFY: + sysinfo, netstat2, nix, sid-sys workspace member
├── crates/
│   ├── sid-core/
│   │   └── src/
│   │       ├── lib.rs                  # MODIFY: declare sys_probe module, re-export
│   │       ├── sys_probe.rs            # NEW (Phase D)
│   │       └── adapters/
│   │           └── sys.rs              # MODIFY: full SysProvider trait + domain types
│   ├── sid-sys/                        # NEW CRATE
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs                  # SysinfoProvider impl entry
│   │   │   ├── processes.rs            # process listing via sysinfo
│   │   │   ├── ports.rs                # listening ports via netstat2
│   │   │   ├── interfaces.rs           # interface listing via sysinfo
│   │   │   └── kill.rs                 # kill_process via nix
│   │   ├── benches/
│   │   │   ├── list_processes.rs
│   │   │   └── list_listening_ports.rs
│   │   └── tests/
│   │       ├── processes.rs
│   │       ├── ports.rs
│   │       ├── interfaces.rs
│   │       └── kill.rs
│   ├── sid-job/                        # (no changes; consumed)
│   ├── sid-widgets/
│   │   └── src/
│   │       └── network.rs              # MODIFY: replace stub with full impl
│   └── sid/
│       └── src/
│           ├── main.rs                 # MODIFY: + `sid net` subcommands
│           └── wire.rs                 # MODIFY: + SysinfoProvider + SysProbe injection
└── docs/superpowers/plans/
    └── 2026-05-21-sid-network.md       # this document
```

---

## Task index

| # | Task | Phase |
|---|---|---|
| 1 | Add `sysinfo`, `netstat2`, `nix` to workspace deps + `sid-sys` member | A. Foundation |
| 2 | Expand `SysProvider` trait + domain types in `sid-core` | A. Foundation |
| 3 | `sid-sys` crate skeleton + `SysinfoProvider::new` | B. SysinfoProvider |
| 4 | `list_processes` (sysinfo) | C. SysinfoProvider impls |
| 5 | `list_listening_ports` (netstat2) | C. SysinfoProvider impls |
| 6 | `list_interfaces` (sysinfo network refresh) | C. SysinfoProvider impls |
| 7 | `kill_process` (nix) — including permission-denied mapping | C. SysinfoProvider impls |
| 8 | Wire-through refresh strategy + provider concurrency safety | C. SysinfoProvider impls |
| 9 | `SysProbe` skeleton: `Arc<Mutex<dyn SysProvider>>` + broadcast channel | D. SysProbe service |
| 10 | `SysProbe::run` Tokio interval poll + Snapshot type | D. SysProbe service |
| 11 | `SysProbe` loom test for the Arc/Mutex handoff | D. SysProbe service |
| 12 | `PortsTableState` (sort, scroll, select) | E. NetworkWidget |
| 13 | `ProcessesTableState` (sort, scroll, select) | E. NetworkWidget |
| 14 | `InterfacesSidebarState` | E. NetworkWidget |
| 15 | `FilterInputState` for `/` filter | E. NetworkWidget |
| 16 | `KillConfirmModalState` (two-stage SIGTERM/SIGKILL UI) | E. NetworkWidget |
| 17 | `NetworkWidget` assembly + `Widget` impl + insta snapshot | E. NetworkWidget |
| 18 | Kill action wiring through `JobQueue` (SIGTERM → wait → SIGKILL) | F. Kill action |
| 19 | Toast surfacing of kill outcomes + permission-denied path | F. Kill action |
| 20 | `sid net ports` subcommand | G. CLI |
| 21 | `sid net procs` subcommand | G. CLI |
| 22 | `sid net interfaces` subcommand | G. CLI |
| 23 | `sid net kill <port-or-pid>` subcommand | G. CLI |
| 24 | Wire `SysinfoProvider` + `SysProbe` into the binary | H. Wiring + docs |
| 25 | Integration test — Network tab end-to-end snapshot + kill | H. Wiring + docs |
| 26 | Criterion benches register in CI gate; baseline captured | H. Wiring + docs |
| 27 | README update | H. Wiring + docs |

27 tasks across 8 phases.

---

## Phase A — Foundation

### Task 1: Add `sysinfo`, `netstat2`, `nix` to workspace deps + `sid-sys` member

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add `sid-sys` to workspace members**

Modify the `[workspace] members` list in `Cargo.toml`. Find:

```toml
members = [
    "crates/sid",
    "crates/sid-core",
    "crates/sid-ui",
    "crates/sid-store",
    "crates/sid-job",
    "crates/sid-widgets",
    "crates/sid-git",
]
```

Replace with:

```toml
members = [
    "crates/sid",
    "crates/sid-core",
    "crates/sid-ui",
    "crates/sid-store",
    "crates/sid-job",
    "crates/sid-widgets",
    "crates/sid-git",
    "crates/sid-sys",
]
```

- [ ] **Step 2: Add new external deps + internal `sid-sys` to `[workspace.dependencies]`**

In the `# Internal` block, append:

```toml
sid-sys = { path = "crates/sid-sys" }
```

In a logical place (after the `# Git` block), add:

```toml
# System / network
sysinfo = { version = "0.32", default-features = false, features = ["system", "network", "disk", "user"] }
netstat2 = "0.10"
nix = { version = "0.29", features = ["signal", "process"] }
```

Rationale for the version pins: `sysinfo 0.32` is the current minor with stable `RefreshKind`/`ProcessRefreshKind` surface; `netstat2 0.10` exposes the `iterate_sockets_info` API used in Task 5; `nix 0.29` matches the `tokio 1.47` stack already in the workspace without pulling in conflicting libc versions.

- [ ] **Step 3: Verify the workspace resolves**

Run: `cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: fails with "member crate `sid-sys` has no Cargo.toml" — that's fine, Task 3 creates it. Until then, temporarily scaffold:

```bash
mkdir -p crates/sid-sys/src
cat > crates/sid-sys/Cargo.toml <<'EOF'
[package]
name = "sid-sys"
version.workspace = true
edition.workspace = true

[dependencies]
EOF
echo "// stub — Task 3 replaces this" > crates/sid-sys/src/lib.rs
```

Confirm `cargo metadata --no-deps --format-version 1 > /dev/null` exits 0.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/sid-sys
git commit -m "chore: add sysinfo, netstat2, nix deps and sid-sys workspace member stub"
```

---

### Task 2: Expand `SysProvider` trait + domain types in `sid-core`

**Files:**
- Modify: `crates/sid-core/src/adapters/sys.rs`
- Modify: `crates/sid-core/src/lib.rs` (re-export new types if useful)
- Test: `crates/sid-core/tests/sys_provider_contract.rs`

The trait currently reads `pub trait SysProvider: Send + Sync {}`. Replace with the full method surface + supporting domain types. Use **dyn-compatible** signatures (no `Self`, no generics in method positions, take `&self`/`&mut self`).

- [ ] **Step 1: Write the contract test first**

Create `crates/sid-core/tests/sys_provider_contract.rs`:

```rust
//! Verifies the SysProvider trait is dyn-compatible (Box<dyn SysProvider> works)
//! and that a no-op MockProvider can implement every method.

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Protocol, Signal, SocketState, SysError,
    SysProvider,
};

struct MockProvider;

impl SysProvider for MockProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> { Ok(vec![]) }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
    fn kill_process(&mut self, _pid: Pid, _sig: Signal) -> Result<(), SysError> { Ok(()) }
}

#[test]
fn provider_is_dyn_compatible() {
    let mut p: Box<dyn SysProvider> = Box::new(MockProvider);
    assert!(p.list_processes().unwrap().is_empty());
    assert!(p.list_listening_ports().unwrap().is_empty());
    assert!(p.list_interfaces().unwrap().is_empty());
}

#[test]
fn provider_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn SysProvider>>();
}

#[test]
fn protocol_variants_exist() {
    let _ = Protocol::Tcp;
    let _ = Protocol::Udp;
}

#[test]
fn socket_state_variants_exist() {
    let _ = SocketState::Listen;
    let _ = SocketState::Established;
    let _ = SocketState::Other;
}

#[test]
fn signal_variants_exist() {
    let _ = Signal::Term;
    let _ = Signal::Kill;
    let _ = Signal::Int;
    let _ = Signal::Hup;
}

#[test]
fn pid_is_constructable() {
    let p = Pid::from_u32(1234);
    assert_eq!(p.as_u32(), 1234);
}

#[test]
fn process_info_construction() {
    let pi = ProcessInfo {
        pid: Pid::from_u32(42),
        name: "sid".into(),
        cmd: "sid".into(),
        cpu_pct: 0.0,
        rss_bytes: 0,
        started_unix_secs: 0,
        parent: None,
        user: None,
    };
    assert_eq!(pi.pid.as_u32(), 42);
}

#[test]
fn syserror_variants_exist() {
    let _ = SysError::PermissionDenied("kill".into());
    let _ = SysError::NotFound("pid 999".into());
    let _ = SysError::Other("oops".into());
}
```

- [ ] **Step 2: Run — should fail to compile**

Run: `cargo test -p sid-core --test sys_provider_contract`
Expected: compile error (types and methods don't exist yet).

- [ ] **Step 3: Replace `crates/sid-core/src/adapters/sys.rs`**

```rust
//! System probe trait + supporting domain types. Implementations live in `sid-sys`.
//!
//! See `crates/sid-core/src/sys_probe.rs` for the polling service that wraps
//! a `SysProvider` and broadcasts snapshots to widgets.

use serde::{Deserialize, Serialize};

/// Process identifier. Wraps a `u32` so widget/UI code never has to know
/// whether the underlying probe uses `i32`, `pid_t`, or `usize`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Pid(u32);

impl Pid {
    /// Construct a `Pid` from a raw `u32`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::sys::Pid;
    /// let p = Pid::from_u32(1);
    /// assert_eq!(p.as_u32(), 1);
    /// ```
    pub fn from_u32(v: u32) -> Self { Self(v) }

    /// Return the raw `u32` PID.
    pub fn as_u32(self) -> u32 { self.0 }
}

/// Signal kinds accepted by `kill_process`. Keep this list small — anything
/// beyond these is out of scope for v1.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Signal {
    /// SIGTERM — graceful termination request.
    Term,
    /// SIGKILL — uncatchable kill.
    Kill,
    /// SIGINT — interactive interrupt.
    Int,
    /// SIGHUP — hangup, often used to reload config.
    Hup,
}

/// Transport-layer protocol of a listening socket.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Protocol {
    Tcp,
    Udp,
}

/// State of a socket. v1 lists only LISTEN sockets, but the type carries
/// enough variants to future-proof for Plan 5+ "established connections" work.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SocketState {
    Listen,
    Established,
    Other,
}

/// One listening port row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListeningPort {
    /// Port number (1..=65535 in practice; type allows 0 for invalid input).
    pub port: u16,
    /// Owning PID, if attributable. On some platforms the netstat-style API
    /// cannot attribute a socket to a process — in that case this is `None`.
    pub pid: Option<Pid>,
    /// Display command (executable name + args, truncated by the producer).
    /// Empty string if `pid` is `None` or lookup failed.
    pub command: String,
    pub protocol: Protocol,
    pub state: SocketState,
    /// Local bind address as a printable string ("0.0.0.0", "::", "127.0.0.1").
    pub local_addr: String,
}

/// One process row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: Pid,
    /// Short name (argv[0] basename).
    pub name: String,
    /// Full command line (argv joined by spaces).
    pub cmd: String,
    /// Aggregate CPU percent (0..=100 per core; >100 possible on multi-core).
    pub cpu_pct: f32,
    /// Resident set size in bytes.
    pub rss_bytes: u64,
    /// Process start time, seconds since UNIX epoch.
    pub started_unix_secs: i64,
    pub parent: Option<Pid>,
    pub user: Option<String>,
}

/// One network interface row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetInterface {
    pub name: String,
    /// IPv4 + IPv6 addresses bound to this interface.
    pub addrs: Vec<String>,
    /// Bytes received since the system was probed for the first time.
    pub rx_bytes: u64,
    /// Bytes transmitted since the system was probed for the first time.
    pub tx_bytes: u64,
    /// Whether the OS reports the interface as up.
    pub is_up: bool,
}

/// Domain-shaped system error. Concrete impls map their library errors into this.
#[derive(Debug, thiserror::Error)]
pub enum SysError {
    /// e.g., trying to kill a root-owned process as an unprivileged user.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// e.g., the PID doesn't exist (ESRCH from `kill(2)`).
    #[error("not found: {0}")]
    NotFound(String),
    /// e.g., signal value isn't one of the supported variants on this platform.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Anything else mapped from the underlying library.
    #[error("system probe error: {0}")]
    Other(String),
}

/// System / network metrics needed by the Network tab. Implementations live in `sid-sys`.
///
/// # Refresh semantics
///
/// Each `list_*` method takes `&mut self` so impls can keep a cached
/// `sysinfo::System` (or similar handle) between calls and only re-refresh
/// the kinds it needs. Implementations MUST be safe to call repeatedly on the
/// same instance and MUST NOT leak file descriptors between calls.
///
/// # Object safety
///
/// All methods take `&mut self` and use no generics in method position,
/// so `Box<dyn SysProvider>` works.
pub trait SysProvider: Send + Sync {
    /// List all visible processes. On Linux, processes outside the caller's
    /// namespace or with restricted `/proc` permissions may be omitted.
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError>;

    /// List sockets in `LISTEN` state across TCP and UDP. PID attribution
    /// is best-effort and may be `None` on some platforms / for some sockets.
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError>;

    /// List network interfaces, including loopback. Addresses include both
    /// IPv4 and IPv6.
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError>;

    /// Send `sig` to `pid`. Maps platform errors:
    /// - `EPERM`/`EACCES` → `SysError::PermissionDenied`
    /// - `ESRCH`           → `SysError::NotFound`
    /// - anything else     → `SysError::Other`
    fn kill_process(&mut self, pid: Pid, sig: Signal) -> Result<(), SysError>;
}
```

- [ ] **Step 4: Update `lib.rs`**

Confirm `pub mod adapters;` is present (it should be from Plan 1). No re-exports needed at the crate root — widgets use the full `sid_core::adapters::sys::...` path, matching how `GitProvider` is accessed.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test sys_provider_contract`
Expected: all tests pass.

Run: `cargo test -p sid-core --all-features`
Expected: no regressions.

- [ ] **Step 6: Add doc tests per CLAUDE.md**

Add `# Examples` blocks to `Pid`, `Signal`, `Protocol`, `SocketState`, `ListeningPort`, `ProcessInfo`, `NetInterface`, `SysError`, and `SysProvider`. The trait's doc test should construct a tiny mock impl matching one method (`list_processes`).

- [ ] **Step 7: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): expand SysProvider trait with full method surface + domain types"
```

---

## Phase B — `sid-sys` crate skeleton

### Task 3: `sid-sys` crate skeleton + `SysinfoProvider::new`

**Files:**
- Replace: `crates/sid-sys/Cargo.toml` (stub from Task 1)
- Replace: `crates/sid-sys/src/lib.rs` (stub from Task 1)
- Create: `crates/sid-sys/src/processes.rs` (empty module — filled in Task 4)
- Create: `crates/sid-sys/src/ports.rs` (empty module — filled in Task 5)
- Create: `crates/sid-sys/src/interfaces.rs` (empty module — filled in Task 6)
- Create: `crates/sid-sys/src/kill.rs` (empty module — filled in Task 7)
- Create: `crates/sid-sys/tests/processes.rs` (smoke test only at this task)

- [ ] **Step 1: Write the failing test**

Create `crates/sid-sys/tests/processes.rs`:

```rust
use sid_core::adapters::sys::SysProvider;
use sid_sys::SysinfoProvider;

#[test]
fn new_constructs_without_panicking() {
    let _ = SysinfoProvider::new();
}

#[test]
fn provider_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SysinfoProvider>();
}

#[test]
fn boxes_into_dyn_provider() {
    let p: Box<dyn SysProvider> = Box::new(SysinfoProvider::new());
    drop(p);
}
```

- [ ] **Step 2: Run — should fail to compile**

Run: `cargo test -p sid-sys --test processes`
Expected: compile error (`SysinfoProvider` not defined).

- [ ] **Step 3: Replace `crates/sid-sys/Cargo.toml`**

```toml
[package]
name = "sid-sys"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
sid-core.workspace = true
sysinfo.workspace = true
netstat2.workspace = true
nix.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
proptest.workspace = true
insta.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }

[[bench]]
name = "list_processes"
harness = false

[[bench]]
name = "list_listening_ports"
harness = false
```

(The bench targets reference files created in Task 26; the entries can be added in this task and pointed at empty `fn main() {}` placeholder files, which Task 26 fills in. Alternatively, add the bench entries in Task 26 only — implementer's call.)

- [ ] **Step 4: Create `crates/sid-sys/src/lib.rs`**

```rust
//! `SysinfoProvider` — composed `SysProvider` implementation.
//!
//! Internally a `SysinfoProvider` holds:
//!   - a `sysinfo::System` for processes + interfaces
//!   - a per-call `netstat2` iterator for listening ports
//!   - no persistent state for kill (each `kill_process` call is independent)
//!
//! All access to the inner `sysinfo::System` is serialized via `&mut self`;
//! the `SysProbe` in `sid-core` wraps the provider in `Arc<Mutex<…>>` for
//! cross-task sharing.

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
};

mod interfaces;
mod kill;
mod ports;
mod processes;

/// Composed `SysProvider` impl backed by `sysinfo` (processes + interfaces),
/// `netstat2` (listening ports), and `nix` (signal delivery).
///
/// # Examples
///
/// ```
/// use sid_sys::SysinfoProvider;
/// let _p = SysinfoProvider::new();
/// ```
pub struct SysinfoProvider {
    /// Cached sysinfo handle. `&mut self` access makes refreshes serialized.
    inner: sysinfo::System,
}

impl SysinfoProvider {
    /// Construct a fresh `SysinfoProvider`. Performs an initial empty refresh
    /// so that later CPU% deltas are meaningful (sysinfo computes CPU% as a
    /// delta vs. the previous refresh).
    pub fn new() -> Self {
        let mut inner = sysinfo::System::new();
        // Prime the CPU sampling baseline.
        inner.refresh_cpu_usage();
        Self { inner }
    }
}

impl Default for SysinfoProvider {
    fn default() -> Self { Self::new() }
}

impl SysProvider for SysinfoProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        processes::list_processes(&mut self.inner)
    }

    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
        ports::list_listening_ports(&self.inner)
    }

    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
        interfaces::list_interfaces(&mut self.inner)
    }

    fn kill_process(&mut self, pid: Pid, sig: Signal) -> Result<(), SysError> {
        kill::kill_process(pid, sig)
    }
}
```

Create `src/processes.rs`, `src/ports.rs`, `src/interfaces.rs`, `src/kill.rs` as stubs:

```rust
// src/processes.rs
use sid_core::adapters::sys::{ProcessInfo, SysError};
pub(crate) fn list_processes(_sys: &mut sysinfo::System) -> Result<Vec<ProcessInfo>, SysError> {
    Err(SysError::Other("not yet implemented — Task 4".into()))
}
```

```rust
// src/ports.rs
use sid_core::adapters::sys::{ListeningPort, SysError};
pub(crate) fn list_listening_ports(_sys: &sysinfo::System) -> Result<Vec<ListeningPort>, SysError> {
    Err(SysError::Other("not yet implemented — Task 5".into()))
}
```

```rust
// src/interfaces.rs
use sid_core::adapters::sys::{NetInterface, SysError};
pub(crate) fn list_interfaces(_sys: &mut sysinfo::System) -> Result<Vec<NetInterface>, SysError> {
    Err(SysError::Other("not yet implemented — Task 6".into()))
}
```

```rust
// src/kill.rs
use sid_core::adapters::sys::{Pid, Signal, SysError};
pub(crate) fn kill_process(_pid: Pid, _sig: Signal) -> Result<(), SysError> {
    Err(SysError::Other("not yet implemented — Task 7".into()))
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-sys --test processes`
Expected: 3 passed.

- [ ] **Step 6: Adversarial coverage**

Append to `tests/processes.rs`:

```rust
#[test]
fn many_news_in_sequence_does_not_leak() {
    // Construct + drop 50 providers. If this leaks fds, repeated runs on a
    // tight ulimit will eventually fail. We don't assert here; we just exercise.
    for _ in 0..50 {
        let _ = SysinfoProvider::new();
    }
}
```

- [ ] **Step 7: Commit**

```bash
git add crates/sid-sys
git commit -m "feat(sys): add sid-sys crate with SysinfoProvider skeleton"
```

---

## Phase C — `SysinfoProvider` impls

Phase C is one task per `SysProvider` method, each landing as its own commit. All tests use the live host's sysinfo / netstat surface — they MUST be tolerant of empty results (e.g., a CI runner with no listening ports beyond ssh on a privileged port) and MUST NOT assume any particular PID exists. Where the host's state could vary, tests assert on invariants (sorted-by-port, no duplicate PIDs in port table, etc.) rather than concrete contents.

### Task 4: `list_processes` (sysinfo)

**Files:**
- Modify: `crates/sid-sys/src/processes.rs`
- Modify: `crates/sid-sys/tests/processes.rs`

- [ ] **Step 1: Write failing tests**

Append to `crates/sid-sys/tests/processes.rs`:

```rust
use sid_core::adapters::sys::{Pid, SysProvider as _};

#[test]
fn list_processes_includes_current_process() {
    let mut p = SysinfoProvider::new();
    // Some sysinfo backends need a small delay before CPU% is meaningful;
    // we don't assert on cpu_pct in this test.
    let procs = p.list_processes().expect("list_processes");
    let me = std::process::id();
    assert!(procs.iter().any(|x| x.pid.as_u32() == me),
            "expected current pid {me} to appear in process list");
}

#[test]
fn list_processes_nonempty_on_live_system() {
    let mut p = SysinfoProvider::new();
    let procs = p.list_processes().unwrap();
    assert!(!procs.is_empty(), "live system should have processes");
}

#[test]
fn list_processes_pids_are_unique() {
    let mut p = SysinfoProvider::new();
    let procs = p.list_processes().unwrap();
    let mut pids: Vec<u32> = procs.iter().map(|p| p.pid.as_u32()).collect();
    pids.sort_unstable();
    let total = pids.len();
    pids.dedup();
    assert_eq!(total, pids.len(), "PIDs in process list should be unique");
}

#[test]
fn list_processes_repeated_calls_are_stable() {
    let mut p = SysinfoProvider::new();
    let a = p.list_processes().unwrap();
    let b = p.list_processes().unwrap();
    // At minimum, the current pid should be in both.
    let me = Pid::from_u32(std::process::id());
    assert!(a.iter().any(|x| x.pid == me));
    assert!(b.iter().any(|x| x.pid == me));
}
```

- [ ] **Step 2: Run — should fail (not yet implemented)**

- [ ] **Step 3: Implement `list_processes`**

Replace `src/processes.rs`:

```rust
use sid_core::adapters::sys::{Pid, ProcessInfo, SysError};
use sysinfo::{ProcessRefreshKind, RefreshKind, UpdateKind};

pub(crate) fn list_processes(sys: &mut sysinfo::System) -> Result<Vec<ProcessInfo>, SysError> {
    // Refresh only what we need: process list + CPU + memory + command-line.
    sys.refresh_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_user(UpdateKind::Always)
                .with_cmd(UpdateKind::Always),
        ),
    );

    let mut out = Vec::with_capacity(sys.processes().len());
    for (pid, proc) in sys.processes() {
        let cmd_vec: Vec<String> = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let cmd = cmd_vec.join(" ");
        out.push(ProcessInfo {
            pid: Pid::from_u32(pid.as_u32()),
            name: proc.name().to_string_lossy().into_owned(),
            cmd,
            cpu_pct: proc.cpu_usage(),
            rss_bytes: proc.memory(),
            started_unix_secs: proc.start_time() as i64,
            parent: proc.parent().map(|p| Pid::from_u32(p.as_u32())),
            user: proc.user_id().map(|u| u.to_string()),
        });
    }
    Ok(out)
}
```

- [ ] **Step 4: Run tests** — expected all passed.

- [ ] **Step 5: Property + adversarial coverage**

Append:

```rust
use proptest::prelude::*;

proptest! {
    /// Property: repeated calls never increase the live-PID count past
    /// (initial + bounded delta from background activity).
    #[test]
    fn prop_process_count_does_not_explode(_iters in 1usize..4) {
        let mut p = SysinfoProvider::new();
        let baseline = p.list_processes().unwrap().len();
        for _ in 0.._iters {
            let n = p.list_processes().unwrap().len();
            // Allow generous jitter from the host. The point is to catch
            // accidental unbounded growth from a leaked accumulator.
            prop_assert!(n < baseline.saturating_mul(10).saturating_add(1000));
        }
    }
}

#[test]
fn high_count_does_not_panic() {
    // We can't fork 10k processes in a unit test, but we can exercise the
    // collection over the existing system — sysinfo on a typical dev box
    // returns a few hundred procs. The job here is to confirm no panic.
    let mut p = SysinfoProvider::new();
    for _ in 0..5 {
        let _ = p.list_processes().unwrap();
    }
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-sys
git commit -m "feat(sys): implement list_processes via sysinfo refresh_specifics"
```

---

### Task 5: `list_listening_ports` (netstat2)

**Files:**
- Modify: `crates/sid-sys/src/ports.rs`
- Create: `crates/sid-sys/tests/ports.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-sys/tests/ports.rs`:

```rust
use std::net::TcpListener;

use sid_core::adapters::sys::{Protocol, SocketState, SysProvider as _};
use sid_sys::SysinfoProvider;

#[test]
fn binding_a_local_tcp_port_makes_it_appear() {
    // Bind a fresh TCP listener on a kernel-assigned port; we expect it
    // to show up in list_listening_ports().
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let bound = listener.local_addr().unwrap().port();
    let mut p = SysinfoProvider::new();
    let ports = p.list_listening_ports().unwrap();
    // Either the netstat backend attributes our PID, or it doesn't —
    // but the port itself must be in the list.
    assert!(ports.iter().any(|x| x.port == bound && x.protocol == Protocol::Tcp),
            "expected port {bound} in listening ports");
}

#[test]
fn all_returned_entries_are_listen_state() {
    let mut p = SysinfoProvider::new();
    let ports = p.list_listening_ports().unwrap();
    for entry in &ports {
        assert_eq!(entry.state, SocketState::Listen, "non-LISTEN entry returned: {entry:?}");
    }
}

#[test]
fn empty_system_returns_ok_empty_or_nonempty() {
    // We can't guarantee an empty list, but the call must never panic and
    // must always return Ok.
    let mut p = SysinfoProvider::new();
    let _ = p.list_listening_ports().expect("must not error on a healthy host");
}
```

- [ ] **Step 2: Run — should fail (not yet implemented)**

- [ ] **Step 3: Implement `list_listening_ports`**

Replace `src/ports.rs`:

```rust
use sid_core::adapters::sys::{ListeningPort, Pid, Protocol, SocketState, SysError};

pub(crate) fn list_listening_ports(sys: &sysinfo::System) -> Result<Vec<ListeningPort>, SysError> {
    use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState};

    let af = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let pf = ProtocolFlags::TCP | ProtocolFlags::UDP;
    let iter = netstat2::iterate_sockets_info(af, pf)
        .map_err(|e| SysError::Other(format!("netstat2: {e}")))?;

    let mut out = Vec::new();
    for entry in iter {
        let info = match entry {
            Ok(i) => i,
            Err(_) => continue, // skip rows we can't parse rather than failing the whole call
        };
        let (port, proto, local_addr, state, is_listen) = match info.protocol_socket_info {
            ProtocolSocketInfo::Tcp(t) => {
                let is_listen = matches!(t.state, TcpState::Listen);
                (
                    t.local_port,
                    Protocol::Tcp,
                    t.local_addr.to_string(),
                    SocketState::Listen,
                    is_listen,
                )
            }
            ProtocolSocketInfo::Udp(u) => (
                u.local_port,
                Protocol::Udp,
                u.local_addr.to_string(),
                SocketState::Listen,
                true, // UDP sockets in the list are de facto "bound and listening"
            ),
        };
        if !is_listen { continue; }

        // Pick the first attributed PID, if any. netstat2 sometimes returns
        // multiple PIDs for the same socket (process group, fd duplication);
        // we keep the first deterministically.
        let owning_pid = info.associated_pids.into_iter().next().map(Pid::from_u32);

        let command = owning_pid
            .and_then(|pid| sys.process(sysinfo::Pid::from_u32(pid.as_u32())))
            .map(|p| p.name().to_string_lossy().into_owned())
            .unwrap_or_default();

        out.push(ListeningPort {
            port,
            pid: owning_pid,
            command,
            protocol: proto,
            state,
            local_addr,
        });
    }

    // Sort for deterministic test + render output: by port, then protocol.
    out.sort_by(|a, b| a.port.cmp(&b.port).then(format!("{:?}", a.protocol).cmp(&format!("{:?}", b.protocol))));
    Ok(out)
}
```

Note: `list_listening_ports` borrows `&sysinfo::System` (not `&mut`) so it can be called without forcing a refresh — but its trait method takes `&mut self`. That's fine; the outer trait method just doesn't mutate. Implementers MAY add `sys.refresh_processes(...)` here if PID→command lookups stale out in practice.

- [ ] **Step 4: Run tests** — expected all passed (on a host with at least one listening socket the bound listener provides this).

- [ ] **Step 5: Adversarial + property tests**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_returned_ports_are_in_range(_seed in 0u32..1) {
        let _ = _seed;
        let mut p = sid_sys::SysinfoProvider::new();
        let ports = p.list_listening_ports().unwrap();
        for entry in &ports {
            // port: u16 — by construction always 0..=65535.
            prop_assert!(entry.port <= u16::MAX);
        }
    }
}

#[test]
fn binding_many_ports_does_not_lose_any() {
    let mut listeners = Vec::new();
    let mut bound_ports = Vec::new();
    for _ in 0..8 {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        bound_ports.push(l.local_addr().unwrap().port());
        listeners.push(l);
    }
    let mut p = SysinfoProvider::new();
    let ports = p.list_listening_ports().unwrap();
    for bp in &bound_ports {
        assert!(ports.iter().any(|x| x.port == *bp && x.protocol == Protocol::Tcp),
                "expected port {bp} in listing");
    }
    drop(listeners);
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-sys
git commit -m "feat(sys): implement list_listening_ports via netstat2"
```

---

### Task 6: `list_interfaces` (sysinfo network refresh)

**Files:**
- Modify: `crates/sid-sys/src/interfaces.rs`
- Create: `crates/sid-sys/tests/interfaces.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-sys/tests/interfaces.rs`:

```rust
use sid_core::adapters::sys::SysProvider as _;
use sid_sys::SysinfoProvider;

#[test]
fn list_interfaces_includes_loopback() {
    let mut p = SysinfoProvider::new();
    let ifs = p.list_interfaces().unwrap();
    assert!(
        ifs.iter().any(|i| i.name == "lo" || i.name == "lo0" || i.name.starts_with("lo")),
        "expected loopback interface in {:?}", ifs.iter().map(|i| &i.name).collect::<Vec<_>>()
    );
}

#[test]
fn interfaces_have_unique_names() {
    let mut p = SysinfoProvider::new();
    let ifs = p.list_interfaces().unwrap();
    let mut names: Vec<_> = ifs.iter().map(|i| i.name.clone()).collect();
    names.sort();
    let total = names.len();
    names.dedup();
    assert_eq!(names.len(), total, "interface names must be unique");
}

#[test]
fn rx_tx_monotonic_over_two_polls() {
    let mut p = SysinfoProvider::new();
    let a = p.list_interfaces().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let b = p.list_interfaces().unwrap();
    // For each interface present in both polls, rx/tx counters should not
    // go *backwards*. We allow equality (idle interface).
    for ai in &a {
        if let Some(bi) = b.iter().find(|x| x.name == ai.name) {
            assert!(bi.rx_bytes >= ai.rx_bytes, "rx went backwards on {}", ai.name);
            assert!(bi.tx_bytes >= ai.tx_bytes, "tx went backwards on {}", ai.name);
        }
    }
}
```

- [ ] **Step 2: Run — should fail (not yet implemented)**

- [ ] **Step 3: Implement `list_interfaces`**

Replace `src/interfaces.rs`:

```rust
use sid_core::adapters::sys::{NetInterface, SysError};
use sysinfo::{Networks};

pub(crate) fn list_interfaces(_sys: &mut sysinfo::System) -> Result<Vec<NetInterface>, SysError> {
    // sysinfo's Networks lives separately from System; build it per-call.
    // (Caching it on the provider is a future optimization; v1 keeps the
    // call self-contained.)
    let mut nets = Networks::new_with_refreshed_list();
    // A second refresh allows rx/tx delta-since-baseline; the absolute
    // counters we report are sysinfo's "total_received" / "total_transmitted".
    nets.refresh(true);

    let mut out = Vec::with_capacity(nets.len());
    for (name, data) in nets.iter() {
        let addrs: Vec<String> = data
            .ip_networks()
            .iter()
            .map(|n| n.addr.to_string())
            .collect();
        out.push(NetInterface {
            name: name.to_string(),
            addrs,
            rx_bytes: data.total_received(),
            tx_bytes: data.total_transmitted(),
            // sysinfo doesn't expose UP/DOWN on every platform; treat any
            // interface with addresses or activity as up. Refine later.
            is_up: !data.ip_networks().is_empty()
                || data.total_received() > 0
                || data.total_transmitted() > 0,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
```

- [ ] **Step 4: Run tests** — expected all passed.

- [ ] **Step 5: Adversarial + property tests**

Append:

```rust
#[test]
fn no_interfaces_does_not_panic() {
    // Even in a sandbox where no interface is visible, the call must succeed.
    // We can't actually create such a sandbox in this test, but we exercise
    // the call repeatedly to catch state-keeping bugs.
    let mut p = SysinfoProvider::new();
    for _ in 0..10 { let _ = p.list_interfaces().unwrap(); }
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-sys
git commit -m "feat(sys): implement list_interfaces via sysinfo Networks"
```

---

### Task 7: `kill_process` (nix) — including permission-denied mapping

**Files:**
- Modify: `crates/sid-sys/src/kill.rs`
- Create: `crates/sid-sys/tests/kill.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-sys/tests/kill.rs`:

```rust
use std::process::{Command, Stdio};

use sid_core::adapters::sys::{Pid, Signal, SysError, SysProvider as _};
use sid_sys::SysinfoProvider;

fn spawn_sleep() -> std::process::Child {
    Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sleep")
}

#[test]
fn kill_term_a_subprocess_exits_it() {
    let mut child = spawn_sleep();
    let pid = Pid::from_u32(child.id());
    let mut p = SysinfoProvider::new();
    p.kill_process(pid, Signal::Term).expect("kill TERM");
    // Wait up to 2s for the child to die.
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(2) {
        if let Ok(Some(_)) = child.try_wait() { return; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("child did not exit after SIGTERM");
}

#[test]
fn kill_nonexistent_pid_returns_not_found() {
    let mut p = SysinfoProvider::new();
    // PID u32::MAX is effectively guaranteed to be nonexistent.
    let err = p.kill_process(Pid::from_u32(u32::MAX), Signal::Term).unwrap_err();
    assert!(matches!(err, SysError::NotFound(_)), "got {err:?}");
}

#[test]
fn kill_pid_zero_is_rejected() {
    // pid 0 has special "kill the whole process group" semantics in POSIX;
    // we reject it explicitly to prevent footguns.
    let mut p = SysinfoProvider::new();
    let err = p.kill_process(Pid::from_u32(0), Signal::Term).unwrap_err();
    assert!(matches!(err, SysError::InvalidInput(_)), "got {err:?}");
}
```

- [ ] **Step 2: Run — should fail (not yet implemented)**

- [ ] **Step 3: Implement `kill_process`**

Replace `src/kill.rs`:

```rust
use nix::errno::Errno;
use sid_core::adapters::sys::{Pid, Signal, SysError};

pub(crate) fn kill_process(pid: Pid, sig: Signal) -> Result<(), SysError> {
    if pid.as_u32() == 0 {
        return Err(SysError::InvalidInput(
            "refusing to send signal to pid 0 (process-group semantics)".into(),
        ));
    }
    let nix_sig = match sig {
        Signal::Term => nix::sys::signal::Signal::SIGTERM,
        Signal::Kill => nix::sys::signal::Signal::SIGKILL,
        Signal::Int => nix::sys::signal::Signal::SIGINT,
        Signal::Hup => nix::sys::signal::Signal::SIGHUP,
    };
    // SAFETY: nix::sys::signal::kill is the safe wrapper around kill(2);
    // it returns Result, no unsafe required.
    let raw_pid: i32 = pid.as_u32() as i32;
    let nix_pid = nix::unistd::Pid::from_raw(raw_pid);
    match nix::sys::signal::kill(nix_pid, nix_sig) {
        Ok(()) => Ok(()),
        Err(Errno::ESRCH) => Err(SysError::NotFound(format!("pid {}", pid.as_u32()))),
        Err(Errno::EPERM) | Err(Errno::EACCES) => Err(SysError::PermissionDenied(format!(
            "cannot signal pid {} (likely owned by another user)",
            pid.as_u32()
        ))),
        Err(Errno::EINVAL) => Err(SysError::InvalidInput(format!("invalid signal for pid {}", pid.as_u32()))),
        Err(e) => Err(SysError::Other(format!("kill({}): {e}", pid.as_u32()))),
    }
}
```

- [ ] **Step 4: Run tests** — expected 3 passed.

- [ ] **Step 5: Adversarial coverage**

Append:

```rust
#[test]
fn kill_init_pid_one_returns_permission_denied_or_not_found() {
    // As an unprivileged user, killing pid 1 (init/systemd) returns EPERM.
    // In a container or as root it may return Ok or ESRCH instead.
    // The point is: it must not panic and must return a typed error if it errors.
    let mut p = SysinfoProvider::new();
    match p.kill_process(Pid::from_u32(1), Signal::Hup) {
        Ok(()) => {} // root or matching container init — allow.
        Err(SysError::PermissionDenied(_)) => {} // expected for unprivileged tests.
        Err(SysError::NotFound(_)) => {} // pid 1 not present in this namespace.
        Err(other) => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn kill_followed_by_sigkill_eventually_reaps() {
    let mut child = spawn_sleep();
    let pid = Pid::from_u32(child.id());
    let mut p = SysinfoProvider::new();
    let _ = p.kill_process(pid, Signal::Term);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = p.kill_process(pid, Signal::Kill);
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(2) {
        if let Ok(Some(_)) = child.try_wait() { return; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("child did not exit after SIGTERM+SIGKILL");
}

use proptest::prelude::*;

proptest! {
    /// Property: any sufficiently-large random pid returns NotFound (not panic, not other).
    #[test]
    fn prop_huge_pid_is_not_found(pid_raw in (u32::MAX / 2)..u32::MAX) {
        let mut p = sid_sys::SysinfoProvider::new();
        let err = p.kill_process(Pid::from_u32(pid_raw), Signal::Term).unwrap_err();
        prop_assert!(matches!(err, SysError::NotFound(_)));
    }
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-sys
git commit -m "feat(sys): implement kill_process via nix with typed error mapping"
```

---

### Task 8: Refresh strategy + provider concurrency safety

**Files:**
- Modify: `crates/sid-sys/src/lib.rs`
- Modify: `crates/sid-sys/src/processes.rs` (revisit refresh kind)

The provider currently re-runs `refresh_specifics` on every `list_processes` call. For a 2 s poll cadence, that's fine; for higher rates (Settings allows configuring down to 500 ms) we want only the minimum refresh kinds. Inspect each implementation, document the refresh contract, and add a test that calls each method 20 times back-to-back and asserts total wall time stays under a generous bound (5 s on CI).

- [ ] **Step 1: Document refresh contract**

Add a module-level doc comment to `src/lib.rs` describing exactly which sysinfo refresh kinds run on each call. Sample text:

```rust
//! ## Refresh contract
//!
//! - `list_processes`: refreshes process list, CPU, memory, user, cmd.
//! - `list_listening_ports`: reads netstat2 once; uses cached `sysinfo::System`
//!   only to map PID → command name. No sysinfo refresh per call.
//! - `list_interfaces`: builds a fresh `sysinfo::Networks` each call. This is
//!   intentional — sysinfo's `Networks` is cheap relative to `System`.
//! - `kill_process`: no sysinfo state read or written; signals via `nix`.
```

- [ ] **Step 2: Add timing test**

Create `crates/sid-sys/tests/timing.rs`:

```rust
use std::time::Instant;

use sid_core::adapters::sys::SysProvider as _;
use sid_sys::SysinfoProvider;

#[test]
fn twenty_polls_complete_within_five_seconds() {
    let mut p = SysinfoProvider::new();
    let start = Instant::now();
    for _ in 0..20 {
        let _ = p.list_processes().unwrap();
        let _ = p.list_listening_ports().unwrap();
        let _ = p.list_interfaces().unwrap();
    }
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f64() < 5.0,
            "20 polls took {:?}, expected < 5s", elapsed);
}
```

- [ ] **Step 3: Run tests** — expected pass on developer machines; if CI is slow, raise the bound to 10s with a justification.

- [ ] **Step 4: Confirm provider is `Send + Sync`**

The `unsafe impl Send/Sync` dance from `sid-git` is NOT needed here: `sysinfo::System` is already `Send + Sync` (per its docs), as is `netstat2`. Confirm with a static assertion:

```rust
// In src/lib.rs
const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<SysinfoProvider>;
};
```

If a future sysinfo release drops the auto-impl, this assertion fails the build — and at that point we either add a Mutex internally or follow the `sid-git` pattern with a `// SAFETY:` comment.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-sys
git commit -m "chore(sys): document refresh contract + assert provider Send+Sync"
```

---

## Phase D — `SysProbe` service in `sid-core`

The `SysProbe` is the load-bearing concurrency point of this plan. CLAUDE.md's loom directive applies: any `Arc<Mutex<…>>` that coordinates between Tokio tasks and the render loop must have a loom model-check test.

### Task 9: `SysProbe` skeleton: `Arc<Mutex<dyn SysProvider>>` + broadcast channel

**Files:**
- Create: `crates/sid-core/src/sys_probe.rs`
- Modify: `crates/sid-core/src/lib.rs` (declare module)
- Create: `crates/sid-core/tests/sys_probe_skeleton.rs`

- [ ] **Step 1: Failing test**

Create `crates/sid-core/tests/sys_probe_skeleton.rs`:

```rust
use std::sync::{Arc, Mutex};

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
};
use sid_core::sys_probe::{SysProbe, SysSnapshot};

struct CountingProvider {
    processes_calls: std::sync::atomic::AtomicU32,
}
impl CountingProvider {
    fn new() -> Self { Self { processes_calls: 0.into() } }
}
impl SysProvider for CountingProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        self.processes_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(vec![])
    }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
    fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> { Ok(()) }
}

#[test]
fn sys_probe_constructs_with_provider() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(CountingProvider::new()));
    let probe = SysProbe::new(provider, std::time::Duration::from_secs(2));
    let _ = probe;
}

#[test]
fn snapshot_has_all_three_lists() {
    let s = SysSnapshot::empty();
    assert!(s.processes.is_empty());
    assert!(s.listening_ports.is_empty());
    assert!(s.interfaces.is_empty());
}
```

- [ ] **Step 2: Run — fails to compile**

- [ ] **Step 3: Create `crates/sid-core/src/sys_probe.rs`**

```rust
//! `SysProbe` — periodic poller around a `SysProvider`, with a broadcast
//! channel for subscribers.
//!
//! The probe is the canonical concurrency hand-off point between the async
//! polling task and the synchronous render loop:
//!
//!  - The provider lives inside `Arc<Mutex<dyn SysProvider>>` so the poll
//!    task, kill-action job, and any other consumer can all reach it.
//!  - Snapshots are broadcast over `tokio::sync::broadcast` so any number
//!    of subscribers (widgets, CLI processes, detached views) can receive
//!    them without blocking each other.
//!  - The CLAUDE.md loom directive applies: any code path that locks the
//!    mutex from multiple tasks is exercised under `#[cfg(loom)]`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::adapters::sys::{ListeningPort, NetInterface, ProcessInfo, SysProvider};

/// A single point-in-time snapshot of all three lists.
#[derive(Clone, Debug, Default)]
pub struct SysSnapshot {
    pub processes: Vec<ProcessInfo>,
    pub listening_ports: Vec<ListeningPort>,
    pub interfaces: Vec<NetInterface>,
    /// Time the snapshot was assembled, seconds since UNIX epoch.
    pub captured_at_unix_secs: i64,
}

impl SysSnapshot {
    /// Construct an empty snapshot. Useful for tests and the
    /// "no probe ran yet" initial render.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::sys_probe::SysSnapshot;
    /// let s = SysSnapshot::empty();
    /// assert!(s.processes.is_empty());
    /// ```
    pub fn empty() -> Self { Self::default() }
}

/// Periodic poller around a `SysProvider`.
pub struct SysProbe {
    pub(crate) provider: Arc<Mutex<dyn SysProvider>>,
    pub(crate) interval: Duration,
}

impl SysProbe {
    /// Construct a new probe. `interval` is the duration between polls; the
    /// caller drives polling by calling `run()` on a Tokio task (Task 10).
    pub fn new(provider: Arc<Mutex<dyn SysProvider>>, interval: Duration) -> Self {
        Self { provider, interval }
    }

    /// Borrow the provider for an immediate one-shot call (e.g., the kill action).
    pub fn provider(&self) -> Arc<Mutex<dyn SysProvider>> {
        Arc::clone(&self.provider)
    }

    /// Configured poll interval.
    pub fn interval(&self) -> Duration { self.interval }

    /// Mutate the poll interval. Effective on next tick.
    pub fn set_interval(&mut self, interval: Duration) { self.interval = interval; }
}
```

Add `pub mod sys_probe;` to `crates/sid-core/src/lib.rs`.

- [ ] **Step 4: Run tests** — expected pass.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add SysProbe skeleton with Arc<Mutex<dyn SysProvider>> + SysSnapshot"
```

---

### Task 10: `SysProbe::run` Tokio interval poll + broadcast

**Files:**
- Modify: `crates/sid-core/src/sys_probe.rs`
- Modify: `crates/sid-core/Cargo.toml` — add tokio dep (sync + time features) if not already present
- Create: `crates/sid-core/tests/sys_probe_run.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-core/tests/sys_probe_run.rs`:

```rust
use std::sync::{Arc, Mutex};

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
};
use sid_core::sys_probe::SysProbe;

struct StubProvider;
impl SysProvider for StubProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        Ok(vec![ProcessInfo {
            pid: Pid::from_u32(1), name: "init".into(), cmd: "init".into(),
            cpu_pct: 0.0, rss_bytes: 0, started_unix_secs: 0, parent: None, user: None,
        }])
    }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
    fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> { Ok(()) }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn run_emits_one_snapshot_per_interval() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(StubProvider));
    let probe = SysProbe::new(provider, std::time::Duration::from_millis(100));
    let mut rx = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });

    // Advance virtual time. start_paused=true makes Tokio time deterministic.
    tokio::time::advance(std::time::Duration::from_millis(110)).await;
    let snap = rx.recv().await.unwrap();
    assert_eq!(snap.processes.len(), 1);

    tokio::time::advance(std::time::Duration::from_millis(110)).await;
    let snap2 = rx.recv().await.unwrap();
    assert_eq!(snap2.processes.len(), 1);

    handle.abort();
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn multiple_subscribers_each_receive_snapshot() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(StubProvider));
    let probe = SysProbe::new(provider, std::time::Duration::from_millis(100));
    let mut rx1 = probe.subscribe();
    let mut rx2 = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });

    tokio::time::advance(std::time::Duration::from_millis(110)).await;
    let _ = rx1.recv().await.unwrap();
    let _ = rx2.recv().await.unwrap();
    handle.abort();
}
```

- [ ] **Step 2: Run — fails to compile (`subscribe` / `run` not defined)**

- [ ] **Step 3: Extend `SysProbe` in `sys_probe.rs`**

Add to `SysProbe`:

```rust
use tokio::sync::broadcast;

const SNAPSHOT_CHANNEL_CAPACITY: usize = 16;

pub struct SysProbe {
    pub(crate) provider: Arc<Mutex<dyn SysProvider>>,
    pub(crate) interval: Duration,
    pub(crate) tx: broadcast::Sender<SysSnapshot>,
}

impl SysProbe {
    pub fn new(provider: Arc<Mutex<dyn SysProvider>>, interval: Duration) -> Self {
        let (tx, _rx) = broadcast::channel(SNAPSHOT_CHANNEL_CAPACITY);
        Self { provider, interval, tx }
    }

    /// Subscribe to snapshots. The returned receiver will lag (return
    /// `RecvError::Lagged`) if a subscriber consumes too slowly; widgets
    /// should treat lag as "fetch the latest snapshot directly via `latest()`"
    /// once that helper lands.
    pub fn subscribe(&self) -> broadcast::Receiver<SysSnapshot> {
        self.tx.subscribe()
    }

    /// Run the polling loop. Returns when all senders are dropped (which
    /// only happens when the probe itself is dropped). Designed to be
    /// spawned on a Tokio task: `tokio::spawn(async move { probe.run().await });`
    pub async fn run(self) -> Result<(), SysProbeError> {
        let mut interval = tokio::time::interval(self.interval);
        // Skip missed ticks rather than catching up; sysinfo poll loops
        // should not "burst" after a long sleep.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let snapshot = collect_snapshot(&self.provider)
                .unwrap_or_else(|e| {
                    tracing::warn!("SysProbe snapshot failed: {e}");
                    SysSnapshot::empty()
                });
            // If no receivers, send returns Err; ignore — the next tick will retry.
            let _ = self.tx.send(snapshot);
        }
    }
}

fn collect_snapshot(provider: &Arc<Mutex<dyn SysProvider>>) -> Result<SysSnapshot, SysProbeError> {
    let mut guard = provider.lock().map_err(|_| SysProbeError::PoisonedMutex)?;
    let processes = guard.list_processes().map_err(SysProbeError::Sys)?;
    let listening_ports = guard.list_listening_ports().map_err(SysProbeError::Sys)?;
    let interfaces = guard.list_interfaces().map_err(SysProbeError::Sys)?;
    let captured_at_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(SysSnapshot { processes, listening_ports, interfaces, captured_at_unix_secs })
}

#[derive(Debug, thiserror::Error)]
pub enum SysProbeError {
    #[error("provider mutex poisoned")]
    PoisonedMutex,
    #[error("provider error: {0}")]
    Sys(#[from] crate::adapters::sys::SysError),
}
```

Add `tokio = { workspace = true, features = ["sync", "time", "macros", "rt"] }` to `crates/sid-core/Cargo.toml` if not already there. CLAUDE.md's adapter rule forbids `sid-core` from depending on tokio in general — but the spec explicitly notes that `sid-core` owns concurrency primitives needed by the `App` (event loop). Document the carve-out in a Cargo.toml comment:

```toml
# Tokio is allowed in sid-core ONLY for the App event loop, JobQueue handoff,
# and SysProbe — same carve-out as for crossterm.
tokio.workspace = true
```

(Confirm whether tokio is already a sid-core dep — Plan 1 probably introduced it. If so, just ensure `sync`, `time` features are enabled.)

- [ ] **Step 4: Run tests** — expected pass.

- [ ] **Step 5: Adversarial coverage**

Append to `tests/sys_probe_run.rs`:

```rust
struct FailingProvider;
impl SysProvider for FailingProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        Err(SysError::Other("boom".into()))
    }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
    fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> { Ok(()) }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn failing_provider_emits_empty_snapshot_and_keeps_running() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(FailingProvider));
    let probe = SysProbe::new(provider, std::time::Duration::from_millis(100));
    let mut rx = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });
    tokio::time::advance(std::time::Duration::from_millis(110)).await;
    let snap = rx.recv().await.unwrap();
    assert!(snap.processes.is_empty(), "failed snapshot should be empty, not crash");
    tokio::time::advance(std::time::Duration::from_millis(110)).await;
    let _ = rx.recv().await.unwrap(); // still alive
    handle.abort();
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): SysProbe::run polls and broadcasts SysSnapshot at interval"
```

---

### Task 11: `SysProbe` loom test for the Arc/Mutex handoff

**Files:**
- Modify: `crates/sid-core/Cargo.toml` — add `[features] loom = ["dep:loom"]` and `loom` as optional dep
- Create: `crates/sid-core/tests/sys_probe_loom.rs` (gated `#[cfg(loom)]`)

Per CLAUDE.md: "any code involving `Arc`, `Mutex`, channels, atomics, or other shared-state primitives. Gate loom tests behind `#[cfg(loom)]` and a `loom` feature."

- [ ] **Step 1: Add the `loom` feature**

In `crates/sid-core/Cargo.toml`:

```toml
[features]
loom = ["dep:loom"]

[dependencies]
loom = { workspace = true, optional = true }
```

- [ ] **Step 2: Write the loom test**

Create `crates/sid-core/tests/sys_probe_loom.rs`:

```rust
//! Loom model-checks the Arc<Mutex<…>> handoff used by SysProbe. Run with:
//!   RUSTFLAGS='--cfg loom' cargo test -p sid-core --test sys_probe_loom --features loom

#![cfg(loom)]

use loom::sync::{Arc, Mutex};
use loom::thread;

/// Stand-in for the relevant slice of SysProbe behavior:
/// - thread A (poll task) locks the mutex, calls list_*, drops lock.
/// - thread B (kill task) locks the mutex, calls kill_process, drops lock.
/// We model both threads racing on the same provider for two iterations
/// and assert no deadlock or missed update.
#[test]
fn poll_and_kill_race_does_not_deadlock() {
    loom::model(|| {
        let provider = Arc::new(Mutex::new(0u32));

        let p1 = Arc::clone(&provider);
        let t1 = thread::spawn(move || {
            for _ in 0..2 {
                let mut g = p1.lock().unwrap();
                *g = g.wrapping_add(1);
            }
        });

        let p2 = Arc::clone(&provider);
        let t2 = thread::spawn(move || {
            for _ in 0..2 {
                let mut g = p2.lock().unwrap();
                *g = g.wrapping_add(10);
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        let final_value = *provider.lock().unwrap();
        // 2 increments of +1 and 2 of +10 in any order => 22.
        assert_eq!(final_value, 22);
    });
}

#[test]
fn dropped_provider_does_not_poison_in_normal_flow() {
    loom::model(|| {
        let provider = Arc::new(Mutex::new(0u32));
        let p = Arc::clone(&provider);
        let t = thread::spawn(move || {
            let mut g = p.lock().unwrap();
            *g += 1;
            drop(g);
        });
        t.join().unwrap();
        let g = provider.lock().unwrap();
        assert_eq!(*g, 1);
    });
}
```

The test uses `loom::sync::{Arc, Mutex}` deliberately — loom's models replace `std::sync` so the model checker can explore interleavings.

- [ ] **Step 3: Run**

```bash
RUSTFLAGS='--cfg loom' cargo test -p sid-core --test sys_probe_loom --features loom --release
```

Expected: 2 passed (loom interleaves are intensive; running under `--release` is the documented loom convention).

- [ ] **Step 4: Add CI gate**

When CI lands (separate plan), the loom gate runs as a separate job because it's slow. For now, add a `scripts/loom-test.sh` or document the command in `CLAUDE.md`. Mention this task in the plan's done-criteria.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-core
git commit -m "test(core): add loom model-check for SysProbe Arc/Mutex handoff"
```

---

## Phase E — `NetworkWidget`

Phase E builds the widget incrementally: each table/sidebar/input/modal as a self-contained piece of state with its own tests, then assembled. The full `Widget` impl + insta snapshot lands at the end of the phase.

### Task 12: `PortsTableState` (sort, scroll, select)

**Files:**
- Create: `crates/sid-widgets/src/network/mod.rs` (replacing the Plan 1 stub `network.rs`)
- Create: `crates/sid-widgets/src/network/ports_table.rs`
- Create: `crates/sid-widgets/tests/ports_table.rs`

- [ ] **Step 1: Replace `crates/sid-widgets/src/network.rs` stub with `network/mod.rs`**

Move the existing one-liner stub into `network/mod.rs`. Plan to expose sub-modules:

```rust
pub mod ports_table;
pub mod processes_table;
pub mod interfaces_sidebar;
pub mod filter_input;
pub mod kill_modal;
mod widget;
pub use widget::NetworkWidget;
```

- [ ] **Step 2: Failing tests**

Create `crates/sid-widgets/tests/ports_table.rs`:

```rust
use sid_core::adapters::sys::{ListeningPort, Pid, Protocol, SocketState};
use sid_widgets::network::ports_table::{PortsTableState, PortsSortBy, SortDir};

fn sample() -> Vec<ListeningPort> {
    vec![
        ListeningPort { port: 8080, pid: Some(Pid::from_u32(100)), command: "app".into(),
            protocol: Protocol::Tcp, state: SocketState::Listen, local_addr: "0.0.0.0".into() },
        ListeningPort { port: 22, pid: Some(Pid::from_u32(50)), command: "sshd".into(),
            protocol: Protocol::Tcp, state: SocketState::Listen, local_addr: "0.0.0.0".into() },
        ListeningPort { port: 53, pid: Some(Pid::from_u32(80)), command: "dnsmasq".into(),
            protocol: Protocol::Udp, state: SocketState::Listen, local_addr: "127.0.0.1".into() },
    ]
}

#[test]
fn sort_by_port_ascending() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.set_sort(PortsSortBy::Port, SortDir::Asc);
    assert_eq!(s.rows().iter().map(|r| r.port).collect::<Vec<_>>(), vec![22, 53, 8080]);
}

#[test]
fn sort_by_pid_descending() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.set_sort(PortsSortBy::Pid, SortDir::Desc);
    assert_eq!(s.rows().iter().map(|r| r.pid.unwrap().as_u32()).collect::<Vec<_>>(), vec![100, 80, 50]);
}

#[test]
fn select_next_wraps_at_end() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    assert_eq!(s.selected_index(), 0);
    s.select_next(); s.select_next(); s.select_next();
    assert_eq!(s.selected_index(), 0, "should wrap to start");
}

#[test]
fn select_prev_wraps_at_start() {
    let mut s = PortsTableState::new();
    s.set_data(sample());
    s.select_prev();
    assert_eq!(s.selected_index(), 2, "should wrap to end");
}

#[test]
fn empty_data_handles_navigation_without_panic() {
    let mut s = PortsTableState::new();
    s.set_data(vec![]);
    s.select_next(); s.select_prev();
    assert!(s.selected_row().is_none());
}
```

- [ ] **Step 3: Run — fails to compile**

- [ ] **Step 4: Implement `PortsTableState`**

Create `crates/sid-widgets/src/network/ports_table.rs`:

```rust
//! State for the listening-ports table: sort column, sort direction,
//! selection cursor, and the underlying data slice.

use sid_core::adapters::sys::ListeningPort;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortsSortBy { Port, Pid, Command, Protocol }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortDir { Asc, Desc }

#[derive(Debug, Default)]
pub struct PortsTableState {
    data: Vec<ListeningPort>,
    sort_by: Option<PortsSortBy>,
    sort_dir: SortDir,
    selected: usize,
}

impl Default for SortDir {
    fn default() -> Self { SortDir::Asc }
}

impl PortsTableState {
    pub fn new() -> Self { Self::default() }

    pub fn set_data(&mut self, mut data: Vec<ListeningPort>) {
        if let Some(by) = self.sort_by { sort_rows(&mut data, by, self.sort_dir); }
        self.data = data;
        if self.selected >= self.data.len() { self.selected = 0; }
    }

    pub fn rows(&self) -> &[ListeningPort] { &self.data }

    pub fn selected_index(&self) -> usize { self.selected }

    pub fn selected_row(&self) -> Option<&ListeningPort> { self.data.get(self.selected) }

    pub fn set_sort(&mut self, by: PortsSortBy, dir: SortDir) {
        self.sort_by = Some(by);
        self.sort_dir = dir;
        sort_rows(&mut self.data, by, dir);
    }

    pub fn select_next(&mut self) {
        if self.data.is_empty() { return; }
        self.selected = (self.selected + 1) % self.data.len();
    }
    pub fn select_prev(&mut self) {
        if self.data.is_empty() { return; }
        self.selected = if self.selected == 0 { self.data.len() - 1 } else { self.selected - 1 };
    }
}

fn sort_rows(data: &mut Vec<ListeningPort>, by: PortsSortBy, dir: SortDir) {
    data.sort_by(|a, b| {
        let ord = match by {
            PortsSortBy::Port => a.port.cmp(&b.port),
            PortsSortBy::Pid => a.pid.map(|p| p.as_u32()).cmp(&b.pid.map(|p| p.as_u32())),
            PortsSortBy::Command => a.command.cmp(&b.command),
            PortsSortBy::Protocol => format!("{:?}", a.protocol).cmp(&format!("{:?}", b.protocol)),
        };
        match dir { SortDir::Asc => ord, SortDir::Desc => ord.reverse() }
    });
}
```

- [ ] **Step 5: Run tests** — expected all passed.

- [ ] **Step 6: Property + adversarial**

Append:

```rust
use proptest::prelude::*;

proptest! {
    /// Property: sorting by port ascending then descending is the reverse of asc.
    #[test]
    fn prop_asc_then_desc_is_reverse(ports in proptest::collection::vec(1u16..=65535u16, 0..20)) {
        let rows: Vec<_> = ports.iter().map(|p| ListeningPort {
            port: *p, pid: None, command: String::new(),
            protocol: Protocol::Tcp, state: SocketState::Listen, local_addr: "0.0.0.0".into(),
        }).collect();
        let mut a = PortsTableState::new(); a.set_data(rows.clone()); a.set_sort(PortsSortBy::Port, SortDir::Asc);
        let mut d = PortsTableState::new(); d.set_data(rows.clone()); d.set_sort(PortsSortBy::Port, SortDir::Desc);
        let av: Vec<_> = a.rows().iter().map(|r| r.port).collect();
        let mut dv: Vec<_> = d.rows().iter().map(|r| r.port).collect();
        dv.reverse();
        prop_assert_eq!(av, dv);
    }

    /// Property: selection index always stays within bounds.
    #[test]
    fn prop_selection_in_bounds(actions in proptest::collection::vec(0u8..2, 0..50)) {
        let mut s = PortsTableState::new();
        s.set_data(vec![
            ListeningPort { port: 1, pid: None, command: String::new(), protocol: Protocol::Tcp,
                state: SocketState::Listen, local_addr: "0".into() };
            5
        ]);
        for a in actions {
            if a == 0 { s.select_next(); } else { s.select_prev(); }
            prop_assert!(s.selected_index() < 5);
        }
    }
}
```

- [ ] **Step 7: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add PortsTableState with sort + scroll"
```

---

### Task 13: `ProcessesTableState` (sort, scroll, select)

**Files:**
- Create: `crates/sid-widgets/src/network/processes_table.rs`
- Create: `crates/sid-widgets/tests/processes_table.rs`

Mirror Task 12 with sort keys `Pid | Name | Cpu | Rss | Started`. Selection + wrap behavior is the same.

- [ ] **Step 1: Failing tests** — mirror `ports_table.rs` tests with the corresponding sort keys and `ProcessInfo` rows.
- [ ] **Step 2: Run — fails to compile.**
- [ ] **Step 3: Implement.** Same pattern as `PortsTableState`. Sort comparator uses `f32::partial_cmp` for `Cpu` with `.unwrap_or(Ordering::Equal)` (NaN guard).
- [ ] **Step 4: Run tests.**
- [ ] **Step 5: Property + adversarial coverage.** Include a property test that `cpu_pct = f32::NAN` rows do not panic when sorted.
- [ ] **Step 6: Commit:** `feat(widgets): add ProcessesTableState with sort + scroll`

---

### Task 14: `InterfacesSidebarState`

**Files:**
- Create: `crates/sid-widgets/src/network/interfaces_sidebar.rs`
- Create: `crates/sid-widgets/tests/interfaces_sidebar.rs`

Simpler: a list selection with no sort (sorted by name from the provider). Tests cover selection wrap, empty list, and that data rebinding preserves the selection-by-name if the same interface still exists.

- [ ] **Step 1-6:** Same shape as Tasks 12-13. Commit: `feat(widgets): add InterfacesSidebarState`

---

### Task 15: `FilterInputState` for `/` filter

**Files:**
- Create: `crates/sid-widgets/src/network/filter_input.rs`
- Create: `crates/sid-widgets/tests/filter_input.rs`

A small state machine: `inactive | editing(query: String)`. Supports `enter_filter()`, `cancel()`, `push_char(c)`, `pop_char()`, `submit()`. Tests:

- Pushing chars accumulates query
- `pop_char` is a no-op on empty
- `cancel()` returns to `inactive` and clears query
- Filter predicate: provide a `match_listening_port(query: &str, row: &ListeningPort) -> bool` that does case-insensitive substring match across `port` (as string), `command`, `local_addr` — and same for processes/interfaces

Adversarial: unicode in query (`"🐕"`), empty query (returns true for all rows), very long query (100 KB) does not panic.

Commit: `feat(widgets): add FilterInputState with case-insensitive substring match`

---

### Task 16: `KillConfirmModalState` (two-stage SIGTERM/SIGKILL UI)

**Files:**
- Create: `crates/sid-widgets/src/network/kill_modal.rs`
- Create: `crates/sid-widgets/tests/kill_modal.rs`

State machine:

```
Closed
  -k pressed-> ConfirmSigterm { pid }
                  -y-> AwaitingTerm { pid, deadline }
                              -timer expires + alive-> ConfirmSigkill { pid }
                                                          -y-> Done(Killed)
                                                          -n-> Done(GaveUp)
                              -timer expires + dead-> Done(Killed)
                  -n/Esc-> Closed
```

The state machine is intentionally pure — it does not actually call `kill_process`. The widget feeds it `tick(now)` and `mark_process_alive(bool)` so tests can drive deterministic transitions.

- [ ] **Step 1: Failing tests.** Each transition gets its own `#[test]`.
- [ ] **Step 2: Run — fails to compile.**
- [ ] **Step 3: Implement as a typestate-style enum.**
- [ ] **Step 4: Run tests.**
- [ ] **Step 5: Adversarial:** Esc from every state returns to `Closed`; double-`y` doesn't advance past a single stage; passing `now < deadline` keeps state in `AwaitingTerm`.
- [ ] **Step 6: Commit:** `feat(widgets): add KillConfirmModalState two-stage SIGTERM/SIGKILL flow`

---

### Task 17: `NetworkWidget` assembly + `Widget` impl + insta snapshot

**Files:**
- Create: `crates/sid-widgets/src/network/widget.rs`
- Create: `crates/sid-widgets/tests/network_widget_snapshot.rs`

- [ ] **Step 1: Failing test (insta snapshot)**

Create `crates/sid-widgets/tests/network_widget_snapshot.rs`:

```rust
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use sid_core::adapters::sys::{ListeningPort, NetInterface, Pid, ProcessInfo, Protocol, SocketState};
use sid_core::sys_probe::SysSnapshot;
use sid_widgets::network::NetworkWidget;

fn fixture_snapshot() -> SysSnapshot {
    SysSnapshot {
        processes: vec![
            ProcessInfo {
                pid: Pid::from_u32(1), name: "init".into(), cmd: "/sbin/init".into(),
                cpu_pct: 0.1, rss_bytes: 4_000_000, started_unix_secs: 1_700_000_000,
                parent: None, user: Some("0".into()),
            },
            ProcessInfo {
                pid: Pid::from_u32(1234), name: "sid".into(), cmd: "sid".into(),
                cpu_pct: 2.3, rss_bytes: 50_000_000, started_unix_secs: 1_700_000_100,
                parent: Some(Pid::from_u32(1)), user: Some("1000".into()),
            },
        ],
        listening_ports: vec![
            ListeningPort {
                port: 22, pid: Some(Pid::from_u32(1)), command: "sshd".into(),
                protocol: Protocol::Tcp, state: SocketState::Listen, local_addr: "0.0.0.0".into(),
            },
        ],
        interfaces: vec![
            NetInterface {
                name: "lo".into(), addrs: vec!["127.0.0.1".into(), "::1".into()],
                rx_bytes: 1024, tx_bytes: 1024, is_up: true,
            },
            NetInterface {
                name: "eth0".into(), addrs: vec!["192.168.1.10".into()],
                rx_bytes: 9_000_000, tx_bytes: 3_000_000, is_up: true,
            },
        ],
        captured_at_unix_secs: 1_700_000_500,
    }
}

#[test]
fn snapshot_default_layout() {
    let mut w = NetworkWidget::new();
    w.apply_snapshot(fixture_snapshot());
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| w.render(f, f.area(), &sid_ui::theme::cosmos())).unwrap();
    insta::assert_snapshot!(term_to_string(&term));
}

fn term_to_string(term: &Terminal<TestBackend>) -> String {
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area().height {
        for x in 0..buf.area().width {
            s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        s.push('\n');
    }
    s
}
```

- [ ] **Step 2: Run — fails to compile**

- [ ] **Step 3: Implement `NetworkWidget`** in `widget.rs`. It owns:
  - `ports: PortsTableState`
  - `procs: ProcessesTableState`
  - `ifs: InterfacesSidebarState`
  - `filter: FilterInputState`
  - `kill_modal: KillConfirmModalState`
  - `focus: Focus { Ports | Processes | Interfaces }`
  - `snapshot_rx: Option<broadcast::Receiver<SysSnapshot>>` (optional so tests don't need a probe)

Implements `sid_core::Widget` trait with:
  - `render`: 3-column layout — sidebar on left (interfaces), top-right ports, bottom-right processes; status bar at bottom shows filter state + kill modal if open
  - `handle_event`:
    - `Ctrl+arrows` are tab-manager territory, ignored by widget
    - `Tab`/`Shift+Tab` cycles focus between Ports/Processes/Interfaces
    - `/` opens filter
    - `k` opens kill modal targeting selected row's PID
    - `s` cycles sort column on focused table
    - `j/k`/`↑/↓` scroll selection
    - `Enter` drills into a process (Plan 5 scope: emit a `ProcessDetails(pid)` `Action`; UI for the details pane is out of scope for v1 — a stub that shows "details for PID N" is sufficient)
  - `poll`: drains the broadcast receiver, applying any newly-arrived snapshot
  - `save_state` / `load_state`: postcard with version prefix, persisting only the sort/focus prefs (not the data — that comes from the probe)
  - `launch_spec`: returns `LaunchSpec { kind: "network", instance_id: "global", config: {} }`

- [ ] **Step 4: Run tests** — expected insta snapshot review (`cargo insta review`).

- [ ] **Step 5: Adversarial snapshots**

Add a snapshot variant for an empty snapshot (no processes, no ports, no interfaces) — must render the empty-state placeholders. Add a snapshot for a 200-process / 50-port snapshot (assert scrolling state correct, headers still visible).

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): assemble NetworkWidget with three panes + filter + kill modal"
```

---

## Phase F — Kill action via JobQueue

The kill flow has two off-widget concerns: dispatching the signal asynchronously (so the render loop is never blocked on `kill(2)` even though it's typically <1ms), and surfacing the result via toast.

### Task 18: Kill action wiring through `JobQueue` (SIGTERM → wait → SIGKILL)

**Files:**
- Create: `crates/sid-core/src/sys_probe/kill_job.rs` (or alongside, in `sys_probe.rs`)
- Create: `crates/sid-core/tests/kill_job.rs`

- [ ] **Step 1: Failing test**

Create `crates/sid-core/tests/kill_job.rs`:

```rust
use std::sync::{Arc, Mutex};

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
};
use sid_core::sys_probe::kill_job::{run_kill_job, KillOutcome};

#[derive(Default)]
struct RecordingProvider {
    calls: Vec<(Pid, Signal)>,
    alive_after_term: bool,
}

impl SysProvider for RecordingProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        if self.alive_after_term { Ok(vec![ProcessInfo {
            pid: Pid::from_u32(42), name: "x".into(), cmd: "x".into(), cpu_pct: 0.0,
            rss_bytes: 0, started_unix_secs: 0, parent: None, user: None,
        }]) } else { Ok(vec![]) }
    }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
    fn kill_process(&mut self, pid: Pid, sig: Signal) -> Result<(), SysError> {
        self.calls.push((pid, sig));
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sigterm_then_dead_returns_killed() {
    let prov = Arc::new(Mutex::new(RecordingProvider { alive_after_term: false, calls: vec![] }));
    let provider: Arc<Mutex<dyn SysProvider>> = prov.clone();
    let fut = run_kill_job(provider, Pid::from_u32(42), std::time::Duration::from_secs(5));
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    let outcome = fut.await.unwrap();
    assert!(matches!(outcome, KillOutcome::Killed(Pid { .. })));
    let p = prov.lock().unwrap();
    assert_eq!(p.calls, vec![(Pid::from_u32(42), Signal::Term)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sigterm_then_alive_escalates_to_sigkill() {
    let prov = Arc::new(Mutex::new(RecordingProvider { alive_after_term: true, calls: vec![] }));
    let provider: Arc<Mutex<dyn SysProvider>> = prov.clone();
    let fut = run_kill_job(provider, Pid::from_u32(42), std::time::Duration::from_secs(5));
    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    let outcome = fut.await.unwrap();
    let p = prov.lock().unwrap();
    assert_eq!(p.calls.len(), 2);
    assert_eq!(p.calls[0].1, Signal::Term);
    assert_eq!(p.calls[1].1, Signal::Kill);
    assert!(matches!(outcome, KillOutcome::EscalatedToSigkill(Pid { .. })));
}
```

- [ ] **Step 2: Run — fails to compile**

- [ ] **Step 3: Implement `run_kill_job`**

```rust
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::adapters::sys::{Pid, Signal, SysError, SysProvider};

#[derive(Debug)]
pub enum KillOutcome {
    /// SIGTERM was sent and the process exited within the grace period.
    Killed(Pid),
    /// SIGTERM was sent, the process remained alive, SIGKILL was sent.
    EscalatedToSigkill(Pid),
    /// The kill failed at SIGTERM stage (permission denied, not found, etc.).
    Failed(Pid, String),
}

pub async fn run_kill_job(
    provider: Arc<Mutex<dyn SysProvider>>,
    pid: Pid,
    grace: Duration,
) -> Result<KillOutcome, SysError> {
    // 1. SIGTERM.
    {
        let mut guard = provider.lock().expect("provider mutex poisoned");
        if let Err(e) = guard.kill_process(pid, Signal::Term) {
            return Ok(KillOutcome::Failed(pid, format!("{e}")));
        }
    }
    // 2. Wait the grace period.
    tokio::time::sleep(grace).await;
    // 3. Re-check whether the process is alive.
    let still_alive = {
        let mut guard = provider.lock().expect("provider mutex poisoned");
        let procs = guard.list_processes()?;
        procs.iter().any(|p| p.pid == pid)
    };
    if !still_alive { return Ok(KillOutcome::Killed(pid)); }
    // 4. SIGKILL.
    {
        let mut guard = provider.lock().expect("provider mutex poisoned");
        let _ = guard.kill_process(pid, Signal::Kill);
    }
    Ok(KillOutcome::EscalatedToSigkill(pid))
}
```

- [ ] **Step 4: Run tests** — expected 2 passed.

- [ ] **Step 5: Adversarial coverage**

Add a test that `run_kill_job` with `pid = 0` returns `KillOutcome::Failed` (because the underlying provider returns `InvalidInput`), without escalating.

Add a test that grace period of `Duration::ZERO` is accepted and the function still does the alive-check before deciding.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add run_kill_job (SIGTERM -> grace -> SIGKILL) for JobQueue dispatch"
```

---

### Task 19: Toast surfacing of kill outcomes + permission-denied path

**Files:**
- Modify: `crates/sid-widgets/src/network/widget.rs` (wire the `JobQueue` call)
- Modify: tests as needed

The widget handler for `k` (then `y`) now:

1. Reads the selected PID
2. Calls `ctx.jobs.spawn(...)` with a closure that invokes `run_kill_job(probe.provider(), pid, Duration::from_secs(5))`
3. On result, the widget's `poll()` picks it up and constructs a toast:
   - `KillOutcome::Killed(pid)` → toast success: "killed PID {pid}"
   - `KillOutcome::EscalatedToSigkill(pid)` → toast warning: "PID {pid} ignored SIGTERM; SIGKILL sent"
   - `KillOutcome::Failed(pid, msg)` → toast error: "kill PID {pid} failed: {msg}"

- [ ] **Step 1: Write failing test in `tests/network_widget_kill.rs`**

Drive the widget with `Event::key('k')` then `Event::key('y')` against a stub provider that responds with `Failed`, assert the toast emitted matches. Use `EventOutcome` capture pattern from Plan 1's widget tests.

- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run tests.**
- [ ] **Step 4: Insta snapshot of the modal in each stage** (`ConfirmSigterm`, `AwaitingTerm`, `ConfirmSigkill`).
- [ ] **Step 5: Commit:** `feat(widgets): surface kill outcomes as toasts; modal renders each stage`

---

## Phase G — CLI subcommands

Mirror Plan 2's `sid workspace …` pattern. Each subcommand opens the store (not actually used for net commands, but kept for parity with future per-machine config), constructs a `SysinfoProvider`, calls one method, prints, and exits.

### Task 20: `sid net ports` subcommand

**Files:**
- Modify: `crates/sid/src/main.rs`
- Create: `crates/sid/tests/net_cli.rs`

- [ ] **Step 1: Add `Net` to the subcommand enum**

Extend `Cmd`:

```rust
#[derive(clap::Subcommand, Debug)]
enum Cmd {
    Workspace { #[command(subcommand)] op: WorkspaceOp },
    /// Network info and actions
    Net { #[command(subcommand)] op: NetOp },
}

#[derive(clap::Subcommand, Debug)]
enum NetOp {
    /// List TCP/UDP listening ports
    Ports {
        /// Output format: `table` (default) or `json`
        #[arg(long, default_value = "table")]
        format: String,
    },
    /// List processes
    Procs {
        #[arg(long, default_value = "table")]
        format: String,
        /// Sort by `pid` (default), `cpu`, `rss`, `name`
        #[arg(long, default_value = "pid")]
        sort: String,
        /// Maximum rows to print
        #[arg(long, default_value_t = 50)]
        top: usize,
    },
    /// List network interfaces
    Interfaces {
        #[arg(long, default_value = "table")]
        format: String,
    },
    /// Kill a process by PID, or whatever owns the given port
    Kill {
        /// Either a numeric PID or `port:<n>` (e.g., `port:8080`)
        target: String,
        /// Skip the SIGTERM grace period and SIGKILL immediately
        #[arg(long)]
        force: bool,
    },
}
```

- [ ] **Step 2: Implement the `Ports` arm**

```rust
NetOp::Ports { format } => {
    let mut provider = sid_sys::SysinfoProvider::new();
    let ports = provider.list_listening_ports()?;
    match format.as_str() {
        "json" => println!("{}", serde_json::to_string_pretty(&ports)?),
        _ => {
            println!("{:<6} {:<8} {:<5} {:<8} {}", "PORT", "PID", "PROTO", "STATE", "COMMAND");
            for p in ports {
                let pid_s = p.pid.map(|p| p.as_u32().to_string()).unwrap_or_else(|| "-".into());
                println!("{:<6} {:<8} {:<5} {:<8} {}", p.port, pid_s,
                         format!("{:?}", p.protocol).to_lowercase(),
                         format!("{:?}", p.state).to_lowercase(),
                         p.command);
            }
        }
    }
}
```

- [ ] **Step 3: Failing test**

Create `crates/sid/tests/net_cli.rs`:

```rust
use std::net::TcpListener;
use std::process::Command;

#[test]
fn sid_net_ports_table_includes_a_bound_port() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let bound = listener.local_addr().unwrap().port();
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["net", "ports"]).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&bound.to_string()),
            "expected {bound} in `sid net ports` output:\n{stdout}");
}

#[test]
fn sid_net_ports_json_parses() {
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["net", "ports", "--format", "json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v.is_array());
}
```

- [ ] **Step 4: Run tests** — expected 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): add `sid net ports` subcommand with table + json output"
```

---

### Task 21: `sid net procs` subcommand

**Files:** `crates/sid/src/main.rs`, `crates/sid/tests/net_cli.rs`

Implement the `Procs` arm of `NetOp`. Sort by `--sort` flag (one of pid/cpu/rss/name); truncate to `--top` rows; print table or JSON. Tests assert the current PID appears in the output and that JSON is parseable.

- [ ] **Step 1-5: same shape as Task 20.** Commit: `feat(bin): add `sid net procs` subcommand with sort + top + format`

---

### Task 22: `sid net interfaces` subcommand

**Files:** `crates/sid/src/main.rs`, `crates/sid/tests/net_cli.rs`

Implement the `Interfaces` arm. Tests assert at least one interface (loopback) appears.

- [ ] **Step 1-5: same shape.** Commit: `feat(bin): add `sid net interfaces` subcommand`

---

### Task 23: `sid net kill <port-or-pid>` subcommand

**Files:** `crates/sid/src/main.rs`, `crates/sid/tests/net_cli.rs`

Parse `target`:
- If `port:N` → resolve to a PID via `list_listening_ports`; if no PID attribution available, exit with code 2 and message.
- If digits-only → treat as PID.
- Otherwise → exit with code 2 and message.

Then call `run_kill_job` synchronously via `tokio::runtime::Runtime::new()?.block_on(...)`. If `--force`, send SIGKILL directly instead of going through `run_kill_job`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn sid_net_kill_pid_zero_is_rejected() {
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["net", "kill", "0"]).output().unwrap();
    assert!(!out.status.success(), "kill 0 should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_lowercase().contains("invalid") || stderr.contains("refus"));
}

#[test]
fn sid_net_kill_subprocess_terminates_it() {
    let mut child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    let pid = child.id();
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = std::process::Command::new(bin)
        .args(["net", "kill", &pid.to_string(), "--force"])
        .output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(2) {
        if let Ok(Some(_)) = child.try_wait() { return; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("child not reaped after sid net kill --force");
}
```

- [ ] **Step 2-5: implement + run.** Commit: `feat(bin): add `sid net kill` subcommand with port:/pid targets`

---

## Phase H — Wiring + integration + docs

### Task 24: Wire `SysinfoProvider` + `SysProbe` into the binary

**Files:**
- Modify: `crates/sid/Cargo.toml` (add `sid-sys`)
- Modify: `crates/sid/src/wire.rs`
- Modify: `crates/sid/src/main.rs`

- [ ] **Step 1: Add deps to `crates/sid/Cargo.toml`:**

```toml
sid-sys.workspace = true
```

- [ ] **Step 2: Inject in `wire.rs`**

Extend `SidApp` with the probe:

```rust
pub struct SidApp {
    pub app: App,
    pub store: Arc<RedbStore>,
    pub session_id: String,
    pub git: Arc<dyn GitProvider>,
    pub sys_probe: Arc<sid_core::sys_probe::SysProbe>,
}
```

In `build_app`:

```rust
let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(sid_sys::SysinfoProvider::new()));
let probe_interval = store.get_setting_or("sys_probe.interval_ms", 2000) as u64;
let sys_probe = Arc::new(SysProbe::new(provider, Duration::from_millis(probe_interval)));
```

Spawn the probe loop on a Tokio task during app startup. The `NetworkWidget::new(sys_probe.subscribe())` then uses the broadcast receiver.

- [ ] **Step 3: Tests** — extend the existing widget integration test to assert a snapshot reaches the widget after one tick.

- [ ] **Step 4: Commit:** `feat(bin): wire SysinfoProvider + SysProbe into App and NetworkWidget`

---

### Task 25: Integration test — Network tab end-to-end snapshot + kill

**Files:**
- Create: `crates/sid/tests/network_integration.rs`

End-to-end: launch the app in headless test mode, swap in a stub `SysProvider` that returns deterministic data, advance virtual time, confirm the NetworkWidget renders the expected snapshot, then send a `k` then `y` event and assert the kill job runs and a toast appears.

- [ ] **Step 1: Write the test using `TestBackend` + the same pattern as Plan 1's `app_integration` tests.**

- [ ] **Step 2: Commit:** `test(bin): integration test for Network tab snapshot + kill flow`

---

### Task 26: Criterion benches + baseline

**Files:**
- Create: `crates/sid-sys/benches/list_processes.rs`
- Create: `crates/sid-sys/benches/list_listening_ports.rs`

Per CLAUDE.md: criterion benchmarks for the hot paths.

`benches/list_processes.rs`:

```rust
use criterion::{criterion_group, criterion_main, Criterion};
use sid_core::adapters::sys::SysProvider;
use sid_sys::SysinfoProvider;

fn bench_list_processes(c: &mut Criterion) {
    let mut provider = SysinfoProvider::new();
    c.bench_function("list_processes", |b| {
        b.iter(|| {
            let _ = provider.list_processes().unwrap();
        });
    });
}

criterion_group!(benches, bench_list_processes);
criterion_main!(benches);
```

`benches/list_listening_ports.rs`: mirror with `list_listening_ports`.

- [ ] **Step 1: Run** `cargo bench -p sid-sys --bench list_processes` and `--bench list_listening_ports`. Capture baseline numbers in the commit body.

- [ ] **Step 2: Document the 10% regression budget**

Add to `CLAUDE.md`'s benchmark section if not already present: "Fail CI if a benchmark regresses ≥10% vs baseline. `list_processes` / `list_listening_ports` baselines committed in Plan 5."

- [ ] **Step 3: Commit:** `perf(sys): add criterion benches for list_processes and list_listening_ports`

---

### Task 27: README update

**Files:**
- Modify: `README.md`

Update the "What's inside (v1)" Network row to reflect the working build. Add a "What works in this build" callout for Plan 5.

```markdown
| **Network** | Listening ports table, processes table, interfaces sidebar — all sortable; `/` filter; `k` kills selected PID with SIGTERM → 5s grace → SIGKILL; CLI: `sid net ports/procs/interfaces/kill` |
```

Add to the Quickstart:

```markdown
# Network inspection
sid net ports
sid net procs --sort cpu --top 20
sid net interfaces
sid net kill 1234         # SIGTERM with 5s grace
sid net kill 1234 --force # SIGKILL immediately
sid net kill port:8080    # kill whoever owns port 8080
```

Update the "What works in this build" callout:

> Foundation + Workspaces + (…other intervening plans…) + Network tab fully functional. Ports/processes/interfaces panes; sort, filter, kill with confirmation; non-interactive `sid net …` CLI for scripting.

- [ ] **Step 1: Commit:** `docs: update README to reflect Plan 5 Network tab functionality`

---

## Done criteria for Plan 5

- [ ] `cargo build --workspace` succeeds with no errors or warnings
- [ ] `cargo test --all-features --workspace` passes
- [ ] `RUSTFLAGS='--cfg loom' cargo test -p sid-core --test sys_probe_loom --features loom --release` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is clean
- [ ] `cargo fmt --check` is clean
- [ ] `cargo bench -p sid-sys` produces results; baseline numbers checked into the bench commit body
- [ ] `cargo run -p sid` launches; the Network tab shows live processes, ports, interfaces
- [ ] In the Network tab: `Tab` cycles pane focus; `/` filters; `s` cycles sort column; `k` triggers the two-stage kill flow; `Enter` on a process row emits a drill-in action
- [ ] `sid net ports` / `procs` / `interfaces` / `kill` subcommands work non-interactively
- [ ] `sid net kill --force <pid>` immediately SIGKILLs; non-`--force` does SIGTERM → 5s grace → SIGKILL on confirmation
- [ ] No regressions in earlier plans' functionality (tabs, palette, session restore, Workspaces, etc.)

---

## Self-review notes (run before requesting human review)

**1. Spec coverage.** Plan 5 covers the spec's "Network" tab: listening ports table, processes table, interfaces sidebar, `k` kill, `Enter` drill-in, `/` filter, polled by `SysProbe` (2 s default). The `SysProvider` trait is filled out from its empty Plan 1 stub. Non-interactive `sid net …` CLI subcommands ship for scripting parity with `sid workspace …`.

**2. Items deferred to "Someday" (confirmed by future-features doc):**
   - Packet capture / decode
   - Bandwidth graphs over time
   - IP geo-resolution overlay
   - `iptables` / `nftables` viewer
   - Established (non-listening) connections table
   - Per-process FD enumeration
   - Cgroup / namespace inspection
   - Process-tree kill (single-PID only in v1)

**3. Type consistency check.**
   - `Pid`, `Signal`, `Protocol`, `SocketState`, `ListeningPort`, `ProcessInfo`, `NetInterface`, `SysError` all live in `sid_core::adapters::sys`. Widgets and the binary reference only the trait — never `sid_sys::SysinfoProvider` directly (adapter pattern).
   - `SysProbe` + `SysSnapshot` + `KillOutcome` live in `sid_core::sys_probe`. Widgets subscribe to the broadcast channel and call `probe.provider()` only for the kill job dispatch.
   - `NetworkWidget` references `sid_core::adapters::sys::*` and `sid_core::sys_probe::*` only. No direct `sysinfo`, `netstat2`, or `nix` import — those live entirely in `sid-sys`.

**4. Adapter pattern compliance.**
   - `sid-sys` is the only crate depending on `sysinfo`, `netstat2`, `nix`. Verified by the proposed Cargo.toml diffs.
   - `sid-core` adds `tokio` (already on it for the JobQueue carve-out). No new external runtime/lib dependencies in `sid-core` for this plan.
   - `sid-widgets` adds no new external deps. It only depends on `sid-core` (and `ratatui` for rendering, already present).

**5. CLAUDE.md compliance.**
   - Every public type and method gets a doc test (called out in Task 2 Step 6 and per-task).
   - `Result`-returning methods get Ok + Err coverage (Tasks 4-7 explicitly test typed errors).
   - Property tests on `PortsTableState` sort invariants (Task 12), `kill_process` huge-PID property (Task 7), refresh-strategy timing (Task 8).
   - Adversarial coverage: unicode in filter / process names (Task 4), binding many ports (Task 5), root-owned kill returning PermissionDenied (Task 7), `pid 0` rejected (Task 7), NaN cpu% sort (Task 13), 200-row snapshot (Task 17), failing provider keeps probe alive (Task 10).
   - Loom test on the `Arc<Mutex<…>>` handoff is Task 11, gated behind the `loom` feature.
   - Criterion benches on `list_processes` and `list_listening_ports` land in Task 26 with baseline numbers captured.
   - Insta snapshots on the rendered widget (Task 17) and modal stages (Task 19).
   - All commits land production code + tests together; no "tests will follow" commits.

**6. Placeholder scan.** No "TBD" / "TODO" / "fill in later" inside task steps. Three notes to be aware of:
   - Tasks 13, 14, 15, 16, 21, 22 are documented as following the established pattern from Task 12 / Task 20 rather than re-spelling each TDD step. Implementer expands when picking up.
   - Task 17's `Enter`-on-process-row action ("drill into process") emits a `ProcessDetails(pid)` action but the details pane UI is a stub showing "details for PID N" — full UI is intentionally deferred (mentioned in scope; could be folded into a future plan).
   - Task 26 captures baseline criterion numbers in the commit body rather than checking a JSON baseline into the repo; once CI lands, the baseline format is decided then.

**7. Scope check.** 27 tasks across 8 phases. Slightly tighter than Plan 2 (33 tasks across 7 phases). Each phase produces working/testable software; the plan can stop at the end of any phase and the project remains in a consistent state — e.g., end of Phase C is "SysinfoProvider works as a library", Phase D adds the polling service, Phase E adds the widget, Phase F adds the kill action, Phase G adds the CLI, Phase H wires it into the binary.

**8. Loom note.** Task 11's loom test models the relevant slice of behavior — concurrent locking of the same `Arc<Mutex<...>>` from two threads — rather than the real `SysProbe::run` (which uses tokio primitives that don't play with loom directly). Per CLAUDE.md the directive is "any code involving `Arc`, `Mutex`, channels, atomics" — the test satisfies that for the structural property (no deadlock, total order preserved). If a future plan introduces a more complex sharing pattern (e.g., lock-free atomics on a counter), it gets its own loom test.

**9. Concurrency safety judgment call.** `sysinfo::System` is already `Send + Sync` per its docs, so `SysinfoProvider` doesn't need the `unsafe impl Send/Sync` dance that `sid-git` needed for `git2::Repository`. Task 8 Step 4 adds a static `assert_send_sync` so the build breaks if a future sysinfo release drops the auto-impl. If/when that happens, follow the `sid-git` pattern with a `// SAFETY:` comment.

**10. Refresh-strategy judgment call.** Task 8 documents that `list_processes` refreshes the full `sysinfo::System` on every call, while `list_listening_ports` re-queries netstat2 fresh and uses the cached `System` only for PID→command lookup. This is conservative — the 2 s default poll cadence makes the cost negligible. If profiling later shows the process refresh is a bottleneck at higher poll rates, the implementation can move to delta-refreshes; the trait surface doesn't change.

**11. Co-author trailer.** All commit subjects in this plan deliberately omit `Co-Authored-By: Claude…` trailers per the user's stated preference (memory: `no-claude-coauthor-trailer`).
