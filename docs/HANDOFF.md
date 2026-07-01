# sid — Handoff / Start Here

**You are picking up an in-progress GPUI rebuild of `sid`.** This is the single
orientation doc: current state, what's next, where the landmines are. Read this,
then the North Star spec, then the relevant plan.

_Last verified: 2026-07-01 — 37 tests green, working tree clean, `main` @ P3.1._

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
| **Plan 2** — [layered store](superpowers/plans/2026-06-27-layered-store-plan.md) | `sid-store` P2.1–P2.7: global(redb)+workspace(TOML), Composer, scoped writes, secret boundary | ✅ green, 37 tests |
| **Plan 3** — SSH slice | P3.1 host list wired to store | 🚧 P3.1 done; P3.2–P3.4 open (below) |

**Crates:** `sid` (GPUI frontend — the only place GPUI may be named), `sid-store`
(layered store), `sid-secrets` (keyring boundary).

**Key files:**
- `crates/sid/src/app.rs` — the single `AppState` entity. Renders from a cache; events call `refresh()` then `cx.notify()`. No I/O in `render`.
- `crates/sid-store/src/store.rs` — the `Store` facade (scoped read/write/promote/demote, guarded against override).
- `crates/sid-store/src/composer.rs` — the attributive union + `ViewFilters` (collapse-duplicates default on, hide-global default off).

## Next work — Plan 3 (SSH slice), remaining

Do these **in order**; each is a shippable increment. P3.2 is buildable on what exists today.

### P3.2 — Editable host list  *(start here)*
Add/edit host form + the **`save to: workspace | global`** dialog (with configurable
`default_scope`), plus **promote/demote** buttons on rows. Wires the store's *write*
side end-to-end (`write_host` / `promote_host` / `demote_host` already exist and are
tested). Pure GPUI + existing store API — no new external deps.

### P3.3 — Connect + embedded terminal
The riskiest step. Needs two things that were **researched but not yet done**:
1. **Salvage survey** of `sid-poc`'s SSH adapter (russh + russh-sftp + portable-pty) — per
   the design doc these carry over near-verbatim behind the existing trait seam.
2. **gpui-terminal research** — how to render a PTY grid inside GPUI (Zed's own terminal
   is the reference implementation to crib from).
Keep russh/PTY behind the adapter trait; GPUI stays out of `sid-store`/adapters.

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
