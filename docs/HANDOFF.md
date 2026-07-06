# sid — Handoff / Start Here

**You are picking up an in-progress GPUI rebuild of `sid`.** This is the single
orientation doc: current state, what's next, where the landmines are. Read this,
then the North Star spec, then the relevant plan.

_Last verified: 2026-07-06 (round F) — 718 tests green (clippy + fmt clean), working tree clean, `main` @ 3c12660._
_**Round F (Fable design review):** ONE top chrome bar (wordmark · tabs · scope chips · badge — the second strip is gone); **SSH home de-duplicated** into a single centered connections surface (quick-connect + folder tree + origin badges; promote/demote in the right-click menu; `ssh_connections_main`/`host_row` deleted); reading surfaces width-capped at 880px (ssh home, settings, config lists); `.interface-design/system.md` records the design system. **Perf:** `SID_PERF=1` render-build timer (measured: zero >4ms builds over 12 rapid Ctrl+Tab cycles in debug — remaining stutter is gpui shaping/layout, mostly a debug-build artifact); **terminal grid render memoized** on (generation, cursor, colors) — no more per-frame 10k-string clone + full reshape. **Terminal fidelity F1/F2 (earlier same-day):** cell height = font ascent+descent (kitty geometry, kills block-art seams); ANSI 0-15 renders through the active theme's new `ansi[16]` palette. `sid-cap --key` supports modifier chords._
_**Round E highlights — the POC design system landed:** `ui::theme` — semantic tokens (bg/surface/well/border/fg/fg_strong/muted/faint/accent/success/warning/danger/selection) as a gpui Global, four built-ins ported from the POC (**cosmos** default `#0b0b14`+`#d44141`, **void**, **dusk**, **cosmos-light**), persisted as `Settings::theme` (v4 postcard migration chain, tested); EVERY UI file swept onto tokens (the old blue accent is now the galaxy red — intentional) + a "spacey" pass (p_4 bodies, taller rows, uppercase-muted section headers, hairlines over fills); gpui-component's ThemeMode follows the active palette (`theme::component_mode`, wired in main.rs + the diagram pop-out + live switch). **Settings screen** — real `Tab::Settings` (Ctrl+6; `Ctrl+,` no longer a System stand-in): theme picker with swatch rows + LIVE persisted switching, behavior (default_scope / file_browser_side / keyring toggle + status), read-only keymap list, storage paths. **Systems tab config-file manager** — pinned (`PinnedFile` store entity, global `pinned_files` table) + curated existence-filtered common configs, arbitrary-path pinning w/ tilde expansion, in-app editor modal (≤1 MiB UTF-8 gate, writability probe, "read-only — needs root; sudoedit" banner, atomic perms-preserving temp+rename save, dirty marker). `SID_THEME` env overrides the theme per-run for captures. **CAVEAT: the entire round is visually UNVERIFIED** — the session was locked throughout (every capture = hyprlock clock) and `sway` isn't installed for `sid-cap.sh`; run an all-tabs × all-themes visual pass first thing._
_**Round D highlights:** `Ctrl+Tab` always cycles primary tabs (sessions moved to `Ctrl+PgDn`/`Ctrl+PgUp` — the "trapped in SSH" fix); **no emojis anywhere in the UI** (standing rule: words or monochrome glyphs only); **secrets are keyring-or-memory** — the encrypted-file vault + passphrase modal were REMOVED from the runtime (crypto kept dormant in `sid-secrets/src/file.rs`; `Settings.secret_file_enabled` persists but is ignored), a degraded backend shows a small `!` badge (click → detail popover), and a missing/dangling password secret triggers a **connect-time password prompt** (one-shot; `put` under the existing `secret_ref` for the session); **Network tables sort** (typed comparators, click headers) + `Ctrl+F`//`Ctrl+/` focuses the filter; **Systems tab v1** (SysProvider::overview, CPU/mem/swap cards, sortable process table with kill, 2s refresh while active); bug-hunt fixes: SQL trailing-comment wrapper bug (lexer-backed `strip_trailing_trivia`), db_tab generation guards (stale schema/query results can't land under a switched connection; switch resets the query pane), kube pods generation guard, workspace duplicate-identity handling (`WorkspaceConfig::duplicates` + status-line warning; edit collapses copies), redb `settings` pseudo-table shows all 4 fields._
_**SSH/SFTP:** MobaXterm **multi-tab shell** — `home` tab + per-connection session tabs, sidebar swaps between folder-grouped connections tree (inline rename, quick-connect filter, **`+ Add` + right-click menu**) and the session's file browser (persisted dock side); multiple concurrent connections; reader/writer split fixed the terminal freeze; SFTP put can create; status bar no longer overlaps the sidebar; `TextInput` clips to its bounds. **Database:** query loop + schema tree + relationships diagram + generic `⭳ Export ▾`; **connections on the LEFT (DBeaver-style)**, folder grouping, inline rename; **demo SQLite seeded with a sample FK-rich schema** (explorable out of the box). **Network v2:** Ports · Services (systemd) · Interfaces · **Docker** · **Kubernetes** sub-tabs + filter + hidden-iface grouping (Docker live-verified; K8s graceful when no kubectl/cluster). **Secrets:** keyring → memory (vault dormant since round D — see the round-D line above), degraded-only `!` badge, connect-time password prompt. **Keyboard:** Ctrl-modifier keymap registry + `Ctrl+K` palette + `Ctrl+1..5` tabs + `Ctrl+Tab` (primary) + `Ctrl+PgUp/PgDn` (sessions) + `Ctrl+F` filter/`Ctrl+T`/`Ctrl+W`; terminal-focus passes `Ctrl+<letter>` to the PTY. **DB drivers hardened** (via a live probe matrix): value-render no longer shows real values as "NULL" (uuid/numeric/timestamptz/jsonb/arrays decode; undecodable → `⟨type?⟩`), `cancel()` no longer deadlocks, TimescaleDB internal schemas excluded from introspection, auth→`DbError::Auth`. **Testing:** 607 unit/prop + docker Postgres/TimescaleDB/sshd integration (`scripts/test-*.sh`, `scripts/test-db-matrix.sh`) + redb + headless Xvfb smoke. Audits: [architecture](design/2026-07-02-architecture-audit.md) + [perf](design/2026-07-02-perf-audit.md). Agents drive the UI via `scripts/sid-cap.sh` (private headless sway — lock-proof, off-screen, scripted clicks/typing; needs `sway`+`wtype`), `scripts/sid-shot.sh` (real-session capture, now via a silent headless output; lock-gated), and `scripts/sid-click.sh`._
_**CI** still stashed at `docs/ci/github-actions-ci.yml` (env token lacks GitHub `workflow` scope — `git mv` into `.github/workflows/` + push with a scoped token). **Deferred:** terminal-grid memoization (perf, needs live gate); Settings→Keymap rebinding UI (keymap registry shipped); drag-to-dock (toggle shipped)._
_**CI:** `.github/workflows/ci.yml` isn't in the repo — this env's token lacks GitHub `workflow` scope; it's stashed at `docs/ci/github-actions-ci.yml`, `git mv` into place + push with a scoped token._
_**Scope-list caveat:** `reload_scopes` was inlined into `apply_seed_lists` (startup-only); the future Workspaces tab must rebuild `self.scopes` when it adds/removes a workspace at runtime._
_**Awaiting Murphy:** ssh-v3 + keyboard mockup verdicts (`docs/mockups/2026-07-02-{ssh-v3-mobaxterm,keyboard-driven}.html`) → then the SSH multi-tab shell rebuild._

