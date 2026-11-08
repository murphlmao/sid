# sid (app shell)

The GPUI frontend and composition root: window, all six tabs, the single `AppState`
entity. One of only two crates (with `sid-ui`) allowed to name GPUI.

## Surface
- `src/main.rs` — startup: `--gpu-probe`/`--gpu-report` argv modes, GPU pre-flight, `Store::open`,
  `open_secrets`, window + `gpui_component::Root`. The ONLY file here that may name `gpui_component`.
- `src/app.rs` — `AppState`, `Tab` (+`Tab::ALL`), `open_secrets`, `tab_from_str`, action dispatch.
- `src/keymap.rs` — `Action`, `ALL_ACTIONS`, `default_bindings()`, `REBINDABLE_KEYS`, `resolve_rebind`.
- `src/db_registry.rs` / `git_registry.rs` — factories for `DbClient` / `GitProvider`.
- `src/ssh_connect.rs`; `src/ui/*_tab.rs` per tab; `ui/session.rs` (SSH session, `ssh_runtime()`).

## Invariants
- Render from cache; I/O only in event handlers / spawned tasks, never in `render`.
- Build UI from `sid_ui` components and typography roles; no raw colours/`text_xs()`/`.truncate(`.
- Concrete adapters are constructed at composition points only (see `docs/ARCHITECTURE.md` seam table).
- Modals close on Esc and refocus `AppState::root_focus`.

## Test
`nice -n 10 cargo test -p sid -j 3` (large unit suite inside `app.rs`/`keymap.rs`/tabs).
Behaviour needs eyes: `scripts/sid-cap.sh --tab <tab> --out target/caps/x.png`.

## Landmines
- `h_flex()` centres cross axis: a `flex_1` table beside a sized sibling gets zero height — use
  `div().flex().flex_row().min_h(0)`.
- gpui text min-content = full string width: `min_w(0)` + `.clamp_one_line()` + `flex_none()` on siblings.
- `tab_group()` only renumbers; wrapping Tab needs `focus_trap`.
- Tests that write+exec a script hit ETXTBSY under parallelism — write scripts before spawning.
- SID_* env: `SID_START_TAB`, `SID_THEME`, `SID_UI_SCALE`, `SID_PERF`, `SID_GALLERY`.
