# The TUI proof-of-concept

`sid` began as a terminal UI. That version is archived, read-only, at
[`murphlmao/sid-poc`](https://github.com/murphlmao/sid-poc). It proved the whole idea was
achievable — every integration, the layered store, the adapter discipline — and then it
was pivoted to a native GPUI desktop app (this repo). This is its record.

## What it was

A fast, focused **Ratatui** TUI cockpit: six tabs, one focused module filling the screen
at a time (`Ctrl+←/→` to switch, `Ctrl+1..6` to jump, `Ctrl+F` for a command palette),
plus a full scriptable CLI.

| Tab | What it did |
|:---|:---|
| **Workspaces** | Browse registered code workspaces (umbrella + sub-repos), drive git operations |
| **SSH** | Hosts list (merged from `~/.ssh/config` + sid-managed), embedded interactive shell via PTY, SFTP browser with download/upload/edit-in-place, per-host command history |
| **Database** | Saved Postgres + SQLite connections, multi-line query editor with SQL syntax highlight, paginated sortable results, copy-cell, CSV export, per-connection query history |
| **Network** | Listening ports, processes, interfaces — all sortable; `/` filter; `k` kills selected PID with SIGTERM → 5s grace → SIGKILL |
| **System** | Pinned config files (Enter launches external `kitty` + `$EDITOR`), systemctl services (start/stop/restart, journal tail), user-defined shell quick-actions (global from `Ctrl+F`) |
| **Settings** | Theme picker (live preview), keybind editor (capture mode + conflict detection), behavior toggles, workspace roots, quick actions, DB path — all in-app |

Plus: **detach** any tab into another terminal with `Ctrl+D` (like `claude --resume`, but for
a focused tab), **session restore** on crash, the near-black **cosmos** theme, and a
scriptable CLI:

```sh
sid net ports --format json           # listening sockets, machine-readable
sid net kill port:8080                # kill whoever owns port 8080
sid db add prod --kind postgres --dsn postgres://app@db/prod --password "$PGPASS"
sid db query local "SELECT * FROM users LIMIT 5"
sid ssh add jp46-dev 10.1.40.102 --user pi
sid ssh connect jp46-dev              # open the TUI on the SSH tab, pre-pointed
sid system pin /etc/nginx/nginx.conf --label nginx
sid settings set theme_name void
```

## Tech stack (POC)

| Layer | Choice |
|:---|:---|
| TUI | Ratatui + crossterm |
| Async | Tokio |
| Storage | redb (versioned-postcard values) |
| Git | git2 |
| SSH / SFTP | russh + russh-sftp |
| PTY | portable-pty + vt100 |
| System | sysinfo + netstat2 |

## What it proved

- The hard integrations all work: russh + PTY + SFTP, git2, Postgres/SQLite, sysinfo, the
  OS keyring, and redb persistence with a versioned-postcard codec.
- The **adapter pattern** — every external library behind a trait — which is exactly what
  made the medium pivot cheap: the integration crates carry over near-verbatim.
- Real, rigorous tests on the critical paths (the layered store, secret handling).

## Why it pivoted (2026-06-27)

The tools `sid` replaces — MobaXterm, DBeaver, VS Code — are all native GUIs, because their
work is spatial and data-dense. sid's taste (galaxy aesthetic, animation) is visual. And a
Rust-native GUI is a single light binary — *GUI does not mean Electron*. So the whole
fast/minimal/keyboard-first thesis ports to a desktop app unharmed, toward a medium that's
better at what sid actually wants. The one TUI advantage — running over SSH on a remote box
— doesn't apply to a cockpit you run locally and reach out from.

The full reasoning, the git-centric layered-scope redesign, and the code-salvage plan live
in [`docs/design/2026-06-27-gpui-rebuild-design.md`](../design/2026-06-27-gpui-rebuild-design.md).

**Salvage:** the integration adapters (russh/PTY/SFTP, git2, keyring, jobs) carry over
near-verbatim; the store was re-founded for the git-centric layered model; the ratatui
frontend was retired.
