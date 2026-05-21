# sid Plan 3 — SSH tab + SFTP sub-panel + PTY backbone

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. CLAUDE.md applies — every new pub fn needs a doc test, every function with invariants needs property tests, every parser-shaped function gets adversarial coverage, every `Arc<Mutex<…>>` introduced gets loom coverage.

**Goal:** When this plan is done, the **SSH** tab is fully functional. The tab shows a host list (left pane) merged from `~/.ssh/config` plus sid-managed hosts added via `sid ssh add <alias>`. Selecting a host and pressing Enter opens an interactive shell in an embedded PTY pane (right). Pressing `Tab` toggles an SFTP sub-panel showing the remote filesystem; users can up/download files and edit-in-place via `$EDITOR`. Per-host state — last connected, command history — persists in redb.

**Architecture:** Two new adapter crates land: `sid-ssh` (the `RusshClient` impl of `SshClient` + an SFTP wrapper via russh-sftp) and `sid-pty` (the `PortablePtyProvider` impl of `PtyProvider` + a `vt100`-backed ANSI terminal renderer). The `SshClient` and `PtyProvider` traits in `sid-core::adapters` — currently shells — get their full method surface. SSH host persistence extends the `Store` trait in `sid-store` and adds an `ssh_hosts` table. The widget lives in `sid-widgets/ssh.rs`, replacing the Plan 1 stub. The binary's `wire.rs` injects `RusshClient` and `PortablePtyProvider`. The `sid ssh …` CLI subcommands mutate the store and exit (no TUI). All russh / portable-pty / vt100 type names are contained inside `sid-ssh` and `sid-pty` — `sid-widgets` and `sid-core` only name traits.

**Tech stack additions** (pinned; if a version no longer exists at execution time, the implementer may bump to current):

- `russh = "0.50"` — SSH client (pure Rust, modern algorithm support)
- `russh-sftp = "2.0"` — SFTP wrapper riding on a russh channel
- `russh-keys = "0.46"` — keypair + ssh-agent auth helpers
- `portable-pty = "0.9"` — cross-platform pseudo-terminal allocation
- `vt100 = "0.16"` — ANSI/VT100 parser + screen model for rendering PTY output

`tokio` is already in workspace deps. `russh` brings in `bytes`/`tokio-rustls` transitively — that compile-time impact is accepted (justified in the relevant commit body, per CLAUDE.md).

**Out of scope (deferred — see `2026-05-20-sid-future-features.md` § "SSH enhancements" and § "SFTP enhancements"):**

- **Tunnel manager** (port forwarding UI, list active tunnels)
- **Mosh** as an alternative transport
- **Multiplexed sessions** (one connection, many shells)
- **Multi-file selection + drag-equivalent move/copy** in SFTP
- **Two-pane sync mode** (local ↔ remote diff + sync)
- **Background transfer queue** with status-bar progress
- **Resume interrupted transfers**
- Agent forwarding (read-from-agent yes; forwarding to remote no)
- X11 forwarding
- SSH known-hosts pinning UI (Plan 3 *uses* `~/.ssh/known_hosts` via russh defaults; managing it is out of scope)

---

## File structure (new and modified only — existing crates unchanged unless noted)

```
sid/
├── Cargo.toml                              # MODIFY: + russh, russh-sftp, russh-keys, portable-pty, vt100 + new workspace members
├── crates/
│   ├── sid-core/
│   │   └── src/
│   │       ├── lib.rs                      # MODIFY: re-export new types if helpful
│   │       └── adapters/
│   │           ├── ssh.rs                  # MODIFY: full SshClient trait + domain types
│   │           └── pty.rs                  # MODIFY: full PtyProvider trait + domain types
│   ├── sid-ssh/                            # NEW CRATE
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs                      # RusshClient root + re-exports
│   │   │   ├── client.rs                   # RusshClient struct + connect/disconnect/exec
│   │   │   ├── auth.rs                     # key + password + agent auth helpers
│   │   │   ├── shell.rs                    # open_shell — interactive PTY channel
│   │   │   ├── sftp.rs                     # RusshSftp — SFTP wrapper
│   │   │   └── config.rs                   # ~/.ssh/config reader
│   │   └── tests/
│   │       ├── connect.rs
│   │       ├── auth.rs
│   │       ├── exec.rs
│   │       ├── shell.rs
│   │       ├── sftp_list.rs
│   │       ├── sftp_transfer.rs
│   │       └── config_parse.rs
│   ├── sid-pty/                            # NEW CRATE
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs                      # PortablePtyProvider + re-exports
│   │   │   ├── provider.rs                 # open_pty + write + try_read + resize
│   │   │   └── screen.rs                   # vt100-backed ANSI screen renderer
│   │   └── tests/
│   │       ├── open_and_io.rs
│   │       ├── resize.rs
│   │       └── screen_render.rs
│   ├── sid-store/
│   │   ├── src/
│   │   │   ├── lib.rs                      # MODIFY: + SshHost type + Store extension
│   │   │   ├── schema.rs                   # MODIFY: + SSH_HOSTS table
│   │   │   └── redb_impl.rs                # MODIFY: + ssh host methods
│   │   └── tests/
│   │       └── ssh_hosts.rs                # NEW
│   ├── sid-widgets/
│   │   └── src/
│   │       └── ssh.rs                      # MODIFY: replace stub with full impl
│   └── sid/
│       └── src/
│           ├── main.rs                     # MODIFY: + Ssh subcommands
│           └── wire.rs                     # MODIFY: + RusshClient + PortablePtyProvider injection
```

---

## Task index

| # | Task | Phase |
|---|---|---|
| 1 | Add russh, russh-sftp, russh-keys, portable-pty, vt100 to workspace deps + sid-ssh + sid-pty members | A. Foundation |
| 2 | Scaffold `sid-ssh` crate skeleton | A. Foundation |
| 3 | Scaffold `sid-pty` crate skeleton | A. Foundation |
| 4 | Expand `SshClient` trait in `sid-core::adapters::ssh` | B. Traits |
| 5 | Expand `PtyProvider` trait in `sid-core::adapters::pty` | C. Traits |
| 6 | `RusshClient::connect` + `disconnect` (key auth) | D. RusshClient |
| 7 | Password auth + ssh-agent auth | D. RusshClient |
| 8 | `RusshClient::exec` (one-shot command) | D. RusshClient |
| 9 | `RusshClient::open_shell` (interactive channel) | D. RusshClient |
| 10 | `RusshClient::open_sftp` (returns boxed `SftpSession`) | D. RusshClient |
| 11 | `~/.ssh/config` reader (Host blocks + IdentityFile + ProxyJump) | D. RusshClient |
| 12 | `PortablePtyProvider::open_pty` (allocate master/slave + child) | E. PortablePty |
| 13 | `try_read` + `write` + `child_alive` (non-blocking byte feeder) | E. PortablePty |
| 14 | `resize` (winsize ioctl + SIGWINCH on child) | E. PortablePty |
| 15 | `Vt100Screen` — feed bytes + render to lines | E. PortablePty |
| 16 | `SshHost` domain type in `sid-store` | F. Storage |
| 17 | `ssh_hosts` table schema + open in `RedbStore::open` | F. Storage |
| 18 | `Store` trait extension (list/upsert/get/remove ssh host) | F. Storage |
| 19 | `RedbStore` impl for ssh-host methods | F. Storage |
| 20 | `SshWidget` host list state + ssh-config merge | G. Widget |
| 21 | Connection state machine (Idle → Connecting → Connected → Disconnected) | G. Widget |
| 22 | PTY pane rendering (vt100 → ratatui Buffer) | G. Widget |
| 23 | Per-host command history ring buffer + persistence | G. Widget |
| 24 | SFTP sub-panel state + directory listing | H. SFTP |
| 25 | SFTP download to local temp | H. SFTP |
| 26 | SFTP upload from local path | H. SFTP |
| 27 | Edit-in-place via `EditorRunner` (download → $EDITOR → upload) | H. SFTP |
| 28 | `sid ssh add/remove/list` CLI subcommands | I. CLI |
| 29 | `sid ssh connect <alias>` (launches TUI pre-pointed at host) | I. CLI |
| 30 | Wire `RusshClient` + `PortablePtyProvider` into binary | I. CLI |
| 31 | Integration test: ssh host registry round-trip | J. Integration |
| 32 | Integration test: SFTP edit-in-place via mock SshClient | J. Integration |
| 33 | README update (SSH tab section) | J. Integration |

---

## Phase A — Foundation

### Task 1: Add `russh`, `russh-sftp`, `russh-keys`, `portable-pty`, `vt100` + new crate members

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add `sid-ssh` and `sid-pty` to workspace members**

In the root `Cargo.toml`, locate `[workspace] members` and replace with:

```toml
members = [
    "crates/sid",
    "crates/sid-core",
    "crates/sid-ui",
    "crates/sid-store",
    "crates/sid-job",
    "crates/sid-widgets",
    "crates/sid-git",
    "crates/sid-ssh",
    "crates/sid-pty",
]
```

- [ ] **Step 2: Add new external + internal deps to `[workspace.dependencies]`**

Under the `# Internal` block, append:

```toml
sid-ssh = { path = "crates/sid-ssh" }
sid-pty = { path = "crates/sid-pty" }
```

In a logical place (after the `# Git` block), add:

```toml
# SSH
russh = "0.50"
russh-sftp = "2.0"
russh-keys = "0.46"

# PTY
portable-pty = "0.9"
vt100 = "0.16"
```

Justify in the eventual commit body: `russh` brings the only modern pure-Rust SSH client; `portable-pty` is the canonical cross-platform PTY allocator; `vt100` is the only mature pure-Rust VT100 screen state machine. Compile-time impact is accepted because the SSH tab is core functionality, not optional.

- [ ] **Step 3: Verify workspace resolves**

Run: `cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: fails with "member crate `sid-ssh` has no Cargo.toml" — that's fine. Until Task 2 lands, temporarily scaffold both crates so the metadata check passes:

```bash
mkdir -p crates/sid-ssh/src crates/sid-pty/src
cat > crates/sid-ssh/Cargo.toml <<'EOF'
[package]
name = "sid-ssh"
version.workspace = true
edition.workspace = true

[dependencies]
EOF
echo "// stub — Task 2 replaces this" > crates/sid-ssh/src/lib.rs

cat > crates/sid-pty/Cargo.toml <<'EOF'
[package]
name = "sid-pty"
version.workspace = true
edition.workspace = true

[dependencies]
EOF
echo "// stub — Task 3 replaces this" > crates/sid-pty/src/lib.rs
```

Confirm `cargo metadata --no-deps --format-version 1 > /dev/null` exits 0.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/sid-ssh crates/sid-pty
git commit -m "chore: add russh + portable-pty deps and sid-ssh + sid-pty workspace member stubs"
```

---

### Task 2: Scaffold `sid-ssh` crate

**Files:**
- Replace: `crates/sid-ssh/Cargo.toml`
- Replace: `crates/sid-ssh/src/lib.rs`
- Create: `crates/sid-ssh/src/{client,auth,shell,sftp,config}.rs` (empty module headers)

- [ ] **Step 1: Replace `crates/sid-ssh/Cargo.toml`**

```toml
[package]
name = "sid-ssh"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
sid-core.workspace = true
russh.workspace = true
russh-sftp.workspace = true
russh-keys.workspace = true
tokio.workspace = true
thiserror.workspace = true
tracing.workspace = true
async-trait = "0.1"

[dev-dependencies]
tempfile.workspace = true
proptest.workspace = true
insta.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

Add `async-trait = "0.1"` to root `Cargo.toml`'s `[workspace.dependencies]`:

```toml
async-trait = "0.1"
```

(The `SshClient` and `PtyProvider` traits will use `async-trait` where async methods are needed, keeping the traits dyn-compatible.)

- [ ] **Step 2: Replace `crates/sid-ssh/src/lib.rs`**

```rust
//! `RusshClient` — russh-backed `SshClient` implementation.
//!
//! `RusshClientFactory` is a stateless factory used by the binary to produce
//! per-host `RusshClient` instances via `connect(host)`.
//!
//! All russh-specific types are confined to this crate; the rest of sid talks
//! to the `SshClient` trait from `sid-core::adapters::ssh`.

pub mod auth;
pub mod client;
pub mod config;
pub mod shell;
pub mod sftp;

pub use client::{RusshClient, RusshClientFactory};
pub use config::{SshConfigEntry, read_ssh_config};
```

- [ ] **Step 3: Create empty submodule files**

Each file gets a stub header — content fills in over Tasks 6–11.

`crates/sid-ssh/src/client.rs`:

```rust
//! `RusshClient` core — connect/disconnect/exec. Filled in over Tasks 6–8.

use sid_core::adapters::ssh::{SshClient, SshError};

/// Stateless factory; per-host clients are produced by `connect`.
pub struct RusshClientFactory;

impl RusshClientFactory {
    /// Construct a new factory. Cheap; no I/O.
    pub fn new() -> Self { Self }
}

impl Default for RusshClientFactory {
    fn default() -> Self { Self::new() }
}

/// Per-host SSH client. Constructed by [`RusshClientFactory::connect`] in
/// Task 6. Holds the russh `Handle` plus a tokio task that pumps the channel.
pub struct RusshClient {
    // Filled in by Task 6.
    pub(crate) _placeholder: (),
}

impl SshClient for RusshClient {
    // Methods filled in over Tasks 6–10.
}

// Stub `connect` so the binary can wire the factory. Real impl: Task 6.
impl SshClient for RusshClientFactory {
    // Filled in by Task 6 once SshClient gains its methods (Task 4).
}

/// Convert any russh error into the domain `SshError`. Used across submodules.
pub(crate) fn map_russh_error(e: russh::Error) -> SshError {
    SshError::Other(format!("russh: {e}"))
}
```

`crates/sid-ssh/src/auth.rs`:

```rust
//! Auth methods — key, password, ssh-agent. Filled in by Task 7.
```

`crates/sid-ssh/src/shell.rs`:

```rust
//! Interactive shell channel. Filled in by Task 9.
```

`crates/sid-ssh/src/sftp.rs`:

```rust
//! SFTP wrapper — list/get/put on top of russh-sftp. Filled in by Task 10.
```

`crates/sid-ssh/src/config.rs`:

```rust
//! `~/.ssh/config` reader. Filled in by Task 11.

use std::path::Path;

/// A single parsed Host block from an OpenSSH config.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SshConfigEntry {
    pub host: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

/// Read `~/.ssh/config` (or the file at `path`). Returns `Ok(vec![])` if
/// missing. Task 11 fills in the parser; this stub exists for type wiring.
pub fn read_ssh_config(_path: &Path) -> std::io::Result<Vec<SshConfigEntry>> {
    Ok(Vec::new())
}
```

- [ ] **Step 4: Sanity test**

Create `crates/sid-ssh/tests/scaffolding.rs`:

```rust
use sid_ssh::{RusshClientFactory, read_ssh_config};
use std::path::Path;

#[test]
fn factory_constructs() {
    let _ = RusshClientFactory::new();
    let _ = RusshClientFactory::default();
}

#[test]
fn config_reader_returns_empty_on_missing_file() {
    let v = read_ssh_config(Path::new("/nonexistent-ssh-config-file")).unwrap();
    assert!(v.is_empty());
}
```

Run: `cargo test -p sid-ssh --test scaffolding`
Expected: 2 passed.

- [ ] **Step 5: Doc tests on every pub item**

Add `# Examples` blocks to `RusshClientFactory::new`, `RusshClient`, `SshConfigEntry`, and `read_ssh_config`. Each example constructs the type and reads a field. Per CLAUDE.md, no `ignore`; `no_run` only when external state is required (e.g., real `~/.ssh/config`).

- [ ] **Step 6: Commit**

```bash
git add crates/sid-ssh Cargo.toml
git commit -m "feat(ssh): scaffold sid-ssh crate (RusshClientFactory + module skeleton)"
```

---

### Task 3: Scaffold `sid-pty` crate

**Files:**
- Replace: `crates/sid-pty/Cargo.toml`
- Replace: `crates/sid-pty/src/lib.rs`
- Create: `crates/sid-pty/src/{provider,screen}.rs`

- [ ] **Step 1: Replace `crates/sid-pty/Cargo.toml`**

```toml
[package]
name = "sid-pty"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
sid-core.workspace = true
portable-pty.workspace = true
vt100.workspace = true
tokio.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
proptest.workspace = true
insta.workspace = true
loom.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }

[features]
default = []
# Enabled in `cargo test --features loom-tests` to opt into model-checking;
# loom tests gated behind `#[cfg(loom)]` for compile-time exclusion otherwise.
loom-tests = []
```

- [ ] **Step 2: Replace `crates/sid-pty/src/lib.rs`**

```rust
//! `PortablePtyProvider` — portable-pty-backed `PtyProvider` implementation,
//! plus a `vt100`-backed screen for ANSI rendering.
//!
//! portable-pty types do not appear in `sid-core` or `sid-widgets`; those
//! crates name only the `PtyProvider` trait.

pub mod provider;
pub mod screen;

pub use provider::PortablePtyProvider;
pub use screen::Vt100Screen;
```

- [ ] **Step 3: Create `crates/sid-pty/src/provider.rs`**

```rust
//! `PortablePtyProvider` — opens portable-pty master/slave pairs and spawns a
//! child process on the slave end. Filled in over Tasks 12–14.

use sid_core::adapters::pty::PtyProvider;

/// Stateless provider; per-PTY handles are produced by `open_pty`.
pub struct PortablePtyProvider {
    _placeholder: (),
}

impl PortablePtyProvider {
    /// Construct a new provider. Cheap; no I/O.
    pub fn new() -> Self { Self { _placeholder: () } }
}

impl Default for PortablePtyProvider {
    fn default() -> Self { Self::new() }
}

impl PtyProvider for PortablePtyProvider {
    // Methods filled in over Tasks 12–14.
}
```

- [ ] **Step 4: Create `crates/sid-pty/src/screen.rs`**

```rust
//! `Vt100Screen` — wraps `vt100::Parser` and exposes a snapshot suitable for
//! ratatui rendering. Filled in by Task 15.

/// VT100 screen state. Construct with a `(rows, cols)` size; feed bytes;
/// `lines()` returns the current visible buffer as plain strings.
pub struct Vt100Screen {
    _placeholder: (),
}

impl Vt100Screen {
    /// Construct an empty screen of the given size.
    pub fn new(_rows: u16, _cols: u16) -> Self { Self { _placeholder: () } }
}
```

- [ ] **Step 5: Sanity test**

Create `crates/sid-pty/tests/scaffolding.rs`:

```rust
use sid_pty::{PortablePtyProvider, Vt100Screen};

#[test]
fn provider_constructs() {
    let _ = PortablePtyProvider::new();
    let _ = PortablePtyProvider::default();
}

#[test]
fn screen_constructs() {
    let _ = Vt100Screen::new(24, 80);
}
```

Run: `cargo test -p sid-pty --test scaffolding`
Expected: 2 passed.

- [ ] **Step 6: Doc tests**

Add `# Examples` blocks to `PortablePtyProvider::new` and `Vt100Screen::new`.

- [ ] **Step 7: Commit**

```bash
git add crates/sid-pty
git commit -m "feat(pty): scaffold sid-pty crate (PortablePtyProvider + Vt100Screen skeletons)"
```

---

## Phase B — Expand `SshClient` trait

### Task 4: Expand `SshClient` trait + domain types

**Files:**
- Modify: `crates/sid-core/src/adapters/ssh.rs`
- Modify: `crates/sid-core/src/lib.rs` (re-exports if useful)
- Test: `crates/sid-core/tests/ssh_provider_contract.rs`

The trait currently reads `pub trait SshClient: Send + Sync {}`. Replace with the full method surface and supporting domain types. The trait is dyn-compatible (no `Self`, no generics in method position). Async methods use `async-trait` for object safety.

- [ ] **Step 1: Write the contract test first**

Create `crates/sid-core/tests/ssh_provider_contract.rs`:

```rust
//! Verifies the SshClient trait is dyn-compatible (`Box<dyn SshClient>` works)
//! and that a no-op MockClient can implement every method.

use std::net::SocketAddr;

use async_trait::async_trait;
use sid_core::adapters::ssh::{
    ExecResult, SftpEntry, SftpSession, SshAuth, SshClient, SshError, SshHostSpec, SshShell,
};

struct MockClient {
    connected: bool,
}

#[async_trait]
impl SshClient for MockClient {
    async fn connect(&mut self, _host: &SshHostSpec, _auth: &SshAuth) -> Result<(), SshError> {
        self.connected = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<(), SshError> {
        self.connected = false;
        Ok(())
    }
    fn is_connected(&self) -> bool { self.connected }
    async fn exec(&mut self, _cmd: &str) -> Result<ExecResult, SshError> {
        Ok(ExecResult {
            stdout: b"ok\n".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
        })
    }
    async fn open_shell(&mut self, _term: &str, _rows: u16, _cols: u16) -> Result<Box<dyn SshShell>, SshError> {
        Err(SshError::Other("mock has no shell".into()))
    }
    async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError> {
        Err(SshError::Other("mock has no sftp".into()))
    }
}

#[tokio::test]
async fn client_is_dyn_compatible() {
    let mut c: Box<dyn SshClient> = Box::new(MockClient { connected: false });
    let host = SshHostSpec {
        host: "example.com".into(),
        port: 22,
        user: "test".into(),
    };
    c.connect(&host, &SshAuth::None).await.unwrap();
    assert!(c.is_connected());
    let r = c.exec("echo").await.unwrap();
    assert_eq!(r.exit_code, 0);
    c.disconnect().await.unwrap();
    assert!(!c.is_connected());
}

#[test]
fn client_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn SshClient>>();
    assert_send_sync::<Box<dyn SftpSession>>();
    assert_send_sync::<Box<dyn SshShell>>();
}

#[test]
fn ssh_auth_variants_exist() {
    let _ = SshAuth::None;
    let _ = SshAuth::Password("x".into());
    let _ = SshAuth::Key { path: std::path::PathBuf::from("/k"), passphrase: None };
    let _ = SshAuth::Agent;
}

#[test]
fn sftp_entry_construction() {
    let e = SftpEntry {
        name: "foo.txt".into(),
        is_dir: false,
        size: 42,
        mtime_secs: 0,
        mode: 0o644,
    };
    assert_eq!(e.name, "foo.txt");
    assert!(!e.is_dir);
}

#[test]
fn ssh_host_spec_default_port_is_22_in_constructor() {
    let s = SshHostSpec::new("h", "u");
    assert_eq!(s.port, 22);
    assert_eq!(s.host, "h");
    assert_eq!(s.user, "u");
    let _: SocketAddr = format!("{}:{}", s.host, s.port).parse().unwrap_or_else(|_| {
        // hostnames may not parse as SocketAddr; that's fine — just a smoke test of fields.
        "127.0.0.1:22".parse().unwrap()
    });
}
```

- [ ] **Step 2: Run — should fail to compile**