---

## What sid is (30 seconds)

An integrated developer **ops-cockpit** — SSH/SFTP, Database, Network — as a native
**GPUI** desktop app, run locally. Built in **vertical slices**: one tab taken to
daily-use quality before the next. **SSH/SFTP is the spearhead.**

The binding rules (adapter pattern, attributive layered scope, secrets-never-committed)
live in [`../CLAUDE.md`](../CLAUDE.md). **Read it — those are invariants, not suggestions.**

## Read order

1. [`../CLAUDE.md`](../CLAUDE.md) — the binding ruleset.
2. [`design/2026-06-27-gpui-rebuild-design.md`](design/2026-06-27-gpui-rebuild-design.md) — North Star: the reframe, the layered-scope model, code disposition.
3. [`design/2026-06-27-store-schema.html`](design/2026-06-27-store-schema.html) — how global + workspace compose (open in a browser).
4. [`mockups/2026-06-27-sid-mockup.html`](mockups/2026-06-27-sid-mockup.html) — the intended UI layout.
5. This file's **Next work** section, below.

## Where things stand

| Plan | Scope | State |
|:--|:--|:--|
| **Plan 1** — [bootstrap & de-risk](superpowers/plans/2026-06-27-gpui-bootstrap-and-derisk.md) | GPUI window on Wayland, render primitives | ✅ green ([spike findings](design/SPIKE-FINDINGS.md)) |
| **Plan 2** — [layered store](superpowers/plans/2026-06-27-layered-store-plan.md) | `sid-store` P2.1–P2.7: global(redb)+workspace(TOML), Composer, scoped writes, secret boundary | ✅ green, 49 tests |
| **Plan 3A** — [editable hosts (P3.2)](superpowers/plans/2026-07-01-p32-editable-hosts.md) | `delete_host`, `Settings`/`default_scope`, `Host` auth v2 migration, keyring (`KeyringStore`+probe), `TextInput`, host form + save-to dialog + row actions (edit/delete/promote/demote) | ✅ green, 150 tests total |
| **Plan 3B** — [SSH adapter port (P3.3 groundwork)](superpowers/plans/2026-07-01-p33-ssh-adapter-port.md) | `sid-core` trait seam, `sid-term` styled vt100 cells, `sid-ssh` russh client/shell/SFTP + **fail-closed known-hosts**, shell `split()` (no writer deadlock) | ✅ merged; ⚠ B5 live-sshd smoke still needs one manual run |
| **Plan 3C** — [terminal view + connect flow](superpowers/plans/2026-07-01-p33c-terminal-connect.md) | GPUI styled terminal grid (arbor-referenced), `Host`/`AuthMethod`→`SshHostSpec`/`SshAuth` mapping, `secret_ref` keyring resolution, `⚡ connect` in the SSH tab, host-key `order_hostkeyalgs` | ✅ merged (C1–C7); ⚠ needs the human observation gate + live-sshd smoke |
| **DB slice** — [backend](superpowers/plans/2026-07-01-db-slice.md) + [Wave-2 UI](superpowers/plans/2026-07-01-db-ui-wave2.md) | Backend: `DbKind`, `Store` connection facade, `sid-db` (Postgres/SQLite/redb-browse), rustls TLS. UI (gpui-component): connection picker + seeded demo, descriptor-driven add/edit form (save-to scope, keyring secret), SQL editor + Run→results grid | ✅ backend + Wave-2 increment-1 merged; ⏳ increment-2 = left-tree schema browser, query history, CSV export, cell copy/view, redb-browse UI. Needs the live observation gate. |
| **SSH split session** — [P3.5](superpowers/plans/2026-07-01-p35-ssh-split-session.md) | MobaXterm layout: terminal + remote file browser on ONE connection; full-filesystem nav, download (traversal-guarded), view (safe text preview), copy-path | ✅ merged; needs the live observation gate |
| **DB increment-2** — [Wave-2 §inc-2](superpowers/plans/2026-07-01-db-ui-wave2.md) | Left schema tree (over existing `schema_introspect`), click-table→`SELECT *`, cell copy/view popover, CSV export (formula-injection + RFC-4180 guarded), in-memory query-history ring | ✅ merged; needs the live observation gate. ⏳ inc-3 = redb-browse UI, sortable/filterable grid, EXPLAIN |
| **Network** — [inc-1](superpowers/plans/2026-07-02-network-slice.md) | Port POC `SysProvider`→`sid-core::sys` + new `sid-sysinfo` crate (netstat2/sysinfo/nix, hardened kill guards). Tab: live listening-ports table (proto·port·pid·process), two-click kill-by-pid, interfaces strip (default-route first). Live/ephemeral — no store | ✅ merged; needs the live observation gate. ⏳ inc-2 = cpu/mem cols, sortable headers, filter, established conns |
| **UX pass 1** (no plan doc — six parallel tracks, 2026-07-02) | `scripts/sid-shot.sh` + `SID_START_TAB` (agent-drivable screenshots); SSH file-browser toolbar restructure + hidden-files toggle + date fix; terminal font → CaskaydiaCove Nerd Font Mono (kitty parity); Tab/Shift-Tab focus traversal in both forms; actionable keyring warning | ✅ merged; browser layout + font need a live connected session to observe |
| **Relationships diagram** | `sid-core`: `SchemaGraph`/`ForeignKey` + defaulted `DbClient::schema_graph`; `sid-db`: FK/PK introspection (Postgres `pg_catalog` w/ ordinality, SQLite `PRAGMA foreign_key_list` incl. implicit-PK refs, redb identity cols); `sid`: ⧉ diagram button → **pop-out OS window** (`Root`-wrapped), draggable table boxes w/ 🔑 PKs, canvas FK lines w/ `1`/`∞` labels that follow drags | ✅ merged; observation gate = open it on a real FK-bearing DB (demo sqlite has no FKs). Polish queue: refresh-in-window, self-ref FK stubs, release-outside-window drag edge |
| **Next UX pass** — settle via [`mockups/2026-07-02-ssh-v2.html`](mockups/2026-07-02-ssh-v2.html) + [`mockups/2026-07-02-db-v2.html`](mockups/2026-07-02-db-v2.html) | Multiple SSH connection tabs, drag-to-resize split divider, file-browser pop-out/in, DB layout variant (tree-left vs selector-right) | ⏳ awaiting Murphy's mockup verdicts |

