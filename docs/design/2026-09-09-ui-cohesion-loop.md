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
- [x] Follow-up icons, done 2026-09-09: `Icon::Trash` → `trash`, `Icon::Rename` → `pencil`,
      new `Run`/`Export`/`Docker`/`Kubernetes`/`Interfaces` on Database buttons and the Network
      segmented control. Two wiring fixes were needed for any of it to render: the registry now
      resolves through `gpui_kit_assets::IconName` (the 1830-name catalog, not
      `gpui_component::IconName`'s 86-entry subset) and `main.rs` embeds
      `gpui_kit_assets::AllAssets`. Left for the SSH pass: the connection card's right-click
      menu (`PopupMenuItem`) still shows rename/delete without icons.

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
- [x] **One panel vocabulary.** Done 2026-09-09 across all six tabs: every region is a
      `Card::panel` (`surface`, hairline, uppercase Label header `NAME · count`, header
      actions). SSH home (connections), Database (connections/schema/history + `QUERY ·
      <connection>` + `RESULTS · n rows · ms` + `QUERY PLAN`), Network (one `sub_view_panel`
      for the five sub-views), Workspaces (`WORKSPACES · n` sidebar, OVERVIEW/BRANCHES/STATUS/
      LOG/REPOS detail panels), System (`SYSTEM` cluster), Settings (rail + panels).
- [x] **One toolbar contract.** Done 2026-09-09: the panel header IS the toolbar row,
      `[label · count] [filter] [secondary] [primary]` at one height; loose status strings
      ("no connection selected") left the toolbars. Two real bugs surfaced and fixed on the
      way: Network's fetch error had no render at all; Workspaces' unregister was a labelled
      button with an empty label (36px box beside a 24px square).
- [ ] **Panel/toolbar follow-ups** (from the data-tabs pass): `sid_ui::Toolbar` is now unused
      on the tabs because `Card::panel`'s header does its job with a different box (`py_2` vs
      `py(6.)`); unify the two or delete `Toolbar`. `db_tab.rs` and `network_tab.rs` keep
      file-private `error_line`/`caveat_line` copies of `sid_ui::notice`'s; swap after a wrap
      check. The Database `diagram` button wants an icon (`git-fork`/`workflow` in Lucide). A
      panel header's title is the only shrinkable slot, so at 700px the results filter sits at
      its 160px floor and `RESULTS` would truncate below that; decide whether filters may
      shrink further or move to a second row.
- [x] **Wide-window layouts: Settings.** Done 2026-09-09: 220px section rail (Appearance ·
      Behaviour · Keyboard · Storage) + panelled content left-aligned to the gutter; collapses
      to a `SegmentedControl` below ~900 design px; Behaviour's chip strips became
      `SegmentedControl`s, the keyring status an `InlineNotice`. `system.md` amended.
- [x] **Wide-window layouts: SSH home.** Done 2026-09-09: the connections surface is one
      `Card` panel (`CONNECTIONS · n`, header = the toolbar row `[label·count] [quick-connect
      filter] [add] [connect]`, legend in the footer). The void is now bounded by the panel and
      empty on purpose; `CardGrid` keeps its 300/340px column caps because stretching two cards
      across 2560px recreates the per-line-stretch defect `grid.rs` documents. No fabricated
      second region: the store has no per-host last-connected fact.
- [x] **Kbd chips lost their chrome under gpui-component 0.6.1**: fixed 2026-09-09.
      `gpui_component::Kbd` painted through the library's own unconfigured theme tokens;
      `sid-ui/src/kbd.rs` now draws its own chip (surface, hairline, `rounded_sm`, Meta ink) and
      keeps the library only as a key-name formatter. Verified in Settings → Keyboard, the
      command palette, the gallery (capture at 2000x2400 to reach the band) and cosmos-light.
- [x] **`InlineNotice` clamps to one line**: fixed 2026-09-09. Body wraps to two lines
      (`line_clamp(2)`), optional `.detail(..)` second line in Meta/muted; Settings splits the
      keyring message into sentence + recommendation. Verified at 1920 and 700px and in the
      gallery.