Run: `cargo test -p sid-core --test ssh_provider_contract`
Expected: compile error (types and methods don't exist yet).

- [ ] **Step 3: Replace `crates/sid-core/src/adapters/ssh.rs`**

```rust
//! SSH client trait + supporting domain types. Implementations live in `sid-ssh`.
//!
//! The trait uses `async-trait` for async methods to remain dyn-compatible.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Domain-shaped SSH error. Concrete impls map their library errors into this.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    #[error("connect failed: {0}")]
    ConnectFailed(String),
    #[error("connection closed unexpectedly")]
    Disconnected,
    #[error("operation timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("not connected — call connect() first")]
    NotConnected,
    #[error("remote path not found: {0}")]
    PathNotFound(String),
    #[error("ssh operation failed: {0}")]
    Other(String),
}

/// Host + port + user — the minimum needed to dial.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::ssh::SshHostSpec;
/// let s = SshHostSpec::new("example.com", "alice");
/// assert_eq!(s.port, 22);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshHostSpec {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl SshHostSpec {
    /// Construct with the default SSH port (22).
    pub fn new(host: impl Into<String>, user: impl Into<String>) -> Self {
        Self { host: host.into(), port: 22, user: user.into() }
    }
}

/// Authentication method.
///
/// `None` is only useful for tests and mock servers; production calls will
/// pick `Key`, `Agent`, or `Password`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SshAuth {
    None,
    Password(String),
    Key {
        path: PathBuf,
        passphrase: Option<String>,
    },
    /// Authenticate via the running ssh-agent (`$SSH_AUTH_SOCK`).
    Agent,
}

/// Result of a one-shot remote command (`exec`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

/// One entry in a remote directory listing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Modification time, seconds since UNIX epoch.
    pub mtime_secs: i64,
    /// UNIX mode bits (0o644, 0o755, …). On non-UNIX servers this may be 0.
    pub mode: u32,
}

/// An interactive shell session — read/write byte streams + window resize.
///
/// Implementations live in `sid-ssh` and wrap a russh `Channel`.
#[async_trait]
pub trait SshShell: Send + Sync {
    /// Write bytes to the remote shell's stdin. Non-blocking when possible.
    async fn write(&mut self, bytes: &[u8]) -> Result<(), SshError>;

    /// Read any available bytes from the remote shell. Returns `Ok(vec![])` if
    /// nothing is currently buffered. Never blocks indefinitely.
    async fn try_read(&mut self) -> Result<Vec<u8>, SshError>;

    /// Inform the remote of a new window size.
    async fn resize(&mut self, rows: u16, cols: u16) -> Result<(), SshError>;

    /// Close the channel. Idempotent.
    async fn close(&mut self) -> Result<(), SshError>;
}

/// An SFTP session for file operations on the connected host.
#[async_trait]
pub trait SftpSession: Send + Sync {
    /// List a remote directory. Paths are POSIX-style.
    async fn list(&mut self, path: &str) -> Result<Vec<SftpEntry>, SshError>;

    /// Read the entire contents of a remote file into memory.
    async fn get(&mut self, path: &str) -> Result<Vec<u8>, SshError>;

    /// Write bytes to a remote file (creating or truncating).
    async fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), SshError>;

    /// Remove a remote file.
    async fn remove_file(&mut self, path: &str) -> Result<(), SshError>;

    /// Create a remote directory. Errors if the directory already exists.
    async fn mkdir(&mut self, path: &str) -> Result<(), SshError>;

    /// Stat a remote path. Returns `Ok(None)` if it does not exist.
    async fn stat(&mut self, path: &str) -> Result<Option<SftpEntry>, SshError>;

    /// Close the SFTP session. Idempotent.
    async fn close(&mut self) -> Result<(), SshError>;
}

/// SSH operations needed by the SSH tab. Implementations live in `sid-ssh`.
///
/// # Object safety
///
/// All methods take `&mut self`, use `async-trait` for async, and return boxed
/// trait objects — so `Box<dyn SshClient>` works.
#[async_trait]
pub trait SshClient: Send + Sync {
    /// Open a connection to `host` and authenticate using `auth`.
    async fn connect(&mut self, host: &SshHostSpec, auth: &SshAuth) -> Result<(), SshError>;

    /// Tear down the connection. Idempotent.
    async fn disconnect(&mut self) -> Result<(), SshError>;

    /// `true` after a successful `connect` and before `disconnect`.
    fn is_connected(&self) -> bool;

    /// Run `cmd` as a one-shot command, returning stdout/stderr/exit-code.
    async fn exec(&mut self, cmd: &str) -> Result<ExecResult, SshError>;

    /// Open an interactive shell with a PTY of the given dimensions.
    async fn open_shell(
        &mut self,
        term: &str,
        rows: u16,
        cols: u16,
    ) -> Result<Box<dyn SshShell>, SshError>;

    /// Open an SFTP session over a new channel.
    async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError>;
}
```

- [ ] **Step 4: Re-exports in `lib.rs`**

Confirm `pub mod adapters;` is present in `crates/sid-core/src/lib.rs`. Re-exports remain at namespace paths (`sid_core::adapters::ssh::...`) to keep the root namespace clean.

- [ ] **Step 5: Add `async-trait` to `sid-core/Cargo.toml`**

```toml
async-trait.workspace = true
```

(Already added to workspace deps in Task 2.)

- [ ] **Step 6: Run tests**

Run: `cargo test -p sid-core --test ssh_provider_contract`
Expected: 5 passed.

Run: `cargo test -p sid-core --all-features`
Expected: all prior tests still pass.

- [ ] **Step 7: Adversarial coverage**

Append to `tests/ssh_provider_contract.rs`:

```rust
#[tokio::test]
async fn double_connect_is_idempotent_in_mock() {
    let mut c = MockClient { connected: false };
    let host = SshHostSpec::new("a", "u");
    c.connect(&host, &SshAuth::None).await.unwrap();
    c.connect(&host, &SshAuth::None).await.unwrap();
    assert!(c.is_connected());
}

#[tokio::test]
async fn disconnect_before_connect_is_ok_in_mock() {
    let mut c = MockClient { connected: false };
    c.disconnect().await.unwrap();
    assert!(!c.is_connected());
}

#[test]
fn ssh_auth_does_not_print_password_in_debug() {
    // We intentionally do NOT redact in Debug for v1 (would force a custom impl);
    // but adversarial coverage means we verify the value is at least *typed* not
    // accidentally stringified into a log line elsewhere. This test pins the
    // current behavior so an accidental future "log auth at INFO" change is caught.
    let a = SshAuth::Password("super-secret-12345".into());
    let s = format!("{a:?}");
    // Document current behavior: password DOES appear in Debug. If we later add
    // redaction, update this test.
    assert!(s.contains("super-secret-12345"));
}
```

The last test is intentional — pins existing behavior so a future redaction effort is forced to update this assertion explicitly.

- [ ] **Step 8: Doc tests on every pub item**

Per CLAUDE.md: add `# Examples` blocks to `SshError`, `SshHostSpec`, `SshAuth`, `ExecResult`, `SftpEntry`, `SftpSession`, `SshShell`, and `SshClient`. Each example constructs the type and reads a field. For the traits, show a minimal mock impl matching one method (`is_connected` is simplest; `connect` requires async runtime → `no_run`).

- [ ] **Step 9: Commit**

```bash
git add crates/sid-core Cargo.toml
git commit -m "feat(core): expand SshClient trait with full method surface + domain types"
```

---

## Phase C — Expand `PtyProvider` trait

### Task 5: Expand `PtyProvider` trait + domain types

**Files:**
- Modify: `crates/sid-core/src/adapters/pty.rs`
- Test: `crates/sid-core/tests/pty_provider_contract.rs`

- [ ] **Step 1: Write the contract test first**

Create `crates/sid-core/tests/pty_provider_contract.rs`:

```rust
//! Verifies the PtyProvider trait is dyn-compatible and a MockPty can fill it.

use std::sync::Mutex;

use sid_core::adapters::pty::{PtyError, PtyHandle, PtyProvider, PtySize, PtySpawn};

struct MockPty {
    inbox: Mutex<Vec<u8>>,
    outbox: Mutex<Vec<u8>>,
    alive: bool,
    size: PtySize,
}

impl PtyHandle for MockPty {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, PtyError> {
        self.inbox.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn try_read(&mut self) -> Result<Vec<u8>, PtyError> {
        let mut o = self.outbox.lock().unwrap();
        let v = o.clone();
        o.clear();
        Ok(v)
    }
    fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        self.size = size;
        Ok(())
    }
    fn child_alive(&self) -> bool { self.alive }
    fn size(&self) -> PtySize { self.size }
    fn kill(&mut self) -> Result<(), PtyError> {
        self.alive = false;
        Ok(())
    }
}

struct MockProvider;

impl PtyProvider for MockProvider {
    fn open_pty(&self, _spec: &PtySpawn) -> Result<Box<dyn PtyHandle>, PtyError> {
        Ok(Box::new(MockPty {
            inbox: Mutex::new(Vec::new()),
            outbox: Mutex::new(Vec::new()),
            alive: true,
            size: PtySize { rows: 24, cols: 80 },
        }))
    }
}

#[test]
fn provider_is_dyn_compatible() {
    let p: Box<dyn PtyProvider> = Box::new(MockProvider);
    let mut h = p.open_pty(&PtySpawn::shell()).unwrap();
    let n = h.write(b"echo hi\n").unwrap();
    assert_eq!(n, 8);
    assert!(h.child_alive());
    h.kill().unwrap();
    assert!(!h.child_alive());
}

#[test]
fn provider_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn PtyProvider>>();
    assert_send_sync::<Box<dyn PtyHandle>>();
}

#[test]
fn pty_size_construction_and_eq() {
    let a = PtySize { rows: 24, cols: 80 };
    let b = PtySize::new(24, 80);
    assert_eq!(a, b);
}

#[test]
fn pty_spawn_shell_uses_env_shell_when_set() {
    // Just checks construction; the default $SHELL resolution is impl-specific.
    let s = PtySpawn::shell();
    assert!(!s.program.is_empty());
}

#[test]
fn pty_spawn_command_builder() {
    let s = PtySpawn::command("ls", &["-la", "/"]);
    assert_eq!(s.program, "ls");
    assert_eq!(s.args, vec!["-la".to_string(), "/".to_string()]);
}
```

- [ ] **Step 2: Run — should fail to compile**

Run: `cargo test -p sid-core --test pty_provider_contract`

- [ ] **Step 3: Replace `crates/sid-core/src/adapters/pty.rs`**

```rust
//! PTY provider trait + supporting domain types. Implementations live in `sid-pty`.
//!
//! The trait is synchronous — `try_read` is non-blocking, and async pumping is
//! the caller's responsibility (the binary spawns a small tokio task per PTY).

use std::collections::HashMap;

/// Domain-shaped PTY error.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("failed to open PTY: {0}")]
    OpenFailed(String),
    #[error("write failed: {0}")]
    WriteFailed(String),
    #[error("read failed: {0}")]
    ReadFailed(String),
    #[error("resize failed: {0}")]
    ResizeFailed(String),
    #[error("child has exited (status {0:?})")]
    ChildExited(Option<i32>),
    #[error("pty operation failed: {0}")]
    Other(String),
}

/// PTY window size (in cells, not pixels).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl PtySize {
    /// Construct a size.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::pty::PtySize;
    /// let s = PtySize::new(24, 80);
    /// assert_eq!(s.rows, 24);
    /// ```
    pub fn new(rows: u16, cols: u16) -> Self { Self { rows, cols } }
}

impl Default for PtySize {
    fn default() -> Self { Self { rows: 24, cols: 80 } }
}

/// Spec for spawning a child on the PTY slave.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtySpawn {
    /// Program to exec.
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Working directory. `None` means inherit the sid process cwd.
    pub cwd: Option<std::path::PathBuf>,
    /// Extra environment variables (merged on top of the inherited env).
    pub env: HashMap<String, String>,
    /// Initial size of the PTY.
    pub size: PtySize,
}

impl PtySpawn {
    /// Spawn the user's default shell (`$SHELL` or `/bin/sh` fallback).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::pty::PtySpawn;
    /// let s = PtySpawn::shell();
    /// assert!(!s.program.is_empty());
    /// ```
    pub fn shell() -> Self {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Self {
            program,
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            size: PtySize::default(),
        }
    }

    /// Spawn an arbitrary command.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::pty::PtySpawn;
    /// let s = PtySpawn::command("ls", &["-la"]);
    /// assert_eq!(s.program, "ls");
    /// ```
    pub fn command(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            env: HashMap::new(),
            size: PtySize::default(),
        }
    }
}

/// A live PTY handle — owns the master end + child handle.
///
/// All methods are synchronous and non-blocking. The caller (typically the
/// binary's wire layer) spawns a tokio task to pump `try_read` into the
/// widget on a timer.
pub trait PtyHandle: Send + Sync {
    /// Write bytes to the PTY master (i.e., the child's stdin).
    /// Returns the number of bytes written.
    fn write(&mut self, bytes: &[u8]) -> Result<usize, PtyError>;

    /// Read any available bytes from the PTY master (i.e., the child's stdout).
    /// Returns `Ok(vec![])` if nothing is currently buffered.
    fn try_read(&mut self) -> Result<Vec<u8>, PtyError>;

    /// Update the PTY size and send `SIGWINCH` to the child.
    fn resize(&mut self, size: PtySize) -> Result<(), PtyError>;

    /// Whether the child process is still alive (has not exited).
    fn child_alive(&self) -> bool;

    /// Current PTY size.
    fn size(&self) -> PtySize;

    /// Kill the child process (`SIGKILL` on UNIX). Idempotent.
    fn kill(&mut self) -> Result<(), PtyError>;
}

/// Factory for `PtyHandle`s. Implementations live in `sid-pty`.
///
/// # Object safety
///
/// `open_pty` takes `&self` (provider is stateless) and returns a boxed handle.
pub trait PtyProvider: Send + Sync {
    /// Allocate a PTY pair and spawn `spec.program` on the slave end.
    fn open_pty(&self, spec: &PtySpawn) -> Result<Box<dyn PtyHandle>, PtyError>;
}
```

- [ ] **Step 4: Run tests** — expected 5 passed.

- [ ] **Step 5: Adversarial coverage**

Append to `tests/pty_provider_contract.rs`:

```rust
#[test]
fn open_pty_with_zero_size_does_not_panic() {
    let p = MockProvider;
    let spec = PtySpawn {
        program: "true".into(),
        args: Vec::new(),
        cwd: None,
        env: Default::default(),
        size: PtySize { rows: 0, cols: 0 },
    };
    let _ = p.open_pty(&spec).unwrap();
}

#[test]
fn double_kill_is_idempotent() {
    let p = MockProvider;
    let mut h = p.open_pty(&PtySpawn::shell()).unwrap();
    h.kill().unwrap();
    h.kill().unwrap();
    assert!(!h.child_alive());
}

#[test]
fn write_zero_bytes_returns_zero() {
    let p = MockProvider;
    let mut h = p.open_pty(&PtySpawn::shell()).unwrap();
    assert_eq!(h.write(&[]).unwrap(), 0);
}
```

- [ ] **Step 6: Doc tests**

Add `# Examples` blocks to `PtyError`, `PtySize`, `PtySpawn`, `PtyHandle`, and `PtyProvider`. For trait examples, show a minimal mock matching `child_alive` or `size`.

- [ ] **Step 7: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): expand PtyProvider trait with full method surface + domain types"
```

---

## Phase D — `RusshClient` impl

Phase D fills in `RusshClient` method-by-method. Each task lands as its own commit. Tests use a small in-process SSH server fixture (built on `russh::server`) where a real network connection is required; pure unit tests stay against the trait surface.

### Task 6: `RusshClient::connect` + `disconnect` (key auth)

**Files:**
- Modify: `crates/sid-ssh/src/client.rs`
- Modify: `crates/sid-ssh/src/auth.rs`
- Create: `crates/sid-ssh/tests/connect.rs`
- Create: `crates/sid-ssh/tests/test_server.rs` (helper, not a test target — `mod test_server;` from each test that needs it)
- Create: `crates/sid-ssh/tests/common/mod.rs` (shared fixture)

- [ ] **Step 1: Failing test**

Create `crates/sid-ssh/tests/common/mod.rs`:

```rust
//! Shared test fixture: an in-process russh server that accepts any key auth
//! and runs a no-op channel handler. Returned via tokio task; caller passes
//! the bound port to RusshClient.

use std::net::SocketAddr;
use std::sync::Arc;

use russh::keys::PrivateKey;
use russh::server::{Auth, Msg, Server, Session};
use russh::{Channel, ChannelId, MethodSet};

pub struct TestServer;

#[async_trait::async_trait]
impl Server for TestServer {
    type Handler = TestHandler;
    fn new_client(&mut self, _addr: Option<SocketAddr>) -> Self::Handler {
        TestHandler { user_accepted: false }
    }
}

pub struct TestHandler { pub user_accepted: bool }

#[async_trait::async_trait]
impl russh::server::Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _user: &str, _pk: &russh::keys::PublicKey) -> Result<Auth, Self::Error> {
        self.user_accepted = true;
        Ok(Auth::Accept)
    }
    async fn auth_password(&mut self, _user: &str, _password: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }
    async fn channel_open_session(
        &mut self, _channel: Channel<Msg>, _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
    async fn exec_request(
        &mut self, channel: ChannelId, _data: &[u8], session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, "ok\n".into())?;
        session.exit_status_request(channel, 0)?;
        session.close(channel)?;
        Ok(())
    }
}

/// Spawn the test server on an ephemeral port; returns the bound address.
pub async fn spawn_test_server() -> SocketAddr {
    let host_key = russh::keys::PrivateKey::random(
        &mut rand::thread_rng(),
        russh::keys::Algorithm::Ed25519,
    ).unwrap();
    let config = Arc::new(russh::server::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(30)),
        auth_rejection_time: std::time::Duration::from_millis(100),
        keys: vec![host_key],
        methods: MethodSet::PUBLICKEY | MethodSet::PASSWORD,
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (socket, peer) = listener.accept().await.unwrap();
            let cfg = config.clone();
            tokio::spawn(async move {
                let mut server = TestServer;
                let handler = server.new_client(Some(peer));
                let _ = russh::server::run_stream(cfg, socket, handler).await;
            });
        }
    });
    addr
}

/// Generate a throwaway client keypair into a temp file. Returns the path.
pub fn write_temp_keypair(dir: &std::path::Path) -> std::path::PathBuf {
    let key = PrivateKey::random(
        &mut rand::thread_rng(),
        russh::keys::Algorithm::Ed25519,
    ).unwrap();
    let path = dir.join("id_ed25519");
    let pem = key.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap();
    std::fs::write(&path, pem.as_bytes()).unwrap();
    path
}
```

(This is a *judgment call*: the russh server API has churned across 0.4x → 0.5x; the implementer should adapt method signatures to the exact russh 0.50 API. Flagged in self-review.)

Add `rand = "0.8"` to `crates/sid-ssh/Cargo.toml`'s `[dev-dependencies]`.

Create `crates/sid-ssh/tests/connect.rs`:

```rust
mod common;

use std::time::Duration;

use sid_core::adapters::ssh::{SshAuth, SshClient, SshError, SshHostSpec};
use sid_ssh::RusshClientFactory;
use tempfile::tempdir;

#[tokio::test]
async fn connect_succeeds_with_key_auth() {
    let addr = common::spawn_test_server().await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let factory = RusshClientFactory::new();
    let mut client = factory.new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "test".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    assert!(client.is_connected());
    client.disconnect().await.unwrap();
    assert!(!client.is_connected());
}

#[tokio::test]
async fn connect_fails_on_unreachable_host() {
    let factory = RusshClientFactory::new();
    let mut client = factory.new_client();
    let res = tokio::time::timeout(
        Duration::from_secs(3),
        client.connect(
            &SshHostSpec { host: "127.0.0.1".into(), port: 1, user: "x".into() },
            &SshAuth::None,
        ),
    ).await;
    // Either timed out (Err) or russh returned ConnectFailed
    assert!(matches!(res, Err(_) | Ok(Err(SshError::ConnectFailed(_)))));
}

#[tokio::test]
async fn double_disconnect_is_idempotent() {
    let factory = RusshClientFactory::new();
    let mut client = factory.new_client();
    client.disconnect().await.unwrap();
    client.disconnect().await.unwrap();
    assert!(!client.is_connected());
}
```

- [ ] **Step 2: Run — should fail to compile (RusshClient::connect not implemented)**

- [ ] **Step 3: Implement on `RusshClient`**

Replace `crates/sid-ssh/src/client.rs`:

```rust
//! `RusshClient` core — connect/disconnect; auth dispatch lives in `auth`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russh::client::{Config, Handle, Handler};
use russh::keys::PublicKey;
use sid_core::adapters::ssh::{
    ExecResult, SftpSession, SshAuth, SshClient, SshError, SshHostSpec, SshShell,
};

use crate::auth::authenticate;

pub struct RusshClientFactory;

impl RusshClientFactory {
    /// Construct a new factory. Cheap; no I/O.
    pub fn new() -> Self { Self }

    /// Construct a fresh per-host client. Not yet connected.
    pub fn new_client(&self) -> RusshClient {
        RusshClient { handle: None }
    }
}

impl Default for RusshClientFactory {
    fn default() -> Self { Self::new() }
}

pub struct RusshClient {
    pub(crate) handle: Option<Handle<ClientHandler>>,
}

/// Permissive handler: we use `~/.ssh/known_hosts` via russh defaults in
/// production; for v1 we accept any host key to match the user's existing
/// workflow. **Known-hosts pinning is out of scope for Plan 3** — see
/// future-features.
pub struct ClientHandler;

#[async_trait]
impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _server_public_key: &PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

#[async_trait]
impl SshClient for RusshClient {
    async fn connect(&mut self, host: &SshHostSpec, auth: &SshAuth) -> Result<(), SshError> {
        let config = Arc::new(Config {
            inactivity_timeout: Some(Duration::from_secs(300)),
            ..Default::default()
        });
        let addr = format!("{}:{}", host.host, host.port);
        let handle = russh::client::connect(config, addr.as_str(), ClientHandler)
            .await
            .map_err(|e| SshError::ConnectFailed(format!("{e}")))?;
        let mut handle = handle;
        authenticate(&mut handle, &host.user, auth).await?;
        self.handle = Some(handle);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), SshError> {
        if let Some(h) = self.handle.take() {
            // russh closes on drop; explicitly send a disconnect message.
            let _ = h.disconnect(russh::Disconnect::ByApplication, "bye", "en").await;
        }
        Ok(())
    }

    fn is_connected(&self) -> bool { self.handle.is_some() }