**Crates:** `sid` (GPUI frontend — the only place GPUI may be named), `sid-store`
(layered store), `sid-secrets` (keyring boundary), `sid-core` (SSH/terminal/db/**sys** trait
seams — no concrete deps), `sid-ssh` (russh impl), `sid-term` (vt100 styled screen),
`sid-db` (Postgres/SQLite/redb clients + TLS), `sid-sysinfo` (netstat2/sysinfo/nix system probe).

**Key files:**
- `crates/sid/src/app.rs` — the single `AppState` entity. Renders from a cache; events call `refresh()` then `cx.notify()`. No I/O in `render`.
- `crates/sid-store/src/store.rs` — the `Store` facade (scoped read/write/promote/demote, guarded against override).
- `crates/sid-store/src/composer.rs` — the attributive union + `ViewFilters` (collapse-duplicates default on, hide-global default off).

## Next work — Plan 3 (SSH slice), remaining

Do these **in order**; each is a shippable increment.

### P3.2 — Editable host list  ✅ DONE (merged f967696)
Add/edit host form + the **`save to: workspace | global`** dialog (configurable
`default_scope`), auth fields (agent/key/password → keyring), and per-row
**edit/delete/promote/demote**. Wires the store's *write* side end-to-end.
**Open gate item (needs human eyes):** the A8 observation checklist in
[the plan](superpowers/plans/2026-07-01-p32-editable-hosts.md) — run the app and exercise
add (both layers, all 3 auth methods), edit, two-click delete, the `vps-1` promote
conflict, and `default_scope` preselection.

### P3.3 — Connect + embedded terminal
**Adapter groundwork ✅ DONE** (Plan 3B, merged): `sid-ssh` (russh client/shell/SFTP,
fail-closed known-hosts, deadlock-free shell) + `sid-term` (styled vt100 cells) +
`sid-core` trait seam, 45 tests. ⚠ the `#[ignore]`d live-sshd smoke still needs one manual
run: `cargo test -p sid-ssh --test live_sshd_smoke -- --ignored --nocapture`.
**Remaining = Plan 3C (not yet written):** render the PTY grid inside GPUI (crib Zed's
terminal), map `Host`/`AuthMethod`→`SshHostSpec`/`SshAuth`, resolve `secret_ref` via
`sid-secrets`, and (deferred from 3B) set `config.preferred.key` per-connect from recorded
known-hosts algorithms (OpenSSH `order_hostkeyalgs`) so imported `~/.ssh/known_hosts`
entries under a non-preferred algorithm don't spuriously fail.

### P3.4 — SFTP browser  ✅ DONE (merged c53bf5b)
`⊞ files` on a host opens a navigable remote browser (breadcrumb, dir nav, download to
`~/Downloads` with a path-traversal guard, text-path upload), mutually exclusive with the
terminal. Plan: [2026-07-01-p34-sftp-browser.md](superpowers/plans/2026-07-01-p34-sftp-browser.md).
Deferred: edit-in-place + per-host command history (nice-to-haves, not yet built).
**Needs the human observation gate** (see the plan's S6 checklist).

**After Plan 3:** write Plan 4 for the next slice (Database or Network) as a dated doc
in `superpowers/plans/`, matching the Plan 1/2 format.

## Landmines (things that already bit us — don't re-learn these)

- **DB filename is `store.redb`, not `sid.redb`.** The poc used `sid.redb` at the same
  XDG path; opening it with the new schema fails silently (seed skips, list is empty).
- **postcard is positional** — no `skip_serializing_if` on stored entities (it desyncs
  the buffer). `#[serde(default)]` only.
- **Edition 2024 RPIT captures `&self`** — `fn foo(&self) -> impl IntoElement` needs
  `+ use<>` or it won't compile against the borrow checker.
- **`cargo build | tail` hides the exit code** (you get tail's 0). Grep the output for
  `error` / check `${PIPESTATUS}`, don't trust the pipe's status.
- **Composition is attributive — never override.** Any code path that shadows or drops a
  same-identity record on write is a data-loss bug (we already fixed two in promote/demote).
  New write paths get a `Conflict` guard + a test.

## Testing & workflow

- **Visual debugging:** `scripts/sid-shot.sh [--tab ssh|database|network|workspaces|system]
  [--real] [--keep] [--out PATH] [--wait SECS]` builds `sid`, launches it (hermetic
  `XDG_*` temp dirs by default, so it boots on the demo seed — pass `--real` for the live
  store), waits for its Hyprland window, and `grim`-captures it to a PNG (path printed as
  the last line of stdout). `--keep` leaves the app running and skips temp-dir cleanup for
  follow-up debugging. Requires a live Hyprland/Wayland session (`hyprctl`, `grim`, `jq`).
- **Click-through driving:** `scripts/sid-click.sh click_at X Y` / `move_to X Y` injects
  pointer input via `ydotool` (user service `ydotool.service`, socket at
  `$XDG_RUNTIME_DIR/.ydotool_socket`). Absolute coords are unreliable on this multi-monitor
  scale-1.57 setup — the script converges with relative moves + `hyprctl cursorpos` read-back
  instead. Coordinate math: screen = window `at` (from `hyprctl clients -j`, matched by PID)
  + png_pixel / monitor_scale. Drag = `ydotool click 0x40` → stepped `mousemove` → `click 0x80`.
  Combined with sid-shot `--keep`, agents can drive any UI flow and verify it visually
  (first proven on the relationships-diagram window, 2026-07-02).
- **Pragmatic mode:** targeted tests per feature; one gate review at end of a slice — not
  per-commit rigor. Critical paths (store, composition, secrets) still get real tests.
- **Commits:** no `Co-Authored-By: Claude` trailer (standing rule). Push gate-green units
  straight to `main` (solo repo).
- **Salvage source:** the archived TUI is read-only at `murphlmao/sid-poc` (also cloned
  locally at `~/vcs/sid-poc`). Crib adapters/view logic; don't carry scaffolding wholesale.