- [x] **Top bar clips at 700px**: fixed 2026-09-09. Below ~900 design px (same currency as the
      Settings rail breakpoint) the tabs collapse to icon-only with tooltips; verified on SSH
      and Database at 700x900 and at 150% zoom.
- [x] **SSH session strip.** Done 2026-09-09: same `chrome_tab` box as the top bar (height,
      accent underline, hover fill); each session tab shows StatusDot + saved alias + a
      hover-revealed close; `+` is a tooltipped IconButton. Still clamps when many.
- [x] **Scope switcher.** Done 2026-09-09: one `SegmentedControl` (recessed track, active
      scope filled); `ScopeChip` stays the per-item origin badge. All six top tabs gained
      icons.
- [x] **System tab.** Done 2026-09-09: meters were already on a framed `StatCluster`/`Card`
      (verified by capture); the Command column now falls back to the process name in
      `muted` ink (`ProcessInfo::cmd_is_fallback`, decided in `sid-sysinfo`).
- [x] **Icon-only buttons.** Done 2026-09-09 for SSH (cards + right-click menu icons),
      Database, Network, Workspaces: every `IconButton` is `.small()` (one 24px square) with a
      tooltip (type-required by the 3-arg constructor); `Tipped::tip` covers non-button
      elements. System's pin/kill controls were not re-audited in this pass; check them in the
      cosmos-light sweep.
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
| 3 | std ctrl operations in fields (ctrl+backspace, ctrl+shift+arrows, Tab) | **done** 2026-09-09: every field is a `sid-ui` input; old widget deleted | finish the migration, then verify in every form |
| 4 | ctrl `+` / `-` zoom | **merged** (`p4-zoom`, 2026-09-09) | one scale factor for the whole UI including the terminal grid (reflows cells), persisted in `Settings`, clamped 50–200%, ctrl+0 resets |

- [ ] Close #1 (needs a GitHub token or Murphy; the GitHub MCP connector failed to
      authenticate this session and `gh` is not installed).
- [x] #3 done 2026-09-09: twelve call sites (not three) migrated to `sid_ui` inputs across
      every tab, both forms, the palette, the password prompt and the config editor;
      `crates/sid/src/ui/text_input.rs` (971 lines) deleted, net −1049 lines. Masking, tab
      order, Enter/Esc, seed prefill and key chips verified by capture.
- [ ] Follow-up from #3: **modals do not trap focus.** `window.focus_next()` walks the
      window's flat tab-stop list, so Tab off a modal's last field can land on a background
      field (SSH home's search box, Database's result filter). Needs a focus-trap primitive in
      `sid-ui` (`Modal` owns a tab group and wraps at its ends); belongs with the interaction
      states pass.
- [x] #2 as decided above (merged 2026-09-09; follow-up: `app.rs:221-224`/`566` doc comments still describe the retired ephemeral-dial case).
- [x] #4 as decided above (merged 2026-09-09; live-verified: `stty size` 56x113 at 100%,
      35x75 at 150% over the docker sshd fixture; `SID_UI_SCALE` env override for captures).
- [x] #4 follow-up done 2026-09-09: `sidebar_width` and `plan_entry_row` take a `UiScale`
      read off `window.rem_size()`; thresholds scale before comparing; pinned by
      `zooming_in_does_not_move_the_column_decision_for_a_proportionally_wider_row`. Live
      150% capture over sshd not done (agent lost to the rate limit); unit-tested only.

## 4. Backlog carried from the July resume doc