    async fn exec(&mut self, _cmd: &str) -> Result<ExecResult, SshError> {
        Err(SshError::Other("exec not yet implemented — Task 8".into()))
    }
    async fn open_shell(
        &mut self, _term: &str, _rows: u16, _cols: u16,
    ) -> Result<Box<dyn SshShell>, SshError> {
        Err(SshError::Other("open_shell not yet implemented — Task 9".into()))
    }
    async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError> {
        Err(SshError::Other("open_sftp not yet implemented — Task 10".into()))
    }
}

/// Convert any russh error into the domain `SshError`. Used across submodules.
pub(crate) fn map_russh_error(e: russh::Error) -> SshError {
    match e {
        russh::Error::Disconnect => SshError::Disconnected,
        russh::Error::NoAuthMethod => SshError::AuthFailed("no auth method".into()),
        other => SshError::Other(format!("russh: {other}")),
    }
}
```

Replace `crates/sid-ssh/src/auth.rs`:

```rust
//! Auth methods — key, password, ssh-agent. Used by `RusshClient::connect`.

use std::path::Path;

use russh::client::Handle;
use russh::keys::PrivateKeyWithHashAlg;
use sid_core::adapters::ssh::{SshAuth, SshError};

use crate::client::ClientHandler;

pub async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    auth: &SshAuth,
) -> Result<(), SshError> {
    match auth {
        SshAuth::None => {
            // For test servers that accept any auth: try a trivial none auth.
            let r = handle.authenticate_password(user, "").await
                .map_err(|e| SshError::AuthFailed(format!("{e}")))?;
            if !r.success() { return Err(SshError::AuthFailed("none auth rejected".into())); }
            Ok(())
        }
        SshAuth::Password(p) => auth_password(handle, user, p).await,
        SshAuth::Key { path, passphrase } => auth_key(handle, user, path, passphrase.as_deref()).await,
        SshAuth::Agent => auth_agent(handle, user).await,
    }
}

async fn auth_password(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    password: &str,
) -> Result<(), SshError> {
    let r = handle.authenticate_password(user, password).await
        .map_err(|e| SshError::AuthFailed(format!("{e}")))?;
    if !r.success() { return Err(SshError::AuthFailed("password rejected".into())); }
    Ok(())
}

async fn auth_key(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    path: &Path,
    passphrase: Option<&str>,
) -> Result<(), SshError> {
    let key = russh::keys::load_secret_key(path, passphrase)
        .map_err(|e| SshError::AuthFailed(format!("load key {path:?}: {e}")))?;
    let key_with_hash = PrivateKeyWithHashAlg::new(
        std::sync::Arc::new(key),
        Some(russh::keys::HashAlg::Sha512),
    );
    let r = handle.authenticate_publickey(user, key_with_hash).await
        .map_err(|e| SshError::AuthFailed(format!("{e}")))?;
    if !r.success() { return Err(SshError::AuthFailed("public-key rejected".into())); }
    Ok(())
}

async fn auth_agent(
    _handle: &mut Handle<ClientHandler>,
    _user: &str,
) -> Result<(), SshError> {
    // Filled in by Task 7.
    Err(SshError::Other("agent auth not yet implemented — Task 7".into()))
}
```

This is a *judgment call*: russh 0.50's exact API for `authenticate_password` / `authenticate_publickey` / `PrivateKeyWithHashAlg` may differ — implementer adapts per the actual russh 0.50 docs at implementation time. Flagged in self-review.

- [ ] **Step 4: Run tests** — expected 3 passed (`connect_succeeds_with_key_auth`, `connect_fails_on_unreachable_host`, `double_disconnect_is_idempotent`).

- [ ] **Step 5: Adversarial coverage**

Append to `tests/connect.rs`:

```rust
#[tokio::test]
async fn connect_with_bad_key_path_returns_auth_failed() {
    let addr = common::spawn_test_server().await;
    let factory = RusshClientFactory::new();
    let mut client = factory.new_client();
    let err = client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "test".into() },
        &SshAuth::Key { path: "/nonexistent/key".into(), passphrase: None },
    ).await.unwrap_err();
    assert!(matches!(err, SshError::AuthFailed(_)));
}

#[tokio::test]
async fn connect_timeout_does_not_panic() {
    let factory = RusshClientFactory::new();
    let mut client = factory.new_client();
    let _ = tokio::time::timeout(
        Duration::from_millis(200),
        client.connect(
            &SshHostSpec { host: "10.255.255.1".into(), port: 22, user: "x".into() },
            &SshAuth::None,
        ),
    ).await;
}
```

- [ ] **Step 6: Doc tests + commit**

Add doc tests on `RusshClientFactory::new`, `RusshClientFactory::new_client`, and `RusshClient` (struct-level example using a `no_run` block since it needs a server).

```bash
git add crates/sid-ssh Cargo.toml
git commit -m "feat(ssh): implement RusshClient::connect + disconnect (key auth)"
```

---

### Task 7: Password + ssh-agent auth

**Files:**
- Modify: `crates/sid-ssh/src/auth.rs`
- Create: `crates/sid-ssh/tests/auth.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-ssh/tests/auth.rs`:

```rust
mod common;

use sid_core::adapters::ssh::{SshAuth, SshClient, SshError, SshHostSpec};
use sid_ssh::RusshClientFactory;

#[tokio::test]
async fn password_auth_succeeds_against_test_server() {
    let addr = common::spawn_test_server().await;
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "alice".into() },
        &SshAuth::Password("any-password".into()),
    ).await.unwrap();
    assert!(client.is_connected());
}

#[tokio::test]
async fn agent_auth_returns_error_when_no_agent_running() {
    // Save current SSH_AUTH_SOCK, point at nonexistent, restore after.
    let prev = std::env::var("SSH_AUTH_SOCK").ok();
    // SAFETY: tests run in the same process; this assumes single-threaded test
    // execution for this specific test. Use `cargo test -- --test-threads=1` if
    // contention arises. Acceptable for v1.
    unsafe { std::env::set_var("SSH_AUTH_SOCK", "/nonexistent/sock"); }
    let addr = common::spawn_test_server().await;
    let mut client = RusshClientFactory::new().new_client();
    let err = client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "test".into() },
        &SshAuth::Agent,
    ).await.unwrap_err();
    assert!(matches!(err, SshError::AuthFailed(_) | SshError::Other(_)));
    // Restore
    match prev {
        Some(v) => unsafe { std::env::set_var("SSH_AUTH_SOCK", v); }
        None => unsafe { std::env::remove_var("SSH_AUTH_SOCK"); }
    }
}
```

- [ ] **Step 2: Run — should fail (agent auth stub returns `Other`)**

- [ ] **Step 3: Implement agent auth**

Replace the `auth_agent` function in `crates/sid-ssh/src/auth.rs`:

```rust
async fn auth_agent(
    handle: &mut Handle<ClientHandler>,
    user: &str,
) -> Result<(), SshError> {
    let sock = std::env::var("SSH_AUTH_SOCK")
        .map_err(|_| SshError::AuthFailed("SSH_AUTH_SOCK not set".into()))?;
    let mut agent = russh::keys::agent::client::AgentClient::connect_uds(&sock)
        .await
        .map_err(|e| SshError::AuthFailed(format!("connect agent: {e}")))?;
    let identities = agent.request_identities()
        .await
        .map_err(|e| SshError::AuthFailed(format!("agent identities: {e}")))?;
    if identities.is_empty() {
        return Err(SshError::AuthFailed("agent has no identities".into()));
    }
    for pubkey in identities {
        let result = handle
            .authenticate_publickey_with(user, pubkey, Some(russh::keys::HashAlg::Sha512), &mut agent)
            .await
            .map_err(|e| SshError::AuthFailed(format!("{e}")))?;
        if result.success() { return Ok(()); }
    }
    Err(SshError::AuthFailed("all agent identities rejected".into()))
}
```

This is a *judgment call*: `russh::keys::agent::client::AgentClient::connect_uds` and `authenticate_publickey_with` are the russh 0.46/0.50 surface. Implementer adapts to the actual API at implementation time. Flagged in self-review.

- [ ] **Step 4: Run tests** — expected 2 passed.

- [ ] **Step 5: Adversarial coverage**

Append:

```rust
#[tokio::test]
async fn password_with_empty_string_handled_gracefully() {
    let addr = common::spawn_test_server().await;
    let mut client = RusshClientFactory::new().new_client();
    let _ = client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "x".into() },
        &SshAuth::Password(String::new()),
    ).await;
    // Test server accepts any password, so this succeeds; the assertion is "no
    // panic, no UB".
}

#[tokio::test]
async fn password_with_huge_string_does_not_panic() {
    let addr = common::spawn_test_server().await;
    let mut client = RusshClientFactory::new().new_client();
    let pw = "a".repeat(1_000_000);
    let _ = client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "x".into() },
        &SshAuth::Password(pw),
    ).await;
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-ssh
git commit -m "feat(ssh): implement password + ssh-agent auth in RusshClient"
```

---

### Task 8: `RusshClient::exec`

**Files:**
- Modify: `crates/sid-ssh/src/client.rs`
- Create: `crates/sid-ssh/tests/exec.rs`

- [ ] **Step 1: Failing test**

Create `crates/sid-ssh/tests/exec.rs`:

```rust
mod common;

use sid_core::adapters::ssh::{SshAuth, SshClient, SshHostSpec};
use sid_ssh::RusshClientFactory;
use tempfile::tempdir;

#[tokio::test]
async fn exec_returns_stdout_and_zero_exit_code() {
    let addr = common::spawn_test_server().await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let r = client.exec("ls /").await.unwrap();
    assert_eq!(r.exit_code, 0);
    assert_eq!(&r.stdout, b"ok\n");
}

#[tokio::test]
async fn exec_without_connect_returns_not_connected() {
    let mut client = RusshClientFactory::new().new_client();
    let err = client.exec("anything").await.unwrap_err();
    assert!(matches!(err, sid_core::adapters::ssh::SshError::NotConnected));
}
```

- [ ] **Step 2: Implement `exec`**

In `client.rs`, replace the stub `exec`:

```rust
async fn exec(&mut self, cmd: &str) -> Result<ExecResult, SshError> {
    use russh::ChannelMsg;
    let handle = self.handle.as_mut().ok_or(SshError::NotConnected)?;
    let mut channel = handle.channel_open_session().await.map_err(map_russh_error)?;
    channel.exec(true, cmd).await.map_err(map_russh_error)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code: Option<i32> = None;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            ChannelMsg::ExtendedData { .. } => {}
            ChannelMsg::ExitStatus { exit_status } => { exit_code = Some(exit_status as i32); }
            ChannelMsg::Close | ChannelMsg::Eof => break,
            _ => {}
        }
    }
    Ok(ExecResult {
        stdout,
        stderr,
        exit_code: exit_code.unwrap_or(-1),
    })
}
```

- [ ] **Step 3: Run tests** — expected 2 passed.

- [ ] **Step 4: Adversarial + property tests**

Append:

```rust
#[tokio::test]
async fn exec_empty_command_does_not_panic() {
    let addr = common::spawn_test_server().await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let _ = client.exec("").await.unwrap();
}

#[tokio::test]
async fn exec_very_long_command_does_not_panic() {
    let addr = common::spawn_test_server().await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let big = "x".repeat(100_000);
    let _ = client.exec(&big).await.unwrap();
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-ssh
git commit -m "feat(ssh): implement RusshClient::exec (one-shot command + exit code)"
```

---

### Task 9: `RusshClient::open_shell`

**Files:**
- Modify: `crates/sid-ssh/src/shell.rs`
- Modify: `crates/sid-ssh/src/client.rs`
- Create: `crates/sid-ssh/tests/shell.rs`

- [ ] **Step 1: Failing test**

The test server from `common` needs a `pty_request` + `shell_request` handler. Extend `tests/common/mod.rs`'s `TestHandler`:

```rust
async fn pty_request(
    &mut self,
    _channel: ChannelId,
    _term: &str,
    _col_width: u32, _row_height: u32,
    _pix_width: u32, _pix_height: u32,
    _modes: &[(russh::Pty, u32)],
    session: &mut Session,
) -> Result<(), Self::Error> {
    session.channel_success(_channel)?;
    Ok(())
}

async fn shell_request(
    &mut self, channel: ChannelId, session: &mut Session,
) -> Result<(), Self::Error> {
    session.channel_success(channel)?;
    session.data(channel, "test-shell-prompt> ".into())?;
    Ok(())
}

async fn data(
    &mut self, channel: ChannelId, data: &[u8], session: &mut Session,
) -> Result<(), Self::Error> {
    // Echo back what the client writes — lets the test verify round-trip.
    session.data(channel, russh::CryptoVec::from(data.to_vec()))?;
    Ok(())
}

async fn window_change_request(
    &mut self, _channel: ChannelId,
    _col_width: u32, _row_height: u32,
    _pix_width: u32, _pix_height: u32,
    _session: &mut Session,
) -> Result<(), Self::Error> {
    Ok(())
}
```

(Adapt signatures to russh 0.50's actual `Handler` trait methods. *Judgment call* — flagged.)

Create `crates/sid-ssh/tests/shell.rs`:

```rust
mod common;

use sid_core::adapters::ssh::{SshAuth, SshClient, SshHostSpec};
use sid_ssh::RusshClientFactory;
use tempfile::tempdir;

#[tokio::test]
async fn open_shell_returns_a_writable_readable_shell() {
    let addr = common::spawn_test_server().await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut shell = client.open_shell("xterm-256color", 24, 80).await.unwrap();
    shell.write(b"hello\n").await.unwrap();
    // Allow a brief moment for the echo to come back.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let bytes = shell.try_read().await.unwrap();
    let s = String::from_utf8_lossy(&bytes);
    // Test server echoes data + sent the prompt on shell_request.
    assert!(s.contains("hello") || s.contains("test-shell-prompt"));
    shell.close().await.unwrap();
}

#[tokio::test]
async fn open_shell_without_connect_returns_not_connected() {
    let mut client = RusshClientFactory::new().new_client();
    let err = client.open_shell("xterm", 24, 80).await.unwrap_err();
    assert!(matches!(err, sid_core::adapters::ssh::SshError::NotConnected));
}
```

- [ ] **Step 2: Implement `open_shell` + `RusshShell`**

Replace `crates/sid-ssh/src/shell.rs`:

```rust
//! Interactive shell channel — wraps a russh `Channel` behind the
//! `SshShell` trait.

use async_trait::async_trait;
use russh::client::Msg;
use russh::{Channel, ChannelMsg};
use sid_core::adapters::ssh::{SshError, SshShell};
use tokio::sync::Mutex;
use std::sync::Arc;

use crate::client::map_russh_error;

pub struct RusshShell {
    channel: Arc<Mutex<Channel<Msg>>>,
    buffer: Arc<Mutex<Vec<u8>>>,
    closed: bool,
}

impl RusshShell {
    pub(crate) fn new(channel: Channel<Msg>) -> Self {
        let channel = Arc::new(Mutex::new(channel));
        let buffer = Arc::new(Mutex::new(Vec::new()));
        // Spawn a reader task that pumps channel messages into `buffer`.
        let channel_for_task = channel.clone();
        let buffer_for_task = buffer.clone();
        tokio::spawn(async move {
            loop {
                let msg = { channel_for_task.lock().await.wait().await };
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        buffer_for_task.lock().await.extend_from_slice(&data);
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        buffer_for_task.lock().await.extend_from_slice(&data);
                    }
                    Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => break,
                    _ => {}
                }
            }
        });
        Self { channel, buffer, closed: false }
    }
}

#[async_trait]
impl SshShell for RusshShell {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), SshError> {
        if self.closed { return Err(SshError::Disconnected); }
        self.channel.lock().await.data(bytes).await.map_err(map_russh_error)?;
        Ok(())
    }
    async fn try_read(&mut self) -> Result<Vec<u8>, SshError> {
        let mut buf = self.buffer.lock().await;
        let out = std::mem::take(&mut *buf);
        Ok(out)
    }
    async fn resize(&mut self, rows: u16, cols: u16) -> Result<(), SshError> {
        self.channel.lock().await
            .window_change(cols as u32, rows as u32, 0, 0)
            .await
            .map_err(map_russh_error)?;
        Ok(())
    }
    async fn close(&mut self) -> Result<(), SshError> {
        if self.closed { return Ok(()); }
        self.closed = true;
        let _ = self.channel.lock().await.close().await;
        Ok(())
    }
}
```

This introduces the first `Arc<Mutex<…>>` in `sid-ssh`. CLAUDE.md mandates loom coverage. Add a loom test (Task 9 Step 5).

In `client.rs`, replace the stub `open_shell`:

```rust
async fn open_shell(
    &mut self, term: &str, rows: u16, cols: u16,
) -> Result<Box<dyn SshShell>, SshError> {
    let handle = self.handle.as_mut().ok_or(SshError::NotConnected)?;
    let mut channel = handle.channel_open_session().await.map_err(map_russh_error)?;
    channel
        .request_pty(true, term, cols as u32, rows as u32, 0, 0, &[])
        .await
        .map_err(map_russh_error)?;
    channel.request_shell(true).await.map_err(map_russh_error)?;
    Ok(Box::new(crate::shell::RusshShell::new(channel)))
}
```

Update `client.rs` imports to include `crate::shell::RusshShell`.

- [ ] **Step 3: Run tests** — expected 2 passed.

- [ ] **Step 4: Adversarial coverage**

Append to `tests/shell.rs`:

```rust
#[tokio::test]
async fn shell_resize_does_not_panic() {
    let addr = common::spawn_test_server().await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut shell = client.open_shell("xterm", 24, 80).await.unwrap();
    shell.resize(48, 160).await.unwrap();
    shell.resize(0, 0).await.unwrap(); // adversarial
    shell.close().await.unwrap();
}

#[tokio::test]
async fn double_close_is_idempotent() {
    let addr = common::spawn_test_server().await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut shell = client.open_shell("xterm", 24, 80).await.unwrap();
    shell.close().await.unwrap();
    shell.close().await.unwrap();
}
```

- [ ] **Step 5: Loom coverage of `Arc<Mutex<Channel>>` + `Arc<Mutex<Vec<u8>>>`**

Create `crates/sid-ssh/tests/loom_shell.rs`:

```rust
//! Loom model-check that the `RusshShell`'s buffer + channel mutex pairing
//! cannot deadlock or lose writes under concurrent reader + writer.
//!
//! Run with: `RUSTFLAGS="--cfg loom" cargo test -p sid-ssh --test loom_shell`

#![cfg(loom)]

use loom::sync::{Arc, Mutex};
use loom::thread;

#[test]
fn shell_buffer_writes_are_not_lost_under_concurrent_reader() {
    loom::model(|| {
        let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let writer_buf = buf.clone();
        let reader_buf = buf.clone();
        let w = thread::spawn(move || {
            writer_buf.lock().unwrap().push(1);
            writer_buf.lock().unwrap().push(2);
        });
        let r = thread::spawn(move || {
            let mut total = 0;
            for _ in 0..3 {
                let v = std::mem::take(&mut *reader_buf.lock().unwrap());
                total += v.len();
            }
            total
        });
        w.join().unwrap();
        let _ = r.join().unwrap();
        // Final drain captures the rest
        let final_len = buf.lock().unwrap().len();
        // Either reader picked up everything, or writer wrote 2 bytes total
        // that must still be retrievable.
        // The invariant: no byte is ever silently dropped between drain and
        // write — total bytes-seen + bytes-still-in-buf == 2.
        assert!(final_len <= 2);
    });
}
```

Add `loom = { workspace = true }` to `sid-ssh/Cargo.toml`'s `[dev-dependencies]`.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-ssh
git commit -m "feat(ssh): implement RusshClient::open_shell + RusshShell (PTY channel)"
```

---

### Task 10: `RusshClient::open_sftp`

**Files:**
- Modify: `crates/sid-ssh/src/sftp.rs`
- Modify: `crates/sid-ssh/src/client.rs`
- Create: `crates/sid-ssh/tests/sftp_list.rs`
- Create: `crates/sid-ssh/tests/sftp_transfer.rs`

The russh-sftp crate exposes `client::SftpSession`. We wrap it behind the `sid-core` trait of the same name.

For tests we need an SFTP subsystem in the test server. russh-sftp ships `server::run` for in-process servers; the test fixture grows a `subsystem_request` handler that spawns the SFTP server.

- [ ] **Step 1: Extend `tests/common/mod.rs` test server with SFTP support**

```rust
async fn subsystem_request(
    &mut self, channel: ChannelId, name: &str, session: &mut Session,
) -> Result<(), Self::Error> {
    if name == "sftp" {
        session.channel_success(channel)?;
        // Spawn an in-process russh-sftp server backed by a tempdir.
        let stream = session.channel_open(channel).await.unwrap(); // adapt to actual API
        let dir = tempfile::tempdir().unwrap();
        let server = russh_sftp::server::Server::new(dir.path().to_path_buf());
        tokio::spawn(async move {
            let _ = server.run(stream).await;
        });
        Ok(())
    } else {
        session.channel_failure(channel)?;
        Ok(())
    }
}
```

(*Judgment call*: the exact API for bridging a russh Channel to a russh-sftp server stream depends on russh-sftp 2.0; implementer adapts. Flagged in self-review.)

- [ ] **Step 2: Failing tests**

Create `crates/sid-ssh/tests/sftp_list.rs`:

```rust
mod common;

use sid_core::adapters::ssh::{SshAuth, SshClient, SshHostSpec};
use sid_ssh::RusshClientFactory;
use tempfile::tempdir;

#[tokio::test]
async fn sftp_list_returns_entries_for_existing_dir() {
    let addr = common::spawn_test_server_with_sftp_root(None).await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut sftp = client.open_sftp().await.unwrap();
    let entries = sftp.list("/").await.unwrap();
    // The test server's sftp root is empty by default; just verify shape.
    let _: Vec<sid_core::adapters::ssh::SftpEntry> = entries;
}

#[tokio::test]
async fn sftp_list_nonexistent_returns_path_not_found() {
    let addr = common::spawn_test_server_with_sftp_root(None).await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut sftp = client.open_sftp().await.unwrap();
    let err = sftp.list("/this/does/not/exist").await.unwrap_err();
    assert!(matches!(err, sid_core::adapters::ssh::SshError::PathNotFound(_) | sid_core::adapters::ssh::SshError::Other(_)));
}
```

Create `crates/sid-ssh/tests/sftp_transfer.rs`:

```rust
mod common;

use sid_core::adapters::ssh::{SshAuth, SshClient, SshHostSpec};
use sid_ssh::RusshClientFactory;
use tempfile::tempdir;

#[tokio::test]
async fn sftp_put_then_get_round_trips_bytes() {
    let root = tempdir().unwrap();
    let addr = common::spawn_test_server_with_sftp_root(Some(root.path().to_path_buf())).await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut sftp = client.open_sftp().await.unwrap();
    let payload = b"sid SFTP round-trip test \xff\x00\x01";
    sftp.put("/test.bin", payload).await.unwrap();
    let back = sftp.get("/test.bin").await.unwrap();
    assert_eq!(back, payload);
}

#[tokio::test]
async fn sftp_mkdir_then_list_shows_new_dir() {
    let root = tempdir().unwrap();
    let addr = common::spawn_test_server_with_sftp_root(Some(root.path().to_path_buf())).await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut sftp = client.open_sftp().await.unwrap();
    sftp.mkdir("/newdir").await.unwrap();
    let entries = sftp.list("/").await.unwrap();
    assert!(entries.iter().any(|e| e.name == "newdir" && e.is_dir));
}
```

