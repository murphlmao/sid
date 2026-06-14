# System Tab — Config File Editing + Editor Preference — Design

**Date:** 2026-06-13  **Owner ask:** can't edit config files in the System tab; want to, with a chosen editor.

## Decisions (owner)
- Settings option chooses how editing happens: **editor = `nano` (default) | `vim` | `vi`**, OR **"spawn a new terminal"** that opens the file.
- Spawn behavior: `cd` into the file's parent directory, then open it.
- sudo: NOT handled by sid. If the file needs root, the user does it in the spawned terminal. (For inline editors this means read-only when not permitted — steer sudo cases to the spawn option.)

## Current state (to confirm during impl)
- System tab "config area" = pinned configs (paths) listed in `sid-widgets` system tab; backed by `list_pinned_configs` in the store. Currently view-only.
- `TerminalSpawner` adapter already exists (`sid_core::adapters::...`; `NoopTerminalSpawner` in tests, a real spawner in prod) — reuse for the spawn-terminal mode.

## Design

### Setting: `EditorChoice`
- New persisted setting key `config_editor` (string). Values: `nano` (default) | `vim` | `vi` | `terminal`.
- Surfaced in Settings (Behavior toggles or a dedicated row) as a Choice.
- Type `EditorChoice` lives in `sid-core` (plain enum, serde), consumed by the binary.

### Edit action (System tab, e.g. `e` / Enter on a config row)
The widget emits an `OpenConfigEditor { path }` action (no external crate names in the widget). The binary handles it:

- **Inline editor** (`nano`/`vim`/`vi`): suspend the TUI and run the editor in the current terminal:
  1. Leave alternate screen, disable raw mode, disable mouse capture, pop keyboard-enhancement flags.
  2. `Command::new(editor).arg(file_name).current_dir(parent_dir)` — spawn, `.wait()`.
  3. Re-enter alternate screen, raw mode, mouse capture; force a full redraw.
  This is the standard ratatui "shell out" pattern; must restore the terminal even if the editor errors (RAII guard).
- **Spawn terminal** (`terminal`): use `TerminalSpawner` to launch a new terminal emulator whose working dir is `parent_dir`, running `<resolved-editor-or-$EDITOR> <file>`. Terminal command resolution: a `terminal_command` setting (default: auto-detect via `$TERMINAL`, then a small candidate list — e.g. `wezterm,kitty,alacritty,foot,gnome-terminal,xterm`). Document the candidate order.

### Editor command resolution
- For inline: map `EditorChoice` → binary name; confirm on PATH, else clear error (logged).
- For terminal: build `cd <parent> && exec <editor> <file>` (or the spawner's API for cwd + argv).

## Tests (scoped: `cargo test -p sid editor`, `-p sid-widgets system`, `-p sid-core`)
- `EditorChoice` round-trips; default is `nano`.
- editor command construction per choice (argv + cwd = parent dir); missing-binary error.
- terminal-spawn path builds the right spawner call (use a recording mock spawner; assert cwd + argv); auto-detect candidate order.
- System-tab edit action emits `OpenConfigEditor { path }`.
- Terminal suspend/resume: factor the suspend/run/restore into a testable unit that takes an injected "run" closure so the restore-even-on-error invariant is unit-tested without a real editor.

## Out of scope
- In-app text editing (we shell out). Privilege escalation. Remote-file editing.
