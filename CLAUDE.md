# CLAUDE.md — sid (GPUI rebuild)

This is the lean, load-bearing ruleset for the rebuilt `sid`. The TUI proof-of-concept
is archived at `murphlmao/sid-poc` — crib adapters and view logic from it, but do not
carry over its scaffolding wholesale. Design source of truth: `docs/design/`.

## What sid is

An integrated developer **ops-cockpit** (SSH/SFTP, Database, Network core), a native
**GPUI** desktop app, run locally to reach out at everything else. Built in **vertical
slices**: one tab is taken all the way to daily-use quality before the next is started.
The **SSH/SFTP** tab is the spearhead.

## Architecture rules (binding)

1. **Adapter pattern everywhere.** Every external library and every OS integration
   hides behind a trait owned by the core. Concrete impls live in their own crates.
   - GPUI may be named **only** in frontend crates (the rendering surface).
   - OS-integration points — keyring, PTY, editor-launch, clipboard, notifications —
     are traits with a Linux/Wayland impl now; other platforms are empty slots.
   - The point is swappability: features connect and reconnect through generic
     interfaces. If a concrete crate name leaks into core/domain, that's a bug.

2. **Layered scope is the core data invariant.**
   - A **global** store (always loaded; "nothing lost"; redb).
   - A **per-workspace** committed `.sid/config.toml` (git-diffable — never redb),
     attributed to that workspace.
   - **Composition is attributive — never override.** A read is the union of global +
     workspace, each tagged by origin; nothing is shadowed or lost. The default view
     collapses true duplicates (workspace wins) with an opt-in "hide global" filter —
     view checkboxes over a lossless store. redb values are `postcard`; the file is TOML.
   - **Secrets are never committed** — they live in the OS keyring, referenced by an
     opaque id from the committed config.
   - **Single process.** Focusing a workspace swaps the active workspace scope; it does not launch a
     second instance.
   - New items prompt for `save to: workspace | global`, with a configurable
     `default_scope`.

3. **Cross-platform is accommodated, not solved.** Wayland/Linux now. Keep the seams;
   don't write Mac/Windows code yet.

## Testing (pragmatic mode)

Targeted tests per feature; one gate review at the end of a slice — not per-commit
rigor. Critical paths (the layered store, scope composition, secret handling) still
get real tests. A rendering spike is gated by observation, not unit tests; that is
correct, not a shortcut.

## Deferred — do NOT rebuild yet

The MCP server, the `.claude` plugin (skills/agents/hooks), and heavy CI are deferred
until the surface stabilizes. Don't scaffold them. When they come back, they get their
own design pass.
