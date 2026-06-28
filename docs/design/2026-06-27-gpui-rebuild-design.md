# sid — GPUI Rebuild Design (North Star)

**Date:** 2026-06-27 · **Status:** Active · **Supersedes:** the TUI POC (`murphlmao/sid-poc`)

This is the durable design the rebuild hangs off. Plans cite it; when intent changes,
edit this in the same change.

## 1. Reframe

sid is an integrated developer **ops-cockpit**, not a tool-belt of six apps sharing a
window. Its earlier pain was being built *horizontally* — six ~70%-done tools, integrated
before any one was finished. The cure is **vertical**: finish one tab to "I open sid every
day for this," ship it to yourself, let daily use teach you what the rest (and the
integration) should be. You cannot design the integration until at least one tab is
load-bearing in real use.

Scope: **SSH/SFTP, Database, Network** are the core. **Workspaces** (vscode already serves
it) and **System** (a convenience layer) are secondary. The spearhead is **SSH/SFTP**.

## 2. Medium — native GPUI desktop app

The tools sid replaces (MobaXterm, DBeaver, vscode) are all native GUIs because their work
is spatial and data-dense. sid's taste (galaxy aesthetic, animation) is visual. And a
Rust-native GUI (`gpui`) is a single light binary — "TUI ≠ lightweight, GUI ≠ Electron."
So the whole fast/minimal/keyboard-first thesis ports unharmed; only the rendering medium
changes, toward the one better at what sid wants.

Workflow note that settles the one TUI advantage: Murphy runs sid **locally and reaches
out** (almost-always-local). The single reason to stay a TUI — running sid *on* a remote
box over SSH — doesn't apply, so a GUI loses nothing.

**Honest caveat:** GPUI's Linux/Wayland support is younger than its macOS support and its
API is unstable/thinly-documented. A spike (window + list + text input + monospace grid)
on real Wayland hardware gates the medium before feature work. The scope model and store
(below) are medium-agnostic and survive even a negative spike.

## 3. Scope model — git-centric, layered (the core idea)

Modeled on Claude Code's `~/.claude` + repo `.claude`:

- **Global layer** — `~/.local/share/sid` (redb). Always loaded. The "everything forever,
  one place, never lost" registry (MobaXterm's strength).
- **Workspace layer** — a repo's `.sid/`, **committed text** files (TOML/JSON — *never*
  redb, which is un-diffable/un-mergeable). Travels with the clone (Bruno's strength).
  **Overlays** global when that workspace is focused; **workspace shadows global** on key
  collision.
- **Secrets** — never committed. Kept in the OS keyring, referenced by an opaque id from
  the committed config (the existing `SecretStore`/`SecretId` already fits).
- **Single process.** "Focus a workspace" swaps the active overlay — it does **not** spawn
  a second instance. (This supersedes the POC's detach/IPC primitive.)
- **New-item default home:** prompt `save to: workspace | global`, with a configurable
  `default_scope` (`ask | workspace | global`); promote/demote between layers is one action.
- Identity-level prefs (theme, keybinds) are **always global**, never layered.

## 4. Layout (theming deferred)

Decided via wireframe (`docs/mockups/sid-mockup.html`). The rule: **navigation and
selection go on the top axis; content gets all horizontal width** (horizontal is the scarce
axis; the data-dense views need it).

- **Titlebar = context axis:** brand + scope switcher (Global / workspaces).
- **Tab strip = function axis:** SSH/SFTP · Database · Network · Workspaces · System · Settings.
- **Content = full width.** Per-tab pickers also go top (DB uses a connection dropdown, not
  a left tree); list panels (SSH host list) collapse to free width.
- Open question deferred to build: scope switcher must become a dropdown past a handful of
  workspaces (segmented control won't scale).

## 5. Code disposition (from the POC)

- **Tier 1 — salvage ~intact (integration adapters):** `sid-ssh` (russh/PTY/SFTP), `sid-git`
  (git2), `sid-secrets` (keyring), `sid-job`. Where the real weeks went; medium- and
  data-model-agnostic. Copied in as each slice needs them.
- **Tier 2 — re-found (domain + store):** the store becomes the layered global+workspace
  architecture; domain types gain a scope concept. The genuinely-new work — needed the
  moment we chose git-centric scope, regardless of medium.
- **Tier 3 — retire (TUI frontend):** `sid-widgets`, `sid-ui`, the wire layer, crossterm
  `Event`, ratatui. Replaced by GPUI. Ported by *reading* the POC, not running it.

## 6. Cross-platform

Wayland/Linux now. GNOME, Windows, macOS later, accommodated via the adapter seams
(keyring, PTY, editor-launch, clipboard, notifications) — empty trait slots now, not solved.

## 7. Out of scope now

Rebuilding the MCP server, the `.claude` plugin (skills/agents/hooks), and heavy CI —
deferred until the surface stabilizes. Other tabs wait until SSH/SFTP is load-bearing.