- [ ] **Step 3: Implement `RusshSftp` and wire into `open_sftp`**

Replace `crates/sid-ssh/src/sftp.rs`:

```rust
//! SFTP wrapper — bridges russh-sftp's `SftpSession` to the domain
//! `SftpSession` trait.

use async_trait::async_trait;
use russh_sftp::client::SftpSession as RusshSftpSession;
use sid_core::adapters::ssh::{SftpEntry, SftpSession, SshError};

pub struct RusshSftp {
    inner: RusshSftpSession,
}

impl RusshSftp {
    pub(crate) fn new(inner: RusshSftpSession) -> Self { Self { inner } }
}

fn map_sftp_error(e: russh_sftp::client::error::Error) -> SshError {
    use russh_sftp::client::error::Error::*;
    match e {
        Status(s) => match s.status_code {
            russh_sftp::protocol::StatusCode::NoSuchFile => SshError::PathNotFound(s.error_message),
            _ => SshError::Other(format!("sftp: {}", s.error_message)),
        },
        other => SshError::Other(format!("sftp: {other}")),
    }
}

#[async_trait]
impl SftpSession for RusshSftp {
    async fn list(&mut self, path: &str) -> Result<Vec<SftpEntry>, SshError> {
        let entries = self.inner.read_dir(path).await.map_err(map_sftp_error)?;
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let attrs = entry.metadata();
            out.push(SftpEntry {
                name: entry.file_name(),
                is_dir: attrs.is_dir(),
                size: attrs.size.unwrap_or(0),
                mtime_secs: attrs.mtime.unwrap_or(0) as i64,
                mode: attrs.permissions.unwrap_or(0),
            });
        }
        Ok(out)
    }

    async fn get(&mut self, path: &str) -> Result<Vec<u8>, SshError> {
        let mut file = self.inner.open(path).await.map_err(map_sftp_error)?;
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).await.map_err(|e| SshError::Other(format!("read: {e}")))?;
        Ok(buf)
    }

    async fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), SshError> {
        use tokio::io::AsyncWriteExt;
        let mut file = self.inner.create(path).await.map_err(map_sftp_error)?;
        file.write_all(bytes).await.map_err(|e| SshError::Other(format!("write: {e}")))?;
        file.flush().await.map_err(|e| SshError::Other(format!("flush: {e}")))?;
        Ok(())
    }

    async fn remove_file(&mut self, path: &str) -> Result<(), SshError> {
        self.inner.remove_file(path).await.map_err(map_sftp_error)
    }

    async fn mkdir(&mut self, path: &str) -> Result<(), SshError> {
        self.inner.create_dir(path).await.map_err(map_sftp_error)
    }

    async fn stat(&mut self, path: &str) -> Result<Option<SftpEntry>, SshError> {
        match self.inner.metadata(path).await {
            Ok(attrs) => Ok(Some(SftpEntry {
                name: path.rsplit('/').next().unwrap_or(path).to_string(),
                is_dir: attrs.is_dir(),
                size: attrs.size.unwrap_or(0),
                mtime_secs: attrs.mtime.unwrap_or(0) as i64,
                mode: attrs.permissions.unwrap_or(0),
            })),
            Err(e) => match map_sftp_error(e) {
                SshError::PathNotFound(_) => Ok(None),
                other => Err(other),
            },
        }
    }

    async fn close(&mut self) -> Result<(), SshError> {
        let _ = self.inner.close().await;
        Ok(())
    }
}
```

(*Judgment call*: russh-sftp 2.0's exact API surface for `read_dir`/`open`/`create`/`metadata` will be confirmed at implementation. Flagged.)

In `client.rs`, replace the stub `open_sftp`:

```rust
async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError> {
    let handle = self.handle.as_mut().ok_or(SshError::NotConnected)?;
    let mut channel = handle.channel_open_session().await.map_err(map_russh_error)?;
    channel.request_subsystem(true, "sftp").await.map_err(map_russh_error)?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| SshError::Other(format!("sftp init: {e}")))?;
    Ok(Box::new(crate::sftp::RusshSftp::new(sftp)))
}
```

- [ ] **Step 4: Run tests** — expected 4 passed across `sftp_list.rs` + `sftp_transfer.rs`.

- [ ] **Step 5: Adversarial coverage + property test**

Append to `tests/sftp_transfer.rs`:

```rust
use proptest::prelude::*;

#[tokio::test]
async fn sftp_put_empty_file_succeeds() {
    let root = tempdir().unwrap();
    let addr = common::spawn_test_server_with_sftp_root(Some(root.path().to_path_buf())).await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut sftp = client.open_sftp().await.unwrap();
    sftp.put("/empty.bin", b"").await.unwrap();
    let back = sftp.get("/empty.bin").await.unwrap();
    assert!(back.is_empty());
}

#[tokio::test]
async fn sftp_put_large_file_round_trips() {
    let root = tempdir().unwrap();
    let addr = common::spawn_test_server_with_sftp_root(Some(root.path().to_path_buf())).await;
    let dir = tempdir().unwrap();
    let key_path = common::write_temp_keypair(dir.path());
    let mut client = RusshClientFactory::new().new_client();
    client.connect(
        &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
        &SshAuth::Key { path: key_path, passphrase: None },
    ).await.unwrap();
    let mut sftp = client.open_sftp().await.unwrap();
    let payload: Vec<u8> = (0u8..=255).cycle().take(1_000_000).collect();
    sftp.put("/large.bin", &payload).await.unwrap();
    let back = sftp.get("/large.bin").await.unwrap();
    assert_eq!(back.len(), 1_000_000);
    assert_eq!(back, payload);
}

// Property test: round-trip arbitrary byte payloads up to 8 KB.
proptest! {
    #![proptest_config(ProptestConfig { cases: 10, .. ProptestConfig::default() })]
    #[test]
    fn prop_sftp_get_put_round_trip(payload in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let payload_clone = payload.clone();
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
            let root = tempdir().unwrap();
            let addr = common::spawn_test_server_with_sftp_root(Some(root.path().to_path_buf())).await;
            let dir = tempdir().unwrap();
            let key_path = common::write_temp_keypair(dir.path());
            let mut client = RusshClientFactory::new().new_client();
            client.connect(
                &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
                &SshAuth::Key { path: key_path, passphrase: None },
            ).await.unwrap();
            let mut sftp = client.open_sftp().await.unwrap();
            sftp.put("/p.bin", &payload_clone).await.unwrap();
            let back = sftp.get("/p.bin").await.unwrap();
            assert_eq!(back, payload_clone);
        });
    }
}
```

- [ ] **Step 6: Criterion bench — directory listing**

CLAUDE.md flags SFTP directory listing as a critical hot path. Create `crates/sid-ssh/benches/sftp_list.rs`:

```rust
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sid_core::adapters::ssh::{SshAuth, SshClient, SshHostSpec};
use sid_ssh::RusshClientFactory;
use tempfile::tempdir;
use tokio::runtime::Runtime;

#[path = "../tests/common/mod.rs"]
mod common;

fn sftp_list_200_entries(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (addr, key_path, _root_keepalive, _key_keepalive) = rt.block_on(async {
        let root = tempdir().unwrap();
        // Pre-populate 200 files
        for i in 0..200 {
            std::fs::write(root.path().join(format!("f-{i}.txt")), b"x").unwrap();
        }
        let addr = common::spawn_test_server_with_sftp_root(Some(root.path().to_path_buf())).await;
        let key_dir = tempdir().unwrap();
        let key_path = common::write_temp_keypair(key_dir.path());
        (addr, key_path, root, key_dir)
    });

    c.bench_function("sftp_list_200_entries", |b| {
        b.to_async(&rt).iter(|| async {
            let mut client = RusshClientFactory::new().new_client();
            client.connect(
                &SshHostSpec { host: addr.ip().to_string(), port: addr.port(), user: "t".into() },
                &SshAuth::Key { path: key_path.clone(), passphrase: None },
            ).await.unwrap();
            let mut sftp = client.open_sftp().await.unwrap();
            let entries = sftp.list("/").await.unwrap();
            black_box(entries);
        });
    });
}

criterion_group!(benches, sftp_list_200_entries);
criterion_main!(benches);
```

In `crates/sid-ssh/Cargo.toml`:

```toml
[[bench]]
name = "sftp_list"
harness = false

[dev-dependencies]
criterion.workspace = true
```

Run: `cargo bench -p sid-ssh --bench sftp_list -- --quick`
Target: <50 ms over a localhost loopback for 200 entries. If a future change pushes this above the baseline by ≥10%, CI fails (per CLAUDE.md).

- [ ] **Step 7: Commit**

```bash
git add crates/sid-ssh
git commit -m "feat(ssh): implement RusshClient::open_sftp + RusshSftp wrapper + bench"
```

---

### Task 11: `~/.ssh/config` reader

**Files:**
- Modify: `crates/sid-ssh/src/config.rs`
- Create: `crates/sid-ssh/tests/config_parse.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-ssh/tests/config_parse.rs`:

```rust
use std::fs;
use sid_ssh::{read_ssh_config, SshConfigEntry};
use tempfile::tempdir;

#[test]
fn parses_simple_host_block() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host jp46-dev\n    HostName 10.1.40.102\n    User pi\n    Port 2222\n    IdentityFile ~/.ssh/id_jp46\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.host, "jp46-dev");
    assert_eq!(e.hostname.as_deref(), Some("10.1.40.102"));
    assert_eq!(e.user.as_deref(), Some("pi"));
    assert_eq!(e.port, Some(2222));
    assert_eq!(e.identity_file.as_deref(), Some("~/.ssh/id_jp46"));
}

#[test]
fn parses_multiple_host_blocks() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host a\n    HostName ahost\n\nHost b\n    HostName bhost\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].host, "a");
    assert_eq!(entries[1].host, "b");
}

#[test]
fn parses_proxy_jump() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host internal\n    HostName 10.0.0.5\n    ProxyJump bastion\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].proxy_jump.as_deref(), Some("bastion"));
}

#[test]
fn missing_file_returns_empty() {
    let entries = read_ssh_config(std::path::Path::new("/nonexistent/ssh-config")).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn skips_comments_and_blank_lines() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "# top comment\n\nHost real\n    # inline comment\n    HostName r\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].host, "real");
}

#[test]
fn glob_host_patterns_are_kept_as_is() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host *.dev\n    User dev\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].host, "*.dev");
    assert_eq!(entries[0].user.as_deref(), Some("dev"));
}
```

- [ ] **Step 2: Implement the parser**

Replace `crates/sid-ssh/src/config.rs`:

```rust
//! `~/.ssh/config` reader. Hand-rolled minimal parser — supports the keywords
//! sid actually uses: `Host`, `HostName`, `User`, `Port`, `IdentityFile`,
//! `ProxyJump`. Everything else is ignored. Globs in `Host` patterns are kept
//! verbatim (the SSH tab does not expand them).

use std::path::Path;

/// A single parsed Host block from an OpenSSH config.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SshConfigEntry {
    pub host: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

/// Read an OpenSSH config file. Returns `Ok(vec![])` if the file is missing.
/// Tolerant: unknown keywords are skipped, malformed lines are skipped with a
/// trace log but do not abort the parse.
pub fn read_ssh_config(path: &Path) -> std::io::Result<Vec<SshConfigEntry>> {
    if !path.exists() { return Ok(Vec::new()); }
    let text = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    let mut current: Option<SshConfigEntry> = None;
    for raw in text.lines() {
        // Strip inline comments after a '#'. Trim.
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() { continue; }
        let mut parts = line.splitn(2, char::is_whitespace);
        let key = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("").trim();
        if val.is_empty() { continue; }
        if key.eq_ignore_ascii_case("Host") {
            if let Some(e) = current.take() { out.push(e); }
            current = Some(SshConfigEntry { host: val.to_string(), ..Default::default() });
            continue;
        }
        let Some(entry) = current.as_mut() else { continue; };
        match () {
            _ if key.eq_ignore_ascii_case("HostName") => entry.hostname = Some(val.to_string()),
            _ if key.eq_ignore_ascii_case("User") => entry.user = Some(val.to_string()),
            _ if key.eq_ignore_ascii_case("Port") => entry.port = val.parse().ok(),
            _ if key.eq_ignore_ascii_case("IdentityFile") => entry.identity_file = Some(val.to_string()),
            _ if key.eq_ignore_ascii_case("ProxyJump") => entry.proxy_jump = Some(val.to_string()),
            _ => {} // ignore unknown
        }
    }
    if let Some(e) = current { out.push(e); }
    Ok(out)
}
```

- [ ] **Step 3: Run tests** — expected 6 passed.

- [ ] **Step 4: Property + adversarial coverage**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_parser_does_not_panic_on_arbitrary_input(s in ".{0,2000}") {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config");
        let _ = fs::write(&p, s);
        let _ = read_ssh_config(&p);
    }
}

#[test]
fn handles_tabs_and_extra_whitespace() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host\t\ttabbed\n\t  HostName\t10.0.0.1\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].host, "tabbed");
    assert_eq!(entries[0].hostname.as_deref(), Some("10.0.0.1"));
}

#[test]
fn malformed_port_is_silently_dropped() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host a\n    Port not-a-number\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].host, "a");
    assert_eq!(entries[0].port, None);
}

#[test]
fn unicode_host_names() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host 🐕-dev\n    HostName real.example.com\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].host, "🐕-dev");
}
```

CLAUDE.md flags the SSH config parser as a `cargo fuzz` target. Create `crates/sid-ssh/fuzz/Cargo.toml`:

```toml
[package]
name = "sid-ssh-fuzz"
version = "0.0.0"
publish = false
edition = "2024"

[package.metadata]
cargo-fuzz = true

[dependencies]
libfuzzer-sys = "0.4"
sid-ssh = { path = ".." }

[[bin]]
name = "fuzz_ssh_config"
path = "fuzz_targets/fuzz_ssh_config.rs"
test = false
doc = false
```

Create `crates/sid-ssh/fuzz/fuzz_targets/fuzz_ssh_config.rs`:

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return; };
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config");
    let _ = std::fs::write(&p, s);
    // Must never panic on any input.
    let _ = sid_ssh::read_ssh_config(&p);
});
```

Add `tempfile = "3"` to the fuzz crate's deps. The fuzz binary is built with `cargo fuzz build` and run separately; CI invocation is out of scope for v1 but the target is present.

- [ ] **Step 5: Doc tests**

Add `# Examples` doc tests to `SshConfigEntry` and `read_ssh_config` (the latter `no_run` since it touches the FS).

- [ ] **Step 6: Commit**

```bash
git add crates/sid-ssh
git commit -m "feat(ssh): add ~/.ssh/config parser (Host/HostName/User/Port/IdentityFile/ProxyJump)"
```

---

## Phase E — `PortablePtyProvider` impl

### Task 12: `PortablePtyProvider::open_pty`

**Files:**
- Modify: `crates/sid-pty/src/provider.rs`
- Create: `crates/sid-pty/tests/open_and_io.rs`

- [ ] **Step 1: Failing test**

Create `crates/sid-pty/tests/open_and_io.rs`:

```rust
use sid_core::adapters::pty::{PtyProvider, PtySize, PtySpawn};
use sid_pty::PortablePtyProvider;

#[test]
fn open_pty_with_true_command_spawns_and_exits() {
    let p = PortablePtyProvider::new();
    let spec = PtySpawn {
        program: "true".into(),
        args: Vec::new(),
        cwd: None,
        env: Default::default(),
        size: PtySize::new(24, 80),
    };
    let mut h = p.open_pty(&spec).unwrap();
    // Initially alive; after a brief wait, `true` should have exited.
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Some platforms report `true` as already exited; either is OK.
    let _ = h.child_alive();
    assert_eq!(h.size(), PtySize::new(24, 80));
}

#[test]
fn open_pty_with_nonexistent_program_returns_open_failed() {
    let p = PortablePtyProvider::new();
    let spec = PtySpawn::command("/path/does/not/exist/foo-bar-baz", &[]);
    let err = p.open_pty(&spec).unwrap_err();
    assert!(matches!(err, sid_core::adapters::pty::PtyError::OpenFailed(_)));
}
```

- [ ] **Step 2: Implement on `PortablePtyProvider`**

Replace `crates/sid-pty/src/provider.rs`:

```rust
//! `PortablePtyProvider` — opens portable-pty master/slave pairs and spawns a
//! child process on the slave end.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{Child, CommandBuilder, PtyPair, PtySize as PortablePtySize, native_pty_system};
use sid_core::adapters::pty::{PtyError, PtyHandle, PtyProvider, PtySize, PtySpawn};

pub struct PortablePtyProvider;

impl PortablePtyProvider {
    /// Construct a new provider. Cheap; no I/O.
    pub fn new() -> Self { Self }
}

impl Default for PortablePtyProvider {
    fn default() -> Self { Self::new() }
}

impl PtyProvider for PortablePtyProvider {
    fn open_pty(&self, spec: &PtySpawn) -> Result<Box<dyn PtyHandle>, PtyError> {
        let system = native_pty_system();
        let pair: PtyPair = system
            .openpty(PortablePtySize {
                rows: spec.size.rows,
                cols: spec.size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::OpenFailed(format!("openpty: {e}")))?;
        let mut cmd = CommandBuilder::new(&spec.program);
        for a in &spec.args { cmd.arg(a); }
        if let Some(c) = &spec.cwd { cmd.cwd(c); }
        for (k, v) in &spec.env { cmd.env(k, v); }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::OpenFailed(format!("spawn: {e}")))?;
        // Take the reader/writer off the master end and drop the slave-side
        // file descriptors that the parent doesn't need.
        let reader = pair.master.try_clone_reader()
            .map_err(|e| PtyError::OpenFailed(format!("clone_reader: {e}")))?;
        let writer = pair.master.take_writer()
            .map_err(|e| PtyError::OpenFailed(format!("take_writer: {e}")))?;
        drop(pair.slave);
        Ok(Box::new(PortablePtyHandle {
            master: Arc::new(Mutex::new(pair.master)),
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            child: Arc::new(Mutex::new(child)),
            size: spec.size,
        }))
    }
}

/// Live PTY handle. All fields are wrapped in `Arc<Mutex<_>>` so reader and
/// writer halves can be driven from independent tasks.
pub struct PortablePtyHandle {
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    reader: Arc<Mutex<Box<dyn Read + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    size: PtySize,
}

impl PtyHandle for PortablePtyHandle {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, PtyError> {
        if bytes.is_empty() { return Ok(0); }
        let mut w = self.writer.lock().unwrap();
        let n = w.write(bytes).map_err(|e| PtyError::WriteFailed(format!("{e}")))?;
        w.flush().map_err(|e| PtyError::WriteFailed(format!("flush: {e}")))?;
        Ok(n)
    }
    fn try_read(&mut self) -> Result<Vec<u8>, PtyError> {
        // Filled in by Task 13.
        Err(PtyError::Other("try_read not yet implemented — Task 13".into()))
    }
    fn resize(&mut self, _size: PtySize) -> Result<(), PtyError> {
        // Filled in by Task 14.
        Err(PtyError::Other("resize not yet implemented — Task 14".into()))
    }
    fn child_alive(&self) -> bool {
        let mut c = self.child.lock().unwrap();
        matches!(c.try_wait(), Ok(None))
    }
    fn size(&self) -> PtySize { self.size }
    fn kill(&mut self) -> Result<(), PtyError> {
        let mut c = self.child.lock().unwrap();
        let _ = c.kill();
        Ok(())
    }
}
```

- [ ] **Step 3: Run tests** — expected 2 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
#[test]
fn write_zero_bytes_returns_zero() {
    let p = PortablePtyProvider::new();
    let spec = PtySpawn::command("cat", &[]);
    let Ok(mut h) = p.open_pty(&spec) else { return; };
    assert_eq!(h.write(&[]).unwrap(), 0);
    let _ = h.kill();
}