- [ ] Table frame cost: ~26% of cell builds are discarded by gpui each frame
      (`docs/design/2026-07-27-table-frame-cost.md`). Started 2026-09-09 and dropped when the
      org spend limit hit; re-verify against gpui-pre 0.3.4 / `DataTable` before optimising.
      Lowest priority on this list: perf, not cohesion.
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
- 2026-09-09: Settings rebuilt as a left section rail (Appearance / Behaviour /
  Keyboard / Storage) beside a left-aligned, 880px-capped content pane of `surface`
  panels, collapsing to a `SegmentedControl` below ~900 design px. The three
  hand-rolled chip strips in Behaviour became `sid_ui::SegmentedControl`, the keyring
  status became an `InlineNotice` with the restart caveat moved under the control it
  applies to, and nav items are Tab-reachable via gpui's own tab-stop ring. Branch
  `settings-layout`, not merged.
- 2026-09-09: `icon-glyphs` branch (not merged): the `gpui-kit-assets` 0.6.1 bump made all
  1830 Lucide SVGs available, so `sid_ui::Icon` now resolves through the full catalog
  instead of `gpui-component`'s 86-icon compatibility subset. `Trash`/`Rename` draw a real
  bin/pencil instead of their `circle-x`/`replace` stand-ins; new `Run`/`Export` icons on
  the Database tab and `Docker`/`Kubernetes`/`Interfaces` icons on three of the Network
  tab's five segments (`Ports`/`Services` stay label-only, deliberately). `main.rs`'s
  `with_assets(..)` had to move from `Assets` to `AllAssets` for any of this to actually
  render. Gate green (fmt/clippy/tests); SSH right-click menu still shows no icons for
  rename/delete — that's `ssh_home.rs`'s `PopupMenuItem` list, out of this branch's scope.
- 2026-09-09 20:38: all five agents (text-input retirement, SFTP sidebar zoom, table frame
  cost, Kbd chrome, SSH/chrome pass) were killed by the org's monthly spend limit (HTTP 429).
  Resumed 21:51 when the limit reset: sidebar zoom gated by the orchestrator and merged
  (c7d31de); text-input, Kbd and SSH/chrome agents resumed with their context; table frame
  cost dropped (see §4). While they were down, the sidebar agent had also been caught editing
  main's worktree instead of its own; its changes were moved and main restored.
- 2026-09-09: `kbd-chrome` branch (not merged): `sid_ui::Kbd` lost its box in the 0.6.1
  upgrade because `gpui_component::kbd::Kbd::render()` paints through the library's own
  (unconfigured) theme rather than sid's tokens, so every chip in Settings → Keyboard,
  the command palette and the modal cheat sheet resolved to bare text. `kbd.rs` now draws
  the chip itself — `surface` fill, hairline `border`, `rounded_sm`, Meta-role text, same
  as `Badge`'s neutral tone — for both a parsed `Chip::Stroke` (still formatted through
  the library's pure, theme-free `Kbd::format()` for the platform key-name table) and an
  unparsed `Chip::Literal`, so the two are indistinguishable side by side again. Public
  API unchanged; no call site moved. Gate green (fmt/clippy/full workspace test suite,
  hygiene.rs included). Gallery's `kbd` card gained a bare single-key example (`Escape`)
  alongside the existing two-key/modifier-heavy/sequence/unparseable ones.
- 2026-09-09: `notice-wrap` branch (not merged): `sid_ui::InlineNotice` was clamping its
  body to one line, so the ~140+ char degraded-keyring message in Settings → Behaviour
  ellipsized mid-sentence. It now wraps to two lines (`min_w_0` + `line_clamp(2)` in place
  of the one-line clamp) and takes an optional `.detail(..)` line — Meta role, always
  `muted` regardless of tone — for a "why" separate from the "what". Settings' behavior
  section now splits `secret_status_message`'s composed line via a depth-counted paren
  scan (`split_status_detail`, `settings_tab.rs`) rather than touching `app.rs`, which two
  other agents had checked out. Gallery's "inline notice" card gained the wrapped-long and
  message+detail examples. Gate green (fmt/clippy/tests, 51 suites). Captures: 1920x1080
  and 700x900 Settings → Behaviour both show the full sentence wrapped and the detail line
  legible with no overflow, though at 700px this sandbox's actual D-Bus probe reason text
  (much longer than the estimate) still needs the ellipsis on the sentence — the detail
  line itself stays whole; 2000x1200 gallery capture (scrolled to the "structure" column)
  confirms all three notice shapes render cleanly in both tones.
