# CLAUDE.md — sid project

## Philosophy

`sid` is a tool I depend on daily. Tests are the price of trusting my own tool — without them, every change becomes a gamble against muscle memory and silent regression. This document is a set of binding rules for any Claude Code session touching this repo: rigor is non-negotiable, untested code is not "done", and adversarial thinking belongs in the same commit as the feature. Treat the directives below as gates, not aspirations.

---

## Testing as a development gate

- A code change is **not complete** until its tests land in the **same commit**.
- New functions, types, and traits **must** have unit tests before being considered done.
- Modified code **must** have updated or new tests covering the change.
- If something is hard to test, **restructure it** (extract traits, inject dependencies, separate pure logic from I/O). Never skip the test because "the shape made it awkward".
- Coverage targets:
  - **80%+** overall across the workspace.
  - **95%+** on critical paths — anything touching persistence (`sid-store`), session state, auth/secrets, the `Store` trait surface, or data integrity invariants.
- Coverage is measured with `cargo llvm-cov` or `cargo tarpaulin` (CLI tools — install with `cargo install cargo-llvm-cov`). Tracked in CI once CI lands.
- Never weaken a coverage target to make a PR green.

---

## Rust-specific testing requirements

- **Unit tests** live in `#[cfg(test)] mod tests` blocks alongside the code under test. Same file when small; sibling `tests.rs` when large.
- **Integration tests** live in `tests/` at the crate root for end-to-end and crate-boundary flows.
- **Doc tests** are required on **every** `pub fn`, `pub struct`, `pub trait`, and `pub enum`. If the doc example is non-trivial, mark it `no_run` only when execution requires external state; never `ignore`.
- **`#[should_panic]`** tests for every code path that must panic (e.g., `TabManager::new(vec![])` asserts non-empty).
- For every `Result`-returning function: test both `Ok` and `Err` paths.
- For every `Option`-returning function where `None` is reachable: test both `Some` and `None`.
- Any `unsafe` block requires:
  - A `// SAFETY:` comment justifying the invariants relied on.
  - Extensive tests of the safety contract boundaries.
  - Miri must pass: `cargo +nightly miri test`.
- Plan 1 contains no `unsafe`. If a future plan introduces it, the unsafe block does not merge without the three items above.
- Lint and format gates (must pass before declaring done):
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo fmt --check`
- Run the test suite under `--release` periodically (at minimum before tagging a release) to catch optimization-sensitive bugs.

---

## Adversarial testing

For every happy-path test, write a try-to-break-it test. Examples of inputs and conditions to attack:

- Malformed input (truncated bytes, wrong magic, mixed encodings)
- Boundary values: `0`, `1`, `usize::MAX`, `i64::MIN`, empty, single-element, huge (multi-MB)
- Invalid UTF-8 in path strings and on-disk blobs
- Concurrent access from multiple tasks or processes
- Operations interrupted mid-flight (drop, panic, signal)
- Disk full, read-only filesystem, permission denied
- Network failure, partial reads, slow loris
- Malformed config (`sid.toml` with bad keys, unknown fields, wrong types)
- Partial writes (process killed between two redb tables)
- Corrupted state blobs (postcard with wrong version prefix)

Tools:

- **`proptest`** for property-based testing on functions with invariants — round-trips, idempotence, ordering, bounds, monotonicity.
- **`cargo fuzz`** (libFuzzer) for parsers, deserializers, and any input-handling code.
- **`loom`** for any code involving `Arc`, `Mutex`, channels, atomics, or other shared-state primitives. Gate loom tests behind `#[cfg(loom)]` and a `loom` feature.
- At least one test per function that **tries to make it fail the way a real user might** — not just the way the code expects.

---

## Consistency and regression prevention

- Every bug fix starts with a **failing regression test**, then the fix that makes it pass. The test must reproduce the bug before the fix is written.
- **`insta`** for snapshot tests of any output that must stay stable: CLI output, serialized formats, generated files, rendered TUI buffers.
- Integration tests cover every end-to-end flow actually in use — if a code path runs on `sid` startup, it has an integration test.
- Tests must be **deterministic**:
  - No `SystemTime::now()` in assertions (inject a clock, or freeze with a test helper).
  - No test-order dependencies (parallel test runners must produce the same result).
  - No shared mutable state across tests (use `tempfile::TempDir` for FS-touching tests).
