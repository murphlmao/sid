# Layered Store (`sid-store`) — Implementation Plan (Plan 2)

> Execution guide. Visual: `docs/design/2026-06-27-store-plan-roadmap.html`. Model: `docs/design/2026-06-27-store-schema.html`.

**Goal:** A headless `sid-store` crate — a machine-local global layer (redb) + a per-workspace layer (committed `.sid/config.toml`) behind one **attributive** `Store` API: a read is the union of both, each item tagged by origin; never override; duplicate-collapse (workspace-primary) and hide-global are view filters over a lossless store.

**Architecture:** Lift the proven redb/postcard/secrets plumbing from `sid-poc`; build only the new attributive layer (Scope/Provenance, WorkspaceStore/TOML, Composer) on top. No GPUI. Fully unit-tested.

**Tech:** Rust 2024 · redb (global) · postcard (redb values) · toml (workspace file) · serde · thiserror · keyring via salvaged `sid-secrets`.

## Global Constraints
- Composition is **attributive — never override**. Storage is lossless; filters are view-only.
- Secrets are never committed and never in redb: committed config holds only a `secret_ref`; the value lives in the OS keyring behind `SecretStore`.
- redb values = **versioned postcard** (`[version: u8][payload]`); workspace file = **TOML**.
- Pragmatic testing: targeted tests per step; **hard gate at P2.7**. Critical-path steps (P2.4 Composer, P2.5 scoped writes, P2.6 secret boundary) get adversarial tests + an end multi-agent review.

## Salvage references (from `sid-poc`)
- Codec (lift verbatim): `crates/sid-store/src/codec.rs` — `encode_versioned` / `decode_versioned`.
- redb helpers (lift): `crates/sid-store/src/redb_impl.rs` — `with_read` / `with_write` / `list_versioned` / `upsert_versioned`; table setup in `schema.rs`.
- Secrets (lift as-is): `crates/sid-secrets/` — `KeyringStore` / `PlainStore` / durability guard / `FakeKeyring`.
- Test style (mirror): `crates/sid-store/tests/sessions.rs` — tempdir + open + builder + round-trip + restart-persistence + proptest.

## Tasks

### P2.1 — Crate + core types
- **Files:** `crates/sid-store/{Cargo.toml, src/{lib,error,codec,scope,entities}.rs, tests/roundtrip.rs}`; add member to workspace `Cargo.toml`.
- **New:** `Scope { Global, Workspace(WorkspaceId) }`, `Attributed<T> { item, origin, duplicate }`, `Identity` trait; entity structs `Host`, `DbConnection`, `QuickAction`.
- **Lift:** `codec.rs` (adapted to local `StoreError`).
- **Tests:** postcard round-trips for every entity + `Scope`; version byte present; empty-input decode errors; optional `secret_ref` round-trips.
- **Deliverable:** crate compiles; serde round-trips green.

### P2.2 — GlobalStore (redb)
- **Files:** `src/global.rs` (+ deps redb; dev-deps tempfile).
- **Lift:** `with_read`/`with_write`, `list_versioned`/`upsert_versioned`, `TableDefinition<&str, &[u8]>` table setup.
- **New:** tables `global_config`, `workspaces` (registry), `sessions`; typed CRUD.
- **Tests:** write→read→delete on a temp db; restart-persistence (drop→reopen→verify); missing-key → `Ok(None)`.
- **Deliverable:** global layer CRUD green.

### P2.3 — WorkspaceStore (TOML)
- **Files:** `src/workspace.rs` (+ dep toml).
- **New:** load/save `<root>/.sid/config.toml`; discover `.sid/` under a root; `[[ssh.host]]` / `[[db.connection]]` / `[[quick_action]]` sections; `secret_ref` preserved.
- **Tests:** TOML round-trip; **missing file = empty layer** (not error); malformed file → clear error; `secret_ref` survives.
- **Deliverable:** workspace layer read/write green.

### P2.4 — Composer *(critical path)*
- **Files:** `src/compose.rs`.
- **New:** `read(scope) -> Vec<Attributed<T>>` = union(global, workspace) with correct `origin`; duplicate detection by `Identity`; view filters `ViewFilters { collapse_duplicates: bool = true, hide_global: bool = false }`.
- **Tests:** union keeps both records; provenance correct; **dedup-default collapses same-identity with workspace winning**; hide-global drops all `⌂`; toggling filters never mutates storage; global-scope read returns global only.
- **Deliverable:** attributive composition green + adversarial dup/filter tests.

### P2.5 — Store API + scoped writes *(critical path)*
- **Files:** `src/lib.rs` (the `Store` facade).
- **New:** `write(item, scope)`, `promote(id, from)` ws→global, `demote(id, to)` global→ws; `read` delegates to Composer.
- **Tests:** write lands in the named layer only; promote moves ws→global (and removes from ws); demote moves global→ws; read reflects moves.
- **Deliverable:** one `Store` facade the tabs consume; scoped-write tests green.

### P2.6 — Secret boundary *(critical path)*
- **Files:** `crates/sid-secrets/` salvaged in; wiring in store.
- **Lift:** `KeyringStore` + `PlainStore` + `FakeKeyring` + durability guard.
- **New:** enforce that committed config carries only `secret_ref`; store round-trips a secret to the keyring by ref.
- **Tests:** committed TOML never contains secret material; put→get→delete via `FakeKeyring`; `secret_ref` in config resolves to keyring value; the two never cross.
- **Deliverable:** secret boundary proven with a fake keyring.

### P2.7 — Gate
- Run tests, clippy, fmt, doc across `sid-store` (+ `sid-secrets`). Then a multi-agent adversarial review of P2.4–P2.6 invariants. Fix findings.
- **Deliverable:** a finished, green store the SSH slice can consume.
