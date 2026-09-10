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
renders coherently. `docs/HANDOFF.md` was two months stale as of this morning; it has since
been rewritten from the current tree (see its own "Last verified" line) and is the accurate
orientation doc again — this checklist's Log section is now the changelog layered on top of it.

What is left is not "the UI is broken" but "the UI is a set of correct panels that do not yet
read as one application". The gaps below are visible in today's captures.

## 0. Prerequisites (Murphy, once)

- [x] `sudo pacman -S sway wtype` — installed by Murphy 2026-09-09 19:34; `scripts/sid-cap.sh`
      has been the loop's capture harness since.
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
- [x] Follow-up: retire `clamp_one_line()` — checked 2026-09-09, **not applicable**. The
      helper is `line_clamp(1).text_ellipsis()` and never used the broken `truncate()` path;
      the hygiene test bans the literal `.truncate(` regardless. Doc comment updated; render
      check at 900x700 on workspace paths, ports and DB paths clean in cosmos and cosmos-light.
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
- [x] **Panel/toolbar follow-ups**: done 2026-09-09. `Toolbar` stays (System still uses it)
      and `Card::panel`'s header now shares its exact box (`px_3 py_2`); Database and Network
      use `sid_ui::error_line`/`caveat_line` (verified with a provoked SQLite syntax error);
      `Icon::Diagram` (Lucide `workflow`) on the diagram button; panel header actions are
      `flex_initial` and filters shrink to `PANEL_FILTER_FLOOR` (120px) before the title
      truncates, verified at 700px on Database and Network.
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
- [x] **Interaction states.** Done 2026-09-09: one `StyledExt::focus_ring` helper (accent
      hairline over a transparent rest hairline, so no layout shift); pressed + ring + tab stop
      added to `Row` (opt-in `tab_index`), `SegmentedControl` segments (+ `tab_index` builder so a
      strip sorts with its form), `StatusItem`; Button/IconButton already had every state
      through the library ring mapped to `accent`. Gallery gained a FOCUS band.
- [x] **Security: the config editor's focus trap scope.** Flagged by the background commit
      review of 2aa4d36 (`focus-trap-scope-leaks-credential`); fixed 2026-09-09 in 3df08d0.
      Root cause: the backdrop trap stayed registered while the sudo prompt also registered
      one, so which trap answered came down to `HashMap` iteration order. Now `trap_owner
      (prompt_open)` gives the one registration to the unlock panel while it is open and the
      backdrop otherwise (unit-tested), and `close_unlock_prompt` scrubs the typed password on
      every close path (cancel, success, fatal, panicked task). Capture: three Tabs from the
      password field over `/etc/sudoers` return to the field; Save/close/body unreachable.
