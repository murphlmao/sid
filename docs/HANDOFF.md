# sid — Handoff / Start Here

**You are picking up an in-progress GPUI rebuild of `sid`.** This is the single
orientation doc: current state, what's next, where the landmines are. Read this,
then the North Star spec, then the relevant plan.

_Last verified: 2026-07-01 — 173 tests green, working tree clean, `main` @ aa8a841 (SSH slice functionally complete through the embedded terminal; DB backend foundation landed)._

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
| **DB slice** — [foundation](superpowers/plans/2026-07-01-db-slice.md) | `DbKind` (sid-core), `DbConnection` v2 + `Store` connection facade (sid-store), `DbClient` trait + `sid-db` crate (Postgres/SQLite/redb-browse) | ✅ Wave-1 backend merged; ⏳ Wave-2 GPUI DB tab UI (adopt gpui-component, lift dbflux's 2 widgets) — collaborative next |

**Crates:** `sid` (GPUI frontend — the only place GPUI may be named), `sid-store`
(layered store), `sid-secrets` (keyring boundary), `sid-core` (SSH/terminal trait seam —
no concrete deps), `sid-ssh` (russh impl), `sid-term` (vt100 styled screen).

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

### P3.4 — SFTP browser
Download / upload / edit-in-place, per-host command history. Builds on P3.3's connection.

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

- **Pragmatic mode:** targeted tests per feature; one gate review at end of a slice — not
  per-commit rigor. Critical paths (store, composition, secrets) still get real tests.
- **Commits:** no `Co-Authored-By: Claude` trailer (standing rule). Push gate-green units
  straight to `main` (solo repo).
- **Salvage source:** the archived TUI is read-only at `murphlmao/sid-poc` (also cloned
  locally at `~/vcs/sid-poc`). Crib adapters/view logic; don't carry scaffolding wholesale.
