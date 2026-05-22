---
name: sid-gate
description: Run the full sid verification gate — cargo test, clippy, fmt, deny, doc — and report green/red per gate. Use before declaring any task done. Refuses to summarize as "ready" if any gate is red. Args: optional crate name to scope (`/sid-gate sid-store`); empty runs workspace-wide.
---

# sid Gate

## Overview

Run the verification gate that `CLAUDE.md` and `docs/TESTING.md` require
before declaring **any** task done. Reports a structured pass/fail
summary so you can see at a glance whether the change is shippable.

The gate is **non-optional**. `CLAUDE.md` is explicit: a code change is
not complete until tests land in the same commit, `cargo clippy
--all-targets --all-features -- -D warnings` and `cargo fmt --check`
pass, and adversarial cases have been considered. This skill is the
machine that walks that checklist for you.

## When to use

- Before claiming "task complete" on any plan-task or feature commit.
- Before opening a PR or pushing to a shared branch.
- After a refactor that touched multiple crates, to confirm nothing
  regressed.
- After a dependency bump, to verify nothing's leaking through
  feature-flag changes.
- Periodically while iterating on a long-running change, to catch
  drift early.

This is **not** a benchmark runner — `criterion` runs are separate and
slow. This gate is the green-light check for ordinary feature work.

## Args

Single positional argument, `$ARGUMENTS`:

| Form | Behaviour |
|---|---|
| empty | run gate over the entire workspace |
| crate name (e.g., `sid-store`) | scope to `-p <crate>` |
| `crate/path` (e.g., `crates/sid-store`) | resolve to crate name |
| `--full` | also run `cargo test --release` and `cargo doc` (slower) |

## Process

### 1. Resolve scope

If `$ARGUMENTS` is empty, set `SCOPE_FLAG=""` (workspace-wide).
Otherwise resolve to a crate name (strip leading `crates/`, trim trailing
slash) and set `SCOPE_FLAG="-p <crate>"`. Verify the crate exists by
listing members of the workspace root `Cargo.toml`.

If the user passed `--full`, set `FULL=1`. Otherwise `FULL=0`.

Report scope back to the user before running anything:

```text
Gate scope: workspace (or sid-store, etc.). Full mode: off.
Running 4 gates in parallel (test, clippy, fmt, deny)...
```

### 2. Run gates in parallel

Run these as background `Bash(run_in_background=true)` invocations so
they execute concurrently. Names are the keys used in the report.

| Gate | Command |
|---|---|
| `test` | `cargo test $SCOPE_FLAG --all-features` |
| `clippy` | `cargo clippy $SCOPE_FLAG --all-targets --all-features -- -D warnings` |
| `fmt` | `cargo fmt --check` (always workspace-wide; rustfmt has no `-p`) |
| `deny` | `cargo deny check` (always workspace-wide) |

If `FULL=1`, also run:

| Gate | Command |
|---|---|
| `doc` | `cargo test --doc $SCOPE_FLAG --all-features` |
| `release` | `cargo test --release $SCOPE_FLAG --all-features` |

If `cargo-deny` is not installed, mark `deny` as `skipped (not installed)`
and continue — `cargo install cargo-deny` is the documented fix.

Collect each background shell's stdout, stderr, and exit code as they
complete. **Don't poll** — use the Monitor tool or wait on completion
notifications.

### 3. Parse outcomes

For each gate, classify:

- **PASS** — exit 0, no warning text in last 20 lines of output
- **FAIL** — exit non-zero
- **WARN** — exit 0 but stdout/stderr contains warnings (clippy
  occasionally exits 0 with `warning:` lines; treat as FAIL because
  we passed `-D warnings`)
- **SKIPPED** — pre-flight detected the tool wasn't installed

For FAIL outcomes, extract the first failing test name (for `test`) or
the first lint name + file:line (for `clippy`). Don't dump full output
into the user-visible report — link to log files instead.

Write each gate's full output to `target/gate-logs/<gate>-<timestamp>.log`
so the user can inspect.

### 4. Render report

Output to the user using this exact format:

```text
GATE STATUS — scope: <scope>

  test     ✓  (847 passed, 2 ignored, in 12.4s)
  clippy   ✗  3 warnings:
              crates/sid-widgets/src/settings/animation.rs:142  unused_variables
              crates/sid-core/src/keybind_profile.rs:88        needless_collect
              ... (see target/gate-logs/clippy-*.log for the rest)
  fmt      ✓
  deny     ✓
  doc      —  (skipped, pass --full to include)
  release  —  (skipped, pass --full to include)

RESULT: ✗ NOT READY — 1 gate failed.

Fix the clippy warnings above before declaring done.
Re-run /sid-gate once you've addressed them.
```

For workspace-wide PASS:

```text
GATE STATUS — scope: workspace

  test     ✓  (1,943 passed, 0 failed in 47.2s)
  clippy   ✓
  fmt      ✓
  deny     ✓

RESULT: ✓ READY.

Per CLAUDE.md you can declare done. If you added a new pub item,
verify a doc test exists. If you touched concurrency, verify the
loom test exists.
```

### 5. Refusal logic

This is the rigor-bar bit. **Do not state "the change is ready"
anywhere in your output if any gate failed.** The skill exists to be
the mechanical check against optimism. The report's RESULT line is
the load-bearing sentence; downstream agents and the human reader
both rely on it.

If the user asks "is this done?" after a red `/sid-gate`, the answer
is "no, gate is red — see above". Don't soften it.

## Tool hints

- Background the long-running gates (`test`, `clippy`) using
  `run_in_background=true` on Bash. They take 10-60s; parallel saves
  half the wall time.
- `cargo fmt --check` is fast (~1s); run it first to fail fast on
  formatting before waiting on tests.
- `cargo deny check` reads `deny.toml` at repo root. If absent, treat
  as SKIPPED with a note.
- For per-crate scope, `cargo fmt -p <crate>` is NOT a flag rustfmt
  supports; the `fmt` gate is always workspace-wide. Note this in
  the report when scope was set.
- `target/gate-logs/` should be in `.gitignore` (it lives under
  `target/`, which already is).

## Output contract for downstream agents

When called by another agent (not the user directly), emit a JSON
summary as the *last* line of stdout so the parent can parse:

```json
{"scope":"sid-store","gates":{"test":"PASS","clippy":"FAIL","fmt":"PASS","deny":"PASS"},"ready":false}
```

The human-readable report above stays unchanged; the JSON line is
additional. This lets the gate be used as a subroutine inside the
`rust-test-writer` agent or any future automation.

## Anti-patterns

| Pattern | Why it's wrong |
|---|---|
| Reporting "ready" when clippy passed but a test failed | Defeats the purpose — the gate exists to be the canonical "ready" check |
| Skipping the `deny` gate to save time | `cargo deny check` is fast; license/advisory regressions are exactly the kind of thing humans miss |
| Running gates sequentially | Wastes wall time when they can run in parallel |
| Dumping all gate output into the report | The user wants the summary; the logs are for follow-up |
| Treating clippy `warning:` lines as PASS | We pass `-D warnings`; warnings are failures by policy |

## Verification before declaring done

- [ ] Scope was resolved and reported to the user before running.
- [ ] Gates ran in parallel (background Bash).
- [ ] Per-gate exit code + first failing detail extracted.
- [ ] Full output saved to `target/gate-logs/` for inspection.
- [ ] RESULT line states READY or NOT READY without ambiguity.
- [ ] JSON summary emitted as the final stdout line for agent
      consumption.
- [ ] If RESULT is NOT READY, the report points at the specific
      file:line of the first failure for each red gate.