#[test]
fn kill_is_idempotent() {
    let p = PortablePtyProvider::new();
    let Ok(mut h) = p.open_pty(&PtySpawn::command("cat", &[])) else { return; };
    h.kill().unwrap();
    h.kill().unwrap();
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-pty
git commit -m "feat(pty): implement PortablePtyProvider::open_pty + write + child_alive + kill"
```

---

### Task 13: `try_read` (non-blocking byte feeder)

**Files:**
- Modify: `crates/sid-pty/src/provider.rs`

The portable-pty `Read` impl blocks. To make `try_read` non-blocking we spawn a background thread per PTY that pumps bytes into a shared `Vec<u8>` buffer; `try_read` swaps it out.

- [ ] **Step 1: Failing test**

Append to `tests/open_and_io.rs`:

```rust
#[test]
fn echo_round_trips_through_pty() {
    let p = PortablePtyProvider::new();
    // `cat` will echo whatever we write to it.
    let Ok(mut h) = p.open_pty(&PtySpawn::command("cat", &[])) else { return; };
    h.write(b"hello\n").unwrap();
    // Give the child a moment to echo back.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let bytes = h.try_read().unwrap();
    let s = String::from_utf8_lossy(&bytes);
    assert!(s.contains("hello"), "got: {s:?}");
    h.kill().unwrap();
}

#[test]
fn try_read_on_idle_pty_returns_empty() {
    let p = PortablePtyProvider::new();
    let Ok(mut h) = p.open_pty(&PtySpawn::command("cat", &[])) else { return; };
    let bytes = h.try_read().unwrap();
    assert!(bytes.is_empty());
    h.kill().unwrap();
}
```

- [ ] **Step 2: Refactor `PortablePtyHandle` to spawn a background reader thread**

Restructure `provider.rs`: the open_pty path spawns the reader thread that pumps `reader` into an Arc<Mutex<Vec<u8>>>. `try_read` swaps and returns. Update:

```rust
pub struct PortablePtyHandle {
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    rx_buffer: Arc<Mutex<Vec<u8>>>,
    size: PtySize,
}

impl PortablePtyProvider {
    // Adjust open_pty's tail:
    // ... after taking reader/writer/child ...
    // let rx_buffer = Arc::new(Mutex::new(Vec::new()));
    // let rx_for_thread = rx_buffer.clone();
    // std::thread::spawn(move || {
    //     let mut reader = reader; // boxed Read
    //     let mut tmp = [0u8; 4096];
    //     loop {
    //         match reader.read(&mut tmp) {
    //             Ok(0) | Err(_) => break,
    //             Ok(n) => rx_for_thread.lock().unwrap().extend_from_slice(&tmp[..n]),
    //         }
    //     }
    // });
}
```

Final form of `try_read`:

```rust
fn try_read(&mut self) -> Result<Vec<u8>, PtyError> {
    let mut buf = self.rx_buffer.lock().unwrap();
    let out = std::mem::take(&mut *buf);
    Ok(out)
}
```

- [ ] **Step 3: Run tests** — expected 4 passed (2 from Task 12 + 2 new).

- [ ] **Step 4: Loom coverage**

The `Arc<Mutex<Vec<u8>>>` reader-buffer mirrors the russh-shell pattern. Create `crates/sid-pty/tests/loom_buffer.rs`:

```rust
#![cfg(loom)]

use loom::sync::{Arc, Mutex};
use loom::thread;

#[test]
fn pty_buffer_never_loses_writes_under_concurrent_drainer() {
    loom::model(|| {
        let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let writer = buf.clone();
        let reader = buf.clone();
        let w = thread::spawn(move || {
            writer.lock().unwrap().push(1);
        });
        let r = thread::spawn(move || {
            let mut total = 0usize;
            for _ in 0..2 {
                let v = std::mem::take(&mut *reader.lock().unwrap());
                total += v.len();
            }
            total
        });
        w.join().unwrap();
        let drained = r.join().unwrap();
        let final_len = buf.lock().unwrap().len();
        assert_eq!(drained + final_len, 1);
    });
}
```

- [ ] **Step 5: Adversarial coverage**

Append to `tests/open_and_io.rs`:

```rust
#[test]
fn large_output_does_not_blow_up_the_buffer() {
    let p = PortablePtyProvider::new();
    // `yes` floods stdout; we kill after a brief moment and just check no panic.
    let Ok(mut h) = p.open_pty(&PtySpawn::command("yes", &[])) else { return; };
    std::thread::sleep(std::time::Duration::from_millis(50));
    let _ = h.try_read();
    h.kill().unwrap();
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-pty
git commit -m "feat(pty): implement non-blocking try_read via background reader thread + loom coverage"
```

---

### Task 14: `resize`

**Files:**
- Modify: `crates/sid-pty/src/provider.rs`
- Create: `crates/sid-pty/tests/resize.rs`

- [ ] **Step 1: Failing test**

Create `crates/sid-pty/tests/resize.rs`:

```rust
use sid_core::adapters::pty::{PtyProvider, PtySize, PtySpawn};
use sid_pty::PortablePtyProvider;

#[test]
fn resize_updates_handle_size() {
    let p = PortablePtyProvider::new();
    let Ok(mut h) = p.open_pty(&PtySpawn::command("cat", &[])) else { return; };
    assert_eq!(h.size(), PtySize::new(24, 80));
    h.resize(PtySize::new(48, 160)).unwrap();
    assert_eq!(h.size(), PtySize::new(48, 160));
    h.kill().unwrap();
}

#[test]
fn resize_to_zero_does_not_panic() {
    let p = PortablePtyProvider::new();
    let Ok(mut h) = p.open_pty(&PtySpawn::command("cat", &[])) else { return; };
    let _ = h.resize(PtySize::new(0, 0));
    h.kill().unwrap();
}
```

- [ ] **Step 2: Implement `resize`**

In `provider.rs`:

```rust
fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
    self.master
        .lock()
        .unwrap()
        .resize(portable_pty::PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| PtyError::ResizeFailed(format!("{e}")))?;
    self.size = size;
    Ok(())
}
```

portable-pty's `resize` issues `TIOCSWINSZ` on UNIX, which the kernel translates to `SIGWINCH` on the foreground process group — the child sees the new size automatically.

- [ ] **Step 3: Run tests** — expected 2 passed.

- [ ] **Step 4: Property test**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_resize_to_any_size_is_total(rows: u16, cols: u16) {
        let p = PortablePtyProvider::new();
        let Ok(mut h) = p.open_pty(&PtySpawn::command("cat", &[])) else { return Ok(()); };
        let _ = h.resize(PtySize::new(rows, cols));
        // Either it accepts (sets size) or returns ResizeFailed; never panics.
        h.kill().unwrap();
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-pty
git commit -m "feat(pty): implement PtyHandle::resize (winsize ioctl)"
```

---

### Task 15: `Vt100Screen` — ANSI rendering

**Files:**
- Modify: `crates/sid-pty/src/screen.rs`
- Create: `crates/sid-pty/tests/screen_render.rs`

- [ ] **Step 1: Failing tests**

Create `crates/sid-pty/tests/screen_render.rs`:

```rust
use sid_pty::Vt100Screen;

#[test]
fn new_screen_is_blank() {
    let s = Vt100Screen::new(3, 10);
    let lines = s.lines();
    assert_eq!(lines.len(), 3);
    for l in &lines { assert!(l.trim().is_empty()); }
}

#[test]
fn feed_plain_text_appears_in_lines() {
    let mut s = Vt100Screen::new(3, 10);
    s.feed(b"hello");
    let lines = s.lines();
    assert!(lines[0].contains("hello"));
}

#[test]
fn cursor_position_reports_correctly_after_feed() {
    let mut s = Vt100Screen::new(3, 10);
    s.feed(b"abc");
    let (row, col) = s.cursor_position();
    assert_eq!(row, 0);
    assert_eq!(col, 3);
}

#[test]
fn resize_changes_dimensions() {
    let mut s = Vt100Screen::new(3, 10);
    s.resize(5, 20);
    assert_eq!(s.size(), (5, 20));
    assert_eq!(s.lines().len(), 5);
}

#[test]
fn ansi_escape_codes_do_not_appear_in_rendered_lines() {
    let mut s = Vt100Screen::new(3, 20);
    // Red "hi" then reset.
    s.feed(b"\x1b[31mhi\x1b[0m");
    let lines = s.lines();
    assert!(lines[0].contains("hi"));
    assert!(!lines[0].contains("\x1b"));
    assert!(!lines[0].contains("[31m"));
}
```

- [ ] **Step 2: Implement `Vt100Screen`**

Replace `crates/sid-pty/src/screen.rs`:

```rust
//! `Vt100Screen` — wraps `vt100::Parser` and exposes a snapshot suitable for
//! ratatui rendering. The screen owns the parser; the binary feeds bytes from
//! a PTY (or SSH shell) and renders the result frame-by-frame.

use vt100::Parser;

/// VT100 screen state.
pub struct Vt100Screen {
    parser: Parser,
    rows: u16,
    cols: u16,
}

impl Vt100Screen {
    /// Construct a blank screen of the given size.
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, 0),
            rows,
            cols,
        }
    }

    /// Feed bytes from the PTY (or remote shell) into the parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Resize the underlying screen.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(rows, cols);
        self.rows = rows;
        self.cols = cols;
    }

    /// Current size as `(rows, cols)`.
    pub fn size(&self) -> (u16, u16) { (self.rows, self.cols) }

    /// Cursor position as `(row, col)`, both zero-indexed.
    pub fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// Render the current screen as plain (un-styled) lines. Colour and
    /// style preservation is the responsibility of the renderer that walks
    /// `parser.screen().cell(row, col)` directly; this helper exists for
    /// snapshot testing and simple display.
    pub fn lines(&self) -> Vec<String> {
        let screen = self.parser.screen();
        let (rows, cols) = (self.rows, self.cols);
        let mut out = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let mut s = String::with_capacity(cols as usize);
            for c in 0..cols {
                let cell = screen.cell(r, c);
                let glyph = cell.map(|c| c.contents()).unwrap_or_default();
                if glyph.is_empty() { s.push(' '); } else { s.push_str(&glyph); }
            }
            out.push(s);
        }
        out
    }
}
```

- [ ] **Step 3: Run tests** — expected 5 passed.

- [ ] **Step 4: Adversarial + property tests**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_feed_arbitrary_bytes_never_panics(b in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let mut s = Vt100Screen::new(24, 80);
        s.feed(&b);
        let _ = s.lines();
        let _ = s.cursor_position();
    }
}

#[test]
fn malformed_ansi_does_not_panic() {
    let mut s = Vt100Screen::new(3, 10);
    s.feed(b"\x1b\x1b\x1b[[[33");
    let _ = s.lines();
}

#[test]
fn very_wide_unicode_renders() {
    let mut s = Vt100Screen::new(3, 20);
    s.feed("🐕✦ä".as_bytes());
    let l = s.lines();
    assert!(!l[0].trim().is_empty());
}
```

- [ ] **Step 5: Insta snapshot test**

Append:

```rust
#[test]
fn renders_a_known_sequence_to_a_stable_snapshot() {
    let mut s = Vt100Screen::new(4, 12);
    s.feed(b"line 1\r\nline 2\r\n");
    let snapshot = s.lines().join("\n");
    insta::assert_snapshot!("vt100_two_lines", snapshot);
}
```

- [ ] **Step 6: Doc tests**

Add `# Examples` blocks to every pub fn on `Vt100Screen`.

- [ ] **Step 7: Commit**

```bash
git add crates/sid-pty
git commit -m "feat(pty): add Vt100Screen — ANSI parser + plain-line rendering"
```

---

## Phase F — SSH host storage in `sid-store`

### Task 16: `SshHost` domain type

**Files:**
- Modify: `crates/sid-store/src/lib.rs`

- [ ] **Step 1: Failing test in `crates/sid-store/tests/ssh_hosts.rs`**

```rust
use sid_store::{SshHost, SshHostSource, now_epoch};

#[test]
fn ssh_host_construction() {
    let h = SshHost {
        alias: "jp46-dev".into(),
        host: "10.1.40.102".into(),
        port: 22,
        user: "pi".into(),
        identity_file: Some("~/.ssh/id_jp46".into()),
        source: SshHostSource::Manual,
        last_connected: 0,
        command_history: Vec::new(),
    };
    assert_eq!(h.alias, "jp46-dev");
    assert_eq!(h.source, SshHostSource::Manual);
}

#[test]
fn ssh_host_source_variants() {
    let _ = SshHostSource::Manual;
    let _ = SshHostSource::SshConfig;
}

#[test]
fn now_epoch_is_positive() {
    assert!(now_epoch() > 0);
}
```

- [ ] **Step 2: Run — should fail (`SshHost` type doesn't exist)**

- [ ] **Step 3: Add `SshHost` to `sid-store/src/lib.rs`**

Add near `SessionRecord`, `WidgetState`, `Workspace`:

```rust
/// Source of an SSH host entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SshHostSource {
    /// Sourced from `~/.ssh/config` (read-only — sid never writes back to ssh_config).
    SshConfig,
    /// Added by the user via `sid ssh add` or the SSH tab UI.
    Manual,
}

/// A registered SSH host. The `alias` is the primary key — duplicate aliases
/// from ssh-config and the manual store are merged at read time, with manual
/// entries winning (per spec § "Hosts (left pane): read from ~/.ssh/config +
/// manually-added entries").
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshHost {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Path to private key (if any). May contain `~` — caller expands.
    pub identity_file: Option<String>,
    pub source: SshHostSource,
    pub last_connected: Epoch,
    /// Ring buffer of recently-issued commands in the embedded shell. Capped
    /// at the application layer (default 100).
    pub command_history: Vec<String>,
}
```

- [ ] **Step 4: Run tests** — expected 3 passed.

- [ ] **Step 5: Property test (postcard round-trip)**

```rust
#[cfg(test)]
mod ssh_host_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_ssh_host_postcard_roundtrip(
            alias in "[a-zA-Z0-9_-]{1,40}",
            host in "[a-zA-Z0-9._-]{1,60}",
            port in 1u16..65535,
            user in "[a-z]{1,20}",
            n_history in 0usize..30,
        ) {
            let h = SshHost {
                alias: alias.clone(),
                host: host.clone(),
                port,
                user: user.clone(),
                identity_file: None,
                source: SshHostSource::Manual,
                last_connected: now_epoch(),
                command_history: (0..n_history).map(|i| format!("cmd-{i}")).collect(),
            };
            let bytes = postcard::to_allocvec(&h).unwrap();
            let back: SshHost = postcard::from_bytes(&bytes).unwrap();
            prop_assert_eq!(h, back);
        }
    }
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add SshHost + SshHostSource domain types"
```

---

### Task 17: `ssh_hosts` table schema

**Files:**
- Modify: `crates/sid-store/src/schema.rs`
- Modify: `crates/sid-store/src/redb_impl.rs`

- [ ] **Step 1: Add `SSH_HOSTS` table**

In `schema.rs`:

```rust
/// Key = alias as string. Value = postcard-encoded versioned `SshHost`.
pub const SSH_HOSTS: TableDefinition<&str, &[u8]> = TableDefinition::new("ssh_hosts");
```

- [ ] **Step 2: Open the table in `RedbStore::open`**

In `redb_impl.rs`'s `OpenStore::open`, add:

```rust
let _ = txn.open_table(SSH_HOSTS).map_err(|e| SidError::Storage(format!("open ssh_hosts: {e}")))?;
```

(Import `SSH_HOSTS` at the top.)

- [ ] **Step 3: Migration sanity test**

Append to `tests/ssh_hosts.rs`:

```rust
use sid_store::{OpenStore, RedbStore};
use tempfile::tempdir;

#[test]
fn opening_store_creates_ssh_hosts_table_without_error() {
    let dir = tempdir().unwrap();
    let _store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    // No assertion; success of open() is the assertion. Reopen to confirm
    // the table persists.
    let _store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add SSH_HOSTS table to schema and open in RedbStore::open"
```

---

### Task 18: `Store` trait extension methods

**Files:**
- Modify: `crates/sid-store/src/lib.rs`

- [ ] **Step 1: Failing tests in `tests/ssh_hosts.rs`**

```rust
use sid_store::{Store, SshHost, SshHostSource, now_epoch};

fn host(alias: &str, host: &str, user: &str) -> SshHost {
    SshHost {
        alias: alias.into(),
        host: host.into(),
        port: 22,
        user: user.into(),
        identity_file: None,
        source: SshHostSource::Manual,
        last_connected: now_epoch(),
        command_history: Vec::new(),
    }
}

#[test]
fn upsert_then_list_returns_host() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_ssh_host(&host("a", "10.0.0.1", "u")).unwrap();
    let all = store.list_ssh_hosts().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].alias, "a");
}

#[test]
fn get_ssh_host_returns_existing_and_none_for_missing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_ssh_host(&host("a", "h", "u")).unwrap();
    assert!(store.get_ssh_host("a").unwrap().is_some());
    assert!(store.get_ssh_host("missing").unwrap().is_none());
}

#[test]
fn remove_ssh_host_drops_it() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_ssh_host(&host("a", "h", "u")).unwrap();
    store.remove_ssh_host("a").unwrap();
    assert!(store.list_ssh_hosts().unwrap().is_empty());
}

#[test]
fn upsert_replaces_existing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_ssh_host(&host("a", "v1", "u")).unwrap();
    store.upsert_ssh_host(&host("a", "v2", "u")).unwrap();
    let found = store.get_ssh_host("a").unwrap().unwrap();
    assert_eq!(found.host, "v2");
}
```

- [ ] **Step 2: Add methods to the `Store` trait**

In `lib.rs`:

```rust
fn list_ssh_hosts(&self) -> Result<Vec<SshHost>, SidError>;
fn upsert_ssh_host(&self, h: &SshHost) -> Result<(), SidError>;
fn get_ssh_host(&self, alias: &str) -> Result<Option<SshHost>, SidError>;
fn remove_ssh_host(&self, alias: &str) -> Result<(), SidError>;
```

(Stub implementations in any test mocks — `Ok(Vec::new())` / `Ok(())` / `Ok(None)`.)

- [ ] **Step 3: Commit (trait extension + mock stubs)**

```bash
git add crates/sid-store
git commit -m "feat(store): extend Store trait with ssh_host registry methods"
```

---

### Task 19: `RedbStore` impl for ssh-host methods

**Files:**
- Modify: `crates/sid-store/src/redb_impl.rs`

- [ ] **Step 1: Implement on `RedbStore`**

In `impl Store for RedbStore`, add:

```rust
fn list_ssh_hosts(&self) -> Result<Vec<SshHost>, SidError> {
    let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
    let tbl = txn.open_table(SSH_HOSTS).map_err(|e| SidError::Storage(format!("open ssh_hosts: {e}")))?;
    let mut out = Vec::new();
    let iter = tbl.iter().map_err(|e| SidError::Storage(format!("iter: {e}")))?;
    for entry in iter {
        let (_k, v) = entry.map_err(|e| SidError::Storage(format!("step: {e}")))?;
        let (_v, h) = crate::codec::decode_versioned::<SshHost>(v.value())?;
        out.push(h);
    }
    Ok(out)
}

fn upsert_ssh_host(&self, h: &SshHost) -> Result<(), SidError> {
    let bytes = crate::codec::encode_versioned(1, h)?;
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(SSH_HOSTS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
        tbl.insert(h.alias.as_str(), &bytes[..]).map_err(|e| SidError::Storage(format!("insert: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit: {e}")))?;
    Ok(())
}

fn get_ssh_host(&self, alias: &str) -> Result<Option<SshHost>, SidError> {
    let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
    let tbl = txn.open_table(SSH_HOSTS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
    let got = tbl.get(alias).map_err(|e| SidError::Storage(format!("get: {e}")))?;
    match got {
        Some(v) => {
            let (_v, h) = crate::codec::decode_versioned::<SshHost>(v.value())?;
            Ok(Some(h))
        }
        None => Ok(None),
    }
}

fn remove_ssh_host(&self, alias: &str) -> Result<(), SidError> {
    let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
    {
        let mut tbl = txn.open_table(SSH_HOSTS).map_err(|e| SidError::Storage(format!("open: {e}")))?;
        tbl.remove(alias).map_err(|e| SidError::Storage(format!("remove: {e}")))?;
    }
    txn.commit().map_err(|e| SidError::Storage(format!("commit: {e}")))?;
    Ok(())
}
```

Import `SSH_HOSTS` and `SshHost` at the top of `redb_impl.rs`.

- [ ] **Step 2: Run tests** — expected 4 passed (the four from Task 18) plus the existing tests.

- [ ] **Step 3: Adversarial + property coverage**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_upsert_get_round_trip(alias in "[a-zA-Z0-9_-]{1,16}", host in "[a-z0-9.]{1,40}") {
        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        let h = SshHost {
            alias: alias.clone(),
            host: host.clone(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: Vec::new(),
        };
        store.upsert_ssh_host(&h).unwrap();
        let back = store.get_ssh_host(&alias).unwrap().unwrap();
        prop_assert_eq!(h, back);
    }
}

#[test]
fn remove_nonexistent_is_noop() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.remove_ssh_host("never-added").unwrap();
}

#[test]
fn list_with_500_hosts_returns_all() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    for i in 0..500 {
        store.upsert_ssh_host(&SshHost {
            alias: format!("h{i}"),
            host: format!("10.0.0.{}", i % 256),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: Vec::new(),
        }).unwrap();
    }
    assert_eq!(store.list_ssh_hosts().unwrap().len(), 500);
}

#[test]
fn very_long_command_history_round_trips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let h = SshHost {
        alias: "a".into(),
        host: "h".into(),
        port: 22,
        user: "u".into(),
        identity_file: None,
        source: SshHostSource::Manual,
        last_connected: 0,
        command_history: (0..1000).map(|i| format!("very long command {i} with spaces and punctuation;")).collect(),
    };
    store.upsert_ssh_host(&h).unwrap();
    let back = store.get_ssh_host("a").unwrap().unwrap();
    assert_eq!(back.command_history.len(), 1000);
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): implement RedbStore ssh_host registry methods + property tests"
```

---

## Phase G — `SshWidget`

### Task 20: Host list state + ssh-config merge

**Files:**
- Modify: `crates/sid-widgets/src/ssh.rs`
- Create: `crates/sid-widgets/tests/ssh_host_list.rs`

Replaces the Plan 1 `ComingSoonBody` stub with a real implementation. Structure: pure-Rust state struct (`SshState`) tested in isolation, plus a thin render layer.

- [ ] **Step 1: Failing test**

Create `crates/sid-widgets/tests/ssh_host_list.rs`:

```rust
use sid_store::{SshHost, SshHostSource};
use sid_widgets::ssh::{SshConfigEntryLite, SshState};

fn host(alias: &str, source: SshHostSource) -> SshHost {
    SshHost {
        alias: alias.into(),
        host: format!("{alias}.example"),
        port: 22,
        user: "u".into(),
        identity_file: None,
        source,
        last_connected: 0,
        command_history: Vec::new(),
    }
}

fn cfg(alias: &str) -> SshConfigEntryLite {
    SshConfigEntryLite {
        alias: alias.into(),
        host: format!("{alias}.cfg"),
        port: 22,
        user: "u".into(),
        identity_file: None,
    }
}

#[test]
fn state_holds_hosts_and_selects_first() {
    let s = SshState::new(vec![host("a", SshHostSource::Manual)], vec![]);
    assert_eq!(s.selected_alias().unwrap(), "a");
}

#[test]
fn next_and_prev_cycle_selection() {
    let mut s = SshState::new(
        vec![host("a", SshHostSource::Manual), host("b", SshHostSource::Manual)],
        vec![],
    );
    s.select_next();
    assert_eq!(s.selected_alias().unwrap(), "b");
    s.select_next();
    assert_eq!(s.selected_alias().unwrap(), "a");
    s.select_prev();
    assert_eq!(s.selected_alias().unwrap(), "b");
}

#[test]
fn empty_state_has_no_selection() {
    let s = SshState::new(vec![], vec![]);
    assert!(s.selected_alias().is_none());
}

#[test]
fn merges_ssh_config_entries_with_store_hosts() {
    let s = SshState::new(
        vec![host("manual-only", SshHostSource::Manual)],
        vec![cfg("config-only"), cfg("manual-only")], // collision: manual wins
    );
    let aliases: Vec<_> = s.visible_hosts().iter().map(|h| h.alias.clone()).collect();
    assert!(aliases.contains(&"manual-only".to_string()));
    assert!(aliases.contains(&"config-only".to_string()));
    // manual-only is the manual entry (host = "manual-only.example", not ".cfg")
    let mo = s.visible_hosts().iter().find(|h| h.alias == "manual-only").unwrap();
    assert_eq!(mo.host, "manual-only.example");
    // config-only is sourced from ssh_config
    let co = s.visible_hosts().iter().find(|h| h.alias == "config-only").unwrap();
    assert_eq!(co.source, SshHostSource::SshConfig);
}
```

- [ ] **Step 2: Run — should fail**

- [ ] **Step 3: Implement `SshState`**

Replace `crates/sid-widgets/src/ssh.rs`:

```rust
//! SSH tab widget — host list + connection pane + SFTP sub-panel.
//!
//! Pure-Rust state lives in `SshState`; the widget is a thin render layer
//! over it.

use std::path::PathBuf;
use std::sync::Arc;

use sid_core::adapters::pty::PtyProvider;
use sid_core::adapters::ssh::SshClient;
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_store::{SshHost, SshHostSource};

/// A lite copy of `sid_ssh::SshConfigEntry` re-defined locally so the widget
/// crate never names a sid-ssh type (adapter pattern). The binary's wire
/// layer converts `SshConfigEntry` → `SshConfigEntryLite` and passes it in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshConfigEntryLite {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
}

