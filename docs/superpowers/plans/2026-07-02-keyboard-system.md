# Keyboard-driven system (revised per Murphy, 2026-07-02)

**Dispatched AFTER the SSH shell rebuild** — session-tab cycling binds to that structure.
Mockup: `docs/mockups/2026-07-02-keyboard-driven.html` (concept approved), with these **revisions**.

## Revised principles (Murphy's call)
1. **Ctrl is the sole modifier.** sid is its own app — NO kitty-parity, NO "Shift-inside-terminal"
   scheme as the general rule. Plain `Ctrl+…` accelerators everywhere.
2. **`Ctrl+Tab` / `Ctrl+Shift+Tab`** = cycle forward/back — session tabs in the SSH shell, AND it
   also works to move between form fields (Murphy: "ctrl tab to go back in forms too"). Plain
   `Tab`/`Shift+Tab` field nav in forms stays (already shipped); `Ctrl+Tab` is the universal cycle.
3. Everything reachable via the command palette (`Ctrl+K`).
4. Rebindable later in Settings → Keymap (build the registry now; the Settings UI can follow).

## The ONE necessary exception (terminal focus)
Inside a **focused terminal**, `Ctrl+<letter>` are shell control codes (`Ctrl+C` SIGINT, `Ctrl+D`
EOF, `Ctrl+K` kill-line, `Ctrl+W` kill-word, `Ctrl+R` reverse-search, `Ctrl+L` clear, …). The
terminal MUST get first dibs on `Ctrl+<letter>` or the shell is broken. So:
- **Non-letter accelerators are global everywhere:** `Ctrl+1..5` (primary tabs), `Ctrl+Tab` /
  `Ctrl+Shift+Tab` (cycle). These don't collide with readline — safe even when the terminal is focused.
- **Letter accelerators** (`Ctrl+K` palette, `Ctrl+W` close tab, `Ctrl+T` new tab, …): when the
  terminal is focused they **pass through to the PTY**, and sid's action is available as
  `Ctrl+Shift+<letter>` in that context only. Everywhere else, plain `Ctrl+<letter>` works.
- This is recorded because it contradicts principle #1 in exactly one place — it's unavoidable for
  a working terminal. Flag to Murphy on delivery; easy to change if he prefers the terminal not
  intercept.

## Bindings (v1)
- **Global:** `Ctrl+K` command palette (fuzzy over actions + connections + open session tabs) ·
  `Ctrl+1..5` primary tabs (SSH/Database/Network/Workspaces/System) · `Ctrl+,` settings ·
  `?` cheat-sheet overlay (only when no text input is focused).
- **SSH shell:** `Ctrl+T` new session (→ home) · `Ctrl+W` close session · `Ctrl+Tab`/`Ctrl+Shift+Tab`
  next/prev session · calls the shell track's `new_session()`/`close_session()`/`activate_session()`.
- **Lists/tables (hosts, connections, ports, services, schema tree):** `↑/↓` or `j/k` move · `Enter`
  primary action · `F2` inline rename · `/` focus filter · `Esc` clear/close.
- **Database:** `Ctrl+Enter` run (exists) · `Ctrl+K`-reachable export/diagram actions.

## Architecture
- New `crates/sid/src/keymap.rs`: an `Action` enum + a binding registry (default map; later
  overridable from `Settings`), with conflict detection. Pure, unit-tested (binding→action lookup,
  conflict detection, the terminal-focus fallback resolution).
- New `crates/sid/src/ui/command_palette.rs`: a fuzzy overlay (reuse the `deferred`/`anchored`
  overlay pattern + `TextInput`), listing actions/connections/tabs with their bindings; Enter
  dispatches the `Action`.
- `app.rs`: a root-level key handler that maps a `Keystroke` (+ current focus context: terminal or
  not) → `Action` via the registry, then `dispatch_action(Action)`. Terminal-focus detection gates
  the letter-accelerator pass-through.
- GPUI note: prefer sid's own key dispatch (the `TextInput` Tab work already proved
  `on_key_down` + `stop_propagation` intercepts before the IME/PTY commit) over gpui global
  `KeyBinding`s if that keeps the terminal-passthrough logic in one place.

## Pragmatic TDD
Registry lookup, conflict detection, terminal-focus fallback resolution (pure). Palette rendering +
live dispatch observation-gated (verify via sid-shot/sid-click).
