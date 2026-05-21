# Settings reference

The Settings tab is sid's user-facing configuration editor. Every knob lives in
the redb database; the **only** filesystem config file sid reads is the optional
one-line `~/.config/sid/sid.toml` for overriding the database path.

This document is the long-form reference. For a quick tour, see the README.

## In-app navigation

Press `Ctrl+,` to focus the Settings tab. Inside:

| Key | Action |
|:---|:---|
| `Tab` / `Shift+Tab` | Cycle the focused category |
| `Enter` | Activate the focused row (apply theme / begin capture / open confirm) |
| `Esc` | Cancel an in-progress action |
| `↑` / `↓` (or `k` / `j`) | Move focus within a category |
| `→` / `←` | Cycle a focused value (booleans, choices, integers) |
| `Home` / `End` | Jump to first / last row |

Categories appear in this order:

1. **Theme** — live-preview picker. `cosmos`, `void`, `dusk`, `cosmos-light` are built-in; user-authored themes are merged from the `themes` table.
2. **Keybinds** — list of registered actions with their currently-bound chord. Pressing `Enter` enters capture mode; press any chord. If the chord is already bound, sid asks before overwriting. Re-binding `app.quit` is allowed but emits a visible warning.
3. **Behavior** — booleans, choices, and bounded integers (auto-restore session, auto-scan workspaces, persist debounce, heartbeat interval, default tab on launch).
4. **Workspace roots** — list of absolute directories the workspace discovery walker scans on startup. Tilde-prefixed paths (`~/vcs`) expand from `$HOME` at add time.
5. **Quick actions** — global commands surfaced in the System tab and the command palette. Each has an id, label, command, optional chord, and scope.
6. **DB path** — read-only display of the active DB path plus an editor for the `db_path_override` value in `sid.toml`. A change here takes effect on the next launch.
7. **Reset to defaults** — confirm-modal that clears the canonical setting keys. Does not touch user data (`themes`, `keybinds`, `quick_actions`, `workspaces` tables survive).

## Setting keys

Canonical names live in `sid_store::settings_keys`:

| Key | Type | Default | Effect |
|:---|:---|:---|:---|
| `theme_name` | string | `"cosmos"` | Name of the active theme. Resolved against the merged theme registry on startup; falls back to `cosmos` with a warning if unknown. |
| `keybind_profile_name` | string | `"cosmos"` | Name of the active keybind profile in the `keybinds` table. First run seeds a `"cosmos"` profile from `KeybindMap::cosmos_default()`. |
| `workspace_roots` | JSON array of paths | `["~/vcs"]` if present | Roots passed to the workspace discovery walker. Persisted as a JSON-encoded `Vec<PathBuf>`. |
| `persist_debounce_ms` | u64 (50..=5000, step 10) | `250` | Debounce window for `StatePersister` flushes. |
| `heartbeat_interval_secs` | u64 (1..=300) | `5` | Detached-session heartbeat cadence. |
| `auto_restore_session` | choice `yes`/`ask`/`no` | `ask` | Whether the previous session is restored on launch. |
| `auto_scan_workspaces` | bool | `true` | Whether workspace discovery runs on startup. |
| `default_tab` | choice (one of the six tab ids) | `workspaces` | Tab to land on when launched without an explicit `--start-tab`. |
| `settings_focused_category` | string id | — | Internal; remembers which Settings sub-view was last focused. |

Out-of-range integers are clamped to the valid bounds when loaded.

## Reset-to-defaults

`Reset` clears these keys (next read falls back to compiled-in defaults):

- `theme_name`
- `keybind_profile_name`
- `workspace_roots`
- `persist_debounce_ms`
- `heartbeat_interval_secs`
- `auto_restore_session`
- `auto_scan_workspaces`
- `default_tab`
- `settings_focused_category`

**Reset does not** delete records in the `themes`, `keybinds`, `quick_actions`, or `workspaces` tables. Reset is idempotent — a second invocation is a no-op.

## `sid.toml`

The optional config file lives at `~/.config/sid/sid.toml` (XDG-rooted). The only key sid reads is:

```toml
db_path_override = "/custom/path/to/sid.redb"
```

Tilde-prefixed paths are expanded against `$HOME` at launch (not at write time — the file stores the literal string the user typed). Unknown TOML keys are silently ignored, so the file is forward-compatible.

## CLI surface

`sid settings` is the scripted equivalent of the in-app Settings tab.

```sh
sid settings list                       # every key, in lexicographic order
sid settings get <key>                  # one value; exits non-zero if unset
sid settings set <key> <value>          # write a UTF-8 value
sid settings delete <key>               # idempotent delete; prints status
```

All four subcommands open the redb database read-only (`get`/`list`) or read-write (`set`/`delete`) and exit without launching the TUI. They respect `--db <path>` and the `sid.toml` override.

Values are stored as raw UTF-8 bytes — booleans as `"true"`/`"false"`, integers as decimal text, choices as the literal option string. The CLI does not validate values; the in-app editor is the place to use the typed bounds.

## Caveats

- The keybind editor's chord encoding is `"{KeyCode:?}|{u8 mod bits}"` (e.g. `"Char('q')|2"` for `Ctrl+Q`). This is human-inspectable but brittle against changes in `crossterm`'s `Debug` impl; a v2 hardening will introduce a stable serializer with a migration step.
- Rebinding `app.quit` is allowed but generates a `dangerous_action_warnings` entry that the UI surfaces as a toast.
- DB path changes do **not** migrate the existing redb file — sid simply opens the new path on the next launch. Copy the file yourself if you want to move data.
