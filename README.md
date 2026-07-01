<div align="center">

# `sid`



**a fast, focused desktop cockpit for developer workflow.**

*named after my loving dog, my first true friend in life.*

`✦`&nbsp;&nbsp;`·`&nbsp;&nbsp;`★`&nbsp;&nbsp;─────────────────────────────────&nbsp;&nbsp;`★`&nbsp;&nbsp;`·`&nbsp;&nbsp;`✦`

</div>

---

`sid` is a personal developer cockpit built in Rust. It puts the things you actually use during a workday — SSH, databases, ports/processes, system tweaks — into one fast native app. One focused tab per concern, without any of the other bloat you see in applications that deal with these many systems at once. Galaxy-themed because... who doesn't like space?

> **Status:** Rebuilding. `sid` started life as a Ratatui **TUI** proof-of-concept (archived at [`murphlmao/sid-poc`](https://github.com/murphlmao/sid-poc)) — that POC proved the whole thing was achievable. This repo is the rebuild as a **native GPUI desktop app**: same soul, better medium. ([why?](docs/history/tui-proof-of-concept.md)) The layered store foundation and the first SSH slice are in; the rest lands tab by tab.

---

## The idea: one cockpit, layered by workspace

Two things make `sid` more than "six tools sharing a window":

- **It's git-centric.** Like the way Claude Code layers `~/.claude` over a repo's `.claude`, sid has a **global** level — your everywhere shelf, always loaded, nothing lost — and a **per-workspace** level: a committed `.sid/config.toml` that travels with the repo. Clone a project and its sid context (hosts, connections, quick-actions) comes with it. Secrets are never committed; they live in the OS keyring, referenced by an opaque id.
- **Composition is attributive — it never overrides.** Focus a workspace and you see the *union* of global + that workspace, each entry tagged by where it came from. Nothing is silently shadowed. A simple checkbox collapses true duplicates (workspace wins), and another hides the global stuff when you want a workspace-only view.

Focusing a workspace swaps the active scope — it does not launch a second instance. One process, one window, everything in its place.

## What's inside

| Tab | What it does |
|:---|:---|
| **SSH / SFTP** | Hosts list (merged from `~/.ssh/config` + sid-managed), embedded interactive shell via PTY, SFTP browser with download/upload/edit-in-place, per-host command history. *The spearhead — built first.* |
| **Database** | Saved Postgres + SQLite connections, multi-line query editor with SQL syntax highlight, paginated sortable results, copy-cell, CSV export, per-connection query history |
| **Network** | Listening ports, processes, interfaces — all sortable and filterable; kill a PID (or a whole port) with SIGTERM → grace → SIGKILL |
| **Workspaces** | Register your code roots; focus one to scope the whole app to it |
| **System** | Pinned config files (open in `$EDITOR`), systemctl services (start/stop/restart, journal tail), user-defined shell quick-actions |
| **Settings** | Theme picker, keybind editor, behavior toggles, workspace roots — all in-app, no config-file scavenger hunt |

## Why

VS Code is slow and visually noisy. `lazygit`, `gitui`, `k9s` are great but each only solves one thing. `tmux` + a bag of CLIs is the closest pre-`sid` setup but requires re-deriving the same layout every session. Zed is cool, but limited in its scope and feature-set.

`sid` is the layer above. One app, focused tabs, an obsession with minimal footprint, fast startup, ease of use, all for the sole purpose of not needing to use slow database apps like DBeaver, or remembering how to kill that one NodeJS instance via some command you Google every other week because you're too lazy to remember it's syntax (`lsof -ti:3000 | xargs kill -9`, `pkill -9 -f "next dev"`, etc). It's not trying to replace your editor or your shell. It's trying to be the cockpit you live in *between* edits and shells.

## Design philosophy

- **Cognitive cleanliness over information density.** btop is beautiful but busy; `sid` is calm & straight to the point.
- **Minimal footprint.** One native binary, one DB file, zero dotfile sprawl. A native Rust GUI is just as light as a TUI — *GUI does not mean Electron.*
- **Adapter pattern everywhere.** Every external library and OS integration hides behind a trait — swappable, testable, future-proof. This is exactly what made the medium pivot cheap, and it stays.
- **Persistence-first.** Your work is saved continuously. There is no "save" because the store *is* the state.
- **Keyboard ergonomics.** Keyboard-first, mouse where it earns its place. Defaults sensible; everything overridable.

## Quickstart

```sh
# Clone, build, run
git clone https://github.com/murphlmao/sid && cd sid
cargo run -p sid

# Tests
cargo test --workspace
```

Wayland/Linux first. GNOME, Windows, and macOS are accommodated through the same adapter seams (keyring, PTY, editor-launch, clipboard, notifications) but not yet implemented.

## Documentation

- [**Design (North Star)**](docs/design/2026-06-27-gpui-rebuild-design.md) — the reframe, the layered scope model, the code disposition
- [**Store schema**](docs/design/2026-06-27-store-schema.html) — how global + workspace compose (open in a browser)
- [**The TUI proof-of-concept**](docs/history/tui-proof-of-concept.md) — what the original was, and why it pivoted

## Tech stack

| Layer | Choice |
|:---|:---|
| Language | Rust (edition 2024) |
| GUI | [GPUI](https://www.gpui.rs) — Zed's GPU-accelerated framework |
| Storage | [redb](https://github.com/cberner/redb) (pure-Rust ACID embedded DB) + a committed TOML file per workspace |
| Encoding | [postcard](https://github.com/jamesmunns/postcard) — compact binary, for the redb values |
| Git | [git2](https://github.com/rust-lang/git2-rs) |
| SSH / SFTP | [russh](https://github.com/Eugeny/russh) + russh-sftp |
| PTY | [portable-pty](https://github.com/wez/wezterm/tree/main/pty) + a terminal grid |
| System | [sysinfo](https://github.com/GuillaumeGomez/sysinfo) |

Every dependency in this list is behind an internal trait, so any of them can be swapped without changing view code.

## License

[GNU GPL v3](LICENSE).

## about sid &nbsp;🐕

The app is named after my dog — a fat little black shih tzu terrier who passed away in 2016. He was *the* sweetest, the funniest, and the most loving dog.

When he passed, I wanted to name something I'd touch every day after him. Creating a custom tool that made my life easier is reflective of who Sid was to me every single day during the darkest period of my life. Every launch is a small hello, and a small reminder of what he continues to do for me every day. 

---

<div align="center">

`✦`&nbsp;&nbsp;`·`&nbsp;&nbsp;`★`&nbsp;&nbsp;&nbsp;*for sid, who liked all of the snow, hotdogs, and cake. *&nbsp;&nbsp;&nbsp;`★`&nbsp;&nbsp;`·`&nbsp;&nbsp;`✦`

</div>
