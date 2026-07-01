# SSH Adapter Port (P3.3 groundwork) — Implementation Plan (Plan 3B)

> Execution guide. Runs **in parallel** with Plan 3A (`2026-07-01-p32-editable-hosts.md`)
> in its own worktree; the only shared file is the workspace `Cargo.toml` (members + deps —
> trivial merge). **No UI in this plan**: it lands the adapter crates + tests the terminal
> view (Plan 3C) will consume. For agentic workers: task-by-task with review; steps are
> checkboxes.

**Goal:** The POC's proven SSH stack lands in the rebuild behind core-owned traits —
`sid-core` (trait seam), `sid-ssh` (russh client/shell/SFTP **with host-key verification**,
closing the POC's accept-any-key hole), `sid-term` (vt100 screen widened to **styled
cells** so a GPUI grid can render colors) — fully tested, no GPUI anywhere.

**Architecture:** Three new crates. Traits are lifted near-verbatim from POC
`sid-core/src/adapters/{ssh,pty}.rs`; impls from POC `sid-ssh/src/*` and `sid-pty/src/screen.rs`.
Two deliberate deviations from the POC: (1) `TerminalScreen` returns styled cells, not
bare strings; (2) `check_server_key` verifies against known_hosts with TOFU instead of
`Ok(true)`. Local-PTY (`portable-pty`) is **not** ported — nothing in the SSH slice needs
a local shell (YAGNI; the seam stays open).

