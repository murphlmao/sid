# sid — Handoff / Start Here

**You are picking up an in-progress GPUI rebuild of `sid`.** This is the single
orientation doc: current state, what's next, where the landmines are. Read this,
then the North Star spec, then the relevant plan.

_Last verified: 2026-07-02 — 368 tests green (clippy + fmt clean), working tree clean, `main` @ b45a17e. **Three tabs do real work.** SSH/SFTP: MobaXterm split session; file-browser toolbar restructured (no overlap), hidden-files toggle, kitty-parity terminal font. Database: connect→query→results, schema tree, cell copy/view, CSV export, query history, **Access-style relationships diagram in a pop-out window** (`schema_graph` FK/PK introspection on all 3 engines). Network: live ports + kill + interfaces. Forms: Tab/Shift-Tab traversal. Visual debugging: `scripts/sid-shot.sh`. Design mockups for the next UX pass: `docs/mockups/{ssh-v2,db-v2}.html`. All observation gates still need human eyes._

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
4. [`mockups/sid-mockup.html`](mockups/sid-mockup.html) — the intended UI layout.
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
| **Next UX pass** — settle via [`mockups/ssh-v2.html`](mockups/ssh-v2.html) + [`mockups/db-v2.html`](mockups/db-v2.html) | Multiple SSH connection tabs, drag-to-resize split divider, file-browser pop-out/in, DB layout variant (tree-left vs selector-right) | ⏳ awaiting Murphy's mockup verdicts |

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
