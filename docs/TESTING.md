# Testing

`sid` is a tool I depend on daily. Tests are the price of trusting my
own tool — without them, every change becomes a gamble against muscle
memory and silent regression. This document explains how the test suite
is organized, what each tier covers, and how to extend it for a new
feature.

The binding rules document is [`CLAUDE.md`] at the repo root; this file
is the operational guide. If the two ever conflict, `CLAUDE.md` wins.

[`CLAUDE.md`]: ../CLAUDE.md

## Philosophy

A code change is **not complete** until its tests land in the same
commit. Untested code is unfinished code. If a thing is hard to test,
that is a design signal — restructure it (extract traits, inject
dependencies, separate pure logic from I/O), don't skip the test.

Coverage targets:

- **80%+** overall across the workspace
- **95%+** on critical paths — anything touching persistence
  (`sid-store`), session state, auth/secrets, the `Store` trait
  surface, or data integrity invariants

Never weaken a coverage target to make a PR green. Flaky tests get
fixed or deleted; they are never marked `#[ignore]`.

## Tiers of tests

Each tier exists for a specific failure mode it catches that the others
don't. The full suite combines them.

### Unit tests

Live in `#[cfg(test)] mod tests` blocks alongside the code under test.
Small modules keep tests inline; larger ones split into a sibling
`tests.rs`.

```sh
cargo test -p sid-core
cargo test -p sid-store --lib
```

Required for every pure function, type, and trait that has any
behaviour worth verifying. For every `Result`-returning function, test
both `Ok` and `Err` paths. For every `Option`-returning function where
`None` is reachable, test both `Some` and `None`.

### Integration tests

Live in `tests/` at the crate root. These exercise end-to-end and
crate-boundary flows — anything that needs more than one type
collaborating, or filesystem / temp-DB setup.

```sh
cargo test -p sid-store --test workspaces
cargo test -p sid-widgets --test workspaces
cargo test -p sid                          # binary integration tests
```

Use `tempfile::TempDir` for any test that touches the filesystem; never
share state across tests, never depend on test order.

### Doc tests

Required on **every** `pub fn`, `pub struct`, `pub trait`, and
`pub enum`. The doc example is also part of the API contract — if it
breaks, the docs are wrong, the code is wrong, or both.

```sh
cargo test --doc -p sid-core
cargo test --doc -p sid-store
cargo test --doc --workspace
```

When an example needs external state (a real terminal, a network), mark
it `no_run`. **Never** use `ignore` — an ignored doc test is invisible
decay.

### Property tests

Driven by `proptest`. Used for any function with an invariant: round-
trips, idempotence, ordering, bounds, monotonicity.

```sh
cargo test -p sid-store --test codec_proptest
cargo test -p sid-core  --test action_registry_proptest
```

Where `sid` already uses proptest:

- `sid-store::codec` — `(version, payload)` postcard round-trip
- `sid-core::action::fuzzy` — scorer monotonicity (longer prefix never
  scores below shorter)
- `sid-core::keybind::KeyChord` — ordering helper round-trip
- `sid-core::tab::TabManager` — `next`/`prev`/`jump` cycling invariants
- Workspace path normalization — canonicalize then re-canonicalize is
  idempotent

Add a property test when you write a function whose correctness is
described by a *relationship between inputs* (commutativity, totality,
bounds) rather than by specific cases.

### Adversarial / boundary tests

For every happy-path test, write a try-to-break-it test. The catalogue
of things to attack:

- Malformed input (truncated bytes, wrong magic, mixed encodings)
- Boundary values: `0`, `1`, `usize::MAX`, `i64::MIN`, empty,
  single-element, multi-megabyte
- Invalid UTF-8 in path strings and on-disk blobs
- Concurrent access from multiple tasks or processes
- Operations interrupted mid-flight (drop, panic, signal)
- Disk full, read-only filesystem, permission denied
- Network failure, partial reads, slow loris
- Malformed config (`sid.toml` with bad keys, unknown fields)
- Partial writes (process killed between two redb tables)
- Corrupted state blobs (postcard with wrong version prefix)

