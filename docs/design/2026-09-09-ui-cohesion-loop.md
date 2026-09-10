# UI cohesion loop — checklist

**Date:** 2026-09-09
**Status:** proposal, awaiting go. Once approved this file is the loop's source of truth:
each iteration takes the first unchecked item, works it on its own branch, gates it
(build + clippy + tests + four-theme captures), merges to `main`, and ticks the box.
**Evidence:** hermetic captures of all six tabs plus the `SID_GALLERY=1` gallery, taken
today with `scripts/sid-shot.sh` (two of seven captures grabbed the wrong output, see §0).

## Where the project actually stands

The July UI overhaul plan (`2026-07-26-ui-overhaul-plan.md`) is mostly executed: `sid-ui`
exists with ~30 modules (buttons, badges, cards, toolbar, tables with fill-width columns,
modal, toast, inputs, type scale, theme bridge), all six tabs sit on it, and the gallery
renders coherently. `docs/HANDOFF.md` is two months stale and describes a much earlier state.
The resume doc `2026-07-27-session-resume.md` is the accurate one.

What is left is not "the UI is broken" but "the UI is a set of correct panels that do not yet
read as one application". The gaps below are visible in today's captures.

## 0. Prerequisites (Murphy, once)

- [ ] `sudo pacman -S sway wtype` — enables `scripts/sid-cap.sh`, the lock-proof headless
      capture and input harness. Without it the loop can only use `sid-shot.sh`, which
      captures the live Hyprland session and today returned the wrong window for two of
      seven shots. Self-verification of visual work depends on this.
- [ ] Optional: a GitHub token with `workflow` scope so `docs/ci/github-actions-ci.yml`
      can move to `.github/workflows/` (blocked since July).

## 1. GPUI base: keep it, but move off the frozen pin

**Verdict on the framework question:** gpui is the right base. The "weird to design
components in" pain was the missing component layer, and that layer now exists. The
alternatives (iced, egui, Slint, a webview) would be a rewrite of ~30k lines of view code
for a worse text pipeline or a non-native surface. The real structural problem is different:
`gpui = "0.2.2"` is the newest crates.io release and it is eleven months old (2025-10-22).
Zed has moved far past it, and `gpui-component` has moved with Zed.

A registry-only upgrade path now exists:

| Crate | sid today | Available | Note |
|:--|:--|:--|:--|
| `gpui` | 0.2.2 | `gpui-pre` 0.3.4 (2026-09-07) | published snapshot of zed `6916400`; `gpui_platform` split → `gpui-pre-platform` |
| `gpui-component` | 0.5.1 | 0.6.1 (2026-09-09) | depends on `gpui-pre ^0.3.1`; crate split into `gpui-base` / `gpui-kit` / assets |
| `gpui-component-assets` | 0.5.1 | replaced by `gpui-kit-assets` 0.6.1 | icon registry path may change |

