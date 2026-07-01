# SSH Terminal View + Connect Flow (Plan 3C) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development
> or superpowers:executing-plans. Steps use `- [ ]` checkboxes.
> **Concurrency:** this is the SSH-track continuation. It is **independent of the Database
> slice** (touches `crates/sid` frontend + the already-merged `sid-ssh`/`sid-term`/`sid-core`
> adapters; no `sid-store` connection work, no dbflux). Runs in parallel with the DB tracks.

**Goal:** Clicking Connect on a host opens a live, styled embedded terminal — SSH connect
(auth from the host's `AuthMethod`, secret from the keyring), a PTY shell, and a GPUI grid
rendering `sid-term`'s styled cells with working keyboard input and resize.

**Architecture:** A `TerminalSession` gpui entity owns `Box<dyn SshClient>` + `Box<dyn SshShell>`
+ a `Vt100Screen` (`Box<dyn TerminalScreen>`); a background task on gpui's executor pumps
`shell.try_read()` → `screen.feed()` → `cx.notify()`. A `TerminalView` element paints the
screen's `cells()` as a monospace grid. Connect/auth mapping and secret resolution are pure,
tested functions; rendering and input are observation-gated (rendering-spike rule).

**Tech Stack:** Rust 2024 · gpui 0.2.2 · sid-ssh (russh 0.61) · sid-term (vt100) · sid-core
(trait seam) · sid-secrets (keyring).

## Global Constraints
- **Adapter pattern:** GPUI only in `crates/sid`. All SSH/terminal access goes through the
  `sid-core` traits (`SshClient`, `SshShell`, `TerminalScreen`) — never name `russh`/`vt100`
  in `crates/sid`.
- **Secrets:** the password/passphrase is fetched from `sid-secrets` at connect time by the
  host's `secret_ref` and passed to `SshAuth` in-memory; it is never logged, never written to
  disk, never held longer than the connect call needs.
- **Blocking rule:** SSH/PTY calls are async/blocking — they run on `cx.background_executor()`
  / a spawned task, never inline in `render` (render stays pure-from-cache, matching the
  existing `AppState` convention).
- **Rust 2024 RPIT:** `-> impl IntoElement` methods borrowing `&self` need `+ use<>`.
- **Pragmatic TDD (per standing rule):** TDD the load-bearing *logic* — the
  `Host`→`SshHostSpec`/`SshAuth` mapping, secret resolution, host-key algorithm ordering —
  with one good check per behavior. Do NOT unit-test rendering/input/grid layout; those are
  observation-gated. Keep the suite fast; no exhaustive permutation tests.

## Adapter API this plan consumes (already on `main`)
- `sid_core::ssh`: `SshHostSpec { host: String, port: u16, user: String }`,
  `SshAuth { None, Password(String), Key { path: PathBuf, passphrase: Option<String> }, Agent }`,
  `trait SshClient { async fn connect(&mut self, &SshHostSpec, &SshAuth); async fn open_shell(&mut self, term, rows, cols) -> Box<dyn SshShell>; async fn disconnect(&mut self); ... }`,
  `trait SshShell { async fn write(&mut self, &[u8]); fn try_read(&mut self) -> Result<Vec<u8>>; async fn resize(&mut self, rows, cols); async fn close(&mut self); }`.
- `sid_ssh::RusshClientFactory::new(app_known_hosts: PathBuf) -> Self`; `.new_client() -> RusshClient`.
- `sid_core::term`: `TermCell { text, fg, bg, bold, italic, underline, inverse }`,
  `TermColor { Default, Indexed(u8), Rgb(u8,u8,u8) }`, `trait TerminalScreen { fn feed(&mut self, &[u8]); fn resize; fn size; fn cursor_position; fn cells() -> Vec<Vec<TermCell>>; }`.
- `sid_term::Vt100Screen::new(rows, cols)`.
- `sid_store` entities: `Host { alias, user, host, port, secret_ref: Option<String>, auth: AuthMethod }`,
  `AuthMethod { Agent, Password, Key { path: String } }`.
- `sid_secrets::SecretStore::get(&SecretId) -> Result<Option<Vec<u8>>>` (already threaded into `AppState.secrets`).

## Tasks

### C1 — Connect mapping: `Host` + secret → `SshHostSpec`/`SshAuth` *(critical path — TDD)*
- **Files:** `crates/sid/src/ssh_connect.rs` (new); tests inline.
- **Produces:** `fn connect_params(host: &Host, secret: Option<Vec<u8>>) -> Result<(SshHostSpec, SshAuth), String>` —
  `AuthMethod::Agent` → `SshAuth::Agent` (secret ignored); `Password` → requires `secret`,
  UTF-8-decode → `SshAuth::Password`; `Key { path }` → `SshAuth::Key { path: path.into(),
  passphrase: secret.map(utf8) }` (passphrase optional). Missing-but-required secret → `Err`.
- **Also:** `fn resolve_secret(secrets: &dyn SecretStore, host: &Host) -> Result<Option<Vec<u8>>, String>`
  (None when `secret_ref` is None; else `get` it, error if the ref is dangling for Password).
- **Tests (TDD, the load-bearing cases only):** agent → no secret needed; password with/without
  secret (missing → err); key with and without passphrase; dangling password ref → err.
- **Deliverable:** the pure, tested bridge from stored host to adapter connect inputs.

### C2 — `TerminalSession` entity (connect + reader pump)
- **Files:** `crates/sid/src/ui/terminal.rs` (new); `crates/sid/src/ui/mod.rs` (export).
- **Consumes:** C1's `connect_params`/`resolve_secret`; `RusshClientFactory`, `Vt100Screen`.
- **Produces:** `TerminalSession` gpui entity holding `screen: Box<dyn TerminalScreen>`,
  `shell: Option<Box<dyn SshShell>>` (Some once connected), `status: SessionStatus
  { Connecting, Connected, Failed(String), Closed }`, `rows/cols`. `TerminalSession::connect(host,
  secrets, known_hosts_path, cx)` spawns a background task: build params (C1) → factory client →
  `connect` → `open_shell("xterm-256color", rows, cols)` → store shell; on error set
  `Failed`. A second spawned loop polls `shell.try_read()` (short interval via
  `cx.background_executor().timer`), feeds bytes into `screen`, and `cx.notify()`s on non-empty
  reads. Status transitions drive re-render.
- **Tests:** none beyond C1 — this is I/O + gpui wiring, observation-gated. Add a `// ponytail:
  poll-loop read; switch to event-driven if the shell adapter grows a readable-notify` note.
- **Deliverable:** a live session object whose `screen.cells()` reflects remote output.

### C3 — `TerminalView` grid rendering *(the novel GPUI piece — observation-gated)*
- **Files:** `crates/sid/src/ui/terminal.rs` (render impl).
- **New:** render `screen.cells()` as a monospace grid — one styled run per cell (or coalesced
  runs per contiguous same-style span, cheap optimization), `TermColor`→`gpui::Hsla`
  (Default→theme fg/bg, Indexed→xterm-256 palette fn, Rgb→direct), bold/italic/underline/inverse
  applied, block cursor at `cursor_position()`. Fixed cell size from a measured monospace glyph;
  compute `rows/cols` from `window.viewport_size()` and the terminal pane bounds.
- **Reference (allowed):** crib GPUI text-grid patterns from dbflux (`gpui = 0.2.2`, compatible)
  and its `gpui-component` usage — read for how they lay out a dense character grid; do not block
  on it. This is a rendering spike: **gate by observation**, no unit tests.
- **Deliverable:** remote shell output visibly rendered with color/attributes and a cursor.

### C4 — Keyboard input + resize
- **Files:** `crates/sid/src/ui/terminal.rs`; key context in `crates/sid/src/ui/mod.rs`.
- **New:** on focus, key events → bytes → `shell.write` on a spawned task (printable + Enter/Tab/
  Backspace/Esc/arrows→ANSI, Ctrl-chords→control bytes). On pane resize (viewport change),
  recompute rows/cols → `shell.resize` + `screen.resize`. Reuse the `EntityInputHandler`/key
  patterns from the A5 `TextInput` where they fit; a terminal takes raw keys, so mostly
  `on_key_down`.
- **Tests:** the key→bytes encoding table is pure — a small TDD check for the handful of
  load-bearing mappings (Enter=`\r`, Ctrl-C=`0x03`, arrows=CSI). Everything else observation.
- **Deliverable:** a usable interactive shell.

### C5 — Wire Connect into the SSH tab
- **Files:** `crates/sid/src/app.rs` (`host_row` action + `AppState` field + render).
- **New:** a `⚡ connect` affordance per host row opens a `TerminalSession` (store
  `Option<Entity<TerminalSession>>` on `AppState`; when Some, the SSH tab shows the terminal with
  a back/disconnect control that calls `shell.close` and returns to the host list). Failures land
  in the existing `AppState.error` line. Pass `AppState.secrets` and the app known-hosts path
  (`<data_dir>/known_hosts`) into `connect`.
- **Tests:** none (wiring/observation).
- **Deliverable:** end-to-end — click a host → connected terminal → back to list.

### C6 — Host-key algorithm ordering (deferred from Plan 3B) *(TDD)*
- **Files:** `crates/sid-ssh/src/known_hosts.rs` (+ `client.rs` connect config).
- **New:** `pub(crate) fn recorded_algorithms(host, port, user_known_hosts, app_known_hosts) ->
  Vec<Algorithm>` (from `known_host_keys_path` across both files); in `connect`, set
  `config.preferred.key` to put recorded algorithms first (OpenSSH `order_hostkeyalgs`), so a host
  recorded under a non-preferred algorithm negotiates the recorded one instead of failing the
  fail-closed check added in 3B.
- **Tests (TDD):** recorded-algorithm extraction from a temp known_hosts (single/multi algorithm,
  hashed entry, absent host → empty). Ordering-applied-to-config verified by unit where reachable.
- **Deliverable:** imported `~/.ssh/known_hosts` entries stop spuriously failing; the 3B security
  guarantee is unchanged.

### C7 — Gate (end of P3.3)
- `cargo test --workspace`, `clippy -D warnings`, `fmt --check` (real exit codes).
- **Manual live-sshd** (this is the moment the whole pipe is testable): run the 3B smoke
  (`cargo test -p sid-ssh --test live_sshd_smoke -- --ignored`) AND launch the app, connect to a
  real host, confirm: prompt renders with color, typing works, `top`/`htop` render, resize
  reflows, disconnect returns cleanly. This is the observation gate — no unit tests substitute.
- Adversarial review focused on: the connect/secret path (no secret leak to logs/disk; dangling
  ref handling), reader-task lifecycle (no leak after disconnect — mirrors the 3B shell fix), and
  C6 not reopening the algorithm-swap hole.
- **Deliverable:** the SSH tab is load-bearing — the North Star's spearhead milestone.