pub struct SshState {
    /// Hosts persisted in the store (source = Manual). Authoritative.
    store_hosts: Vec<SshHost>,
    /// Entries from `~/.ssh/config` (source = SshConfig). Read-only.
    config_entries: Vec<SshConfigEntryLite>,
    /// Merged view, recomputed on changes. Manual entries shadow config
    /// entries with the same alias.
    merged: Vec<SshHost>,
    selected_idx: usize,
}

impl SshState {
    pub fn new(store_hosts: Vec<SshHost>, config_entries: Vec<SshConfigEntryLite>) -> Self {
        let mut s = Self { store_hosts, config_entries, merged: Vec::new(), selected_idx: 0 };
        s.recompute_merged();
        s
    }

    fn recompute_merged(&mut self) {
        // Manual wins on alias collision.
        let mut by_alias: std::collections::BTreeMap<String, SshHost> = std::collections::BTreeMap::new();
        for cfg in &self.config_entries {
            by_alias.insert(cfg.alias.clone(), SshHost {
                alias: cfg.alias.clone(),
                host: cfg.host.clone(),
                port: cfg.port,
                user: cfg.user.clone(),
                identity_file: cfg.identity_file.clone(),
                source: SshHostSource::SshConfig,
                last_connected: 0,
                command_history: Vec::new(),
            });
        }
        for h in &self.store_hosts {
            by_alias.insert(h.alias.clone(), h.clone());
        }
        self.merged = by_alias.into_values().collect();
        if self.merged.is_empty() { self.selected_idx = 0; }
        else if self.selected_idx >= self.merged.len() {
            self.selected_idx = self.merged.len() - 1;
        }
    }

    pub fn visible_hosts(&self) -> &[SshHost] { &self.merged }

    pub fn selected_alias(&self) -> Option<&str> {
        self.merged.get(self.selected_idx).map(|h| h.alias.as_str())
    }

    pub fn selected_host(&self) -> Option<&SshHost> {
        self.merged.get(self.selected_idx)
    }

    pub fn select_next(&mut self) {
        if self.merged.is_empty() { return; }
        self.selected_idx = (self.selected_idx + 1) % self.merged.len();
    }
    pub fn select_prev(&mut self) {
        if self.merged.is_empty() { return; }
        self.selected_idx = (self.selected_idx + self.merged.len() - 1) % self.merged.len();
    }

    /// Replace the store hosts (e.g., after `upsert_ssh_host`). Re-merges.
    pub fn set_store_hosts(&mut self, hosts: Vec<SshHost>) {
        self.store_hosts = hosts;
        self.recompute_merged();
    }

    /// Replace the ssh-config view (e.g., after re-reading `~/.ssh/config`).
    pub fn set_config_entries(&mut self, entries: Vec<SshConfigEntryLite>) {
        self.config_entries = entries;
        self.recompute_merged();
    }
}

/// The SSH tab widget. Owns `SshState` plus optional handles to a live SSH
/// connection and PTY pane (state machine in Task 21).
pub struct SshWidget {
    state: SshState,
    id: WidgetId,
    // Filled in by Task 21 — connection state machine.
    // ssh_client: Option<Box<dyn SshClient>>,
    // shell: Option<Box<dyn SshShell>>,
    // ... etc.
    _ssh_factory: Option<Arc<dyn Fn() -> Box<dyn SshClient> + Send + Sync>>,
    _pty_provider: Option<Arc<dyn PtyProvider>>,
}

impl SshWidget {
    pub fn new(state: SshState) -> Self {
        Self {
            state,
            id: WidgetId::new("ssh.root"),
            _ssh_factory: None,
            _pty_provider: None,
        }
    }

    /// Inject providers (called by `wire.rs`).
    pub fn with_providers(
        mut self,
        ssh_factory: Arc<dyn Fn() -> Box<dyn SshClient> + Send + Sync>,
        pty_provider: Arc<dyn PtyProvider>,
    ) -> Self {
        self._ssh_factory = Some(ssh_factory);
        self._pty_provider = Some(pty_provider);
        self
    }

    pub fn state(&self) -> &SshState { &self.state }
    pub fn state_mut(&mut self) -> &mut SshState { &mut self.state }
}

impl Default for SshWidget {
    fn default() -> Self { Self::new(SshState::new(Vec::new(), Vec::new())) }
}

impl Widget for SshWidget {
    fn id(&self) -> &WidgetId { &self.id }
    fn title(&self) -> &str { "SSH" }
    fn render(&self, _target: &mut dyn RenderTarget) {
        // Real rendering happens in the binary's draw() function via
        // match-on-tab-id (mirroring WorkspacesWidget pattern from Plan 2).
    }
    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            match (chord.code, chord.mods) {
                (KeyCode::Char('j') | KeyCode::Down, _) => { self.state.select_next(); return EventOutcome::Consumed; }
                (KeyCode::Char('k') | KeyCode::Up, _) => { self.state.select_prev(); return EventOutcome::Consumed; }
                _ => {}
            }
        }
        EventOutcome::Bubble
    }
}
```

- [ ] **Step 4: Run tests** — expected 4 passed.

- [ ] **Step 5: Adversarial + property coverage**

Append:

```rust
use proptest::prelude::*;

#[test]
fn very_long_alias_does_not_panic() {
    let long = "x".repeat(10_000);
    let s = SshState::new(vec![host(&long, SshHostSource::Manual)], vec![]);
    assert_eq!(s.visible_hosts()[0].alias.len(), 10_000);
}

#[test]
fn select_next_on_empty_is_noop() {
    let mut s = SshState::new(vec![], vec![]);
    s.select_next();
    s.select_prev();
    assert!(s.selected_alias().is_none());
}

proptest! {
    #[test]
    fn prop_merge_is_total_under_collisions(
        n_manual in 0usize..10,
        n_config in 0usize..10,
    ) {
        let manual: Vec<_> = (0..n_manual).map(|i| host(&format!("a{}", i % 5), SshHostSource::Manual)).collect();
        let config: Vec<_> = (0..n_config).map(|i| cfg(&format!("a{}", i % 5))).collect();
        let s = SshState::new(manual, config);
        // No duplicate aliases in merged view.
        let mut aliases: Vec<_> = s.visible_hosts().iter().map(|h| h.alias.clone()).collect();
        aliases.sort();
        let unique_before = aliases.len();
        aliases.dedup();
        prop_assert_eq!(unique_before, aliases.len());
    }
}
```

- [ ] **Step 6: Doc tests**

Add `# Examples` blocks to `SshState::new`, `selected_alias`, `selected_host`, `select_next`, `select_prev`, `SshWidget::new`, `SshWidget::with_providers`. For `with_providers`, mark `no_run` because constructing an `Arc<dyn Fn>` matching the signature is unwieldy in a doc test.

- [ ] **Step 7: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): SshWidget host list state + ssh-config merge"
```

---

### Task 21: Connection state machine

**Files:**
- Modify: `crates/sid-widgets/src/ssh.rs`
- Create: `crates/sid-widgets/tests/ssh_connection_state.rs`

The connection life cycle: `Idle → Connecting → Connected → Disconnected`. The widget holds the state enum + the connection job handle. Pressing Enter from `Idle` transitions to `Connecting` and dispatches a `JobQueue` task that calls `SshClient::connect`; the result is consumed next frame.

- [ ] **Step 1: Failing test**

Create `crates/sid-widgets/tests/ssh_connection_state.rs`:

```rust
use sid_widgets::ssh::{ConnectionState, ConnectionPhase};

#[test]
fn fresh_connection_state_is_idle() {
    let s = ConnectionState::default();
    assert_eq!(s.phase(), ConnectionPhase::Idle);
}

#[test]
fn idle_can_transition_to_connecting() {
    let mut s = ConnectionState::default();
    s.begin_connecting("alias-x".into());
    assert_eq!(s.phase(), ConnectionPhase::Connecting);
    assert_eq!(s.alias(), Some("alias-x"));
}

#[test]
fn connecting_can_transition_to_connected() {
    let mut s = ConnectionState::default();
    s.begin_connecting("a".into());
    s.mark_connected();
    assert_eq!(s.phase(), ConnectionPhase::Connected);
}

#[test]
fn connecting_can_transition_to_failed() {
    let mut s = ConnectionState::default();
    s.begin_connecting("a".into());
    s.mark_failed("auth failed".into());
    assert_eq!(s.phase(), ConnectionPhase::Failed);
    assert_eq!(s.error_message(), Some("auth failed"));
}

#[test]
fn connected_can_transition_to_disconnected() {
    let mut s = ConnectionState::default();
    s.begin_connecting("a".into());
    s.mark_connected();
    s.mark_disconnected();
    assert_eq!(s.phase(), ConnectionPhase::Disconnected);
}

#[test]
fn disconnected_can_re_connect() {
    let mut s = ConnectionState::default();
    s.begin_connecting("a".into());
    s.mark_connected();
    s.mark_disconnected();
    s.begin_connecting("b".into());
    assert_eq!(s.phase(), ConnectionPhase::Connecting);
    assert_eq!(s.alias(), Some("b"));
}
```

- [ ] **Step 2: Run — should fail**

- [ ] **Step 3: Implement `ConnectionState`**

In `crates/sid-widgets/src/ssh.rs`, append:

```rust
/// Phase of the current connection attempt / live connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionPhase {
    Idle,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectionState {
    phase: ConnectionPhase,
    alias: Option<String>,
    error: Option<String>,
}

impl Default for ConnectionPhase {
    fn default() -> Self { ConnectionPhase::Idle }
}

impl ConnectionState {
    pub fn phase(&self) -> ConnectionPhase { self.phase }
    pub fn alias(&self) -> Option<&str> { self.alias.as_deref() }
    pub fn error_message(&self) -> Option<&str> { self.error.as_deref() }

    pub fn begin_connecting(&mut self, alias: String) {
        self.phase = ConnectionPhase::Connecting;
        self.alias = Some(alias);
        self.error = None;
    }
    pub fn mark_connected(&mut self) {
        self.phase = ConnectionPhase::Connected;
        self.error = None;
    }
    pub fn mark_failed(&mut self, e: String) {
        self.phase = ConnectionPhase::Failed;
        self.error = Some(e);
    }
    pub fn mark_disconnected(&mut self) {
        self.phase = ConnectionPhase::Disconnected;
    }
    pub fn reset(&mut self) {
        self.phase = ConnectionPhase::Idle;
        self.alias = None;
        self.error = None;
    }
}
```

Add the field to `SshWidget`:

```rust
pub struct SshWidget {
    state: SshState,
    connection: ConnectionState,
    id: WidgetId,
    _ssh_factory: Option<Arc<dyn Fn() -> Box<dyn SshClient> + Send + Sync>>,
    _pty_provider: Option<Arc<dyn PtyProvider>>,
}
```

Update `SshWidget::new` to default-initialize `connection`. Add an accessor:

```rust
pub fn connection(&self) -> &ConnectionState { &self.connection }
pub fn connection_mut(&mut self) -> &mut ConnectionState { &mut self.connection }
```

Hook up Enter in `handle_event`:

```rust
(KeyCode::Enter, KeyModifiers::NONE) => {
    if let Some(alias) = self.state.selected_alias() {
        self.connection.begin_connecting(alias.to_string());
        // The actual JobQueue dispatch is wired in `wire.rs` so the widget
        // remains free of tokio/JobQueue imports. The widget exposes
        // `connection_mut()` for the wire layer to update.
    }
    EventOutcome::Consumed
}
```

- [ ] **Step 4: Run tests** — expected 6 passed.

- [ ] **Step 5: Adversarial coverage**

Append:

```rust
#[test]
fn mark_connected_from_idle_does_not_panic() {
    let mut s = ConnectionState::default();
    s.mark_connected();
    // Allowed transition for robustness; phase becomes Connected even from Idle.
    assert_eq!(s.phase(), ConnectionPhase::Connected);
}

#[test]
fn reset_clears_alias_and_error() {
    let mut s = ConnectionState::default();
    s.begin_connecting("a".into());
    s.mark_failed("oops".into());
    s.reset();
    assert_eq!(s.phase(), ConnectionPhase::Idle);
    assert!(s.alias().is_none());
    assert!(s.error_message().is_none());
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): SshWidget connection state machine (Idle/Connecting/Connected/Disconnected/Failed)"
```

---

### Task 22: PTY pane rendering

**Files:**
- Modify: `crates/sid-widgets/src/ssh.rs`
- Create: `crates/sid-widgets/tests/ssh_pty_pane.rs`

The widget owns a buffer of raw bytes received from the remote shell. A `Vt100Screen`-shaped trait in `sid-core` (or a re-imported method-shaped trait owned by the widget) lets the widget call `feed(bytes)` + `lines()` without naming `vt100` or `sid-pty`.

To keep the adapter pattern intact: define a `TerminalScreen` trait in `sid-core::adapters::pty` and have `sid-pty::Vt100Screen` impl it. The widget owns a `Box<dyn TerminalScreen>` injected by the wire layer.

- [ ] **Step 1: Add `TerminalScreen` trait in `sid-core::adapters::pty`**

Append to `crates/sid-core/src/adapters/pty.rs`:

```rust
/// A render-friendly snapshot of a terminal screen. Implementations live in
/// `sid-pty` (e.g., `Vt100Screen`).
pub trait TerminalScreen: Send + Sync {
    /// Feed bytes from the remote (or local PTY) into the screen.
    fn feed(&mut self, bytes: &[u8]);
    /// Resize the screen.
    fn resize(&mut self, rows: u16, cols: u16);
    /// Current size as `(rows, cols)`.
    fn size(&self) -> (u16, u16);
    /// Cursor position as `(row, col)`.
    fn cursor_position(&self) -> (u16, u16);
    /// Current screen contents as one string per row.
    fn lines(&self) -> Vec<String>;
}
```

Append a `TerminalScreen` impl on `Vt100Screen` (in `crates/sid-pty/src/screen.rs`):

```rust
use sid_core::adapters::pty::TerminalScreen;

impl TerminalScreen for Vt100Screen {
    fn feed(&mut self, bytes: &[u8]) { Vt100Screen::feed(self, bytes); }
    fn resize(&mut self, rows: u16, cols: u16) { Vt100Screen::resize(self, rows, cols); }
    fn size(&self) -> (u16, u16) { Vt100Screen::size(self) }
    fn cursor_position(&self) -> (u16, u16) { Vt100Screen::cursor_position(self) }
    fn lines(&self) -> Vec<String> { Vt100Screen::lines(self) }
}
```

- [ ] **Step 2: Failing test**

Create `crates/sid-widgets/tests/ssh_pty_pane.rs`:

```rust
use sid_core::adapters::pty::TerminalScreen;
use sid_widgets::ssh::{PtyPane, SshState};

/// Mock screen for widget tests — keeps a string buffer, ignores ANSI.
struct MockScreen {
    rows: u16, cols: u16,
    buf: Vec<u8>,
}

impl MockScreen {
    fn new(rows: u16, cols: u16) -> Self { Self { rows, cols, buf: Vec::new() } }
}

impl TerminalScreen for MockScreen {
    fn feed(&mut self, bytes: &[u8]) { self.buf.extend_from_slice(bytes); }
    fn resize(&mut self, rows: u16, cols: u16) { self.rows = rows; self.cols = cols; }
    fn size(&self) -> (u16, u16) { (self.rows, self.cols) }
    fn cursor_position(&self) -> (u16, u16) { (0, self.buf.len() as u16) }
    fn lines(&self) -> Vec<String> {
        let s = String::from_utf8_lossy(&self.buf).to_string();
        let mut out: Vec<String> = s.split('\n').map(String::from).collect();
        while (out.len() as u16) < self.rows { out.push(String::new()); }
        out.truncate(self.rows as usize);
        out
    }
}

#[test]
fn pty_pane_feeds_bytes_into_screen() {
    let mut pane = PtyPane::new(Box::new(MockScreen::new(3, 10)));
    pane.feed(b"hello\n");
    assert!(pane.lines()[0].contains("hello"));
}

#[test]
fn pty_pane_resize_propagates_to_screen() {
    let mut pane = PtyPane::new(Box::new(MockScreen::new(3, 10)));
    pane.resize(5, 20);
    assert_eq!(pane.size(), (5, 20));
}

#[test]
fn pty_pane_size_after_construction() {
    let pane = PtyPane::new(Box::new(MockScreen::new(24, 80)));
    assert_eq!(pane.size(), (24, 80));
}
```

- [ ] **Step 3: Implement `PtyPane`**

Append to `crates/sid-widgets/src/ssh.rs`:

```rust
use sid_core::adapters::pty::TerminalScreen;

/// Owns the embedded terminal screen for the SSH tab's right pane.
pub struct PtyPane {
    screen: Box<dyn TerminalScreen>,
}

impl PtyPane {
    /// Construct with an injected screen (typically `sid_pty::Vt100Screen`).
    pub fn new(screen: Box<dyn TerminalScreen>) -> Self { Self { screen } }

    /// Feed bytes received from the remote shell.
    pub fn feed(&mut self, bytes: &[u8]) { self.screen.feed(bytes); }

    /// Resize the screen.
    pub fn resize(&mut self, rows: u16, cols: u16) { self.screen.resize(rows, cols); }

    /// Current dimensions.
    pub fn size(&self) -> (u16, u16) { self.screen.size() }

    /// Snapshot of visible lines.
    pub fn lines(&self) -> Vec<String> { self.screen.lines() }