- [x] **Focus follow-ups**: done 2026-09-09. Settings rail item uses `focus_ring`; `chrome_tab`
      (top tabs + session strip) has pressed, ring and a tab stop (Tab walks SSH → Database →
      Network → Workspaces …; the active tab's underline no longer reads as a false ring);
      config editor save/close are `Button`/`IconButton` with `tab_index` (the builders were
      added to Button/IconButton). `.focus_visible()` not adopted; revisit only if a click
      ring reads as noise.
- [x] **Empty states.** Already covered: `ssh_home.rs::home_empty_state` renders the icon +
      headline + "add connection" action for zero hosts (landed July, 762aea9), and the
      no-match case centres inside the connections panel with the add row above it (verified in
      the SSH pass captures). Item was stale when written.
- [x] **cosmos-light pass.** Done 2026-09-09, measured not eyeballed: `warning` 2.94→4.70,
      `success` 3.73→4.88, `muted` 4.05→5.47 against `surface`; zero call-site changes (every
      consumer reads the token; `bridge::contrast_ink` flipped the solid warning badge's label
      by itself). New guard test sweeps every light ink against bg/surface/well at 4.5:1. Dark
      palettes byte-identical; void and dusk re-checked.
- [x] **Light follow-ups**: done 2026-09-10, test-first. Warning badge ink via
      `bridge::contrast_ink` (raw-hex exemption retired from `system.md`); the scrim is
      `bridge::SCRIM` at all five sites (hygiene allowlist tightened first, watched go red on
      the five literals); popovers and tooltips sit on `bridge::raised_surface(t) = mix(surface,
      fg, 0.07)`, which lightens the three dark palettes and darkens cosmos-light (three tests
      observed red against a stub first). Right-click menu verified raised in all four themes.
- [x] **Library boundary.** Done 2026-09-10, ratchet first: the hygiene test
      `the_rendering_library_is_named_only_in_sid_ui_and_the_composition_root` failed on the 15
      sites, then `sid_ui::component` re-exports `Root`, the menu, table and input types the
      tabs need and the nine modules import from it; the two `Tooltip` sites in `session.rs`
      use `sid_ui::Tipped`. Only `main.rs` names `gpui_component::` now. (`table::state::
      render_cell` was a private upstream module all along; its mentions were prose.)
- [x] **Gate fixes (sid-ui + theme)**: done 2026-09-10, every one red-first. `card::
      PANEL_HEADER_HEIGHT` (40px, zooms) so a header without actions is the same height;
      `card::header_bar` rules every titled card's header off its body; `table::rows_to_paint
      (data, viewport) = data` and `FillTable` only stripes when the data reaches the floor
      (the blocker: a two-row Ports table now ends at its data); cosmos-light `danger`
      #c03040→#8c1550 (ΔE from accent 36→94), cosmos-light `well` #ffffff→#e6e6ee (now below
      bg), dusk `danger` #d04a4a→#e05070 (4.29→5.00 on bg). Three new guards sweep all four
      palettes: status inks ≥4.5:1 on bg/surface/well, accent/danger ΔE ≥70, well recessed
      relative to bg (void's pure-black bg exempt, documented).
- [x] **Gate fixes (tabs)**: done 2026-09-10, seven commits, decisions as pure functions with
      red-first tests (`connections_count`, `port_action`, `ports_without_owner`,
      `scope_counts_label`, `workspace_option_note`); Network's footer now says `n ports
      without owner info (needs root)`; captures verified per defect. Original list: Database `CONNECTIONS · n` excludes the always-present store row;
      Database row `delete` is a labelled button beside two icon squares (mirror Workspaces:
      `IconButton` at rest, `ConfirmButton` when armed) and the two row templates place the
      origin chip differently; Network Ports renders a bare `—` in the action column for rows
      without an owner (blank it) and no explanation that owners need root; System Command
      cells hard-clip without an ellipsis (`clamp_one_line` + `min_w(0)`); Workspaces detail
      empty state is unframed canvas (wrap in `Card::panel`) and `3h · 0c` is unexplained
      (spell it out); SSH quick-connect field is 32px so the panel header is 49px not 41px
      (`.small()`); host/DB forms: the disabled `workspace` radio gives no reason and the
      enabled ring is fainter than the disabled one.
- [x] **Nit sweep**: done 2026-09-10, eight of nine items (legend under the header; lowercase
      menu; System inset 12px; Swap says "none configured" once; scope switcher breathing room;
      popovers clear a 0.06 relative-luma floor above surface, test red on all three dark
      palettes; swatch rings; neutral key chip with an accent hairline). Skipped on purpose:
      re-weighting the header vs per-card `connect`. Original list: the accent `connect`
      in the SSH header outranks the per-card connect; the status legend sits 800px from the
      dots it explains; right-click menu Title Case vs lowercase buttons; System panels inset
      16px vs 12px elsewhere; the scope switcher's track is flush to the window's top edge;
      theme swatches for bg/surface invisible on the selected row; Swap meter says "none"
      three ways; void popover raise is only 4/255; key chip in the host form is accent-filled.
- [ ] **Final gate.** Ran 2026-09-10 as a workflow (4 opus reviewers, one per theme, 34 PNGs;
      8 sonnet refuters, one per screen): 66 raw findings, 41 non-nit, 28 confirmed (12
      distinct), 1 blocker. Re-run after the two gate-fix branches land. Six tabs × four themes, gallery, `tests/hygiene.rs` green, and no tab
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
- [x] Follow-up from #3: **modals trap focus** as of 2026-09-09. `Modal` calls `tab_group()`
      + gpui-component's `focus_trap` (trap handle kept in `use_keyed_state`); `TextInput`'s
      Tab/Shift-Tab route through `sid_ui::focus::{next,prev}`; the config editor registers its
      own trap on its backdrop. A/B capture: 25 Tabs stay inside the host form; with the trap
      removed the same run lands on a background card's SFTP button.
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
- [x] `sid-shot.sh`: decided 2026-09-09 to keep it as the fail-safe fallback for machines
      without sway (it refuses rather than mis-captures on Murphy's Lua-parsed Hyprland);
      `sid-cap.sh` is the harness. Revisit only if someone needs live-session captures here.
- [x] `docs/HANDOFF.md` rewritten from the current tree 2026-09-09 (219 lines; verified 51
      suites / 1580 tests; per-tab shipped/gaps table; today's round indexed; landmines carried
      forward plus today's).
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

**Design discipline (Murphy, 2026-09-09 23:42): every change follows systems-first design.**
Ports and adapters as CLAUDE.md already binds, sharpened: (1) a decision (which element owns
the focus trap, which chrome variant fits the width, whether a fallback name is shown, what
colour clears contrast) lives in a pure function with no window/clock/env/I/O in its
signature, and is reached through a named port; (2) that function gets its failing test
first, then the minimum code to pass, then refactor (RED must be observed; a test that
passes on first run is mutated to prove it can fail); (3) tests attach at ports with in-memory
fakes, never through the compositor or Docker, and each behaviour has exactly one test home;
(4) adapters (gpui element code, sysinfo, russh, redb) translate and never decide; (5) time,
randomness and env enter through ports. Rendering itself stays observation-gated by capture,
per CLAUDE.md. Agent briefs must state which decision is being extracted and name its test.

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
- 2026-09-09: interaction states + modal focus trap on `focus-states` (not merged). One
  helper, `sid_ui::StyledExt::focus_ring`, is now the only spelling of a keyboard focus
  ring: an `accent` hairline over a transparent rest border, so the ring costs no layout
  when it appears. `Button`/`IconButton` already had every state (hover, pressed,
  disabled, and a ring `gpui-component` paints from `ThemeColor::ring`, which the bridge
  already maps to `accent`); the gaps were elsewhere. `SegmentedControl` segments gained a
  pressed fill, the ring and a tab stop — plus `SegmentedControl::tab_index`, without
  which every strip sorted ahead of the numbered fields of the form it sits in.
  `list::Row` gained a pressed fill and an **opt-in** `Row::tab_index` (a 400-row process
  table must not enrol every row); wired at the two `save to:` pickers, which were the one
  control in either form Tab could not reach. A clickable `StatusItem` gained pressed, the
  ring and a stop. The gallery has a `FOCUS` band — the tab-stop route, not a specimen,
  since focus is the one state a still image cannot draw twice. Not touched, by scope:
  the Settings rail item (`settings_tab.rs` hand-rolls the same ring — should move onto the
  helper), `app.rs`'s `chrome_tab` (hover only; not a tab stop), and the config editor's
  hand-rolled save/close divs (still unreachable by keyboard).
- 2026-09-09: modal focus trap, same branch, closing the §3 follow-up from #3.
  `gpui-pre` 0.3.4's `tab_group` only **renumbers** (`tab_stop.rs`: `next()` walks straight
  out of a group), but `gpui-component` 0.6.1 already ships the missing half —
  `FocusTrapElement::focus_trap` plus `Root`'s trap-aware Tab/Shift-Tab handlers. `Modal`
  now does both: `tab_group()` so its stops sort as one run, and `focus_trap` so the run
  wraps at both ends, with the trap's `FocusHandle` kept in gpui element state
  (`use_keyed_state`, the same trick the library's own `Button` uses) since `Modal` is a
  `RenderOnce` with no lifecycle. All three `Modal` call sites inherit it unchanged. Two
  places needed more: `sid_ui::TextInput` re-binds the library's `tab`/`shift-tab` indent
  actions and so never reaches the `Root` action — it now routes through
  `sid_ui::focus::{next,prev}` — and the config editor is not a `Modal`, so it registers
  its own trap on the backdrop (its sudo-unlock field is single-line, and Tab in it used to
  leave). A/B capture proves it: 25 Tabs from the open Add-host form land on `Save` with
  the trap and on the background `vps-1` card's SFTP button without it. Gate: fmt, clippy,
  51 suites / 1577 tests green. `gpui-base` 0.6.1 is now a named workspace dep (already in
  the lock as gpui-component's own) for `active_focus_trap`, which the styled crate does
  not re-export.
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
- 2026-09-09: §1 follow-up (retire `clamp_one_line()`) checked and left open, doc-only change
  on `clamp-retire`. `clamp_one_line()`'s body (`line_clamp(1).text_ellipsis()`) never used
  `white_space: Nowrap`, so it never depended on the cache bug gpui-pre 0.3.4 fixed — nothing
  to retire in the implementation. Two reasons to keep the seam rather than inline gpui's now-
  fixed `truncate()` at its ~40 call sites: `tests/hygiene.rs`'s `no_banned_calls` still bans
  the literal `.truncate(` spelling outright, upstream fix or not; and the helper name is the
  contract, not which primitive backs it. Doc comment updated to say so. Confirmed the other
  two Landmines are still open and unrelated: every examined call site (`workspaces_tab.rs`,
  `network_tab.rs`, `db_tab.rs`) still pairs its own `min_w(0)` beside `.clamp_one_line()` —
  that half of the fix trio stays a caller responsibility, not folded into the helper. Gate:
  fmt, clippy, 51 suites green. Captures at 900x700 (Workspaces, Network → Ports, Database,
  and Workspaces in cosmos-light): every long path/address ellipsizes inside its box, nothing
  overflows or pushes a sibling off the right edge.
- 2026-09-09: light-theme pass over `cosmos-light` on `light-pass` (not merged). Every screen
  captured through the headless harness in `cosmos-light` (six tabs, the schema-loaded
  Database, the Add-host modal over the scrim, the card context menu, the command palette,
  Settings → Keyboard, System → Config files, and `SID_GALLERY=1` at 2000x2400), then measured
  rather than eyeballed: `warning` was **2.94:1** on `surface`, `success` **3.73:1** and
  `muted` **4.05:1** — the palette had carried cosmos's *pale* status hues onto an off-white
  canvas, where a pale hue is a smear. Fixed at the token level, three values in
  `cosmos_light()`: `warning` `0xb08030` → `0x8a5f1a` (4.70:1 on `surface`), `success`
  `0x408090` → `0x246e7c` (4.88:1), `muted` `0x707082` → `0x5c5c6e` (5.47:1, matching cosmos's
  own 5.42:1). No call site changed: badge/toast/dot/meter all read the token, so the soft
  wash, the outline hairline and the dot follow it, and `bridge::contrast_ink` — which derives
  a filled swatch's label from the *fill's* brightness, not the tone's name — flips the solid
  warning badge to a light label on its own, exactly the case it was written for. Dark palettes
  byte-identical; the bridge mapping is unchanged (checked void and dusk on SSH and Database).
  New guard `theme.rs::the_light_palettes_inks_clear_aa_on_every_surface_they_land_on` sweeps
  every ink against `bg`/`surface`/`well` at 4.5:1 (`faint` exempt — it is the
  decorative/disabled tone; `selection` exempt as a bed — a row's ink is `fg`). Gate: fmt,
  clippy, 51 suites, `hygiene.rs` green. Left open, wrong file for this branch: `app.rs`'s
  `gpu_status_badge` hard-codes `rgb(0x1a1a1a)` as the label on a `warning` fill — with the
  deeper amber that pill is now 2.7:1 and it should be `bridge::contrast_ink(t, warning)`
  (which also retires one of the two raw-hex exemptions in `system.md`). Also seen, not a light
  bug: a `PopupMenu` is `popover` = `surface` over a `surface` panel in *all four* palettes, so
  only its hairline separates it; and the modal scrim is typed out as `rgba(0x000000a8)` at
  five call sites instead of the `bridge::SCRIM` that exists for it.
- 2026-09-09: panel/toolbar follow-ups, on `panel-followups` (worktree branch, not merged).
  `sid_ui::Toolbar` stays — `systems_tab.rs` still renders one outside a `Card::panel` — but
  its box now matches `Card::panel`'s header (`px_3().py_2()` both places; the header used
  `py(6.)`). `db_tab.rs`/`network_tab.rs` dropped their file-private `error_line`/`caveat_line`
  copies for `sid_ui::notice`'s; confirmed against a provoked SQLite syntax error that the
  notice still reads correctly (red icon, two-line wrap). Added `Icon::Diagram` (Lucide
  `workflow`, not `git-fork` or `network` — the latter is already `Icon::Interfaces`), wired
  as the Database `diagram` button's leading icon. And the narrow-header item from the
  previous entry: `Card::panel`'s header actions cluster was `flex_none` (rigid), so a wide
  fixed-width filter never gave up width and the title elided first; it is now `flex_initial`
  (grow 0, shrink 1), and the Database/Network filters shrink down to a new
  `sid_ui::PANEL_FILTER_FLOOR` (120px) before the title does. Gate: fmt, clippy
  (`--workspace --all-targets -D warnings`), full `cargo test --workspace` all green.
  Captures: Database and Network at 700x900 (`RESULTS`/`PORTS` read whole), Database with the
  demo sqlite connection selected (diagram icon visible), and the gallery at 2000x2400
  (`ICON · 52`, `Icon::Diagram` included).
- 2026-09-09: `docs/HANDOFF.md` rewritten from the current tree on branch `handoff`
  (worktree `~/vcs/sid-wt/handoff`, not merged). Verified against source rather than
  copied forward: 51 suites / 1580 tests passing at `main` @ 066baa2, the six-tab keymap
  in `keymap.rs`, the `sid-cap.sh` flag set, the `sid-ui` module list, and that
  `docs/ci/github-actions-ci.yml` is still not under `.github/`. Also fixed this file's
  "Where the project actually stands" paragraph, which pointed at the old HANDOFF as
  stale and the session-resume doc as the accurate one — backwards now that HANDOFF is
  current again.
- 2026-09-09: the three "not touched, by scope" items from the interaction-states pass, on
  `focus-followups` (not merged). The Settings rail item now calls `StyledExt::focus_ring`
  instead of hand-rolling the same accent-on-focus border. `app.rs`'s `chrome_tab` (top bar +
  SSH session strip) gained a pressed fill, a tab stop at index 0, and the ring; the first
  cut reused `border_color` for both the ring and the active-tab underline and painted the
  underline's accent on all four edges the instant a merely-selected, unfocused tab rendered
  — caught by the `ff-tabs` capture and fixed by giving the underline its own absolutely-
  positioned bottom bar, independent of the ring. The config editor's Save/close controls are
  now `sid_ui::Button` (which needed a new `Button`/`IconButton::tab_index` builder — neither
  exposed one before) at indices 1/2, after the editor buffer's own default 0. A security
  review of the same code found a real bug in the trap this morning's modal-focus-trap pass
  landed: the backdrop's `focus_trap` stayed registered the whole time the sudo-unlock prompt
  was open too, so `FocusTrapManager`'s one `HashMap` held both it and the panel's own trap,
  and which one answered `active_focus_trap` came down to iteration order — when the backdrop
  won, Tab out of the password field could rest on the file body or Save/close behind the
  scrim. `trap_owner(prompt_open)` now hands the one registration to the panel while it's open
  and the backdrop while it isn't, never both; the typed password is scrubbed
  (`close_unlock_prompt`) before the field drops on every path that closes the prompt, not
  only the retry path that already did. Gate: fmt, clippy, workspace tests green. Captures:
  `ff-tabs.png` (4 Tabs on SSH lands on the 4th top tab, Workspaces, ringed — confirmed 1/2/3
  Tabs land on SSH/Database/Network in turn), `ff-rail.png` (Settings rail item ringed via the
  shared helper, confirmed with a click into the rail first since a fresh launch's first ~13
  Tab stops are all in the top chrome), `ff-editor.png` (opened `/etc/sudoers` — root-owned,
  unreadable — via System → Config files, clicked unlock, 3 Tabs from the password field lands
  back on the password field, proving the trap wraps within password ↔ cancel ↔ unlock and
  never reaches the editor behind the scrim).
- 2026-09-10: the three "Light follow-ups" items, on `light-followups` (worktree
  `~/vcs/sid-wt/light-followups`, not merged), one commit each. `app.rs`'s `gpu_status_badge`
  now calls `bridge::contrast_ink(t, warning)` instead of a hard-coded `rgb(0x1a1a1a)` label
  (2.7:1 on cosmos-light's deeper amber); the case was already covered by
  `bridge::the_ink_follows_the_fill_not_the_tone_name`, so no new test was needed there, and
  the near-black-label exemption is gone from both `system.md` and `hygiene.rs`'s
  `EXEMPT_LITERALS`. The modal scrim's five stray `rgba(0x000000a8)` call sites (`app.rs`,
  `command_palette.rs`, `config_editor.rs`, `session.rs`, `db_tab.rs`) now read
  `bridge::SCRIM`; `hygiene.rs`'s literal exemption is replaced with a file exemption
  (`defines_the_scrim`, `bridge.rs` only) so the hex can't quietly reappear elsewhere —
  confirmed red on exactly those five lines before the swap, green after. Popover depth:
  `bridge::raised_surface(t)` (`mix(t.surface, t.fg, 0.07)`, tests written and observed red
  against a stub before the real mix) replaces `colors.popover = hex(t.surface)`, so a
  `PopupMenu`/`Tooltip` (same `popover` field in gpui-component 0.6.1 — no separate `tooltip`
  colour to touch) now sits one step above the panel it floats over instead of sharing its
  fill: cosmos `0x13131f`→`0x22222e`, void `0x0a0a0a`→`0x1a1a1a`, dusk `0x1c1812`→`0x2b261f`,
  cosmos-light `0xeaeaf2`→`0xdbdbe4` (darkens, the other three lighten — one formula, no
  light/dark branch, because `fg` sits at the opposite brightness extreme from `surface` in
  every built-in). All three tested well past the 4.5:1 floor against `fg`
  (`popover_foreground`). Gate: fmt, clippy (`--workspace --all-targets -D warnings`), full
  `cargo test --workspace` green, `hygiene.rs` green. Captures: `lf-menu-{cosmos,cosmos-
  light,void,dusk}.png` (right-click a connections card — the popover now reads as a distinct
  panel over the card, not just a hairline, in all four), `lf-scrim-light.png` (Add host modal
  in cosmos-light — scrim unchanged, same 66%-black wash). Skipped: the software-rendering
  badge capture — no `SID_GPU=software`-style preflight override exists in `sid-gpu`/`main.rs`
  (only `SID_GPU_SKIP_PREFLIGHT`, which skips the check rather than forcing the degraded
  path), so `gpu_status_badge` was verified by its existing bridge test instead of a capture.
- 2026-09-10: library boundary closed (`lib-facade` branch). RED:
  `tests/hygiene.rs`'s `the_rendering_library_is_named_only_in_sid_ui_and_the_composition_root`
  found the 15 sites across nine `crates/sid` modules the checklist named. GREEN:
  `sid_ui::component` re-exports `Root`, `menu::{ContextMenuExt, PopupMenu, PopupMenuItem}`,
  `table::{Column, ColumnSort, TableDelegate, TableState}` and
  `input::{Editor, EditorState, InputEvent, InputState, Position}`; all nine modules import
  from it instead. `table::state::render_cell` dropped from the facade — it turned out to be a
  private upstream module only ever *named* in two doc comments, never actually imported, so
  the two mentions became prose instead of a (nonexistent) re-export. `session.rs`'s two
  `tooltip::Tooltip` call sites became `sid_ui::Tipped::tip(...)` rather than a tooltip
  re-export, since a wrapper already covered them. Gate: fmt, clippy
  (`--workspace --all-targets -D warnings`), full `cargo test --workspace` green. Capture:
  `lf-db.png` (Database tab) — connections panel, schema panel, SQL editor, results and the
  `secrets in memory` status all render as before. Merged main mid-task (light follow-ups:
  `bridge::SCRIM`, `raised_surface`) — one import-list conflict in `session.rs`, resolved as
  the union; re-gated clean after.

- 2026-09-10: the four **Gate fixes (sid-ui + theme)** defects, on `gate-ui` (worktree
  `sid-wt/gate-ui`), one commit each, every decision red first. (1) `card::PANEL_HEADER_HEIGHT`
  = 40px (the 24px `ButtonSize::Sm` box + `py_2`), applied by `panel_header()` and asserted by
  `a_panel_header_is_one_height_whatever_it_carries` — RED as `left: None`; the gate's 41/36/40
  are now one strip, no per-call-site knob. (2) `card::header_bar(chrome, theme)` extracted, then
  switched from `body_fills_container()` to `is_raised()`, so `Card::new().title(..)` (Settings
  APPEARANCE, the System panels) gets the same ruled header as `Card::panel`; a titled raised
  card moves its `p_3` onto the body so the rule spans the card, and `Card::section` stays
  unruled on purpose. RED: "Raised: header hairline". (3) `table::rows_to_paint(data, viewport)`
  = `data` — the library ties its striped *fake-row* fill to the same flag as the zebra
  (`state.rs` `calculate_extra_rows_needed`), so `FillTable` reads the viewport off the vertical
  scroll handle and only asks for `stripe` when the data already reaches the floor. RED:
  `fewer_rows_than_the_viewport_paints_only_the_data`, left 36 right 10. Network PORTS filtered
  to 2 rows now ends at the data on plain surface. (4) tokens: cosmos-light `danger`
  `#c03040`→`#8c1550` (36→94 apart from `accent` on a redmean ΔE, 7.5:1 on surface),
  cosmos-light `well` `#ffffff`→`#e6e6ee` (below `surface` and `bg`; as dark as `warning`'s
  4.5:1 floor allows), dusk `danger` `#d04a4a`→`#e05070` (4.00→4.67:1 on surface, 4.29→5.00 on
  bg). `bridge::light_ink` follows: a light palette's lightest surface is `bg` now, not `well`.
  Three guards added, all observed RED: the AA sweep covers all four palettes' status inks,
  `accent_and_danger_are_distinguishable_in_every_palette` (ΔE ≥ 70), and
  `well_is_recessed_relative_to_bg_in_every_palette` (void's pure-black `bg` is the one
  documented exemption — nothing can sit below it). `ansi` untouched in both palettes; the dark
  palettes are otherwise byte-identical. Gate: fmt, clippy `--workspace --all-targets
  -D warnings`, `cargo test --workspace` green. Captures: `gu-net-light.png` +
  `gu-net-light-few.png`, `gu-db.png`, `gu-settings-light.png`, `gu-ssh-dusk.png`,
  `gu-gallery-light.png`. NOT merged — left on the branch for review.
- 2026-09-10: Final-gate tab defects fixed on `gate-tabs` (seven commits, not merged).
  Database: `CONNECTIONS · n` now counts the always-rendered store-browse row
  (`connections_count`, RED first); its `delete` control is an `IconButton(Trash)` at
  rest and only becomes the labelled `ConfirmButton` once armed, mirroring
  `workspaces_tab`'s unregister; `store_browse_row` and `render_connection_row` share one
  trailing-chip layout. Network: `port_action(pid)` (RED first) turns an ownerless port's
  action cell empty instead of a stray `—`, plus one Meta-role footer line, `n ports
  without owner info (needs root)`, from a tested pure counter. System: the Command
  column clamps to one line so a long argv ellipsizes instead of hard-clipping into User.
  Workspaces: both `workspaces_detail_panel` empty states are now framed in
  `Card::panel("workspace"/"detail")` like Database's RESULTS; the row's `{n}h · {n}c`
  became `scope_counts_label` (RED first, singular/plural tested), and the redundant
  "not a git repo" line under the `no git` chip is gone. SSH home: `quick_connect_field`'s
  `TextInput` is `.small()`, matching the 41px header rung everywhere else. Host/DB
  connection forms: the disabled `workspace` save-to radio explains itself via
  `workspace_option_note` (RED first, duplicated per-file on purpose, same as the rest of
  `save_to_selector`), and `radio_mark`'s enabled/disabled ring colours (`border`/`faint`)
  were swapped to `muted`/`border` so the choosable option reads brighter than the
  disabled one. Gate: fmt, clippy `--workspace --all-targets -D warnings`, `cargo test
  --workspace` all green, re-run after merging main's sid-ui gate-fix pass
  (`a51db99`: fixed 40px panel-header height, raised-card header hairline, `FillTable`'s
  phantom-row fix, three palette tokens) — no conflicts, none of it touched these seven
  files. Captures at 1920x1080 confirm each defect gone, including the db delete control
  armed and the add-host modal's radio band. Left on the branch for review.
- 2026-09-10: Nit sweep, eight small visual decisions, on `nit-sweep` (not merged).
  SSH home: the status-dot legend moved from a footer ~800px below the dots to one
  Meta-role row under the CONNECTIONS header; the right-click menu's `Connect`/
  `Rename`/`Edit…`/`Assign folder…`/`Delete` are lowercase like every other button.
  System tab: root inset matches every other tab's `p_3` (was `p_4`); an unconfigured
  swap now renders only the `Swap` label and `none configured` caption via
  `Meter::hide_bar` (was an em-dash value, an empty bar, and the caption, three ways
  of saying the same thing). Top chrome: the scope switcher and badge share one
  right cluster with `py_1` so the track floats instead of sitting flush against the
  bar's edges; verified no clipping at 700px. `sid_ui::bridge::raised_surface` (the
  popover raise) now enforces a 0.06 relative-luma floor above `surface` instead of a
  flat 7% mix — RED first
  (`a_popover_clears_a_minimum_step_above_surface_in_every_palette` failed on
  cosmos/void/dusk, ~0.007-0.011 relative luma against the 0.06 floor); the existing
  4.5:1 contrast-to-fg test held unchanged (6.9-7.9:1 on the boosted palettes).
  Settings' theme-row swatches ring in `chrome.faint` instead of `chrome.border`,
  which was a token step from `selection`/`well` by design and disappeared under a
  near-black/near-white swatch on the active row. Host form: the selected `~/.ssh`
  key chip is neutral solid with an accent hairline border instead of a solid accent
  fill, so it is no longer the loudest mark in the form; unselected chips are neutral
  outline. Gate: fmt, clippy `--workspace --all-targets -D warnings`, `cargo test
  --workspace` all green, zero failures. Captures at default size, void, 700x900 and
  the add-host modal confirm each fix. Left on the branch for review.
