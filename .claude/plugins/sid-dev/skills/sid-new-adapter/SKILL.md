---
name: sid-new-adapter
description: Add a new external-library or OS integration to sid behind a sid-core trait - trait, impl crate, composition-root wiring, hygiene hook update, tests. Use when the app needs a capability that would otherwise name a concrete crate (redb, russh, git2, nix, zbus, ...) outside its adapter crate.
---

# sid-new-adapter

CLAUDE.md rule 1: every external lib / OS integration hides behind a trait owned by
`sid-core`; the concrete crate is named only in its impl crate. Platform-specific impls
(keyring, PTY, clipboard, notifications, editor-launch) are cfg-gated slots, never in core.

## Steps

1. **Trait** in `crates/sid-core/src/<area>.rs` (+ `pub mod <area>;` in `lib.rs`).
   `Send + Sync`, domain error types (not library errors), no concrete dependency added
   to `crates/sid-core/Cargo.toml`. Look at `svc.rs` (small) or `git.rs` (richer) as models.
2. **Impl crate** `crates/sid-<x>/`: add to root `Cargo.toml` `members` and
   `[workspace.dependencies]` (`sid-<x> = { path = "crates/sid-<x>" }`); depend on `sid-core`
   only (plus the concrete lib). Prefer a stateless struct with `new()`; shell out via
   `tokio::process::Command` (never `std::process`) for CLI-backed impls (model: `sid-svcctl`).
   Add `crates/sid-<x>/CLAUDE.md` (purpose, surface, invariants, test, landmines; <= 40 lines).
3. **Wire** it in exactly one place in `crates/sid/src/`: add the dependency to
   `crates/sid/Cargo.toml`; construct it at the composition point (a `*_registry.rs` for
   factories, like `db_registry.rs`/`git_registry.rs`, or the tab state that owns it) and pass
   `Arc<dyn Trait>` downstream. All I/O runs in event handlers / spawned tasks, never `render`.
4. **Hygiene**: add the concrete lib name to `core_forbidden` in
   `.claude/hooks/adapter-violation.sh` and a case in `.claude/hooks/test-adapter-violation.sh`.
   Update the seam table in `docs/ARCHITECTURE.md` and the crate graph.
5. **Tests**: unit tests on parsing/classification with captured fixtures; a fake for the
   trait where the app needs one; Docker-dependent integration tests are `#[ignore]` with a
   `scripts/test-*.sh` driver. Use the `rust-test-writer` agent if helpful.
6. **Audit**: spawn `adapter-auditor`; it must report no violations.
7. Gate (`sid-gate`), commit `feat(sid-<x>): ...`, push.

## Cross-platform
If the capability differs per OS, define one trait and put each OS impl behind
`#[cfg(target_os = ...)]` in the impl crate (or sibling impl crates); the composition root
picks via cfg. Core and domain code stay platform-free.
