<div align="center">

# `sid`



**a fast, focused TUI cockpit for developer workflow.**

*named after my loving dog, my first true friend in life.*

`✦`&nbsp;&nbsp;`·`&nbsp;&nbsp;`★`&nbsp;&nbsp;─────────────────────────────────&nbsp;&nbsp;`★`&nbsp;&nbsp;`·`&nbsp;&nbsp;`✦`

</div>

---

`sid` is a personal developer cockpit built in Rust. It puts the things you actually use during a workday — git, SSH, databases, ports/processes, system tweaks — into a quick little TUI. One focused tab per concern without any of the other bloat that you may see with applications that deal with these many systems at once. Galaxy-themed because... who doesn't like space?

> **Status:** Early WIP. Spec drafted, implementation forthcoming.

---

## What's inside (v1)

| Tab | What it does |
|:---|:---|
| **Workspaces** | Browse registered code workspaces (umbrella + sub-repos), drive git operations |
| **SSH** | Hosts list (merged from `~/.ssh/config` + sid-managed), embedded interactive shell via PTY, SFTP browser with download/upload/edit-in-place, per-host command history |
| **Database** | Saved Postgres + SQLite connections, multi-line query editor with SQL syntax highlight, paginated sortable results, copy-cell, CSV export, per-connection query history; CLI: `sid db add/remove/list/query` |
| **Network** | Listening ports, processes, interfaces — all sortable; `/` filter; `k` kills selected PID with SIGTERM → 5s grace → SIGKILL; CLI: `sid net ports/procs/interfaces/kill` |
| **System** | Pinned config files (Enter launches external kitty + `$EDITOR`), systemctl services (start/stop/restart, journal tail), user-defined shell quick-actions (available globally from `Ctrl+F`) |
| **Settings** | Theme picker (live preview), keybind editor (capture mode + conflict detection), behavior toggles, workspace roots, quick actions, DB path — all in-app, no config-file scavenger hunt |

Plus:

- **Detach** any tab into another terminal with `Ctrl+D` — like `claude --resume`, but for a focused tab
- **Session restore** on launch if you crash or accidentally close the terminal
- **Cosmos theme** by default — near-black with red accents and `✦` glyph cues; user-authorable

## Why

VS Code is slow and visually noisy. `lazygit`, `gitui`, `k9s` are great but each only solves one thing. `tmux` + a bag of CLIs is the closest pre-`sid` setup but requires re-deriving the same layout every session. Zed is cool, but limited in its scope and feature-set.

`sid` is the layer above. One TUI, six tabs (detachable, re-attachable), an obsession with minimal footprint, fast startup, ease of use, all for the sole purpose of not needing to use slow database apps like DBeaver, or remembering how to kill that one NodeJS instance via some command you Google every other week because you're too lazy to remember it's syntax (`lsof -ti:3000 | xargs kill -9`, `pkill -9 -f "next dev"`, etc). It's not trying to replace your editor or your shell. It's trying to be the cockpit you live in *between* edits and shells.

## Design philosophy

- **Cognitive cleanliness over information density.** btop is beautiful but busy; `sid` is calm & straight to the point.
- **Minimal footprint.** One binary, one DB file, zero dotfile sprawl.
- **Adapter pattern everywhere.** Every external library hides behind a trait — swappable, testable, future-proof.
- **Persistence-first.** Your work is saved continuously. There is no "save" because the DB *is* the state.
- **Keyboard ergonomics.** `Ctrl+arrows` to switch tabs, `Ctrl+F` for the command palette, `Tab` for in-pane focus cycling. Defaults sensible; everything overridable.

## Quickstart

```sh
# Clone, build, run
git clone https://github.com/murphlmao/sid && cd sid
cargo build --release
./target/release/sid

# Or run from source
cargo run -p sid

# Tests
cargo test --workspace

# Override the DB location (otherwise XDG default applies)
sid --db /tmp/sid.redb

# Network inspection (CLI, no TUI needed)
sid net ports                  # listening sockets, table form
sid net ports --format json    # same data, machine-readable
sid net procs --sort cpu --top 20
sid net interfaces
sid net kill 1234              # SIGTERM with 5s grace, then SIGKILL
sid net kill 1234 --force      # SIGKILL immediately
sid net kill port:8080         # kill whoever owns port 8080

# Settings (scripted)
sid settings list                   # dump every setting key + value
sid settings get theme_name         # print one value
sid settings set theme_name void    # change theme (takes effect next launch)
sid settings set default_tab workspaces
sid settings delete theme_name      # remove an override

# System management
sid system pin /etc/nginx/nginx.conf --label "nginx"
sid system pins
sid system unpin /etc/nginx/nginx.conf
sid system services --user
sid system action add "kill 5432" "fuser -k 5432/tcp"
sid system action list
sid system action run <id>

# Database management
sid db add local --kind sqlite --name "Local" --dsn ./data.db
sid db add prod --kind postgres --name "Prod" --dsn postgres://app@db.example.com/prod --password "$PGPASS"
sid db query local "SELECT * FROM users LIMIT 5"   # CSV on stdout
sid db query local "INSERT INTO t VALUES (1)"      # rows-affected summary
sid db list
sid db remove local

# SSH host management
sid ssh add jp46-dev 10.1.40.102 --user pi --port 22
sid ssh list                  # merged manual + ~/.ssh/config
sid ssh connect jp46-dev      # launches the TUI on the SSH tab, pre-pointed
sid ssh remove jp46-dev
```

