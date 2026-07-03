# SSH multi-tab shell rebuild (the MobaXterm model)

**Approved design:** `docs/mockups/2026-07-02-ssh-v3-mobaxterm.html` (Murphy: "looks good as fuck").
Build it. This replaces the current single-session SSH tab with a **session-tabbed shell**.

## The model
- **Session tab strip** at the top of the SSH tab: `🏠` home (leftmost, icon-only) · one tab per
  live connection (`user@host ×`) · `＋`. Clicking a tab activates it; `×` disconnects+closes it;
  `＋` goes to home.
- **Home tab active** (`active_session == None`): MAIN area = the connection manager (today's host
  list). SIDEBAR = the **saved-connections tree**: hosts grouped by `folder` (collapsible headers;
  `None`→top level), origin badges (global/workspace), a quick-connect search box on top, per-row
  hover actions (connect / edit / delete), and **inline rename** (F2 or double-click → the label
  becomes a `TextInput`; Enter commits via `Store::rename_host`, Esc cancels — VS Code style, NOT
  the full form).
- **Session tab active** (`active_session == Some(i)`): MAIN = that session's terminal. SIDEBAR
  swaps to that session's **file browser** (the current split-session browser). Browser side from
  `Settings.file_browser_side` (default Left); a `⇄ dock` toggle in the browser header flips it and
  **persists** via `Store` settings. (Drag-to-dock is a nice-to-have — the toggle is required, the
  drag can be deferred with a `// ponytail:` note.)

## Architecture
- **`AppState`** currently holds one `SshSession`. Rework to `ssh_sessions: Vec<Entity<SshSession>>`
  + `active_session: Option<usize>` (None = home). Each `SshSession` is fully independent (its own
  client/reader/writer/shell/sftp — the reader/writer split already merged). Connecting from home
  pushes a session and switches to it; closing disconnects (existing `disconnect`) and removes it,
  adjusting `active_session`.
- **`SshSession`** (session.rs) already owns terminal + file-browser + connect/disconnect. Keep its
  internals; the rebuild is mostly (a) making the tab own a Vec of them, (b) the tab strip, (c) the
  home-vs-session content switch, (d) the connections tree sidebar with folders + inline rename.
- **Inline rename** is a small reusable pattern: a `renaming: Option<{alias, TextInput}>` on the
  home/tree state; render the edit field in place of the label for that row.
- **Folders:** display grouping from the `folder` field (already in the store). Assigning a folder
  can be minimal for v1 (e.g. an entry in the row's hover menu → a small input, calling
  `Store::set_host_folder`) or deferred to a follow-up; **grouping + collapse is the v1 requirement**.

## Keep the invariants
- Adapter pattern: `session.rs` names only `sid_core` SSH trait types + existing constructors.
  No blocking in render; connect/IO on the shared runtime; render pure-from-cache.
- `Store::rename_host`/`set_host_folder` already exist (with Conflict guards) — use them; a rename
  to an existing alias must surface the `Conflict` error, not clobber.
- Secrets resolved via the existing `secret_ref`→keyring/vault path (unchanged).

## Ownership (this track)
`crates/sid/src/ui/session.rs`, the SSH-tab region + session model in `crates/sid/src/app.rs`,
`crates/sid/src/ui/mod.rs` (any new submodule). A parallel branch owns `db_tab.rs`/`db_conn_form.rs`
(DB polish) and must not be touched. **Do NOT build the keyboard system** — that's the next track
and binds to the session-tab structure this one creates (it will add `Ctrl+Tab` cycling + `Ctrl+W`
close etc. on top). Just make the tabs/close/switch reachable by mouse; expose clean methods
(`activate_session(i)`, `close_session(i)`, `go_home()`, `new_session()`) the keyboard track can call.

## Verify
`cargo test/clippy/fmt` (real exit codes). Then drive it live: `scripts/test-ssh.sh`'s docker sshd
gives a real host — use `scripts/sid-shot.sh --keep` + `scripts/sid-click.sh` (see HANDOFF) to
connect two sessions, switch tabs (sidebar swaps terminal↔files), inline-rename a host, toggle dock
side, close a tab. Screenshot-verify each. Pragmatic TDD: unit-test the pure bits (folder grouping
transform, rename-index/active-session bookkeeping on close); rendering is observation-gated.

## Durability
`git push -u origin HEAD` after every commit.