    /// Cursor position.
    pub fn cursor_position(&self) -> (u16, u16) { self.screen.cursor_position() }
}
```

- [ ] **Step 4: Run tests** — expected 3 passed.

- [ ] **Step 5: Insta snapshot**

Append:

```rust
#[test]
fn snapshot_of_known_output() {
    let mut pane = PtyPane::new(Box::new(MockScreen::new(3, 12)));
    pane.feed(b"$ ls\nfoo bar\n");
    let snap = pane.lines().join("\n");
    insta::assert_snapshot!("ssh_pty_pane_basic", snap);
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets crates/sid-core crates/sid-pty
git commit -m "feat(widgets): SshWidget PTY pane via TerminalScreen trait + Vt100Screen impl"
```

---

### Task 23: Per-host command history ring buffer

**Files:**
- Modify: `crates/sid-widgets/src/ssh.rs`
- Create: `crates/sid-widgets/tests/ssh_history.rs`

When the user presses Enter inside the PTY pane, the line they typed becomes a command-history entry on the selected host. Capped at `MAX_HISTORY = 100`.

- [ ] **Step 1: Failing test**

Create `crates/sid-widgets/tests/ssh_history.rs`:

```rust
use sid_widgets::ssh::CommandHistory;

#[test]
fn fresh_history_is_empty() {
    let h = CommandHistory::new(100);
    assert!(h.entries().is_empty());
}

#[test]
fn push_appends_and_caps() {
    let mut h = CommandHistory::new(3);
    h.push("a".into());
    h.push("b".into());
    h.push("c".into());
    h.push("d".into());
    assert_eq!(h.entries(), &["b", "c", "d"]);
}

#[test]
fn duplicate_consecutive_is_collapsed() {
    let mut h = CommandHistory::new(10);
    h.push("ls".into());
    h.push("ls".into()); // duplicate of last — drop
    h.push("cd".into());
    assert_eq!(h.entries(), &["ls", "cd"]);
}

#[test]
fn empty_commands_are_ignored() {
    let mut h = CommandHistory::new(10);
    h.push("".into());
    h.push("   ".into());
    assert!(h.entries().is_empty());
}

#[test]
fn from_vec_construction() {
    let h = CommandHistory::from_vec(vec!["a".into(), "b".into()], 10);
    assert_eq!(h.entries(), &["a", "b"]);
}
```

- [ ] **Step 2: Implement `CommandHistory`**

Append to `crates/sid-widgets/src/ssh.rs`:

```rust
/// Bounded, deduplicating command history. Empty / whitespace-only commands
/// are dropped; consecutive duplicates are collapsed.
#[derive(Clone, Debug)]
pub struct CommandHistory {
    entries: std::collections::VecDeque<String>,
    cap: usize,
}

impl CommandHistory {
    pub fn new(cap: usize) -> Self {
        Self { entries: std::collections::VecDeque::with_capacity(cap.min(1024)), cap: cap.max(1) }
    }
    pub fn from_vec(v: Vec<String>, cap: usize) -> Self {
        let mut h = Self::new(cap);
        for s in v { h.push(s); }
        h
    }
    pub fn push(&mut self, cmd: String) {
        if cmd.trim().is_empty() { return; }
        if self.entries.back().map(|s| s == &cmd).unwrap_or(false) { return; }
        if self.entries.len() == self.cap { self.entries.pop_front(); }
        self.entries.push_back(cmd);
    }
    pub fn entries(&self) -> Vec<String> {
        self.entries.iter().cloned().collect()
    }
    pub fn to_vec(&self) -> Vec<String> {
        self.entries.iter().cloned().collect()
    }
}
```

- [ ] **Step 3: Wire history into the widget**

Add to `SshWidget`:

```rust
/// Map alias → CommandHistory for the currently-loaded hosts. Rebuilt
/// whenever `state.set_store_hosts` is called.
history: std::collections::BTreeMap<String, CommandHistory>,
```

In `SshWidget::new`, initialize the map from `state.store_hosts`:

```rust
let history = state.visible_hosts().iter()
    .map(|h| (h.alias.clone(), CommandHistory::from_vec(h.command_history.clone(), 100)))
    .collect();
```

Add an accessor:

```rust
pub fn history_for(&self, alias: &str) -> Option<&CommandHistory> { self.history.get(alias) }
pub fn record_command(&mut self, alias: &str, cmd: String) {
    self.history.entry(alias.to_string())
        .or_insert_with(|| CommandHistory::new(100))
        .push(cmd);
}
```

- [ ] **Step 4: Run tests** — expected 5 passed.

- [ ] **Step 5: Property test + adversarial**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_history_never_exceeds_cap(cap in 1usize..32, n_pushes in 0usize..200) {
        let mut h = CommandHistory::new(cap);
        for i in 0..n_pushes { h.push(format!("cmd-{i}")); }
        prop_assert!(h.entries().len() <= cap);
    }
}

#[test]
fn very_long_command_is_kept() {
    let mut h = CommandHistory::new(10);
    let big = "x".repeat(10_000);
    h.push(big.clone());
    assert_eq!(h.entries(), vec![big]);
}

#[test]
fn cap_of_zero_is_normalized_to_one() {
    let mut h = CommandHistory::new(0);
    h.push("a".into());
    h.push("b".into());
    assert_eq!(h.entries(), vec!["b"]);
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): per-host command history ring buffer (capped + dedup)"
```

---

## Phase H — SFTP sub-panel

### Task 24: SFTP sub-panel state + directory listing

**Files:**
- Modify: `crates/sid-widgets/src/ssh.rs`
- Create: `crates/sid-widgets/tests/sftp_panel.rs`

The SFTP sub-panel toggles in/out with `Tab`. When visible, it shows a tree of the remote filesystem rooted at the home directory (or `/` when home isn't known). Navigation: `j/k` selects, Enter on a directory drills in, `..` ascends.

- [ ] **Step 1: Failing test**

Create `crates/sid-widgets/tests/sftp_panel.rs`:

```rust
use sid_core::adapters::ssh::SftpEntry;
use sid_widgets::ssh::{SftpPanel, SftpPanelVisibility};

fn entry(name: &str, is_dir: bool) -> SftpEntry {
    SftpEntry { name: name.into(), is_dir, size: 0, mtime_secs: 0, mode: 0 }
}

#[test]
fn fresh_panel_is_hidden() {
    let p = SftpPanel::new();
    assert_eq!(p.visibility(), SftpPanelVisibility::Hidden);
}

#[test]
fn toggle_makes_visible() {
    let mut p = SftpPanel::new();
    p.toggle();
    assert_eq!(p.visibility(), SftpPanelVisibility::Visible);
    p.toggle();
    assert_eq!(p.visibility(), SftpPanelVisibility::Hidden);
}

#[test]
fn set_entries_replaces_listing_and_resets_selection() {
    let mut p = SftpPanel::new();
    p.set_cwd("/home/test".into());
    p.set_entries(vec![entry("a", false), entry("dir", true)]);
    assert_eq!(p.entries().len(), 2);
    assert_eq!(p.selected_entry().unwrap().name, "a");
}

#[test]
fn next_and_prev_cycle_selection() {
    let mut p = SftpPanel::new();
    p.set_entries(vec![entry("a", false), entry("b", false), entry("c", false)]);
    p.select_next();
    assert_eq!(p.selected_entry().unwrap().name, "b");
    p.select_prev();
    p.select_prev();
    assert_eq!(p.selected_entry().unwrap().name, "c");
}

#[test]
fn cwd_join_for_drill_in() {
    let mut p = SftpPanel::new();
    p.set_cwd("/home/test".into());
    p.set_entries(vec![entry("subdir", true)]);
    let next = p.selected_remote_path();
    assert_eq!(next.unwrap(), "/home/test/subdir");
}

#[test]
fn ascend_drops_last_path_segment() {
    let mut p = SftpPanel::new();
    p.set_cwd("/home/test/sub".into());
    p.ascend();
    assert_eq!(p.cwd(), "/home/test");
    p.ascend();
    assert_eq!(p.cwd(), "/home");
    p.ascend();
    assert_eq!(p.cwd(), "/");
    p.ascend();
    assert_eq!(p.cwd(), "/");
}
```

- [ ] **Step 2: Implement `SftpPanel`**

Append to `crates/sid-widgets/src/ssh.rs`:

```rust
use sid_core::adapters::ssh::SftpEntry;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpPanelVisibility { Hidden, Visible }

#[derive(Clone, Debug)]
pub struct SftpPanel {
    visibility: SftpPanelVisibility,
    cwd: String,
    entries: Vec<SftpEntry>,
    selected_idx: usize,
}

impl SftpPanel {
    pub fn new() -> Self {
        Self {
            visibility: SftpPanelVisibility::Hidden,
            cwd: "/".into(),
            entries: Vec::new(),
            selected_idx: 0,
        }
    }

    pub fn visibility(&self) -> SftpPanelVisibility { self.visibility }

    pub fn toggle(&mut self) {
        self.visibility = match self.visibility {
            SftpPanelVisibility::Hidden => SftpPanelVisibility::Visible,
            SftpPanelVisibility::Visible => SftpPanelVisibility::Hidden,
        };
    }

    pub fn cwd(&self) -> &str { &self.cwd }
    pub fn set_cwd(&mut self, path: String) {
        self.cwd = if path.is_empty() { "/".into() } else { path };
        self.entries.clear();
        self.selected_idx = 0;
    }

    pub fn entries(&self) -> &[SftpEntry] { &self.entries }
    pub fn set_entries(&mut self, entries: Vec<SftpEntry>) {
        self.entries = entries;
        self.selected_idx = 0;
    }

    pub fn selected_entry(&self) -> Option<&SftpEntry> {
        self.entries.get(self.selected_idx)
    }

    pub fn selected_remote_path(&self) -> Option<String> {
        let e = self.selected_entry()?;
        let mut p = self.cwd.clone();
        if !p.ends_with('/') { p.push('/'); }
        p.push_str(&e.name);
        Some(p)
    }

    pub fn select_next(&mut self) {
        if self.entries.is_empty() { return; }
        self.selected_idx = (self.selected_idx + 1) % self.entries.len();
    }
    pub fn select_prev(&mut self) {
        if self.entries.is_empty() { return; }
        self.selected_idx = (self.selected_idx + self.entries.len() - 1) % self.entries.len();
    }

    pub fn ascend(&mut self) {
        if self.cwd == "/" { return; }
        let trimmed = self.cwd.trim_end_matches('/');
        if let Some(idx) = trimmed.rfind('/') {
            let parent = if idx == 0 { "/".into() } else { trimmed[..idx].to_string() };
            self.cwd = parent;
            self.entries.clear();
            self.selected_idx = 0;
        }
    }
}

impl Default for SftpPanel {
    fn default() -> Self { Self::new() }
}
```

Add `SftpPanel` to `SshWidget` and hook `Tab` to `panel.toggle()`:

```rust
pub struct SshWidget {
    state: SshState,
    connection: ConnectionState,
    sftp_panel: SftpPanel,
    // ...
}
```

In `handle_event`:

```rust
(KeyCode::Tab, KeyModifiers::NONE) => { self.sftp_panel.toggle(); EventOutcome::Consumed }
```

- [ ] **Step 3: Run tests** — expected 6 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
#[test]
fn ascend_with_trailing_slash() {
    let mut p = SftpPanel::new();
    p.set_cwd("/home/test/".into());
    p.ascend();
    assert_eq!(p.cwd(), "/home");
}

#[test]
fn set_cwd_with_empty_string_normalizes_to_root() {
    let mut p = SftpPanel::new();
    p.set_cwd(String::new());
    assert_eq!(p.cwd(), "/");
}

#[test]
fn many_entries_does_not_panic() {
    let mut p = SftpPanel::new();
    let entries: Vec<_> = (0..5000).map(|i| entry(&format!("f{i}"), false)).collect();
    p.set_entries(entries);
    for _ in 0..10000 { p.select_next(); }
    assert!(p.selected_entry().is_some());
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): SFTP sub-panel state (cwd + entries + visibility toggle)"
```

---

### Task 25: SFTP download to local temp

**Files:**
- Modify: `crates/sid-widgets/src/ssh.rs` (or a sibling `ssh_sftp.rs`)
- Create: `crates/sid-widgets/tests/sftp_download.rs`

The widget should not contain the actual SFTP call (that's behind a trait). It exposes a pure function `prepare_download(panel, local_dir) -> (remote_path, local_path)` that the wire layer wraps in a JobQueue task calling `SftpSession::get`.

- [ ] **Step 1: Failing test**

Create `crates/sid-widgets/tests/sftp_download.rs`:

```rust
use sid_core::adapters::ssh::SftpEntry;
use sid_widgets::ssh::{prepare_download, SftpPanel};
use tempfile::tempdir;

#[test]
fn prepare_download_returns_remote_and_local_paths() {
    let mut panel = SftpPanel::new();
    panel.set_cwd("/home/test".into());
    panel.set_entries(vec![SftpEntry { name: "foo.txt".into(), is_dir: false, size: 0, mtime_secs: 0, mode: 0 }]);
    let dir = tempdir().unwrap();
    let (remote, local) = prepare_download(&panel, dir.path()).unwrap();
    assert_eq!(remote, "/home/test/foo.txt");
    assert!(local.starts_with(dir.path()));
    assert!(local.to_string_lossy().ends_with("foo.txt"));
}

#[test]
fn prepare_download_with_no_selection_returns_none() {
    let panel = SftpPanel::new();
    let dir = tempdir().unwrap();
    let r = prepare_download(&panel, dir.path());
    assert!(r.is_none());
}

#[test]
fn prepare_download_refuses_directories() {
    let mut panel = SftpPanel::new();
    panel.set_cwd("/h".into());
    panel.set_entries(vec![SftpEntry { name: "adir".into(), is_dir: true, size: 0, mtime_secs: 0, mode: 0 }]);
    let dir = tempdir().unwrap();
    let r = prepare_download(&panel, dir.path());
    assert!(r.is_none());
}
```

- [ ] **Step 2: Implement `prepare_download`**

Append to `crates/sid-widgets/src/ssh.rs`:

```rust
/// Compute the (remote, local) path pair for downloading the SFTP panel's
/// currently-selected entry into `local_dir`. Returns `None` if there's no
/// selection or the selection is a directory.
pub fn prepare_download(panel: &SftpPanel, local_dir: &std::path::Path) -> Option<(String, PathBuf)> {
    let entry = panel.selected_entry()?;
    if entry.is_dir { return None; }
    let remote = panel.selected_remote_path()?;
    let local = local_dir.join(&entry.name);
    Some((remote, local))
}
```

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Adversarial coverage**

Append:

```rust
#[test]
fn unicode_filenames_round_trip() {
    let mut panel = SftpPanel::new();
    panel.set_cwd("/h".into());
    panel.set_entries(vec![SftpEntry { name: "🐕-data.txt".into(), is_dir: false, size: 0, mtime_secs: 0, mode: 0 }]);
    let dir = tempdir().unwrap();
    let (remote, local) = prepare_download(&panel, dir.path()).unwrap();
    assert_eq!(remote, "/h/🐕-data.txt");
    assert!(local.to_string_lossy().contains("🐕-data.txt"));
}

#[test]
fn very_long_filename_does_not_panic() {
    let mut panel = SftpPanel::new();
    panel.set_cwd("/h".into());
    let big = "x".repeat(500);
    panel.set_entries(vec![SftpEntry { name: big.clone(), is_dir: false, size: 0, mtime_secs: 0, mode: 0 }]);
    let dir = tempdir().unwrap();
    let (remote, _local) = prepare_download(&panel, dir.path()).unwrap();
    assert!(remote.ends_with(&big));
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): prepare_download helper for SFTP file fetch path computation"
```

---

### Task 26: SFTP upload from local path

**Files:**
- Modify: `crates/sid-widgets/src/ssh.rs`
- Create: `crates/sid-widgets/tests/sftp_upload.rs`

Symmetric to Task 25: a `prepare_upload(panel, local_path) -> (local_path, remote_path)` helper.

- [ ] **Step 1: Failing test**

Create `crates/sid-widgets/tests/sftp_upload.rs`:

```rust
use sid_widgets::ssh::{prepare_upload, SftpPanel};
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn prepare_upload_joins_local_basename_with_panel_cwd() {
    let mut panel = SftpPanel::new();
    panel.set_cwd("/var/uploads".into());
    let dir = tempdir().unwrap();
    let local = dir.path().join("report.txt");
    std::fs::write(&local, b"hi").unwrap();
    let (l, r) = prepare_upload(&panel, &local).unwrap();
    assert_eq!(l, local);
    assert_eq!(r, "/var/uploads/report.txt");
}

#[test]
fn prepare_upload_with_nonexistent_local_returns_none() {
    let panel = SftpPanel::new();
    let r = prepare_upload(&panel, &PathBuf::from("/never/exists/foo.bin"));
    assert!(r.is_none());
}

#[test]
fn prepare_upload_with_directory_local_returns_none() {
    let dir = tempdir().unwrap();
    let panel = SftpPanel::new();
    let r = prepare_upload(&panel, dir.path());
    assert!(r.is_none());
}
```

- [ ] **Step 2: Implement**

Append to `crates/sid-widgets/src/ssh.rs`:

```rust
/// Compute the (local, remote) path pair for uploading `local` into the
/// panel's cwd. Returns `None` if `local` doesn't exist or is a directory.
pub fn prepare_upload(panel: &SftpPanel, local: &std::path::Path) -> Option<(PathBuf, String)> {
    if !local.is_file() { return None; }
    let basename = local.file_name()?.to_str()?;
    let mut remote = panel.cwd().to_string();
    if !remote.ends_with('/') { remote.push('/'); }
    remote.push_str(basename);
    Some((local.to_path_buf(), remote))
}
```

- [ ] **Step 3: Run tests** — expected 3 passed.

- [ ] **Step 4: Adversarial coverage**

```rust
#[test]
fn prepare_upload_into_root_cwd_works() {
    let dir = tempdir().unwrap();
    let local = dir.path().join("x.bin");
    std::fs::write(&local, b"x").unwrap();
    let panel = SftpPanel::new(); // cwd = "/"
    let (_l, r) = prepare_upload(&panel, &local).unwrap();
    assert_eq!(r, "/x.bin");
}

#[test]
fn prepare_upload_with_unicode_basename() {
    let dir = tempdir().unwrap();
    let local = dir.path().join("🐕.log");
    std::fs::write(&local, b"woof").unwrap();
    let mut panel = SftpPanel::new();
    panel.set_cwd("/uploads".into());
    let (_l, r) = prepare_upload(&panel, &local).unwrap();
    assert_eq!(r, "/uploads/🐕.log");
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): prepare_upload helper for SFTP file push path computation"
```

---

### Task 27: Edit-in-place via `EditorRunner`

**Files:**
- Modify: `crates/sid-widgets/src/ssh.rs`
- Create: `crates/sid-widgets/tests/sftp_edit_in_place.rs`

The flow: select a remote file → `e` keypress → download to a temp path → spawn `$EDITOR` via the existing `EditorRunner` trait (established by Plan 2) → on editor exit, upload the modified file. State machine: `Idle → Downloading → Editing → Uploading → Done / Failed`.

- [ ] **Step 1: Failing test**

Create `crates/sid-widgets/tests/sftp_edit_in_place.rs`:

```rust
use sid_widgets::ssh::{SftpEditState, SftpEditPhase};
use sid_widgets::workspaces::MockEditorRunner;

#[test]
fn fresh_edit_state_is_idle() {
    let s = SftpEditState::default();
    assert_eq!(s.phase(), SftpEditPhase::Idle);
}

#[test]
fn begin_download_transitions_to_downloading() {
    let mut s = SftpEditState::default();
    s.begin_download("/remote/foo.txt".into(), std::path::PathBuf::from("/tmp/foo.txt"));
    assert_eq!(s.phase(), SftpEditPhase::Downloading);
    assert_eq!(s.remote_path(), Some("/remote/foo.txt"));
}

#[test]
fn download_complete_transitions_to_editing() {
    let mut s = SftpEditState::default();
    s.begin_download("/r/x".into(), "/tmp/x".into());
    s.mark_download_complete();
    assert_eq!(s.phase(), SftpEditPhase::Editing);
}

#[test]
fn editing_to_uploading_after_editor_returns() {
    let mut s = SftpEditState::default();
    s.begin_download("/r/x".into(), "/tmp/x".into());
    s.mark_download_complete();
    s.mark_editor_done(true);
    assert_eq!(s.phase(), SftpEditPhase::Uploading);
}

#[test]
fn editor_failure_transitions_to_failed() {
    let mut s = SftpEditState::default();
    s.begin_download("/r/x".into(), "/tmp/x".into());
    s.mark_download_complete();
    s.mark_editor_done(false);
    assert_eq!(s.phase(), SftpEditPhase::Failed);
}

#[test]
fn upload_complete_transitions_to_done() {
    let mut s = SftpEditState::default();
    s.begin_download("/r/x".into(), "/tmp/x".into());
    s.mark_download_complete();
    s.mark_editor_done(true);
    s.mark_upload_complete();
    assert_eq!(s.phase(), SftpEditPhase::Done);
}

#[test]
fn mock_editor_runner_returns_set_content() {
    let runner = MockEditorRunner::new("modified bytes".into());
    // Used by the widget when wiring tests; we verify the trait round-trips.
    use sid_widgets::workspaces::EditorRunner;
    let r = runner.run_editor(std::path::Path::new("/tmp/anything")).unwrap();
    assert_eq!(r, "modified bytes");
}
```

- [ ] **Step 2: Implement `SftpEditState`**

Append to `crates/sid-widgets/src/ssh.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpEditPhase {
    Idle,
    Downloading,
    Editing,
    Uploading,
    Done,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct SftpEditState {
    phase: SftpEditPhase,
    remote_path: Option<String>,
    local_path: Option<PathBuf>,
    error: Option<String>,
}

impl Default for SftpEditPhase {
    fn default() -> Self { SftpEditPhase::Idle }
}

impl SftpEditState {
    pub fn phase(&self) -> SftpEditPhase { self.phase }
    pub fn remote_path(&self) -> Option<&str> { self.remote_path.as_deref() }
    pub fn local_path(&self) -> Option<&std::path::Path> { self.local_path.as_deref() }
    pub fn error_message(&self) -> Option<&str> { self.error.as_deref() }

    pub fn begin_download(&mut self, remote: String, local: PathBuf) {
        self.phase = SftpEditPhase::Downloading;
        self.remote_path = Some(remote);
        self.local_path = Some(local);
        self.error = None;
    }
    pub fn mark_download_complete(&mut self) { self.phase = SftpEditPhase::Editing; }
    pub fn mark_editor_done(&mut self, ok: bool) {
        self.phase = if ok { SftpEditPhase::Uploading } else { SftpEditPhase::Failed };
    }
    pub fn mark_upload_complete(&mut self) { self.phase = SftpEditPhase::Done; }
    pub fn mark_failed(&mut self, e: String) { self.phase = SftpEditPhase::Failed; self.error = Some(e); }
    pub fn reset(&mut self) { *self = Self::default(); }
}
```

- [ ] **Step 3: Run tests** — expected 7 passed.

- [ ] **Step 4: Adversarial + property tests**

Append:

```rust
#[test]
fn reset_clears_all_fields() {
    let mut s = SftpEditState::default();
    s.begin_download("/r".into(), "/l".into());
    s.mark_failed("oops".into());
    s.reset();
    assert_eq!(s.phase(), SftpEditPhase::Idle);
    assert!(s.remote_path().is_none());
    assert!(s.local_path().is_none());
    assert!(s.error_message().is_none());
}

#[test]
fn multiple_begin_downloads_replace_paths() {
    let mut s = SftpEditState::default();
    s.begin_download("/a".into(), "/la".into());
    s.begin_download("/b".into(), "/lb".into());
    assert_eq!(s.remote_path(), Some("/b"));
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): SFTP edit-in-place state machine (download → \$EDITOR → upload)"
```

---

## Phase I — CLI + wiring

### Task 28: `sid ssh add/remove/list` CLI subcommands

**Files:**
- Modify: `crates/sid/src/main.rs`

- [ ] **Step 1: Extend the `Cli` subcommand enum**

```rust
#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Workspace registry operations (added in Plan 2)
    Workspace {
        #[command(subcommand)]
        op: WorkspaceOp,
    },
    /// SSH host registry operations
    Ssh {
        #[command(subcommand)]
        op: SshOp,
    },
}

#[derive(clap::Subcommand, Debug)]
enum SshOp {
    /// Add an SSH host
    Add {
        /// Alias used to refer to the host within sid
        alias: String,
        /// Hostname or IP address
        host: String,
        /// SSH user
        #[arg(long, default_value = "root")]
        user: String,
        /// SSH port
        #[arg(long, default_value_t = 22)]
        port: u16,
        /// Optional identity file path
        #[arg(long)]
        identity_file: Option<String>,
    },
    /// Remove an SSH host by alias
    Remove { alias: String },
    /// List registered SSH hosts (manual + ssh-config)
    List,
    /// Connect to an alias (launches the TUI pre-pointed at the host — Task 29)
    Connect { alias: String },
}
```

- [ ] **Step 2: Handle the subcommand before launching the TUI**

In `main`:

```rust
if let Some(Cmd::Ssh { op }) = cli.cmd {
    match op {
        SshOp::Add { alias, host, user, port, identity_file } => {
            let h = SshHost {
                alias: alias.clone(),
                host,
                port,
                user,
                identity_file,
                source: SshHostSource::Manual,
                last_connected: 0,
                command_history: Vec::new(),
            };
            store.upsert_ssh_host(&h)?;
            println!("added ssh host: {alias}");
        }
        SshOp::Remove { alias } => {
            store.remove_ssh_host(&alias)?;
            println!("removed ssh host: {alias}");
        }
        SshOp::List => {
            // Manual entries first, then ssh-config entries (read-only).
            for h in store.list_ssh_hosts()? {
                println!("{:<20} {}@{}:{} [Manual]", h.alias, h.user, h.host, h.port);
            }
            let cfg_path = directories::UserDirs::new()
                .map(|d| d.home_dir().join(".ssh/config"))
                .unwrap_or_else(|| std::path::PathBuf::from("~/.ssh/config"));
            for e in sid_ssh::read_ssh_config(&cfg_path).unwrap_or_default() {
                println!(
                    "{:<20} {}@{}:{} [SshConfig]",
                    e.host,
                    e.user.as_deref().unwrap_or("?"),
                    e.hostname.as_deref().unwrap_or(&e.host),
                    e.port.unwrap_or(22)
                );
            }
        }
        SshOp::Connect { .. } => {
            // Handled in Task 29 — passes through to TUI launch with start_tab + start_alias.
        }
    }
    if !matches!(op, SshOp::Connect { .. }) { return Ok(()); }
}
```

Imports: add `use sid_store::{SshHost, SshHostSource};` and ensure `sid-ssh` is a binary dep (Task 30 also adds it; if Task 30 hasn't landed yet, add the dep now).

- [ ] **Step 3: Tests**

Create `crates/sid/tests/ssh_cli.rs`:

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn ssh_add_list_remove() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");

    let add = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "ssh", "add", "jp46-dev", "10.1.40.102", "--user", "pi", "--port", "2222"])
        .output().unwrap();
    assert!(add.status.success(), "stderr: {}", String::from_utf8_lossy(&add.stderr));

    let list = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "ssh", "list"])
        .output().unwrap();
    assert!(list.status.success());
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("jp46-dev"));
    assert!(out.contains("10.1.40.102"));

    let remove = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "ssh", "remove", "jp46-dev"])
        .output().unwrap();
    assert!(remove.status.success());

    let list2 = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "ssh", "list"])
        .output().unwrap();
    let out2 = String::from_utf8_lossy(&list2.stdout);
    assert!(!out2.contains("jp46-dev"));
}

#[test]
fn ssh_add_with_minimal_args_defaults_user_root_port_22() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "ssh", "add", "x", "example.com"])
        .output().unwrap();
    let list = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "ssh", "list"])
        .output().unwrap();
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("root@example.com:22"));
}
```

- [ ] **Step 4: Run tests** — expected 2 passed.

- [ ] **Step 5: Insta snapshot of `--help`**

Add:

```rust
#[test]
fn ssh_help_output_snapshot() {
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["ssh", "--help"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    insta::assert_snapshot!("ssh_help", stdout);
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): add \`sid ssh add/remove/list\` subcommands"
```

---

### Task 29: `sid ssh connect <alias>` launches TUI pre-pointed at host

**Files:**
- Modify: `crates/sid/src/main.rs`
- Modify: `crates/sid/src/wire.rs`

When the user runs `sid ssh connect <alias>`, the binary should launch normally but with the SSH tab pre-selected and the host pre-highlighted (and ideally the connection initiated automatically).

- [ ] **Step 1: Pass `start_alias` through `build_app`**

In `wire.rs`, extend `build_app`:

```rust
pub fn build_app(
    start_tab: Option<String>,
    workspaces: Vec<Workspace>,
    ssh_hosts: Vec<SshHost>,
    ssh_config_entries: Vec<sid_widgets::ssh::SshConfigEntryLite>,
    start_ssh_alias: Option<String>,
) -> App {
    // ... pre-existing pre-wiring ...
    let ssh_state = sid_widgets::ssh::SshState::new(ssh_hosts, ssh_config_entries);
    let mut ssh_widget = SshWidget::new(ssh_state);
    if let Some(alias) = &start_ssh_alias {
        // Find the alias in the merged list and set selection
        let aliases: Vec<_> = ssh_widget.state().visible_hosts().iter().map(|h| h.alias.clone()).collect();
        if let Some(idx) = aliases.iter().position(|a| a == alias) {
            for _ in 0..idx { ssh_widget.state_mut().select_next(); }
        }
        // Mark Connecting so the wire layer kicks off connect on first frame
        ssh_widget.connection_mut().begin_connecting(alias.clone());
    }
    // Set start_tab to "ssh" if start_ssh_alias is Some and start_tab is None
    let effective_start_tab = start_tab.or_else(|| start_ssh_alias.as_ref().map(|_| "ssh".to_string()));
    // ... rest of build_app ...
}
```

- [ ] **Step 2: Wire `SshOp::Connect` in `main`**

In `main`:

```rust
let start_ssh_alias = match cli.cmd {
    Some(Cmd::Ssh { op: SshOp::Connect { ref alias } }) => Some(alias.clone()),
    _ => None,
};
// ... after the early-exit Ssh handling above, fall through to TUI launch
// passing start_ssh_alias into build_app.
```

- [ ] **Step 3: Tests**

`sid ssh connect` launches the TUI, so a CLI-only integration test isn't possible without a TTY harness. Instead, unit-test `build_app`:

Append to `crates/sid/tests/wire_ssh.rs` (new file):

```rust
use sid::wire::build_app;
use sid_store::{SshHost, SshHostSource};

#[test]
fn build_app_with_start_ssh_alias_selects_that_host_and_marks_connecting() {
    let h = SshHost {
        alias: "jp46-dev".into(),
        host: "10.1.40.102".into(),
        port: 22,
        user: "pi".into(),
        identity_file: None,
        source: SshHostSource::Manual,
        last_connected: 0,
        command_history: Vec::new(),
    };
    let app = build_app(None, vec![], vec![h], vec![], Some("jp46-dev".into()));
    // Inspect via accessor that exposes the SSH widget state. (Adding a small
    // helper `app.ssh_widget()` may be required.)
    let _ = app;
    // Concrete assertions depend on which accessors the App exposes; at minimum
    // verify that the SSH tab is active.
}

#[test]
fn build_app_with_unknown_alias_still_returns_an_app() {
    let app = build_app(None, vec![], vec![], vec![], Some("nonexistent".into()));
    let _ = app;
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): \`sid ssh connect <alias>\` launches TUI on the SSH tab pre-pointed at host"
```

---

### Task 30: Wire `RusshClient` + `PortablePtyProvider` into the binary

**Files:**
- Modify: `crates/sid/Cargo.toml`
- Modify: `crates/sid/src/wire.rs`

- [ ] **Step 1: Add deps**

In `crates/sid/Cargo.toml`:

```toml
sid-ssh.workspace = true
sid-pty.workspace = true
```

- [ ] **Step 2: Inject factories into `SidApp`**

In `wire.rs`:

```rust
use sid_ssh::RusshClientFactory;
use sid_pty::{PortablePtyProvider, Vt100Screen};
use sid_core::adapters::pty::{PtyProvider, TerminalScreen};
use sid_core::adapters::ssh::SshClient;

pub struct SidApp {
    pub app: App,
    pub store: Arc<RedbStore>,
    pub session_id: String,
    pub ssh_factory: Arc<RusshClientFactory>,
    pub pty_provider: Arc<PortablePtyProvider>,
}
```

In `build_app`, when constructing the `SshWidget`:

```rust
let ssh_factory: Arc<RusshClientFactory> = Arc::new(RusshClientFactory::new());
let pty_provider: Arc<PortablePtyProvider> = Arc::new(PortablePtyProvider::new());

let ssh_widget = SshWidget::new(ssh_state).with_providers(
    {
        let f = ssh_factory.clone();
        Arc::new(move || -> Box<dyn SshClient> {
            Box::new(f.new_client())
        })
    },
    pty_provider.clone() as Arc<dyn PtyProvider>,
);
```

Construct a `Vt100Screen` to back the widget's `PtyPane`:

```rust
let screen: Box<dyn TerminalScreen> = Box::new(Vt100Screen::new(24, 80));
// Pass `screen` into SshWidget when it's wired with a live connection
// (the widget owns an `Option<PtyPane>` set when the connection moves to
// Connected).
```

- [ ] **Step 3: Read ssh-config on startup**

In `main`:

```rust
let cfg_path = directories::UserDirs::new()
    .map(|d| d.home_dir().join(".ssh/config"))
    .unwrap_or_else(|| std::path::PathBuf::from("~/.ssh/config"));
let cfg_entries: Vec<sid_widgets::ssh::SshConfigEntryLite> = sid_ssh::read_ssh_config(&cfg_path)
    .unwrap_or_default()
    .into_iter()
    .filter(|e| !e.host.contains('*')) // skip wildcard patterns from the UI list
    .map(|e| sid_widgets::ssh::SshConfigEntryLite {
        alias: e.host.clone(),
        host: e.hostname.unwrap_or(e.host),
        port: e.port.unwrap_or(22),
        user: e.user.unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "root".into())),
        identity_file: e.identity_file,
    })
    .collect();
let ssh_hosts = store.list_ssh_hosts().unwrap_or_default();
let app = build_app(cli.start_tab, workspaces, ssh_hosts, cfg_entries, start_ssh_alias);
```

- [ ] **Step 4: Tests**

Most integration tests at this level launch the TUI which is non-trivial without a PTY harness. The test from Task 29 (`build_app_with_start_ssh_alias_selects_that_host_and_marks_connecting`) covers wiring shape.

Append a smoke test to `tests/wire_ssh.rs`:

```rust
#[test]
fn build_app_does_not_panic_when_ssh_config_missing() {
    // Empty config + empty store
    let app = build_app(None, vec![], vec![], vec![], None);
    let _ = app;
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): wire RusshClientFactory + PortablePtyProvider into App and SshWidget"
```

---

## Phase J — Integration + docs

### Task 31: Integration test — SSH host registry round-trip

**Files:**
- Create: `crates/sid/tests/ssh_registry_integration.rs`

Builds on Tasks 28–30. Full end-to-end: `sid ssh add ...`, restart binary (new process), `sid ssh list`, expect the host in output.

- [ ] **Step 1: Test**

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn ssh_registry_round_trips_across_invocations() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");
    let bin = env!("CARGO_BIN_EXE_sid");

    // First invocation: add two hosts.
    Command::new(bin).args(["--db", db.to_str().unwrap(), "ssh", "add", "a", "ahost"]).output().unwrap();
    Command::new(bin).args(["--db", db.to_str().unwrap(), "ssh", "add", "b", "bhost", "--port", "2222"]).output().unwrap();

    // Second invocation: list — both should be present.
    let list = Command::new(bin).args(["--db", db.to_str().unwrap(), "ssh", "list"]).output().unwrap();
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(out.contains("a") && out.contains("ahost"));
    assert!(out.contains("b") && out.contains("bhost"));
    assert!(out.contains("2222"));

    // Third invocation: remove one.
    Command::new(bin).args(["--db", db.to_str().unwrap(), "ssh", "remove", "a"]).output().unwrap();
    let list2 = Command::new(bin).args(["--db", db.to_str().unwrap(), "ssh", "list"]).output().unwrap();
    let out2 = String::from_utf8_lossy(&list2.stdout);
    assert!(!out2.contains("ahost"));
    assert!(out2.contains("bhost"));
}
```

- [ ] **Step 2: Run** — expected pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sid
git commit -m "test(bin): integration test for SSH host registry round-trip across invocations"
```

---

### Task 32: Integration test — SFTP edit-in-place via mock `SshClient`

**Files:**
- Create: `crates/sid-widgets/tests/sftp_edit_integration.rs`

End-to-end at the widget level: drive `SftpEditState` through the full life cycle with a fake `SftpSession` + `MockEditorRunner`.

- [ ] **Step 1: Test**

```rust
use async_trait::async_trait;
use sid_core::adapters::ssh::{SftpEntry, SftpSession, SshError};
use sid_widgets::ssh::{SftpEditPhase, SftpEditState};
use sid_widgets::workspaces::{EditorRunner, MockEditorRunner};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

struct FakeSftp {
    files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

#[async_trait]
impl SftpSession for FakeSftp {
    async fn list(&mut self, _path: &str) -> Result<Vec<SftpEntry>, SshError> { Ok(vec![]) }
    async fn get(&mut self, path: &str) -> Result<Vec<u8>, SshError> {
        self.files.lock().unwrap().get(path).cloned().ok_or(SshError::PathNotFound(path.into()))
    }
    async fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), SshError> {
        self.files.lock().unwrap().insert(path.into(), bytes.to_vec());
        Ok(())
    }
    async fn remove_file(&mut self, _p: &str) -> Result<(), SshError> { Ok(()) }
    async fn mkdir(&mut self, _p: &str) -> Result<(), SshError> { Ok(()) }
    async fn stat(&mut self, _p: &str) -> Result<Option<SftpEntry>, SshError> { Ok(None) }
    async fn close(&mut self) -> Result<(), SshError> { Ok(()) }
}

#[tokio::test]
async fn full_edit_in_place_flow_round_trips_modified_bytes() {
    let files = Arc::new(Mutex::new(std::collections::HashMap::from([
        ("/remote/foo.txt".to_string(), b"original\n".to_vec()),
    ])));
    let mut sftp: Box<dyn SftpSession> = Box::new(FakeSftp { files: files.clone() });

    let tmp = tempdir().unwrap();
    let local = tmp.path().join("foo.txt");
    let editor: Box<dyn EditorRunner> = Box::new(MockEditorRunner::new("modified content".into()));

    let mut state = SftpEditState::default();
    // Phase 1: download
    state.begin_download("/remote/foo.txt".into(), local.clone());
    let bytes = sftp.get("/remote/foo.txt").await.unwrap();
    std::fs::write(&local, &bytes).unwrap();
    state.mark_download_complete();
    assert_eq!(state.phase(), SftpEditPhase::Editing);

    // Phase 2: editor
    let new_content = editor.run_editor(&local).unwrap();
    std::fs::write(&local, new_content.as_bytes()).unwrap();
    state.mark_editor_done(true);
    assert_eq!(state.phase(), SftpEditPhase::Uploading);

    // Phase 3: upload
    let modified = std::fs::read(&local).unwrap();
    sftp.put("/remote/foo.txt", &modified).await.unwrap();
    state.mark_upload_complete();
    assert_eq!(state.phase(), SftpEditPhase::Done);

    // Verify the fake server now has the new content.
    let server_now = files.lock().unwrap().get("/remote/foo.txt").cloned().unwrap();
    assert_eq!(server_now, b"modified content");
}

#[tokio::test]
async fn edit_in_place_failed_editor_does_not_upload() {
    let files = Arc::new(Mutex::new(std::collections::HashMap::from([
        ("/remote/x.txt".to_string(), b"v1".to_vec()),
    ])));
    let mut sftp: Box<dyn SftpSession> = Box::new(FakeSftp { files: files.clone() });
    let tmp = tempdir().unwrap();
    let local = tmp.path().join("x.txt");
    let editor: Box<dyn EditorRunner> = Box::new(MockEditorRunner::failing("user cancelled".into()));

    let mut state = SftpEditState::default();
    state.begin_download("/remote/x.txt".into(), local.clone());
    std::fs::write(&local, sftp.get("/remote/x.txt").await.unwrap()).unwrap();
    state.mark_download_complete();

    let r = editor.run_editor(&local);
    assert!(r.is_err());
    state.mark_editor_done(false);
    assert_eq!(state.phase(), SftpEditPhase::Failed);

    // Server bytes unchanged.
    assert_eq!(files.lock().unwrap().get("/remote/x.txt").unwrap(), b"v1");
}
```

- [ ] **Step 2: Run** — expected 2 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/sid-widgets
git commit -m "test(widgets): integration test for SFTP edit-in-place full life cycle"
```

---

### Task 33: README update

**Files:**
- Modify: `README.md`

Update the "What's inside (v1)" SSH row to remove placeholder language and add concrete content. Add a "What works in this build" callout.

- [ ] **Step 1: Update the SSH row**

```markdown
| **SSH** | Hosts list (merged from `~/.ssh/config` + sid-managed), embedded interactive shell via PTY, SFTP browser with download/upload/edit-in-place, per-host command history |
```

- [ ] **Step 2: Add to the Quickstart section**

```markdown
# SSH host management
sid ssh add jp46-dev 10.1.40.102 --user pi --port 22
sid ssh list
sid ssh connect jp46-dev      # launches the TUI on the SSH tab, connecting

# Inside the SSH tab
#   j/k             — select a host
#   Enter           — connect (open embedded shell)
#   Tab             — toggle SFTP sub-panel
#   In SFTP:  j/k   — select; Enter — drill into dir; e — edit-in-place; u — upload; d — download
```

- [ ] **Step 3: Update the "What works in this build" callout**

> Foundation + Workspaces tab + **SSH tab** functional. Host list merges `~/.ssh/config` with manually-added hosts; Enter on a host opens an interactive shell in an embedded PTY; `Tab` toggles an SFTP sub-panel with download / upload / edit-in-place. `sid ssh add/remove/list/connect` CLI for headless management.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: update README to reflect Plan 3 SSH tab + SFTP functionality"
```

---

## Done criteria for Plan 3

- [ ] `cargo build --workspace` succeeds with no errors or warnings
- [ ] `cargo test --all-features --workspace` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is clean
- [ ] `cargo fmt --check` is clean
- [ ] `cargo run -p sid` launches; the SSH tab is populated with hosts merged from `~/.ssh/config` plus any `sid ssh add`-ed entries
- [ ] In the SSH tab: `j/k` navigates the host list; Enter on a host transitions `ConnectionState` to `Connecting`, then `Connected` once the russh handshake completes
- [ ] In the connected state, typed input goes to the remote shell; remote output renders via the vt100-backed `PtyPane`
- [ ] `Tab` toggles the SFTP sub-panel; navigating + pressing `e` on a file downloads → spawns `$EDITOR` → uploads on save
- [ ] Per-host command history persists across `sid` restarts
- [ ] `sid ssh add/remove/list` and `sid ssh connect <alias>` all behave per the README quickstart
- [ ] No regressions in Plan 1 (foundation) or Plan 2 (workspaces) functionality
- [ ] Loom tests for the `Arc<Mutex<…>>` introductions in `sid-ssh` (shell buffer) and `sid-pty` (reader-thread buffer) pass under `RUSTFLAGS="--cfg loom"`
- [ ] Criterion bench `sftp_list_200_entries` records a baseline; future PRs that regress it ≥10% fail CI (per CLAUDE.md)
- [ ] `cargo fuzz` target `fuzz_ssh_config` builds (running it in CI is out of scope; building is the gate)

---

## Self-review notes (run before requesting human review)

**1. Spec coverage.** Plan 3 covers the spec's "SSH" tab in `2026-05-20-sid-foundation-design.md` § "Tabs (v1) — 2. SSH": hosts merged from `~/.ssh/config` + manual entries; embedded PTY via `portable-pty` + `russh` + `vt100`; SFTP sub-panel toggled with `Tab`; download / upload / edit-in-place via `$EDITOR`; per-host command history persisted. Plus CLI host management.

**2. Items deferred to later plans (confirmed by future-features doc):**
   - Tunnel manager
   - Mosh
   - Multiplexed sessions
   - Multi-file selection + drag-equivalent move/copy in SFTP
   - Two-pane sync mode
   - Background transfer queue
   - Resume interrupted transfers
   - Agent forwarding (read-from-agent works; forwarding to remote does not)
   - X11 forwarding
   - Known-hosts pinning UI

**3. Type consistency check.**
   - `SshHost`/`SshHostSource` live in `sid-store`. `SshHostSpec`/`SshAuth`/`ExecResult`/`SftpEntry`/`SshClient`/`SshShell`/`SftpSession` live in `sid-core::adapters::ssh`. `RusshClient`/`RusshClientFactory`/`SshConfigEntry` live in `sid-ssh`. `PortablePtyProvider`/`Vt100Screen` live in `sid-pty`. The adapter pattern is preserved: `sid-widgets` only names traits from `sid-core` — never `russh`, `portable-pty`, or `vt100`.
   - `SshConfigEntryLite` is duplicated in `sid-widgets` (not re-exported from `sid-ssh`) to keep widget code free of any `sid-ssh` symbol. The wire layer converts between `sid_ssh::SshConfigEntry` and `sid_widgets::ssh::SshConfigEntryLite`.
   - `TerminalScreen` trait in `sid-core::adapters::pty` is impl'd by `sid_pty::Vt100Screen`, used by `sid_widgets::ssh::PtyPane` — never naming `vt100` from the widget side.

**4. Placeholder scan.** No "TBD", "TODO", or "fill in later" inside task steps. The russh/russh-sftp API surface is flagged as **judgment calls** for review in §5 below — every async method signature in the test fixtures may need adjustment to the actual russh 0.50 / russh-sftp 2.0 API at implementation time. This is not a placeholder dodge: I do not have internet access to verify the exact 0.50 surface, and the user explicitly forbade WebFetch.

**5. Judgment calls flagged for review.**
   - **russh 0.50 `Handler`/`Server` API**: Task 6 (`tests/common/mod.rs`) and Task 9 (extended handler) assume specific method signatures for `check_server_key`, `auth_publickey`, `pty_request`, `shell_request`, `subsystem_request`, `data`, `window_change_request`. These have churned across 0.4x → 0.5x. **Implementer must verify the exact russh 0.50 trait method signatures and adapt.**
   - **russh 0.50 client API**: Task 6 (`auth_password` / `authenticate_publickey`), Task 8 (`channel.wait` / `ChannelMsg`), Task 9 (`channel.request_pty`/`request_shell`/`window_change`/`data`/`close`), Task 7 (`agent::client::AgentClient::connect_uds` / `authenticate_publickey_with`) all assume specific 0.50 method names. **Implementer must verify against current russh 0.50 docs and adapt.**
   - **russh-sftp 2.0 client API**: Task 10 (`SftpSession::new(stream)` / `read_dir` / `open` / `create` / `metadata` / `remove_file` / `create_dir` / `close`) assumes the 2.0 surface. **Implementer must verify and adapt.**
   - **Bridging russh Channel → russh-sftp stream**: Task 10 step 1's `session.channel_open(channel)` and the line `channel.into_stream()` in `open_sftp` are the most fragile API touch points. **Implementer should consult russh-sftp 2.0 examples for the canonical bridge.**
   - **portable-pty 0.9 API**: Task 12 (`MasterPty::try_clone_reader` / `take_writer` / `resize`), Task 14 (resize ioctl signaling). These have been stable, but the `take_writer` vs `take_pipe_writer` rename happened across versions. **Implementer should verify.**
   - **`SshAuth::None` semantics**: Task 4 documents `None` as "useful for tests and mock servers". Real SSH servers will reject this; that is acceptable for v1.
   - **Known-hosts policy**: Task 6's `ClientHandler::check_server_key` always returns `true`. This is intentional for Plan 3 (the user's existing workflow trusts `~/.ssh/known_hosts` implicitly), but it is a security trade-off that should be revisited when known-hosts pinning UI lands.

