# Resume point — 2026-07-27

**`main` is gate-green** (51 suites, clippy `-D warnings`, fmt, all clean) and pushed.
Nothing is half-merged. Work still in flight lives on its own branch in its own worktree;
merge one at a time, re-gate on the merged tree, then push. Expect mechanical union
conflicts in `crates/sid-ui/src/lib.rs` (module + re-export lists), `crates/sid-ui/tests/hygiene.rs`
(the sweep allowlist) and `crates/sid/src/app.rs`.

## In flight

| Topic | Owns |
|:--|:--|
| `sid-ui` `TextInput`/`SearchInput` + polish list + **GitHub #3** (ctrl+backspace, ctrl+shift+arrows, Tab) | `crates/sid-ui/**` except `src/table/**` |
| Typography wave 2 + overflow — data tabs | `network_tab.rs`, `workspaces_tab.rs`, `db_tab.rs` |
| Typography wave 2 + overflow — chrome, session, forms (incl. the ~620px top-bar overflow) | `app.rs`, `session.rs`, `ssh_home.rs`, `command_palette.rs`, the three form files, `db_diagram.rs`, `config_editor.rs` |
| Table frame cost — the ~26% of cell builds gpui discards | `crates/sid-ui/src/table/**` |

## Queued

1. **GitHub #2** — quick-connect "add this host". DECIDED: an inline `add user@host…` row opens
   the add-connection form prefilled; pressing Enter on a host that does not exist must **say so
   and open that same prefilled form** rather than dialling ad-hoc (so every connection becomes a
   saved one); saving then connects. Key handling: scan `~/.ssh`, default `id_ed25519` → `id_rsa`,
   allow choosing or browsing; when agent auth is selected but `SSH_AUTH_SOCK` is unset, say that
   plainly and offer key auth instead of the raw error.
2. **GitHub #4** — ctrl `+`/`-` zoom. DECIDED: one scale factor for the whole UI *including* the
   SSH terminal (it reflows to fewer/more cells), persisted, clamped ~50–200%, ctrl+0 resets.
   Both #2 and #4 were blocked only by file ownership above.
3. **Delete `crates/sid/src/ui/text_input.rs`** (946 lines duplicating `gpui_component::input::Input`)
   once the sid-ui replacement lands, and migrate its ~15 call sites. The replacement must declare
   its own width rather than inherit one by percentage — that bug made the old widget render as a
   ~20px stub that silently ate clicks.
4. **Overflow sweep remainder** — whatever the two sweeps report as out of scope.
5. **Final gate** — six tabs × four themes captures, then refresh `docs/HANDOFF.md` (badly stale).

## Shipped (all on `origin/main`)

GPU preflight, and then **GitHub #1**: the diagnosis had no vendor awareness, so "Intel GPU +
only `radeon_icd.json`" fell through to "unrecognized reason — file a bug"; it now names both
sides and the exact package, conservatively (an unrecognized GPU vendor or unmapped manifest
silences the claim), and every diagnosis lists the hardware it saw · `sid-ui` component crate
(theme bridge, Button/IconButton/Badge/Kbd/Card/Toolbar/EmptyState/SegmentedControl/Meter/
StatCluster/ActionCell/List/Row/ScopeChip/StatusDot/CardGrid/Modal/Toast, dev gallery) ·
fill-width table model (`Fixed|Min|Grow`) + full-header sort · **all six tabs migrated** ·
SSH home card-grid dashboard · responsive drag-resizable SFTP sidebar · semantic type scale
(3 sizes / 2 weights / 1 mono) with a hygiene ratchet · keymap rebinding + persistence ·
sudo-elevated config editing, now with **no shell in the elevated path** (`head`/`cp`/`cp`/`mv`,
pinned by an argv-log test) · DB increment-3 · all 8 bug-hunt findings · System-tab tick perf
(6 frames → 1; release CPU 10.5% → 4.7%) · text-overflow class fix · navbar-shift fix ·
capture-harness fixes (below).

## Capture harness (`scripts/sid-cap.sh`) — corrected facts

- `--dclick X,Y` exists now. Two `--click`s never worked as a double-click: gpui's window is
  400ms and the script's inter-click gap defeated it.
- **`--key` was never broken.** The headless compositor has no input devices, so gpui binds
  `wl_keyboard` only once `wtype` attaches one — after the focus handoff — and never gets the
  `wl_keyboard::enter` it gates keystrokes on. A pointer **button** (motion is not enough) makes
  the compositor redo the handoff. The harness now clicks one inert pixel before the first
  `--key`/`--type` unless a pointer action already ran (`SID_CAP_FOCUS_CLICK` to move or disable).
  Any older "unverified keyboard flow" note should be re-checked.
- `--drag X1,Y1,X2,Y2[,STEPS]` exists (press → interpolated motions → release), pacing measured,
  not guessed. **Put `--drag` last**: the synthetic mouse-up does not clear
  `SshSession::on_sidebar_drag_up`, so a later pointer action in the same run re-drags the divider.
- The harness now builds `sid` by default and refuses to capture if the build fails; `--no-build`
  keeps the old behaviour behind a stale-binary banner. `cargo clippy` is check-only, so several
  agents had captured a stale binary.
- Pointer commands block on the driver's ack instead of a guessed sleep — this was silently
  dropping characters from `--type`.
- The DB relationships diagram opens as a second OS window that sway keeps behind the fullscreen
  main window, so `grim` never sees it. Verifying diagram drag needs an app or harness change.
- `sid-cap.sh` and `sid-shot.sh` duplicate ~60 lines (repo-root discovery, hermetic XDG, launch
  and poll, cleanup trap). A shared `scripts/lib/sid-app.sh` is the shape of the fix.

## Landmines

- gpui reports a text element's **min-content width as its full string width**, so `flex_1` alone
  never shrinks text — it overflows, and a centred parent spills it out *both* edges. Fix trio:
  `min_w(0)` + `clamp_one_line()` + `flex_none()` on the sibling that must not grow.
- gpui's `truncate()` is broken (`Nowrap` pins the measured-layout cache's `wrap_width` to `None`,
  so the ellipsis pass never runs). Use `sid_ui::StyledExt::clamp_one_line()`.
- `h_flex()` centres on the cross axis: a `flex_1` table beside a sized sibling resolves to **zero
  height** (header paints, no rows). Use `div().flex().flex_row().min_h(0)`.
- `FillTable` measures in prepaint and its widths land on the *next* layout pass; it now schedules
  that frame itself (`Window::on_next_frame`), because `Window::refresh` is a no-op mid-draw.
- Writing an executable and exec'ing it from concurrent tests hits `ETXTBSY` — a `fork` in another
  thread momentarily inherits the write descriptor. Write test scripts once, before any spawn.
- `cargo build | tail` hides the exit code — check `$pipestatus`.
- postcard is positional: `#[serde(default)]` only, never `skip_serializing_if`.