- [x] **Upgrade spike** — DONE 2026-09-09, merged to main. gpui-pre 0.3.4 + gpui-pre-platform
      + gpui-component 0.6.1 + gpui-kit-assets 0.6.1. 29 files, ~50 min. Churn: `Window::focus`
      takes `&mut App`, `Line::paint` takes `TextAlign`, `Table` → `DataTable`, `InputState` split
      (`EditorState` for code editors), `Style::text` no longer `Option`, `flex_grow(f)`. Zero of
      the ~450 `div()` sites changed. One regression caught by A/B capture and fixed in
      `sid-ui` `Button`: 0.6 draws a `Custom` variant's rest fill at 20% alpha and drops its
      border. Original spec: bump to
      `gpui-pre` 0.3.4 + `gpui-pre-platform` + `gpui-component` 0.6.1, fix the entry point
      (`Application::new()` becomes the platform crate's application constructor), the
      `sid-ui` theme bridge (`ThemeColor` field set), the Table/Input/menu/tooltip call
      sites (16 direct `gpui_component::` uses in `sid`), and whatever `Styled`/element API
      churn hits the ~450 `div()` sites. Gate: clean build, clippy, tests, gallery + six
      tabs captured in all four themes with no regressions. Merge if green.
- [x] (moot: the spike converged.)
- [x] Landmines re-tested against gpui-pre 0.3.4 source: `truncate()` is FIXED upstream
      (`text.rs` caches `truncate_width`; `TruncateStart`/`TruncateMiddle` added); text
      min-content width, `h_flex()` zero height, and `Window::refresh` mid-draw are UNCHANGED,
      keep those workarounds. `Column.width` is still `Pixels`-only in 0.6.1, so `FillTable` stays.
- [ ] Follow-up: retire `clamp_one_line()` (~40 call sites) in favour of gpui's fixed
      `truncate()`, after a targeted render check on the longest strings (workspace paths,
      IPv6 addresses, DB connection paths) in all four themes.
- [ ] Follow-up: `gpui-kit-assets` 0.6.1 ships all 1830 Lucide icons (0.5.1 had 86). Give
      `Icon::Trash` a bin (not `circle-x`), `Icon::Rename` a pencil (not `replace`), and real
      glyphs to Database Run/Export and Network's Docker/Kubernetes/Interfaces sub-views. Each
      is a decision about a live screen; do them in the cohesion pass for that tab.

Recommended order: spike **first**. Doing the visual passes on 0.2.2 and migrating afterwards
means verifying every screen twice.

## 2. Visual cohesion (the main loop body)

Every item is gated by before/after captures in all four themes and a
`interface-design:design-review` pass on the touched screen.

- [x] **App-wide status bar.** Done 2026-09-09: `sid_ui::StatusBar`/`StatusItem`, 26px
      `surface` strip. Left: keyring state in words (click opens the old popover; the
      top-right `!` badge is gone), `N ssh sessions`, `db: <name>` with its dot. Right: zoom
      percent when ≠100% (click resets), last-frame ms under `SID_PERF`. Terminal pane
      reflows by test (`a_shorter_pane_reflows_to_fewer_rows_and_the_same_columns`).
      `sid-cap.sh` gained `--scroll`.
- [ ] **One panel vocabulary.** Today Database uses bordered, rounded, headed panels; SSH
      home floats cards on the bare background; Workspaces uses a hairline sidebar; System
      has an unframed meter strip. Pick the Database treatment (surface fill, hairline
      border, uppercase label header with count and actions) and apply it to every region on
      every tab.
- [ ] **One toolbar contract.** Every tab's top row becomes `[label · count] [filter]
      [secondary actions] [primary action]` at one height. Today SSH has a legend strip and
      an add button, Database has a status string plus filter plus three buttons, Workspaces
      has count plus refresh plus add. Same order, same height, same gaps everywhere.
- [ ] **Wide-window layouts.** Settings is an 880px centered column in a 2000px window with
      dead margins on both sides. Move to a left settings nav plus content pane, or at least
      left-align to the content gutter. SSH home: two ~340px cards then a void. Cards fill a
      responsive grid, and the home surface gets a second region (recent sessions or
      per-host last-connected) so the screen does not read as empty when it has data.
- [ ] **SSH session strip.** The `home  +` strip under the top bar is a row of tiny chips
      that looks unfinished. Make it a proper tab bar with the same height and active
      treatment as the top tabs, or fold it into the panel header.
- [ ] **Scope switcher.** `Global` and `acme-api (demo)` in the top-right read as two
      unrelated chips. Render them as one segmented scope control with the active scope
      filled.
- [x] **System tab.** Done 2026-09-09: meters were already on a framed `StatCluster`/`Card`
      (verified by capture); the Command column now falls back to the process name in
      `muted` ink (`ProcessInfo::cmd_is_fallback`, decided in `sid-sysinfo`).
- [ ] **Icon-only buttons.** File and gear on SSH cards, diagram and delete on Database
      connections, git and close on Workspaces rows: same size, same hit area, tooltip
      present on every one (the July plan made tooltips type-required; verify no call site
      bypasses it).
- [ ] **Interaction states.** Hover, pressed, focused, disabled on every control, and a
      visible focus ring for keyboard navigation (this is a keyboard-first app with no
      visible focus indication in any capture). The gallery has a `BUTTON STATES` section;
      make the app match it.
- [ ] **Empty states.** Database and Workspaces have the icon + headline + action pattern.
      SSH home has none for zero hosts, and the quick-connect field is the only thing on
      screen. Add the same pattern.
- [ ] **cosmos-light pass.** Capture every tab in the light theme and fix contrast, borders
      that vanish, and any token that only works on dark.
- [ ] **Final gate.** Six tabs × four themes, gallery, `tests/hygiene.rs` green, and no tab
      module naming `gpui_component` directly.

## 3. Open GitHub issues

| # | Title | State | Decision already made |
|:--|:--|:--|:--|
| 1 | init error (GPU preflight panic) | **fixed** in July, still open on GitHub | close it |
| 2 | quick connect: add connection from the field | **merged** (`p2-quick-connect`, 2026-09-09) | inline `add user@host…` row opens the prefilled add form; Enter on an unknown host opens that form instead of dialling; scan `~/.ssh` for keys, prefer `id_ed25519` then `id_rsa`; when agent auth is chosen and `SSH_AUTH_SOCK` is unset, say so and offer key auth |
| 3 | std ctrl operations in fields (ctrl+backspace, ctrl+shift+arrows, Tab) | `sid-ui` `TextInput` shipped with these; **old `ui/text_input.rs` still used by `host_form`, `db_conn_form`, `app.rs`** | finish the migration, then verify in every form |
| 4 | ctrl `+` / `-` zoom | **merged** (`p4-zoom`, 2026-09-09) | one scale factor for the whole UI including the terminal grid (reflows cells), persisted in `Settings`, clamped 50–200%, ctrl+0 resets |

- [ ] Close #1 (needs a GitHub token or Murphy; the GitHub MCP connector failed to
      authenticate this session and `gh` is not installed).
- [ ] #3: migrate the remaining call sites to `sid_ui` inputs, then **delete
      `crates/sid/src/ui/text_input.rs`** (959 lines duplicating `gpui_component::input`).
- [x] #2 as decided above (merged 2026-09-09; follow-up: `app.rs:221-224`/`566` doc comments still describe the retired ephemeral-dial case).
- [x] #4 as decided above (merged 2026-09-09; live-verified: `stty size` 56x113 at 100%,
      35x75 at 150% over the docker sshd fixture; `SID_UI_SCALE` env override for captures).
- [ ] #4 follow-up: the SFTP sidebar's own pixel arithmetic (`sidebar_metrics::MIN/MAX/
      TERMINAL_MIN`, `plan_entry_row`'s 318/406px thresholds) is still authored at 100% against
      an already-zoomed viewport, so size/date columns truncate at 150%. Same fix class as
      table columns and modal geometry: scale the declarations, thread `UiScale` through
      `sidebar_width`/`plan_entry_row`.

## 4. Backlog carried from the July resume doc

- [ ] Table frame cost: ~26% of cell builds are discarded by gpui each frame
      (`docs/design/2026-07-27-table-frame-cost.md`).
- [x] `scripts/lib/sid-app.sh` extracted (92 lines shared); `sid-shot.sh` now verifies the
      sid window sits on its headless output before `grim` and checks the PNG dimensions
      afterwards, refusing with a one-line reason otherwise. Merged 2026-09-09.
- [ ] `sid-shot.sh` cannot capture at all on Murphy's Hyprland (Lua parser rejects
      `hyprctl keyword` and legacy two-arg `dispatch`, exit 0); it now fails safely. Either
      port its placement to `hyprctl eval`/new-syntax `dispatch` around workspace 4, or
      delete it and make `sid-cap.sh` the only harness. Low priority: `sid-cap.sh` is what
      the loop uses.
- [ ] Rewrite `docs/HANDOFF.md` from the current tree. It still describes July 6.
- [ ] CI: `docs/ci/github-actions-ci.yml` → `.github/workflows/ci.yml` (blocked on token scope).

## Deliberately not on the list

- Motion and transitions. gpui can animate, but nothing here needs it yet.
- A left icon rail instead of top tabs. The top bar works; changing the navigation model
  is a design decision for Murphy, not a cohesion fix.
- Mac/Windows. Seams stay, no code (CLAUDE.md rule 3).
- MCP server, `.claude` plugin, heavy CI (CLAUDE.md deferred list).

## 5. Branches left over from earlier agent worktrees

Found on `origin` on 2026-09-09, no local worktrees remained. Recreated as named branches
under `~/vcs/sid-wt/` (never under `/tmp`: it is a 16 GB tmpfs and a cargo target dir
there evicts RAM).

| Branch (origin) | Local branch / worktree | State | Action |
|:--|:--|:--|:--|
| `worktree-agent-ae741e579535ab9ca` | `p2-quick-connect` at `~/vcs/sid-wt/p2-quick-connect` | 2 commits, 0 behind main, `cargo check --tests` clean; implements #2 (add-from-quick-connect, key scan port + `~/.ssh` adapter, agent-gap messages) | gate, review, merge |
| `worktree-agent-aafbb83305bc45d0b` | `p4-zoom` at `~/vcs/sid-wt/p4-zoom` | 4 commits, 0 behind main, **does not compile** (`UiScale` not imported at 2 sites, `SshSession::set_ui_scale` missing); has the ladder math, Settings v5 persistence, rem-based type scale, table/modal scaling | finish, gate, merge |
| `worktree-agent-a0dc992fc7f17e063` | none | 3 commits, 55 behind; a `sid-privileged` crate that main superseded with `sid-privfs` (c33d329, 8099059, 3c46099) | nothing to merge; delete the remote branch |

- [x] `p2-quick-connect`: gate + review + merge to main (fda1893, fast-forward, 2026-09-09).
- [x] `p4-zoom`: finished, gated on the merged tree (51 suites, 1561 tests, clippy clean),
      merged to main 2026-09-09.
- [x] Deleted `origin/worktree-agent-a0dc992fc7f17e063` (superseded) and the two merged
      `worktree-agent-*` branches.

## How to run the loop

Each iteration:

1. Read this file. Take the first unchecked item outside §0 (§0 is Murphy's). §5 comes
   before §1; otherwise top to bottom.
2. Work it on its own branch in a worktree under `~/vcs/sid-wt/<branch>`. Never leave a
   worktree with uncommitted changes at the end of an iteration.
3. Gate before merging: `cargo fmt --check`, `cargo clippy --workspace --all-targets --
   -D warnings`, `cargo test --workspace`. Visual items additionally need before/after
   captures: `scripts/sid-cap.sh` when `sway` is installed, otherwise `scripts/sid-shot.sh`
   and open the PNG to confirm it shows sid and not another window.
4. Merge to `main`, push `main`, tick the box here, add one line to the Log below.
5. If an item is blocked on Murphy, write the blocker under Blocked and move on.

Subagent policy (Murphy's instruction, 2026-09-09): at most **5 subagents at a time**.
Route by difficulty, not by habit:

| Model | Use for |
|:--|:--|
| haiku | mechanical work: running gates, grepping, inventories, capture runs, counting |
| sonnet | well-specified coding: migrations of call sites, harness script changes, tests, finishing work whose design is already written down |
| opus | work that needs design judgment or fights gpui: the upgrade spike, the terminal-reflow half of zoom, the cohesion passes per tab, the design-review fixes |
| fable | orchestration, reviewing diffs before they merge, and only the work that stalled twice at opus |

Commit messages follow the trailer convention already on `main` (check the last ten
commits; do not invent one).

## Blocked

- Closing GitHub #1 (fixed in July) and #2 (merged 3afd482) needs Murphy: `gh` is not
  installed and the GitHub MCP connector fails to authenticate. `gh issue close 1 2` or two clicks.
- Murphy's Hyprland rule for sid → workspace 4 is written to
  `~/dotfiles/config/hypr/hyprland.lua` (applied live via `hyprctl eval`) but is uncommitted
  in the dotfiles repo.

## Log

- 2026-09-09: checklist written; `p2-quick-connect` and `p4-zoom` worktrees recreated from
  origin; agents dispatched to gate/finish them.
- 2026-09-09: `p2-quick-connect` merged (merge commit 3afd482). Gate on the branch: fmt/clippy/tests green; review a–f all pass; zero new deps. Root cause of sid windows landing on Murphy's screen: `hyprctl keyword` is rejected by this Hyprland's Lua config parser ("keyword can't work with non-legacy parsers. Use eval."), so `sid-shot.sh`'s silent windowrule and headless-monitor keywords were failing behind `|| true`.
- 2026-09-09: sid windows now open on Murphy's workspace 4 silently (persistent
  `murphy_sid_capture` rule; probed: a hermetic launch landed on workspace 4 and the active
  workspace was untouched). `p2-quick-connect` merge pushed and its origin branch deleted.
  Harness agent re-briefed to drop `hyprctl keyword` and never fall back to on-screen
  capture; zoom agent re-briefed onto `sid-cap.sh`.
- 2026-09-09: `p4-zoom` merged (one import conflict in `ssh_home.rs` against the
  quick-connect merge, resolved as the union). Zoom rides `Window::set_rem_size`; the
  terminal grid re-shapes from `rem_size` each frame, no second channel. Also lands
  `fix(scripts): sid-cap's click support dies on a namespace package`, so `harness-lib`
  must merge main before it finishes.
- 2026-09-09: app-wide status bar landed on `status-bar` (`sid_ui::StatusBar`/`StatusItem`,
  wired under the active tab in `app.rs`). Left: the secrets backend in words (`keyring` /
  `secrets in memory`, clicking opens the detail the retired `!` badge opened), the open SSH
  session count (hidden at zero), and `db: <name>` with the same dot `connection_dot` gives
  its own row. Right: the zoom readout when it is not 100% (click = ctrl+0) and, under
  `SID_PERF`, the last frame's ms — read from a static the paint closure stores into, since a
  notify-per-frame from inside paint is an infinite render loop. The top-right `!` badge is
  gone (`sw` stays). Terminal reflow verified: `grid_size` measures the pane it is given, so
  26px less window is one row fewer, not a clipped one. Also `scripts/sid-cap.sh --scroll`,
  which is how the Settings capture proved a scrolling tab still reaches its last line.
- 2026-09-09: gpui upgrade merged (ff to 9e2300e). Gate on the merged tree: 51 suites, 1560
  tests (one retired icon-ratchet test), clippy clean; captures of SSH, Database, Network,
  gallery, 150% zoom and a real ctrl+= chord all match. Lockfile grew by the gpui-pre
  family, wgpu, accesskit and platform crates; 86 old entries dropped.
- 2026-09-09: `harness-lib` merged: `scripts/lib/sid-app.sh`, a pywayland namespace-package
  fix in `sid-cap.sh` (superset of the zoom branch's), `sid-shot.sh` placement verification.
  `sid-cap.sh` verified from main against the upgraded renderer (Workspaces capture).
  Note for Murphy: while probing the Lua `hyprctl eval` API the harness agent ran an
  untargeted `hl.dsp.window.close()` on the live session; it confirmed nothing closed.
- 2026-09-09: System tab, two defects (`system-tab` branch). Command column: 20 of 31 rows
  at 1272px were showing a bare `—` because another user's/kernel processes' cmdline is
  unreadable; `sid-sysinfo::processes::resolve_cmd` now falls back to the process name at
  the mapping layer (`ProcessInfo::cmd_is_fallback` carries the fact), and the Command cell
  renders that fallback in `muted` ink versus `fg` for a real argv, same Mono size either
  way. Meter cards: re-verified by capture rather than re-built — `overview_cluster` already
  puts CPU/Memory/Swap on a `StatCluster`/`Card` with a `SYSTEM` header, summary line and a
  labelled `16 cores` per-core strip, matching the Database panel's `Elevation::Surface`
  chrome exactly (same helper, not just the same look). Captures at 1272x900, 1920x1080
  (cosmos + cosmos-light) and 2560x1400 confirm both: no dash-only Command cells for named
  processes, fallback ink visibly dimmer, meters framed, table fills the width, nothing
  clipped.