- Flaky tests get **fixed or deleted**. **Never** `#[ignore]`d. An ignored test is invisible decay.

---

## Performance testing

- **`criterion`** for benchmarks on critical paths. Commit baseline `Cargo.toml` and baseline results.
- Fail CI if a benchmark regresses **≥10%** vs baseline.
- **`dhat`** for heap profiling on long-running or data-heavy code (the redb-backed `Store` impl, the render loop under load, the JobQueue under burst).
- **Profile before optimizing.** Never optimize without measurement. "Looks slow" is not a benchmark.
- Watch compile time when adding a dependency. If a new dep adds >5s to clean build, justify it in the commit body.

---

## Other forms of testing

- **Smoke tests on startup**: env vars resolved, XDG paths usable, DB openable, permissions OK. Fail fast with a clear error.
- **Contract tests** for external APIs and subprocesses (use `wiremock` or an equivalent fake). Plan 4 introduces the first DB client, which gets contract tests.
- **Migration tests** for any persisted format or schema change. Forward compatibility is required; backward compatibility is required where it matters (anything a running detached process might write).
- **Chaos / failure injection** for stateful or networked code. Use the `fail` crate to inject failpoints; a hand-rolled mock works too.
- **Cross-platform CI** runs on Linux and macOS. `directories` is XDG-only on Linux/macOS; Windows path resolution differs and must be tested separately if/when Windows is supported.

---

## Workflow rules

- Run `cargo test --all-features --workspace` before declaring **any** task done.
- Run `cargo clippy --all-targets --all-features -- -D warnings` before declaring done.
- If tests fail: **fix the code or the test**. **Never** `#[ignore]`, comment out, or weaken assertions to make red turn green.
- Prefer TDD: write the failing test first, see it fail, then write the implementation. At minimum, write the test signature and expected behavior before the implementation.
- When adding a dependency:
  - State why in the commit body.
  - Confirm `cargo test --all-features --workspace` still passes.
  - Check `cargo deny check` does not regress.
  - Confirm the new dep does not contradict the adapter pattern (no external crate names leak into `sid-core` or `sid-widgets`).
- When finishing a task, report to the user **what was tested**, including adversarial cases. Bullet list. No fluff.

---

## Cargo.toml conventions

- Test-only deps go in `[dev-dependencies]`. They must **never** leak into the main binary.
- Feature flags gate expensive or environment-specific tests. Examples: `loom` (model-checking), `dhat-heap` (profiling), `slow-tests` (large fuzz corpora).
- Standard dev tooling for new crates includes: `proptest`, `criterion`, `insta`, and a fuzz harness if the crate handles parsed input.

---

## Commit conventions

- Production code and tests land in the **same commit**. No "tests will follow" commits.
- Commit body notes which failure modes were considered and tested.
- Any intentionally-untested case has a one-sentence justification in the body explaining why — e.g., "code path is unreachable per the type system".
- Conventional Commits prefix: `feat(<crate>)`, `fix(<crate>)`, `chore`, `docs`, `test`, `refactor`, `perf`.

---

## Where each testing tool applies in sid (project-specific)

This list is precise about what is in scope for Plan 1 versus upcoming plans. When you add code to one of these surfaces, the corresponding tool is the **default** unless there is a clear reason otherwise.

- **`proptest`**
  - `sid-store::codec` — versioned-postcard round-trip on arbitrary `(version, payload)` pairs.
  - `sid-core::action::fuzzy` — scorer monotonicity: longer prefix match never scores below shorter.
  - `sid-core::keybind::KeyChord` — round-trip through the ordering helper.
  - `sid-core::tab::TabManager` — `next`/`prev`/`jump` cycling invariants; `next` then `prev` returns the original active index.
  - Workspace path normalization (Plan 2): canonicalize then re-canonicalize is idempotent.
- **`cargo fuzz`**
  - `sid-store::codec::decode_versioned` — arbitrary bytes must never panic, must never invoke UB, must return `Err` on malformed input.
  - Workspace `.sid/_metadata.sid` parser (Plan 2).
  - SQL lexer for the Database tab (Plan 4).
  - SSH config parser (Plan 3).
