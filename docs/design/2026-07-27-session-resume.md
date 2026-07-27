# Resume point — 2026-07-27 (paused mid-wave-4)

**`main` is at `a0935da`, pushed, gate-green** (47 suites, clippy `-D warnings`, fmt all clean).
Nothing is half-merged; the working tree is clean. Everything below is additive work that
was in flight when the session paused.

## How to resume in one paragraph

Five agents were paused mid-task; each wrote a handoff doc (paths below) and committed or
WIP-committed onto its own branch in its own worktree. To pick any one up: read its handoff,
`cd` to its worktree, continue, gate, then `git merge --no-edit <branch>` into `main`,
re-gate on the merged tree, and push. Merge one at a time — several touch adjacent lines in
`crates/sid-ui/src/lib.rs` (module + re-export lists) and `crates/sid/src/app.rs`; those
conflicts are mechanical unions.

## Paused work

| Topic | Branch / worktree | Handoff doc |
|:--|:--|:--|
| Scroll-lag perf (Network/System, release build) + FillTable self-scheduling | see handoff | `scratchpad/handoff-scroll-perf.md` |
| Sudo-elevated config file read/write (`PrivilegedFs` port + adapter) | see handoff | `scratchpad/handoff-sudo-privfs.md` |
| Modals + Toast onto sid-ui (host form, DB conn form, password prompt) | see handoff | `scratchpad/handoff-modals.md` |
| Settings → Keymap rebinding UI (+ persistence) | see handoff | `scratchpad/handoff-keymap.md` |
| DB increment-3 (sortable/filterable grid, EXPLAIN, redb browse) + Export/Run button sizing | see handoff | `scratchpad/handoff-db-inc3.md` |

Scratchpad root: `/tmp/claude-1000/-home-murphy-vcs-sid/f354a0cd-6c84-4e2f-a63f-dfdaec88f10c/scratchpad/`
(also holds every capture PNG referenced in the handoffs). Copy anything worth keeping into
`docs/` before `/tmp` is cleared.

## Shipped today (all on `origin/main`)

GPU preflight (`3c9243b`) · `sid-ui` crate: theme bridge, Button/IconButton/Badge/Kbd/Card/
Toolbar/EmptyState/SegmentedControl/Meter/StatCluster/ActionCell/List/Row/ScopeChip/StatusDot/
CardGrid, dev gallery · fill-width table model (`Fixed|Min|Grow`) + full-header sort ·
**all six tabs migrated**: System (segmented sub-views, working two-step kill), SSH home
(card-grid dashboard), Database, Network, Workspaces, Settings/chrome · responsive
drag-resizable SFTP sidebar (280–480px, terminal floor wins) · semantic type scale
(3 sizes / 2 weights / 1 mono) enforced by a hygiene test · all 8 bug-hunt findings ·
System-tab tick perf (6 frames → 1; release CPU 10.5% → 4.7%) · text-overflow class fix ·
navbar-shift fix.

## Open queue (see the task board)

1. **Overflow sweep wave 2** — ~12 mapped sites in `app.rs`, `db_tab.rs`, `config_editor.rs`,
   and `sid-ui` (`gallery`, `toolbar`, `segmented`, `badge`, `empty_state`, `card`,
   `table/state`). Also: ban `.truncate(` in `sid-ui/tests/hygiene.rs` — it is currently on
   the *allowed* list, which is how 17 sites of this bug survived.
2. **`sid-ui` polish** — FillTable quiescent-table fix (then delete the Workspaces 120ms
   workaround); `SID_PERF` should time the paint phase, not just element build; `ConfirmArm<K>`
   needs `Clone + PartialEq` (not `Copy`) for String-keyed rows; icon-registry gaps;
   `Button` trailing-icon slot; `Card::panel()` scrolling body; `error_line` into sid-ui.
3. **Delete `crates/sid/src/ui/text_input.rs`** (946 lines duplicating
   `gpui_component::input::Input`) — deliberately deferred because it touches every agent's
   files. The replacement must declare its own width rather than inherit one by percentage.
4. **Typography wave 2** — sweep the freshly-migrated tabs; the hygiene allowlist ratchet
   fails on dead entries, so each sweep must delete its own exemption.
5. **Harness** — `sid-cap.sh` has no `--dclick` (gpui's double-click window is 400ms, the
   script's gap is 400ms, so double-clicks never register; a working variant exists in the
   overflow agent's handoff) and `--key` chord injection no-ops entirely.
6. Narrow-window top bar (~620px) pushes scope chips and badges off the right edge.
7. `docs/HANDOFF.md` is badly stale — refresh it at the next gate.

## Landmines re-confirmed today

- gpui reports a text element's **min-content width as its full string width**, so `flex_1`
  alone never shrinks text — it overflows, and a centred parent spills it out *both* edges.
  Fix trio: `min_w(0)` + `clamp_one_line()` + `flex_none()` on the sibling that must not grow.
- gpui's `truncate()` is broken (its `Nowrap` pins the measured-layout cache's `wrap_width`
  to `None`, so the ellipsis pass never runs). Use `sid_ui::StyledExt::clamp_one_line()`.
- `h_flex()` centres on the cross axis: a `flex_1` table beside a sized sibling resolves to
  **zero height** (header paints, no rows). Use `div().flex().flex_row().min_h(0)`.
- `cargo build | tail` hides the exit code — check `$pipestatus`.
- postcard is positional: `#[serde(default)]` only, no `skip_serializing_if`.
