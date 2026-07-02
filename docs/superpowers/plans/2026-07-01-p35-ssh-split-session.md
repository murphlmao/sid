# SSH Split Session — terminal + file browser side-by-side (P3.5)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development /
> executing-plans. Steps use `- [ ]`. **Runs concurrently with the DB Wave-2 UI track.**
> Both touch `crates/sid` incl. `app.rs` — keep `app.rs` edits localized (new logic in
> modules) to minimize the cross-track merge.

**Goal:** A live SSH connection shows the **terminal and a remote file browser side-by-side**
(MobaXterm-style), over **one shared SSH connection** — navigate the whole remote filesystem,
download, view (preview), and copy-path. Replaces today's mutually-exclusive terminal/SFTP modes.

**Architecture:** A combined `SshSession` gpui entity connects the `SshClient` **once**, then
opens **both** a shell channel (`open_shell`) and an SFTP channel (`open_sftp`) on that same
client — no second connection, no second auth. It composes the existing terminal render/input
(from `ui/terminal.rs`) and the file-browse logic (from `ui/sftp.rs`) into a split layout: file
sidebar + terminal main. The current standalone `⊞ files` mode is retired (folded into the session).

**Tech Stack:** Rust 2024 · gpui 0.2.2 · sid-ssh (russh) · sid-core (`SshClient`/`SshShell`/`SftpSession`/`TerminalScreen`) · sid-secrets.

## Global Constraints
- **One connection.** The shell and SFTP MUST share a single `SshClient` (one connect, one auth
  from the keyring). Do NOT open two independent connections.
- **Adapter pattern:** GPUI only in `crates/sid`; SSH/SFTP via the `sid_core` traits — never name
  `russh`/`russh_sftp` here.
- **Reuse, don't duplicate:** move/lift the existing terminal reader-pump + render/input and the
  SFTP list/nav/`safe_local_name`/download logic into the session — don't rewrite them, and don't
  leave the old mutually-exclusive paths behind as dead code.
- **Security carried over:** the P3.4 `safe_local_name` path-traversal guard (bare-component +
  `dest.parent()==downloads_dir` check) MUST remain on every local write. Never log secrets/paths' contents.
- **Blocking rule:** SSH/SFTP calls on the shared Tokio runtime / spawned tasks (as 3C/3.4 do),
  never in `render`. Rust 2024: `-> impl IntoElement` borrowing `&self` needs `+ use<>`.
- **Pragmatic TDD (standing rule):** TDD only pure logic (path join/normalize already tested in
  3.4 — reuse it; any new pure helper like an absolute-path builder gets one test). Layout, split,
  preview, clipboard, live ops are observation-gated. Keep the suite fast.

## Salvage/reference (already on `main`)
- `crates/sid/src/ui/terminal.rs` — `TerminalSession` (connect, reader pump, styled render, input,
  status-checked teardown). `terminal::ssh_runtime()` is `pub(crate)` — reuse it.
- `crates/sid/src/ui/sftp.rs` — `SftpBrowser` (list/`sort_entries`/`join_path`/`parent_path`/
  `safe_local_name`/download/upload/breadcrumb). `crates/sid/src/ssh_connect.rs` — `connect_params`/`resolve_secret`.
- SSH-tab hook points in `crates/sid/src/app.rs` (`⚡ connect` action, `terminal`/`sftp` fields, panes).

## Tasks

### P5.1 — `SshSession`: one connection, shell + sftp channels
- **Files:** `crates/sid/src/ui/session.rs` (new); `crates/sid/src/ui/mod.rs`.
- **New:** `SshSession` entity owning `client: Arc<AsyncMutex<Box<dyn SshClient>>>`, a `shell:
  Box<dyn SshShell>` + `screen: Box<dyn TerminalScreen>` + reader pump (from terminal.rs), and
  `sftp: Box<dyn SftpSession>` + `path`/`entries` (from sftp.rs), plus `status`. `SshSession::open(host,
  secrets, known_hosts, cx)` spawns: `connect_params` → connect client ONCE → `open_shell(...)` +
  `open_sftp()` on that client → start the reader pump → `sftp.list(start_dir)`. Start dir: try the
  home dir (sftp default `.` resolves to home) — keep `path` as the server-returned canonical path.
- **Refactor:** repoint `⚡ connect` to `SshSession`; move the terminal render/input + sftp browse
  helpers into/shared-with this entity; delete the standalone `SftpBrowser` open-path and the
  `terminal`/`sftp` mutually-exclusive fields once `SshSession` subsumes them (keep the pure helpers).
- **Tests:** none (I/O + gpui wiring, observation-gated).
- **Deliverable:** connecting opens one session with both a live shell and a live SFTP channel.

### P5.2 — split layout: file sidebar + terminal main
- **Files:** `crates/sid/src/ui/session.rs` (render); `crates/sid/src/app.rs` (show the session in the SSH tab).
- **New:** a horizontal split — left **file panel** (~320px, or a draggable divider if `gpui-component`
  offers one cheaply; else fixed width with a collapse toggle), right **terminal** filling the rest.
  A session header shows `user@host` + a `← disconnect` (closes shell + sftp + client, returns to host
  list). Match the app's dark palette.
- **Deliverable:** terminal and file browser visible at once for a live connection.

### P5.3 — file panel: full-filesystem nav + download · view · copy-path
- **Files:** `crates/sid/src/ui/session.rs` (file-panel render + actions).
- **New:** breadcrumb of the current absolute `path` (segment-click navigates); an `↑ up` + `⟳`
  refresh; rows (dir/file glyph, name, size, mtime) — click a dir to descend; a text-path "go to" box
  (reuse A5 `TextInput`) so the user can jump anywhere in the filesystem (e.g. `/etc`, `/var/log`).
  Per-row actions:
  - **⭳ download** — `get(abs_path)` → `safe_local_name` → `~/Downloads` (traversal guard intact).
  - **👁 view** — `get(abs_path)` → show contents in a preview overlay (a `deferred`/`anchored` modal):
    UTF-8 text rendered read-only (monospace, scrollable); if the bytes aren't valid UTF-8 or exceed a
    cap (e.g. 1 MiB), show a "binary / too large — download instead" notice rather than dumping bytes.
    `// ponytail: text preview only; no image/hex viewer yet`.
  - **⧉ copy path** — copy the file's **absolute remote path** to the clipboard (`cx.write_to_clipboard`).
  - (Directories: copy-path applies too; view/download are file-only.)
- **Pure helper (TDD):** `fn abs_remote_path(dir: &str, name: &str) -> String` (reuse/adapt 3.4's
  `join_path`; one test that a nested name yields the correct absolute path). Everything else observation-gated.
- **Deliverable:** the requested MobaXterm-style file operations during the session.

### P5.4 — Gate (end of P3.5)
- `cargo test --workspace`, `clippy -D warnings`, `fmt --check` (real exit codes).
- **Observation (the real gate):** connect to a real host → terminal AND file panel show together;
  type in the terminal while browsing files; navigate to `/`, `/etc`, home; download a file (lands in
  `~/Downloads`, traversal-safe); view a text file (preview overlay) and a binary (safe notice); copy a
  path (paste elsewhere to confirm); disconnect closes everything cleanly; no second auth occurred
  (one connection).
- Adversarial review: the download traversal guard still fires; the shared connection's shell+sftp
  lifecycle (both closed on disconnect, no leaked reader task — mirror 3C's status-checked teardown);
  clipboard copy carries only the path, never file contents/credentials.
- **Deliverable:** the SSH/SFTP tab matches the MobaXterm split-session UX.
