# SFTP Browser (P3.4) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development /
> executing-plans. Steps use `- [ ]`. **Runs concurrently with DB Wave-2.** Touches
> `crates/sid` (frontend) + consumes the already-merged `sid-ssh` SFTP adapter. Independent
> of the DB tracks by crate.

**Goal:** From a host, open an SFTP file browser — navigate directories, download/upload
files — completing the "SSH/SFTP" tab's file half alongside the terminal (3C).

**Architecture:** Mirror the 3C `TerminalSession` pattern: an `SftpBrowser` gpui entity owns
`Box<dyn SftpSession>` (from `RusshClient::open_sftp()`), a current path, and a cached
`Vec<SftpEntry>`; async SFTP ops run on the shared Tokio runtime (3C's), results marshalled
back to the entity via channels; render paints purely from the cache. A `⊞ files` row action
opens it, parallel to 3C's `⚡ connect`.

**Tech Stack:** Rust 2024 · gpui 0.2.2 · sid-ssh (russh-sftp) · sid-core (`SftpSession` trait)
· sid-secrets (keyring, for the connect).

## Global Constraints
- **Adapter pattern:** GPUI only in `crates/sid`; all SFTP goes through `sid_core::ssh::SftpSession`
  — never name `russh_sftp` in `crates/sid`.
- **Secrets:** the connect reuses 3C's `connect_params`/`resolve_secret` (keyring by `secret_ref`);
  no new secret handling. Never log paths' contents or credentials.
- **Blocking rule:** SFTP calls are async — run on the shared runtime / spawned tasks, never in
  `render`. Render is pure-from-cache (existing convention).
- **Rust 2024 RPIT:** `-> impl IntoElement` borrowing `&self` needs `+ use<>`.
- **Pragmatic TDD (standing rule):** TDD only the pure logic — path join/normalize (`..`, root,
  trailing slash) and entry sort (dirs-first, then name). Do NOT unit-test rendering or live SFTP
  ops — observation-gated. Keep the suite fast; no exhaustive permutations.

## Adapter API this plan consumes (already on `main`)
- `sid_core::ssh`: `trait SftpSession { async fn list(&mut self, path: &str) -> Result<Vec<SftpEntry>>; async fn get(&mut self, path: &str) -> Result<Vec<u8>>; async fn put(&mut self, path: &str, bytes: &[u8]) -> Result<()>; async fn remove_file(&mut self, path: &str) -> Result<()>; async fn mkdir(&mut self, path: &str) -> Result<()>; async fn stat(&mut self, path: &str) -> Result<Option<SftpEntry>>; async fn close(&mut self) -> Result<()>; }`,
  `SftpEntry { name: String, is_dir: bool, size: u64, mtime_secs: i64, mode: u32 }`.
- `sid_core::ssh::SshClient::open_sftp(&mut self) -> Result<Box<dyn SftpSession>>`.
- 3C's `crates/sid/src/ssh_connect.rs`: `connect_params(host, secret)`/`resolve_secret(secrets, host)`.
- 3C's `crates/sid/src/ui/terminal.rs` — the entity+reader-pump+status pattern to mirror.
- SSH-tab hook points in `crates/sid/src/app.rs` (`host_row` — the `⚡ connect` action added by 3C
  is the sibling to copy).

## Tasks

### S1 — `SftpBrowser` entity: connect + open session + list root
- **Files:** `crates/sid/src/ui/sftp.rs` (new); `crates/sid/src/ui/mod.rs` (export).
- **New:** `SftpBrowser` entity: `session: Option<...>` (the `Box<dyn SftpSession>` behind the same
  `Arc<AsyncMutex<..>>` wrapper 3C uses), `status: SftpStatus { Connecting, Ready, Failed(String), Closed }`,
  `path: String` (start `"."` or `"/"`), `entries: Vec<SftpEntry>`, `error: Option<String>`.
  `SftpBrowser::open(host, secrets, known_hosts_path, cx)` spawns a task: `connect_params` →
  `RusshClientFactory::new(known_hosts).new_client()` → `connect` → `open_sftp()` → store session →
  `list(path)` → cache entries → `cx.notify()`. Errors → `Failed`.
- **Consumes:** `connect_params`/`resolve_secret` (C1). **Produces:** the browser entity the tab shows.
- **Tests:** none (I/O + gpui wiring, observation-gated).

### S2 — path logic + entry ordering *(pure — TDD)*
- **Files:** `crates/sid/src/ui/sftp.rs` (free fns + tests).
- **New:** `fn join_path(base: &str, name: &str) -> String` and `fn parent_path(p: &str) -> String`
  (POSIX semantics: normalize `//`, handle root, `parent("/")=="/"`); `fn sort_entries(&mut Vec<SftpEntry>)`
  (dirs before files, each alphabetical, case-insensitive). Call `sort_entries` after every `list`.
- **Tests (TDD):** `join_path("/home","a")=="/home/a"`, `join_path("/","a")=="/a"`; `parent_path("/a/b")=="/a"`,
  `parent_path("/")=="/"`; sort puts dirs first then names.

### S3 — render: breadcrumb + entry list + navigation
- **Files:** `crates/sid/src/ui/sftp.rs` (render); navigation actions.
- **New:** breadcrumb of the current `path` (click a segment → navigate there); a scrollable list of
  entries — dir/file glyph, name, right-aligned size (human bytes) + mtime; clicking a dir → `path =
  join_path` → re-`list`; an `↑ up` control → `parent_path` → re-`list`; a `⟳` refresh. Match the app's
  dark palette. Observation-gated (no unit tests).
- **Deliverable:** a navigable remote file browser.

### S4 — download + upload
- **Files:** `crates/sid/src/ui/sftp.rs`.
- **New:** per-file `⭳ download` → `get(path)` → write bytes to `<downloads>/<name>` (use `dirs`-style
  data/home dir already available, or `$HOME/Downloads`); surface success/failure in the error line.
  An `⭱ upload` control → for P3.4, upload a fixed-picker path is out of gpui's scope without a native
  file dialog — instead accept a path via a small `TextInput` (from A5) prompt, `get` local bytes via
  `std::fs::read`, `put(join_path(path,name), bytes)`, re-`list`. `// ponytail: text-path upload until a
  native file dialog adapter exists`.
- **Tests:** none (filesystem + SFTP I/O, observation-gated); the target-path building reuses S2's tested
  `join_path`.
- **Deliverable:** files move both directions.

### S5 — wire `⊞ files` into the SSH tab
- **Files:** `crates/sid/src/app.rs` (`host_row` + `AppState` field + render).
- **New:** a `⊞ files` row action beside `⚡ connect`; store `Option<Entity<SftpBrowser>>` on `AppState`
  (when Some, the SSH tab shows the browser with a `← back` that calls `session.close` and returns to the
  host list). Pass `AppState.secrets` + the app known-hosts path (same as 3C). Failures → `AppState.error`.
  Terminal and SFTP are mutually exclusive views of the tab (opening one closes the other).
- **Deliverable:** click `⊞ files` on a host → browse its filesystem → back.

### S6 — Gate (end of P3.4)
- `cargo test --workspace`, `clippy -D warnings`, `fmt --check` (real exit codes).
- **Manual/observation** (the real gate): connect via `⊞ files` to a real host — list a dir, navigate in
  and up via breadcrumb, download a file (verify it lands locally), upload a file (verify it appears
  remotely), confirm errors surface, back returns cleanly, and terminal/SFTP don't both show at once.
- Adversarial review focus: the connect/secret path (shared with 3C — no new leak), path handling (no
  traversal surprise in `join_path`/`parent_path`), reader/session lifecycle (no leak after `close` —
  mirror 3C's status-checked teardown).
- **Deliverable:** the SSH/SFTP tab is whole — terminal + files. (SSH spearhead feature-complete.)