Inside the SSH tab: `j`/`k` select a host, Enter connects (opens embedded shell via PTY), `Tab` toggles the SFTP sub-panel.

**Keybinds in this build:** `Ctrl+←/→` switch tabs · `Ctrl+1..6` jump · `Ctrl+F` command palette · `Ctrl+Q` quit · `Ctrl+,` open Settings.

> **What works in this build:** Foundation + Workspaces + Network + Settings + **System** + **Database** data surfaces fully functional. Settings tab carries theme picker with live preview, keybind editor with capture-mode + conflict detection, behavior toggles, workspace roots editor, quick actions editor, DB path override (writes the one-line `~/.config/sid/sid.toml`), and reset-to-defaults flow; `sid settings get/set/list/delete` provides scripted access. System tab data surface is wired: pinned configs and global quick-actions persist in redb; `sid system pin/unpin/pins`, `sid system services [--user|--system] [--state STATE]`, and `sid system action add/list/remove/run` are scriptable from the CLI; global quick-actions hydrate into the `Ctrl+F` palette at startup. Database tab has full pure-state coverage (connection list, multi-line editor with SQL syntax highlight via a hand-rolled lexer, paginated sortable results, copy-cell, CSV export, per-connection history), plus `sid db add/remove/list/query` for scripting; the interactive ratatui chrome for the Database tab lands once the cosmos render harness is built, Database tab interactive ratatui chrome lands once the cosmos render harness is built. SSH tab is now functional: hosts list merges `~/.ssh/config` with manually-added hosts; Enter on a host opens an interactive shell in an embedded PTY; `Tab` toggles an SFTP sub-panel with download/upload/edit-in-place; `sid ssh add/remove/list/connect` CLI for headless management.

## Documentation

- [**Architecture**](docs/ARCHITECTURE.md) — crate layout, data flow, theming, persistence
- [**Installation**](docs/INSTALLATION.md) — prerequisites and first-run
- [**Settings**](docs/Settings.md) — every setting key, the in-app editor, the CLI, `sid.toml`
- [**Development**](docs/DEVELOPMENT.md) — how to extend sid (new widgets, themes, tabs, adapters)
- [**Testing**](docs/TESTING.md) — running tests, coverage, mutation testing, MC/DC
- [**Contributing**](docs/CONTRIBUTING.md) — pull request rules
- [**Troubleshooting**](docs/TROUBLESHOOTING.md) — common problems

Specs and plans:

- [**Foundation design (v1 spec)**](docs/superpowers/specs/2026-05-20-sid-foundation-design.md) — architecture, all six tabs, storage, themes, keybinds, adapter layers
- [**Future features**](docs/superpowers/specs/2026-05-20-sid-future-features.md) — planned v2+ work and "someday" ideas

## Tech stack

| Layer | Choice |
|:---|:---|
| Language | Rust |
| TUI | [Ratatui](https://ratatui.rs) + crossterm |
| Async | [Tokio](https://tokio.rs) |
| Storage | [redb](https://github.com/cberner/redb) (pure-Rust ACID embedded DB, multi-process readers) |
| Git | [git2](https://github.com/rust-lang/git2-rs) |
| SSH / SFTP | [russh](https://github.com/Eugeny/russh) + russh-sftp |
| PTY | [portable-pty](https://github.com/wez/wezterm/tree/main/pty) + vt100 |
| System | [sysinfo](https://github.com/GuillaumeGomez/sysinfo) + netstat2 |

Every dependency in this list is behind an internal trait, so any of them can be swapped without changing widget code.

## License

[GNU GPL v3](LICENSE).

## about sid &nbsp;🐕

The app is named after my dog — a fat little black shih tzu terrier who passed away in 2016. He was *the* sweetest, the funniest, and the most loving dog.

When he passed, I wanted to name something I'd touch every day after him. Creating a custom tool that made my life easier is reflective of who Sid was to me every single day during the darkest period of my life. Every launch is a small hello, and a small reminder of what he continues to do for me every day. 

---

<div align="center">

`✦`&nbsp;&nbsp;`·`&nbsp;&nbsp;`★`&nbsp;&nbsp;&nbsp;*for sid, who liked all of the snow, hotdogs, and cake. *&nbsp;&nbsp;&nbsp;`★`&nbsp;&nbsp;`·`&nbsp;&nbsp;`✦`

</div>
