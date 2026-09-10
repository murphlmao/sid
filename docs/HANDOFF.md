# sid — Handoff / Start Here

**You are picking up an in-progress GPUI rebuild of `sid`.** This is the single
orientation doc: current state, where things stand tab by tab, where the landmines are.

**Last verified:** 2026-09-09, `main` @ `066baa2`. `cargo test --workspace`: **51 test
suites, 1580 tests passing, 0 failed** (expect ~51/~1577 — a handful of tests were added
same-day, that's normal drift). `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets -- -D warnings` both clean.

## What sid is (30 seconds)

An integrated developer **ops-cockpit** — SSH/SFTP, Database, Network, Workspaces,
System, Settings — as a native **GPUI** desktop app, run locally. Built in **vertical
slices**: one tab taken to daily-use quality before the next. SSH/SFTP is the spearhead.
The binding rules (adapter pattern, attributive layered scope, secrets-never-committed,
what's deliberately deferred) live in [`../CLAUDE.md`](../CLAUDE.md) — read them, they're
invariants, not suggestions.

## Read order

1. [`../CLAUDE.md`](../CLAUDE.md) — the binding ruleset.
2. [`design/2026-06-27-gpui-rebuild-design.md`](design/2026-06-27-gpui-rebuild-design.md) — North Star: the reframe, the layered-scope model, code disposition.
3. [`../.interface-design/system.md`](../.interface-design/system.md) — design law. In three lines: semantic tokens only (`crates/sid-ui/src/theme.rs`), depth via borders/surface shifts not shadows, one top chrome bar; type is three sizes/two weights/one mono family named by role (`crates/sid-ui/src/typography.rs`), enforced by a hygiene test that bans raw `text_xs()`/`font_weight()` calls outside it; one list per fact, `accent` means "engage" and is used sparingly.
4. [`design/2026-09-09-ui-cohesion-loop.md`](design/2026-09-09-ui-cohesion-loop.md) — today's checklist and the loop's own source of truth going forward; its **Log** section is the changelog for 2026-09-09, its **Blocked** section is what needs Murphy.
5. [`design/2026-07-27-session-resume.md`](design/2026-07-27-session-resume.md) — the prior state (pre-cohesion-loop) and the landmine list this file carries forward.
6. This file's **Where things stand** and **Open** sections, below, for what's next.

## Where things stand

| Tab | Shipped | Known gaps |
|:--|:--|:--|
| **SSH/SFTP** | MobaXterm-style multi-tab shell (home + per-connection sessions); folder-grouped tree with inline quick-connect "add this host" (#2); SFTP browser (download/upload, traversal-guarded); terminal fidelity A/B-matched to kitty; `ctrl +/-/0` zoom incl. terminal reflow (#4); on the shared panel/toolbar vocabulary end to end. | Config editor's focus trap can leak a sudo password into the file body (open security item, see Open); the session-tab chrome and Settings rail item still hand-roll their own focus ring instead of the shared helper. |
| **Database** | Query editor (Run/Explain) + left schema tree + relationships diagram (pop-out window, draggable, FK-labelled) + CSV export + query history; connections folder-grouped left; demo SQLite seeded with an FK-rich schema; hardened Postgres/TimescaleDB value decoding; on the panel vocabulary (`QUERY`/`RESULTS`/`QUERY PLAN` as sibling panels). | ~26% of table cell builds are discarded by gpui every frame (perf backlog, lowest priority, not yet re-measured against gpui-pre 0.3.4); the diagram's pop-out window can't be `grim`-captured by the harness (a verification gap, not a user-facing one). |
| **Network** | `Ports`/`Services` (systemd)/`Interfaces`/`Docker`/`Kubernetes` sub-tabs, sortable tables, two-click kill-by-pid; on the panel vocabulary, with fetch errors now actually rendered (`error_line`, previously invisible). | No CPU/mem columns or established-connections view — July's inc-2 backlog, not picked back up. |
| **Workspaces** | Workspace list with a live git chip; add/rename/armed-unregister; Repo detail (Overview/Branches-with-checkout/Status/Log); Umbrella fleet table (verified over ~40 real repos); on the panel vocabulary (`WORKSPACES` sidebar + per-shape detail panels). | Mutation ceiling is still checkout only — commit/diff/stash/other actions are unbuilt (v2; the archived POC is the reference). |
| **System** | CPU/mem/swap `StatCluster`; sortable process table with kill and a Command-column fallback to the process name for unreadable cmdlines; pinned config-file manager with a sudo-elevated, no-shell editor. | Pin/kill icon-only buttons weren't re-audited in the icon-only pass; shares the config-editor focus-trap bug above. |
| **Settings** | 220px section rail (Appearance/Behaviour/Keyboard/Storage) collapsing to a `SegmentedControl` below ~900 design px; live theme switching across four palettes (`cosmos-light` now AA-contrast-checked by a guard test); full keymap rebinding UI (validate/reserve/conflict/reset, persisted); keyring status as an `InlineNotice`. | The rail's active-item ring is still hand-rolled instead of calling the shared `focus_ring` helper. |

## Today's UI cohesion round (2026-09-09)

Full detail — the spec, the gate results, and every branch's own log entry — lives in
[`design/2026-09-09-ui-cohesion-loop.md`](design/2026-09-09-ui-cohesion-loop.md); this is
the index.

- **gpui-pre 0.3.4 upgrade.** `gpui` 0.2.2 → `gpui-pre` 0.3.4 + `gpui-pre-platform` +
  `gpui-component` 0.6.1 + `gpui-kit-assets` 0.6.1 — a registry-only jump, 29 files, zero
  of the ~450 `div()` call sites touched. Upstream fixed `truncate()`'s cache bug (sid
  still bans the literal — see Landmines); `Column.width` stays `Pixels`-only.
- **Zoom** (GitHub #4): `ctrl +`/`ctrl -`/`ctrl 0`, one scale factor for the whole UI
  including the SSH terminal grid (it reflows cells, not just fonts), persisted, clamped
  50–200%.
- **App-wide status bar:** `sid_ui::StatusBar` — keyring state in words, open SSH session
  count, `db: <name>`, zoom % when ≠100%, frame ms under `SID_PERF`. Retires the old
  top-right `!` badge.
- **One panel vocabulary + one toolbar contract:** every region on every tab is now a
  `Card::panel` whose header *is* the toolbar row; two real bugs (an invisible Network
  fetch error, a mislabelled Workspaces button) surfaced and were fixed along the way.
- **Focus ring + modal Tab-trap:** one `StyledExt::focus_ring` helper everywhere;
  `Modal` now traps Tab at both ends (gpui-component 0.6.1's `focus_trap`).
- **Settings rail:** 220px section nav replacing the old flat list, collapsing below
  ~900px.
- **cosmos-light pass:** measured contrast fixes (`warning`/`success`/`muted` all now
  ≥4.5:1 on `surface`), guarded by a new theme test.
- **Icons:** `gpui-kit-assets`' full 1830-glyph Lucide catalog wired in; real bin/pencil
  icons replace stand-ins.
- **Text-input retirement** (GitHub #3): the 946-line `crates/sid/src/ui/text_input.rs`
  is deleted (net −1049 lines); every field across every tab and form is a `sid-ui` input.
- **Quick-connect add** (GitHub #2): typing `user@host` and pressing Enter on an unknown
  host opens a prefilled add-connection form instead of dialling ad-hoc.

## Open

Unchecked items from the checklist (§ numbers refer to
[`design/2026-09-09-ui-cohesion-loop.md`](design/2026-09-09-ui-cohesion-loop.md)):

- §0 (Murphy): `sudo pacman -S sway wtype`; optional a GitHub token with `workflow` scope.
- §2: the config-editor focus-trap security item (assigned, branch `focus-followups`);
  the focus follow-ups list (Settings rail ring, `chrome_tab`, config editor save/close
  tab stops); an SSH-home empty state for zero hosts (note: `ssh_home.rs::home_empty_state`
  already renders one with an "add connection" action — worth re-checking whether this
  item is actually still open before picking it up); the light-theme follow-ups
  (`app.rs`'s `gpu_status_badge` hard-coded ink, `PopupMenu` popover-on-surface, the
  un-derived modal scrim); the final six-tabs × four-themes gate.
- §3: close GitHub #1 (fixed in July, still open on GitHub) — needs `gh` or Murphy.
- §4 backlog: the table-frame-cost perf item (lowest priority); `sid-shot.sh`'s dead
  Hyprland placement code (port to `hyprctl eval` or delete it); this rewrite of
  `docs/HANDOFF.md` (now done by this commit — the checklist box ticks on the loop's next
  pass, not this one); the CI file's move into `.github/workflows/` (blocked on token
  scope).

**Blocked** (from the checklist's own Blocked section):

- Closing GitHub #1/#2 needs Murphy: `gh` isn't installed and the GitHub MCP connector
  fails to authenticate.
- Murphy's Hyprland rule routing sid to workspace 4 lives in
  `~/dotfiles/config/hypr/hyprland.lua` (applied live via `hyprctl eval`) but is
  uncommitted in the dotfiles repo.

## Crates and key files

14 workspace members. `sid` is the **only** crate allowed to name GPUI (CLAUDE.md rule 1).

| Crate | What it is |
|:--|:--|
| `sid` | The GPUI frontend — the app shell, all six tabs, the single `AppState` entity. |
| `sid-ui` | The component crate: the design system, compiled (~30 modules — buttons, badges, cards, panels, tables, inputs, type scale, theme bridge, gallery). |
| `sid-store` | The layered, attributive store: global (redb) + per-workspace `.sid/config.toml`. |
| `sid-secrets` | The secret-storage adapter seam — keyring, in-memory fallback, a dormant encrypted-file backend. |
| `sid-core` | The pure trait seam (ssh/term/db/git/sys/gpu/svc/containers/privfs) — no concrete deps, no GPUI. |
| `sid-ssh` | `russh`-backed `SshClient` impl (fail-closed known-hosts, deadlock-free shell). |
| `sid-term` | `vt100`-backed styled terminal screen. |
| `sid-db` | `DbClient` impls: Postgres (`tokio-postgres`), SQLite (`rusqlite`), redb-browse. |
| `sid-git` | `git2`-backed `GitProvider` impl (the only crate allowed to name `git2`). |
| `sid-gpu` | Linux GPU pre-flight — probes renderer init in a subprocess so a driver crash can't take down the app. |
| `sid-sysinfo` | Composed `SysProvider`: `sysinfo` + `netstat2` + `nix` for processes/interfaces/ports/kill. |
| `sid-svcctl` | CLI-shelling `ServiceProvider` impl (`systemctl`). |
| `sid-containers` | CLI-shelling `ContainerProvider`/`KubeProvider` impls (`docker`, `kubectl`). |
| `sid-privfs` | `sudo`-backed `PrivilegedFs` impl for editing root-owned config files. |

Key files:

- `crates/sid/src/app.rs` — the single `AppState` entity; `Tab` (6 variants); renders
  from a cache, I/O only in event handlers, never in `render`.
- `crates/sid/src/keymap.rs` — `Action`, `ALL_ACTIONS`, `default_bindings()`, and the
  rebind policy (`REBINDABLE_KEYS`, `resolve_rebind`) behind Settings → Keyboard.
- `crates/sid-ui/src/theme.rs` — the token source (bg/surface/well/border/fg/muted/
  accent/success/warning/danger/selection + `ansi[16]`), four built-in palettes.
- `crates/sid-ui/src/typography.rs` — the role-based type scale + its hygiene ratchet.
- `crates/sid-ui/src/bridge.rs` — projects sid's palette onto gpui-component's own
  theming layer; `contrast_ink` derives a filled swatch's label ink.
- `crates/sid-store/src/store.rs` / `composer.rs` — the scoped read/write facade and the
  attributive union + `ViewFilters`.
- `.interface-design/system.md` — the design law (see Read order above).
- `scripts/sid-cap.sh`, `scripts/sid-shot.sh`, `scripts/lib/sid-app.sh` — capture/input
  harnesses (see Testing & workflow).

## Landmines (things that already bit us — carried forward, don't re-learn these)

- gpui reports a text element's **min-content width as its full string width**, so
  `flex_1` alone never shrinks text. Fix trio: `min_w(0)` + `.clamp_one_line()` +
  `flex_none()` on the sibling that must not grow.
- gpui's `truncate()` bug (measured-layout cache pinning `wrap_width` to `None`) is
  **fixed upstream as of gpui-pre 0.3.4** — but `tests/hygiene.rs` still bans the literal
  `.truncate(` call regardless of the upstream fix. Keep using
  `sid_ui::StyledExt::clamp_one_line()`; it never depended on the broken path anyway.
- `h_flex()` centres on the cross axis: a `flex_1` table beside a sized sibling resolves
  to **zero height** (header paints, no rows). Use `div().flex().flex_row().min_h(0)`.
- `FillTable` measures in prepaint and its widths land on the *next* layout pass; it
  schedules that frame itself (`Window::on_next_frame`) because `Window::refresh` is a
  no-op mid-draw. `Column.width` is still `Pixels`-only in gpui-component 0.6.1.
- gpui-pre 0.3.4's own `tab_group()` only **renumbers** tab stops — it doesn't wrap Tab
  at the ends. Wrapping needs gpui-component 0.6.1's `FocusTrapElement::focus_trap`
  (keep the trap's `FocusHandle` in `use_keyed_state` if the caller is a `RenderOnce`
  with no lifecycle, same trick `Modal` uses).
- A `gpui-component` widget that isn't threaded through `sid_ui::bridge` (e.g. `Kbd`)
  renders through the library's own **unconfigured** theme, not sid's tokens — it looks
  like a missing style, not a missing bridge call. `sid_ui::Kbd` now draws its own chip
  and keeps the library only as a key-name formatter.
- Writing an executable and exec'ing it from concurrent tests hits `ETXTBSY` — a `fork`
  in another thread momentarily inherits the write descriptor. Write test scripts once,
  before any spawn.
- `cargo build | tail` hides the exit code — check `$pipestatus`.
- postcard is positional: `#[serde(default)]` only, never `skip_serializing_if`.
- `hyprctl keyword` (and legacy two-arg `dispatch`) is **rejected by Murphy's Lua-parsed
  Hyprland config** — it exits 0 and silently does nothing, which is why `sid-shot.sh`'s
  placement/headless-monitor rules were failing behind an `|| true` for weeks unnoticed.
  Use `scripts/sid-cap.sh` (its own hermetic headless sway compositor, no Hyprland
  dependency) instead of `sid-shot.sh` on this machine.
- Never put worktrees or cargo build targets under `/tmp` — on this machine it's a 16GB
  tmpfs, and a cargo target dir there evicts RAM.
- Agents must work in their own worktree under `~/vcs/sid-wt/<branch>` — during the
  2026-09-09 spend-limit outage one agent was caught editing `main`'s own worktree by
  mistake; its changes had to be moved and `main` restored by hand.
- This repo's commits carry **no trailers** — not `Co-Authored-By`, not anything else.
  Check the last ten commits before inventing a convention.

## Testing & workflow

- **Gate** (every merge to `main`): `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
  (51 suites / ~1580 tests as of this doc). Visual/UI changes additionally need
  before/after captures in all four themes and, ideally, an
  `interface-design:design-review` pass on the touched screen.
- **`scripts/sid-cap.sh`** is the primary capture *and* input harness — its own hermetic
  headless `sway` compositor (needs `sway` + `wtype`), so it works while the session is
  locked, off-screen, and in parallel across agents. Builds `sid` itself by default
  (`--no-build` to skip). Recipes:
  - `--tab ssh|database|network|workspaces|system|settings` (`SID_START_TAB`)
  - `--theme cosmos|void|dusk|cosmos-light` (`SID_THEME`)
  - `--size WxH` (virtual output size, default `1920x1080`)
  - `--click X,Y` (repeatable) / `--dclick X,Y` (a true double-click inside gpui's 400ms
    window, issued as one driver command — two `--click`s can never trigger a
    `click_count() >= 2` handler) / `--rclick X,Y` (context menus)
  - `--drag X1,Y1,X2,Y2[,STEPS]` — put it **last** in a run; the synthetic mouse-up
    doesn't clear the app's drag state, so a later pointer action would re-drag it
  - `--scroll X,Y,N` — park the pointer and turn the wheel N notches; the only way to
    reach the foot of a scrolling tab (Settings, System) without activating a row
  - `--key KEYS` (chords like `ctrl+shift+t`) — needs a prior pointer **click** (motion
    alone doesn't trigger the compositor's keyboard-focus handoff on a headless seat)
  - `--type TEXT` (needs `wtype`), `--env KEY=VALUE`, `--xdg DIR` (a prepared hermetic
    store), `--real` (the live store instead of the hermetic demo), `--keep`, `--tree`
  - `SID_UI_SCALE` env var — per-run zoom override for captures, same lever as
    `ctrl +`/`ctrl -`.
- **`scripts/sid-shot.sh`** (the older live-Hyprland harness) currently **cannot capture
  at all** on Murphy's Hyprland — see the `hyprctl keyword` landmine above. Prefer
  `sid-cap.sh`.
- **Docker fixtures:** `scripts/test-ssh.sh` (sshd), `scripts/test-integration.sh`
  (Postgres), `scripts/test-db-matrix.sh` (Postgres + TimescaleDB) bring up
  `docker/docker-compose.test.yml` services and run the `#[ignore]`d integration tests
  against them; `scripts/test-smoke-headless.sh` is the Xvfb+Lavapipe launch-survives
  smoke.
- **Worktree convention:** every branch of work happens in its own `git worktree` under
  `~/vcs/sid-wt/<branch>` — never under `/tmp`, never in another branch's worktree. Leave
  no worktree with uncommitted changes at the end of an iteration.
- **Commits:** no trailers (see Landmines); push gate-green units. Match the phrasing
  style of the last ten commits rather than inventing a new convention.
- **Salvage source:** the archived TUI is read-only at `murphlmao/sid-poc`. Crib
  adapters/view logic; don't carry scaffolding wholesale.