These don't get a separate runner — they live next to the unit and
integration tests. They get a section of the PR description.

### Snapshot tests

Driven by `insta`. Used for any output that must stay stable: rendered
TUI buffers, serialized formats, generated text.

```sh
cargo test -p sid-ui
cargo insta review            # accept or reject pending snapshots
```

Where `sid` already uses insta:

- Rendered widget snapshots (render into a fixed `Buffer`, serialize to
  ASCII, golden-file it)
- `sid --help` output
- Serialized `SessionRecord` JSON (postcard byte stability is property-
  tested separately)
- Theme palette serialization

The first run writes a `.snap.new` file. Run `cargo insta review` to
accept (Enter) or reject (`r`) each pending snapshot.

### Criterion benchmarks

Driven by `criterion`. Used for hot paths: anything in the render loop,
the StatePersister flush, the action-registry fuzzy filter, etc.

```sh
cargo bench -p sid-job --no-run            # compile only
cargo bench -p sid-job                      # run
cargo bench -p sid                          # binary-level (e.g., draw_bench)
```

Targets:

- `App::handle_event` dispatch — a no-op event under 1 µs
- `RedbStore::recent_queries` reverse range scan
- `ActionRegistry::fuzzy` against ~200 actions
- Tab render frame: Ratatui buffer fill under the cosmos theme

Bench output saves to `target/criterion/`. Watch for regressions
greater than ±10% vs the committed baseline; CI fails on that
threshold once benchmarks are wired into CI.

### Loom tests

Driven by `loom`. Used for any code involving `Arc`, `Mutex`, channels,
atomics, or other shared-state primitives. Loom enumerates execution
orderings; a passing loom test is far stronger than a passing
`tokio::test`.

```sh
RUSTFLAGS="--cfg loom" cargo test --test loom_concurrency -p sid-job
```

Where loom applies in `sid`:

- `sid-job::JobQueue` — `Arc<Mutex<...>>` completion handoff between
  worker tasks and the render loop
- `StatePersister` debounce + concurrent dirty-marking (multiple
  widgets marking dirty during a flush must not lose writes)
- `SshPool` checkout (Plan 3)
- Detach IPC socket reader/writer (Plan 8)

Loom is *slow*. Restrict the iteration space (`LOOM_MAX_PREEMPTIONS`,
`LOOM_MAX_THREADS`) when iterating locally; CI runs the full space.

## Coverage

Install once:

```sh
cargo install cargo-llvm-cov
```

Then:

```sh
# Full HTML report
cargo llvm-cov --workspace --branch --html
xdg-open target/llvm-cov/html/index.html

# Single crate
cargo llvm-cov -p sid-store --branch --html

# Just print summary numbers
cargo llvm-cov --workspace --branch --summary-only
```

Use the report to find branches that no test reaches. If a critical-
path file (anything in `sid-store`, session, auth) is under 95%
coverage, add tests until it isn't. Never bump the target downward.

## Mutation testing

Coverage measures which lines run during tests. Mutation testing
measures which lines a test would *actually catch the corruption of*.

Install once:

```sh
cargo install cargo-mutants
```

Then run against the most safety-critical crate:

```sh
cargo mutants --in crates/sid-core
cargo mutants --in crates/sid-store
```

`cargo mutants` introduces small changes (negate a condition, swap an
operator, return `Default`) and reruns the tests. A surviving mutant is
a coverage gap: the test suite did not notice the change. Each
surviving mutant is a candidate test.

Run mutation testing periodically (not on every commit — it is slow).
Treat surviving mutants as bug reports.

## MC/DC

Modified Condition / Decision Coverage is the gold standard for
boolean-decision testing: for every condition in every decision, there
exists a test pair where flipping just that condition changes the
outcome. It's required by safety-critical certifications and a great
target for the `Store` trait.