- **`loom`**
  - `sid-job::JobQueue` — `Arc<Mutex<...>>` completion handoff between worker tasks and the render loop.
  - `StatePersister` debounce + concurrent dirty-marking (Plan 1, Task 32). Multiple widgets marking dirty during a flush must not lose writes.
  - `SshPool` checkout (Plan 3).
  - Detach IPC socket reader/writer (Plan 8).
- **`criterion`**
  - `App::handle_event` dispatch hot path (Plan 1, Task 33). Target: a no-op event under 1us.
  - `RedbStore::recent_queries` reverse range scan (Plan 4).
  - `ActionRegistry::fuzzy` filter against a registry of ~200 actions.
  - Tab render frame: ratatui buffer fill under the cosmos theme.
- **`insta`**
  - Rendered widget snapshots — render into a fixed `Buffer`, serialize to ASCII, golden-file it.
  - `sid --help` output.
  - Serialized `SessionRecord` JSON (postcard byte stability is separately property-tested).
  - Theme palette serialization.
- **`wiremock`** (deferred to Plan 4)
  - DB client tests (Postgres).
  - SSH-over-mock-transport tests (Plan 3) — may use a hand-rolled mock instead.
- **`fail` crate**
  - `StatePersister` — simulate write failure mid-flush; assert no dirty state is dropped.
  - `RedbStore` — simulate disk full, simulate corrupted blob on read.
  - `JobQueue` — simulate task panic; assert queue stays usable.
- **`miri`**
  - Covers any `unsafe` blocks added later. None in Plan 1.
- **Cross-platform CI**
  - `directories` is XDG-only on Linux/macOS; Windows path layout differs.
  - Test path resolution on both Linux and macOS; document Windows behavior when/if supported.

---

## Missing dev-dependencies to add

These were added to `[workspace.dependencies]` in the same commit as this file. Pinned exactly:

- `criterion = { version = "0.7", default-features = false, features = ["html_reports", "cargo_bench_support"] }` — benchmarking.
- `loom = "0.7"` — concurrency model checker; gate use behind `#[cfg(loom)]`.
- `fail = "0.5"` — failpoint injection for chaos tests.
- `dhat = "0.3"` — heap profiling.

**Not added:**

- `cargo-llvm-cov` / `cargo-tarpaulin` — these are CLI binaries, not crates. Install with `cargo install cargo-llvm-cov`.
- `wiremock` — only matters once Plan 4 lands. Add then.

Already present in workspace deps and kept: `insta = "1"`, `proptest = "1"`, `tempfile = "3"`.

---

## Adapter pattern enforcement (binding)

This is a structural rule, not a style preference:

- Widget code (`crates/sid-widgets/`) **must never** name an external crate — `git2`, `russh`, `redb`, etc. It names only traits from `sid-core`.
- `sid-core` **must not** depend on `ratatui`, `tokio`, or `redb`. It owns `crossterm` only because it owns the `Event` type.
- `sid-store` is the **only** crate that depends on `redb`. If another crate ends up needing `redb`, that is a design bug.
- Concrete adapter impls (e.g., `Git2Provider`, `RussshClient`) live in their own crates (`sid-git`, `sid-ssh`, ...). The binary crate `sid/` is the only place wiring concrete impls to trait slots.
- Any PR that violates this rule fails review. No exceptions for "just this once".

---

## Documentation rules

- Every public item gets a doc comment. Doc tests where a usage example is informative.
- `CLAUDE.md` (this file) is the binding rules document. When rules change, edit this file in the same commit.
- Plan docs in `docs/superpowers/plans/` are the source of truth for task ordering. Spec docs in `docs/superpowers/specs/` are the source of truth for design intent. Do not silently diverge from either.

---

## What to do when uncertain

- If a directive in this file conflicts with the user's instruction, **ask** before bypassing the directive.
- If a task feels too small for a test, write the test anyway — the smallest tests catch the longest-lived bugs.
- If a test is hard to write because of structure, treat that as a design signal, not a testing problem.