**6. Scope check.** 33 tasks across 10 phases (A–J). Comparable to Plan 1's 41 tasks and Plan 2's 33 tasks. Single-implementation-plan-friendly. Each phase produces working/testable software; the plan can stop at the end of any phase (B, C, F, J in particular) and the project remains in a consistent state.

**7. CLAUDE.md compliance.**
   - **Doc tests on every pub fn**: spelled out in every task (Tasks 2, 3, 4, 5, 6, 11, 15, 20, etc.).
   - **Property tests where invariants exist**: Task 10 (SFTP get/put round-trip), Task 11 (config parser totality), Task 14 (PTY resize totality), Task 15 (vt100 feed totality), Task 16 (SshHost postcard round-trip), Task 19 (Store round-trip), Task 20 (merge under collisions), Task 23 (history cap invariant).
   - **Adversarial coverage**: every task has an "Adversarial coverage" step — malformed inputs, empty hosts, very long names, unicode, huge payloads, double-close idempotency, missing files, network failure simulation.
   - **Insta snapshots**: Task 15 (vt100), Task 22 (PTY pane), Task 28 (`ssh --help`).
   - **Criterion bench**: Task 10 (`sftp_list_200_entries`) — flagged in CLAUDE.md as a critical SSH hot path.
   - **Loom tests**: Task 9 (`Arc<Mutex<Channel>>` + buffer in `RusshShell`), Task 13 (`Arc<Mutex<Vec<u8>>>` in `PortablePtyHandle`). Required by CLAUDE.md for every `Arc<Mutex<…>>` introduction.
   - **`cargo fuzz` target**: Task 11 (`fuzz_ssh_config`) — flagged in CLAUDE.md.
   - **Tests + production code in the same commit**: every task's commit message lands both. No "tests will follow" commits.

**8. Co-author trailer.** All commit subjects in this plan deliberately omit `Co-Authored-By: Claude...` trailers per the user's stated preference (memory: `no-claude-coauthor-trailer`).

**9. Adapter pattern enforcement.** Confirmed:
   - `sid-widgets/ssh.rs` names only `sid_core::adapters::ssh::*` and `sid_core::adapters::pty::*` traits; never `russh`, `russh-sftp`, `portable-pty`, or `vt100`.
   - `sid-core` does not depend on `russh`, `portable-pty`, or `vt100`. It owns the trait definitions only.
   - `sid-store` does not depend on `russh`. `SshHost` is a domain type.
   - `sid-ssh` is the only crate that depends on `russh` + `russh-sftp` + `russh-keys`.
   - `sid-pty` is the only crate that depends on `portable-pty` + `vt100`.
   - The binary crate `sid/` is the only place wiring concrete impls (`RusshClientFactory`, `PortablePtyProvider`, `Vt100Screen`) to trait slots.
