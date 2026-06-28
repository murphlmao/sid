<div align="center">

# `sid`

**a fast, focused desktop cockpit for developer workflow.**

*named after my loving dog, my first true friend in life.*

`✦`&nbsp;&nbsp;`·`&nbsp;&nbsp;`★`&nbsp;&nbsp;─────────────────────────────────&nbsp;&nbsp;`★`&nbsp;&nbsp;`·`&nbsp;&nbsp;`✦`

</div>

---

> **Status: rebuilding.** `sid` began as a Rust TUI proof-of-concept (archived at
> [`murphlmao/sid-poc`](https://github.com/murphlmao/sid-poc)). That POC proved the
> features are achievable. This repo is the rebuild as a **native desktop app** —
> same soul, better medium.

`sid` is a personal developer **ops-cockpit**: the things you reach for during a
workday — SSH/SFTP, databases, network/process control — in one fast, keyboard-first,
galaxy-themed native app. You run it locally and it reaches out at everything else.
It is not trying to replace your editor or your shell; it is the cockpit you live in
*between* edits and shells.

## What's different from the POC

- **Native GUI** (GPUI) instead of a terminal UI — real data grids, real animation,
  a launchable app. The "fast / minimal / one-binary / keyboard-first" thesis is kept
  intact; a native Rust GUI is just as light as a TUI.
- **Git-centric, layered workspace scope** (modeled on how Claude Code layers
  `~/.claude` + a repo's `.claude`):
  - a **global** layer — your everywhere shelf, always loaded, nothing lost;
  - a **per-workspace** layer — a committed `.sid/` that travels with the repo and
    overlays global when you focus that workspace.
  - Secrets never get committed: they live in the OS keyring, referenced by id.
  - One process. "Focusing a workspace" swaps the overlay — it does not launch a
    second instance.

## Core tabs

| Tab | What it does |
|:---|:---|
| **SSH / SFTP** | Host manager (aliases, merged ssh config), embedded terminal, SFTP browse/transfer. *The spearhead — built first.* |
| **Database** | Saved connections, query editor, sortable result grids, export. |
| **Network** | Listening ports, processes, interfaces; kill-by-pid / kill-by-port. |
| Workspaces · System | Secondary — workspace registration / scope, pinned configs + services. |

## Design philosophy

- **Cognitive cleanliness over density.** Calm, focused, fast.
- **Minimal footprint.** One native binary.
- **Adapter pattern everywhere.** Every external library and OS integration hides
  behind a trait — swappable, testable, future-proof. This is what made the medium
  pivot cheap, and it stays.
- **Persistence-first.** The store *is* the state.
- **Vertical slices.** One tab finished to daily-use quality before the next.

## Platform

Built on Wayland/Linux first. GNOME, Windows, and macOS are accommodated via the
adapter seams (keyring, PTY, editor-launch, clipboard, notifications) but not yet
implemented.

---

Design source of truth: [`docs/design/`](docs/design/). Plans: [`docs/superpowers/plans/`](docs/superpowers/plans/).