- 2026-09-09: SSH-home + top-chrome cohesion pass on `ssh-chrome`. The scope chips are one
  `SegmentedControl` (`ScopeChip` keeps the per-item origin badge); the six tabs gained icons
  and collapse to icon-only with tooltips below 900 *design* px (`TabChrome::for_window`,
  tested against the 150%-zoom case), so nothing clips at 700px on any tab. The session strip
  is a real tab bar on the same `chrome_tab` box as the chrome above it — `StatusDot`, the
  saved **alias** rather than `user@host`, a close `IconButton` that fades in on hover, `+`
  tooltipped with its chord — and it still shrinks/scrolls with many sessions. SSH home is
  one `Card::panel` whose header is the toolbar row
  (`[CONNECTIONS · n] [quick-connect] [add connection] [connect]`, one height); the dot legend
  moved to the panel footer as Meta text. The right-click menu has icons —
  `PopupMenuItem::icon` **does** exist in gpui-component 0.6.1 — closing the icon pass's
  leftover. New `sid_ui::Tipped::tip` (tooltips on non-button elements) and `Icon::Database`.
  Deliberately not done: `CardGrid`'s `MIN_COL`/`MAX_COL` are unchanged. Letting two cards
  grow to fill a 2560px row makes them 1200px full-bleed rows, which is what the grid module
  was written to stop; the void is now bounded by the panel and left empty on purpose, since
  the store holds no second per-host fact to put there. Gate: fmt, clippy, 51 suites green;
  eight captures at 1920/700/2560/150%/cosmos-light incl. a live session tab and the card menu.
- 2026-09-09: panel-vocabulary + toolbar-contract pass over Database, Network and Workspaces
  on `data-tabs` (not merged). Every region on the three tabs is a `sid_ui::Card::panel` —
  `surface` fill, hairline, ruled Label header `NAME · count`, actions right-aligned in the
  header row at one height and one gap rhythm (`p_3`/`gap_2` on all three tabs). Database's
  local `panel_header()` helper is deleted; its three left panels move `well` → `surface`, and
  the editor and results become peers of them (`QUERY · <connection>` with Explain/Run,
  `RESULTS · 340 rows · 12 ms` with the filter, next page and Export, `QUERY PLAN · n` as a
  sibling panel rather than a frame nested inside the results one). Network keeps the sub-view
  segmented strip and puts the table in `PORTS · 13` / `SYSTEM SERVICES · 146` /
  `INTERFACES · 7`, built by one `sub_view_panel()`; Interfaces lost its inner `Card` (two
  frames for one list). Workspaces' sidebar is `WORKSPACES · n` with refresh and `+ add` in
  its header, and the detail pane is `OVERVIEW` / `BRANCHES · n` / `STATUS · n` / `LOG · n` /
  `REPOS · n` under a fixed-height heading strip so the panel starts at the same y for every
  workspace shape. The loose status strings are gone from every toolbar: `no connection
  selected` is the results empty state, `error: …` is an `error_line` in the panel body (which
  is how `network.error` became visible at all — it had no other render), `refreshing…` is the
  refresh button's spinner, `docker/kubectl not installed` is already the body's empty state.
  Workspaces' unregister was a `ConfirmButton` with an empty label — a 36px labelled box beside
  rename's 24px square — and is now an `IconButton` at rest and the `ConfirmButton` word when
  armed, same id, same two-step. Note for the narrow-window item: a panel header's actions are
  `flex_none`, so every pixel a filter takes comes out of the title beside it; Database's
  results filter is at `FieldWidth`'s 160px floor for that reason and `RESULTS` still reads
  whole at 700px. Gate: fmt, clippy, 51 suites, `hygiene.rs` green. Captures: three tabs ×
  {1920, cosmos-light, 700x900}, both selected states, all five Network sub-views, the query
  run and EXPLAIN panels, and a real git repo registered live to reach Branches/Status/Log.