Planned tooling: the `sid-testing` plugin's `/mc-dc-audit` skill. Once
plugin discovery is wired (see
[TROUBLESHOOTING.md → Plugin discovery](TROUBLESHOOTING.md#plugin-discovery-mc-dc-audit-not-found)),
invoke from within Claude Code:

```
/mc-dc-audit crates/sid-store
```

The skill scans for decisions, lists the conditions in each, and
generates the minimal test pair set. Until plugin discovery is fixed,
the workaround is to invoke the skill directly via the marketplace.json
path — see the troubleshooting doc.

## Fuzz testing

`cargo fuzz` (libFuzzer) is planned for any code path that handles
externally-controlled bytes:

- `sid-store::codec::decode_versioned` — arbitrary bytes must never
  panic, must return `Err` on malformed input
- Workspace `.sid/_metadata.sid` parser (Plan 2)
- SQL lexer for the Database tab (Plan 4)
- SSH config parser (Plan 3)

In the current build, `proptest` strategies over `Vec<u8>` cover the
same surface for the codec. Real `cargo fuzz` setup with corpus
checkpoints is deferred — when it lands, it gets its own
`fuzz/` directory at the workspace root.

## What to test for a new feature

A checklist for every feature commit:

- [ ] **Unit tests** for the core behaviour (happy path + each branch)
- [ ] **Doc test** on every new `pub fn` / `pub struct` / `pub trait`
- [ ] **Adversarial test** — at least one input that tries to break it
      the way a real user might (empty, huge, malformed, racy)
- [ ] **Property test** if there's an invariant (round-trip, ordering,
      idempotence, bounds)
- [ ] **Insta snapshot** if there's stable output (rendered UI,
      serialized format, generated text)
- [ ] **Criterion benchmark** if it's on a hot path (render loop, store
      hot read, palette filter)
- [ ] **Loom test** if it touches shared state (`Arc`, `Mutex`, atomics,
      channels)
- [ ] **Integration test** if it's an end-to-end flow the user runs
- [ ] All gates green: `cargo test --workspace --all-features`,
      `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo fmt --check`

Anything *intentionally* untested gets a one-sentence justification in
the commit body — e.g., "code path is unreachable per the type system".

## Test counts by crate

A snapshot of the current test count by crate (subject to change as the
project grows). Run `cargo test -p <crate> 2>&1 | grep 'test result'` to
get the latest:

| Crate | Tests |
|:---|---:|
| `sid-core` | 429 |
| `sid-widgets` | 235 |
| `sid-store` | 123 |
| `sid-ui` | 73 |
| `sid-git` | 61 |
| `sid-job` | 22 |
| `sid` (binary) | varies (smoke tests + benchmarks) |

These numbers include unit, integration, doc, and property tests.

## Tools at a glance

| Tool | Install | Use |
|:---|:---|:---|
| `proptest` | dep in `Cargo.toml` | property-based tests |
| `insta` | dep in `Cargo.toml` | snapshot tests |
| `criterion` | dep in `Cargo.toml` | benchmarks |
| `loom` | dep, gated `#[cfg(loom)]` | concurrency model-check |
| `fail` | dep, gated by feature | failpoint injection |
| `dhat` | dep, gated by feature | heap profiling |
| `cargo-llvm-cov` | `cargo install cargo-llvm-cov` | coverage |
| `cargo-mutants` | `cargo install cargo-mutants` | mutation testing |
| `tempfile` | dep in `Cargo.toml` | temp dirs / files |

For the full project-wide testing conventions and where each tool
applies in `sid`, see [`CLAUDE.md`].

---

See also:

- [DEVELOPMENT.md](DEVELOPMENT.md) — how to extend `sid`
- [CONTRIBUTING.md](CONTRIBUTING.md) — PR rules
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — common problems
