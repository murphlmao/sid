<div align="center">

# `sid`

**a fast, focused TUI cockpit for developer workflow.**

*named after my dog, who was the best.*

`✦`&nbsp;&nbsp;`·`&nbsp;&nbsp;`★`&nbsp;&nbsp;─────────────────────────────────&nbsp;&nbsp;`★`&nbsp;&nbsp;`·`&nbsp;&nbsp;`✦`

</div>

---

`sid` is a personal developer cockpit built in Rust. It puts the things you actually use during a workday — git, SSH, databases, ports/processes, system tweaks — into one fast, keyboard-driven TUI. One focused tab per concern. No clutter, no daemon, no metadata-file zoo. Crash-safe. Detachable. Galaxy-themed.

> **Status:** Early WIP. Spec drafted, implementation forthcoming.

---

## What's inside (v1)

| Tab | What it does |
|:---|:---|
| **Workspaces** | Browse registered code workspaces (umbrella + sub-repos), drive git operations |
| **SSH** | Connect to hosts, embedded terminal, SFTP browser |
| **Database** | Postgres + SQLite. Query editor, paginated results, history |
| **Network** | Listening ports, processes, interfaces. Kill PIDs from the keyboard |
| **System** | Pinned config files, systemctl services, custom shell quick-actions |
| **Settings** | Theme, keybinds, behavior — all in-app, no config-file scavenger hunt |

Plus:

- **Detach** any tab into another terminal with `Ctrl+D` — like `claude --resume`, but for a focused tab
- **Session restore** on launch if you crash or accidentally close the terminal
- **Cosmos theme** by default — near-black with red accents and `✦` glyph cues; user-authorable

## Why

VS Code is slow and visually noisy. `lazygit`, `gitui`, `k9s` are great but each only solves one thing. `tmux` + a bag of CLIs is the closest pre-`sid` setup but requires re-deriving the same layout every session.

`sid` is the layer above. One TUI, six tabs, an obsession with minimal footprint and fast startup. It's not trying to replace your editor or your shell. It's trying to be the cockpit you live in *between* edits and shells.

## Design philosophy

- **Cognitive cleanliness over information density.** btop is beautiful but busy; `sid` is calm.
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
```

**Keybinds in this build:** `Ctrl+←/→` switch tabs · `Ctrl+1..6` jump · `Ctrl+F` command palette · `Ctrl+Q` quit · `Ctrl+,` open Settings.

> **What works in this build:** Foundation complete. Six tabs render as labelled stubs in the cosmos theme; navigation, command palette, theme, and active-tab persistence work. Real tab content arrives in subsequent plans.

## Documentation

- [**Architecture**](docs/ARCHITECTURE.md) — crate layout, data flow, theming, persistence
- [**Installation**](docs/INSTALLATION.md) — prerequisites and first-run
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
| Language | Rust (edition 2024) |
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

The app is named after my dog — a fat little black shih tzu terrier who passed away. He was the best.

When he passed, I wanted to name something I'd touch every day after him. So the cockpit I live in between edits and shells got his name. Every launch is a small hello.

---

<div align="center">

`✦`&nbsp;&nbsp;`·`&nbsp;&nbsp;`★`&nbsp;&nbsp;&nbsp;*for sid, who liked all of the snow*&nbsp;&nbsp;&nbsp;`★`&nbsp;&nbsp;`·`&nbsp;&nbsp;`✦`

</div>