**Tech:** Rust 2024 · russh 0.61 (keys in `russh::keys`; **do not** add the retired
`russh-keys` crate) · russh-sftp 2.3 · vt100 0.16 · tokio 1.x (crates + tests only — the
GPUI bridge is Plan 3C's problem) · async-trait 0.1.

## Global Constraints
- **Adapter pattern:** GPUI is never named in these crates; `sid-core` depends on no
  concrete impl; russh/vt100 appear only in `sid-ssh`/`sid-term`. No `sid-store` dependency
  either — the connect flow maps `Host`/`AuthMethod` → `SshHostSpec`/`SshAuth` in Plan 3C.
- **No accept-any-key ships.** The permissive handler may exist mid-plan but the gate
  requires TOFU verification wired and tested.
- Edition-2024 landmine: `-> impl Trait` methods capturing `&self` need `+ use<>`.
- Pragmatic testing: unit tests per crate; live-sshd integration test is `#[ignore]`d and
  run manually. Commits: no Claude trailer; push gate-green to `main` (after worktree merge).

## Salvage references (from `~/vcs/sid-poc`, read-only)
- Traits: `crates/sid-core/src/adapters/ssh.rs` (SshError, SshHostSpec, SshAuth, ExecResult,
  SftpEntry, SshShell, SftpSession, SshClient) · `adapters/pty.rs:132-145` (TerminalScreen —
  the part being widened).
- Impls: `crates/sid-ssh/src/client.rs` (factory + connect/exec/open_shell/open_sftp; the
  Eof-vs-Close exit-status comment at `client.rs:135-141` is load-bearing — keep it),
  `auth.rs` (none/password/key/agent; the russh-0.61 `AgentIdentity` certificate handling
  at `auth.rs:82-91` is load-bearing), `shell.rs` (reader-task + buffer pattern),
  `sftp.rs` (error mapping incl. `PathNotFound`), `crates/sid-pty/src/screen.rs` (vt100 wrap).
- Dep pins + comments: POC root `Cargo.toml:85-99` (russh/russh-sftp/vt100 rationale).

## Tasks

### B1 — `sid-core`: the trait seam (lift + widen)
- **Files:** `crates/sid-core/{Cargo.toml,src/lib.rs,src/ssh.rs,src/term.rs}`; workspace
  `Cargo.toml` members + deps.
- **Lift:** POC `adapters/ssh.rs` into `src/ssh.rs` near-verbatim (types + three traits),
  plus one **new** error variant: `SshError::HostKeyMismatch(String)` ("host key for {host}
  changed — possible MITM; remove the entry from known_hosts to re-trust").
- **New (`src/term.rs`):** the widened screen seam:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
  pub enum TermColor { #[default] Default, Indexed(u8), Rgb(u8, u8, u8) }

  #[derive(Clone, Debug, PartialEq, Eq, Default)]
  pub struct TermCell {
      pub text: String,      // grapheme(s); empty ⇒ blank cell
      pub fg: TermColor,
      pub bg: TermColor,
      pub bold: bool,
      pub italic: bool,
      pub underline: bool,
      pub inverse: bool,
  }

  pub trait TerminalScreen: Send + Sync {
      fn feed(&mut self, bytes: &[u8]);
      fn resize(&mut self, rows: u16, cols: u16);
      fn size(&self) -> (u16, u16);
      fn cursor_position(&self) -> (u16, u16);
      /// Row-major styled snapshot; blank cells are `TermCell::default()`.
      // ponytail: full-grid clone per frame; damage-tracking iterator if profiling demands
      fn cells(&self) -> Vec<Vec<TermCell>>;
      /// Plain-text convenience (tests, logging).
      fn lines(&self) -> Vec<String> {
          self.cells().iter()
              .map(|row| row.iter().map(|c| if c.text.is_empty() { " " } else { &c.text }).collect())
              .collect()
      }
  }
  ```
- **Tests:** doctest-level construction (POC style); `Box<dyn SshClient>` / `Box<dyn
  TerminalScreen>` object-safety compile checks; `lines()` default impl over a hand-built grid.
- **Deliverable:** the seam Plan 3C's terminal view and connect flow compile against.

### B2 — `sid-term`: styled `Vt100Screen`
- **Files:** `crates/sid-term/{Cargo.toml,src/lib.rs,src/screen.rs}`.
- **Lift:** POC `sid-pty/src/screen.rs` (`Parser` wrap, feed/resize/size/cursor_position).
- **New:** `cells()` — map `screen.cell(r, c)` → `TermCell` (contents; `vt100::Color::{Default,Idx,Rgb}`
  → `TermColor`; bold/italic/underline/inverse). Drop the bespoke `lines()` (trait default covers it).
- **Tests:** SGR coverage — `\x1b[31m` (indexed fg) · `\x1b[1;4m` (bold+underline) ·
  `\x1b[38;5;196m` (256-color) · `\x1b[38;2;10;20;30m` (truecolor) · `\x1b[7m` (inverse) ·
  reset; resize preserves size/cursor invariants; plain text round-trips through `lines()`
  identically to the POC behavior (cursor after `abc` at `(0,3)`, POC `screen.rs:89`).
- **Deliverable:** a render-ready styled snapshot the GPUI grid can paint 1:1.

### B3 — `sid-ssh`: russh client/shell/SFTP (lift)
- **Files:** `crates/sid-ssh/{Cargo.toml,src/lib.rs,src/client.rs,src/auth.rs,src/shell.rs,src/sftp.rs}`.
- **Lift:** all four modules near-verbatim (imports move `sid_core::adapters::ssh` →
  `sid_core::ssh`). Keep: the exec Eof/Close handling, the agent-certificate branch, the
  shell reader-task pattern, the SFTP error mapping. `ClientHandler` stays permissive
  **only until B4 lands** — mark it `// B4 replaces this` and keep B3+B4 in one push.
- **Tests:** POC-style doctests (factory, spec defaults); `map_russh_error` /
  `map_sftp_error` table tests. Network paths are covered in B5.
- **Deliverable:** the SSH stack compiles in the rebuild against `sid-core`.

### B4 — known-hosts verification (TOFU) *(critical path — the security fix)*
- **Files:** `crates/sid-ssh/src/known_hosts.rs`; wire into `client.rs` (`ClientHandler`).
- **New:** `RusshClientFactory::new(app_known_hosts: PathBuf)` (caller passes
  `<data_dir>/known_hosts`; no XDG logic in the adapter). `ClientHandler { host, port,
  app_known_hosts }`; `check_server_key`:
  1. **Match** in user's `~/.ssh/known_hosts` (read-only) or the app file → `Ok(true)`.
  2. **Mismatch** in either → `Err` surfaced as `SshError::HostKeyMismatch` (hard fail, no
     prompt path yet).
  3. **Unknown** in both → TOFU: append `[host]:port key-type base64` to the app file
     (create `0600`), log-worthy but `Ok(true)`.
  Prefer russh 0.61's built-in helpers (`russh::keys` known-hosts check/learn — they handle
  hashed `|1|` entries); **verify the exact API against docs at implementation time**; only
  hand-parse (plain entries; skip hashed with a comment) if the helpers are absent.
- **Tests:** temp-file matrix — known+match → ok; known+different-key → `HostKeyMismatch`;
  unknown → appended to app file (mode `0600`) and second connect matches; user-file
  match never writes; port-qualified entries (`[h]:2222`) distinguished from port 22.
- **Deliverable:** the POC's accept-any-key hole is closed at the adapter.

### B5 — Gate (end of Plan 3B)
- `cargo test`, `clippy`, `fmt` across the workspace (exit codes, not `| tail`).
- Live smoke, manual: `#[ignore]`d `#[tokio::test]` against a local sshd —
  agent-auth connect → `exec("echo ok")` exit 0 → `open_shell` → feed output through
  `Vt100Screen` → styled cells non-empty → SFTP `list("/tmp")`. Run once on this machine.
- Adversarial review focused on: B4 (bypass routes — empty files, hashed entries, port
  confusion, TOCTOU on append), shell reader-task lifecycle (no leak after `close`).
  Note-and-defer: password `String`s are not zeroized (POC parity) — file as a P3.3-UI
  concern, don't fix here.
- Merge worktree → `main` (Cargo.toml members/deps is the only expected touch-point with
  Plan 3A).
- **Deliverable:** Plan 3C ("terminal view + connect flow", to be written after P3.2)
  starts from a tested, secure adapter layer.
