---
name: mutation-hunt
description: Run cargo-mutants on a crate, identify surviving mutants (mutations that tests failed to catch), and generate tests that would have killed each one. Use when you want behavior coverage, not just code coverage — surviving mutants reveal tests that touch code without verifying it. Args: crate name (e.g., sid-core) or path. Requires `cargo install cargo-mutants`.
---

# Mutation Hunt

## Overview

Run `cargo-mutants` against a target crate, parse the surviving mutants
(mutations the test suite failed to catch), and dispatch fixer-subagents to
write the minimal tests that would have killed each one. Report kill-rate
before and after, list the commit SHAs the fixers produced, and surface any
remaining survivors.

Mutation testing answers a different question than `cargo test` and
`cargo llvm-cov`: not *"was this code run?"* but *"did the tests actually
verify what this code does?"*. A surviving mutant is the test suite
confessing that it touched a piece of logic without checking the outcome.

This skill is **slow**. Expect minutes to tens of minutes per crate
depending on test runtime and mutation count. Warn the user with an
estimate before starting.

## When to use

- After the unit-test pass when you want behavior coverage, not just code
  coverage.
- Before declaring a Plan task "done" if the spec says "behavior coverage"
  (per `CLAUDE.md`, anything in `sid-store`, the `Store` trait, session
  state).
- After receiving code review feedback that a test "doesn't actually verify
  anything" — let mutation testing confirm or refute.

## Args

Single positional argument, `$ARGUMENTS`:

| Form | Behavior |
|---|---|
| crate name (e.g., `sid-core`) | resolve to `crates/sid-core` and target that |
| path (e.g., `crates/sid-store`) | target that path directly |
| empty | target all crates in the workspace (very slow — warn loudly) |

## Process

### 1. Prerequisite check

Run:

```bash
command -v cargo-mutants
```

If absent, instruct the user:

```text
mutation-hunt requires cargo-mutants. Install with:
  cargo install cargo-mutants
Re-run this skill once it's on PATH.
```

Then exit gracefully without running anything else.

### 2. Baseline check

Mutation testing is meaningful only on a green baseline. Confirm:

```bash
cargo test --all-features -p <crate>
```

If anything fails, report the failures and stop. Do not proceed — survivors
on a red suite are noise.

If the user has just run `cargo test` successfully and wants to skip this
step for speed, accept that pragmatically but note it in the final report.

### 3. Resolve scope and estimate

Resolve `$ARGUMENTS` to a crate path. If empty, list all workspace members
and warn:

```text
No target specified. Will run cargo-mutants against every crate in the
workspace. This typically takes 15-60 minutes total for sid. Press Ctrl+C
to abort and pass a specific crate (e.g., /mutation-hunt sid-core).
Proceeding in 5 seconds...
```

Estimate runtime: mutation count is roughly proportional to function count
× expressions per function. As a rough cut, `rg --type rust -c '^(pub )?fn '
<crate>/src | awk -F: '{s+=$2}END{print s}'` × your typical test runtime is
a decent first-order estimate. Print the estimate.

### 4. Run the mutation sweep

```bash
cargo mutants \
  --package <crate> \
  --no-shuffle \
  --output target/mutants \
  --jobs 4
```

Flags rationale:

- `--no-shuffle`: deterministic order so subsequent runs compare cleanly.
- `--output target/mutants`: keep artifacts inside `target/`, gitignored.
- `--jobs 4`: parallel test runs. Tune down to 2 if the machine is small.

Stream output. Surface the running survivor count to the user every minute
or so by tailing `target/mutants/outcomes.json`.

If the user passes a path instead of a crate, use `--file` or `--in-diff`
as appropriate. See `cargo mutants --help` for current flag names.

### 5. Parse survivors

After the sweep finishes, parse:

```bash
cat target/mutants/outcomes.json
```

The JSON schema (current cargo-mutants):

```jsonc
{
  "outcomes": [
    {
      "scenario": {
        "Mutant": {
          "package": "sid-core",
          "file": "src/tab.rs",
          "line": 24,
          "function": "TabManager::next",
          "replacement": "replace `&&` with `||`",
          // ...
        }
      },
      "summary": "CaughtMutant" | "MissedMutant" | "Timeout" | "Unviable",
      // ...
    }
  ]
}
```

Filter `summary == "MissedMutant"`. These are the survivors.

For each survivor, extract:

- `package`
- `file`, `line`
- `function`
- `replacement` (human-readable description of what was mutated)
- Original source snippet (read `<file>` around `line`)
- Mutated source snippet (cargo-mutants writes the diff to
  `target/mutants/<id>/mutant.diff`)

### 6. Cap and prioritize

Hard cap: **20 mutants per invocation**. If there are more, focus on the
lowest line numbers first (heuristic: earlier code in a file is typically
more central — parsers, constructors, core methods — and survivors there
matter more than survivors in `Debug` impls at the bottom).

Report:

```text
49 mutants survived. Addressing the lowest-line-number 20 this run.
Remaining 29 will be reported at the end so you can re-invoke for them.
```

### 7. Dispatch fixer subagents

For each capped survivor, dispatch a fixer subagent using the Task tool.

Subagent dispatch rules:

- Group survivors by file. **One fixer per file at a time** to avoid race
  conditions on the test file. Multiple files can run in parallel.
- Use a `general-purpose` agent with the `sonnet` model.
- Each fixer's prompt includes:
  - Crate, file, line, function name.
  - Mutation description and the original/mutated snippets.
  - List of existing test files in the crate (`rg --type rust -l '#\[test\]'
    <crate>`).
  - Instruction: "write the minimal test that would fail under this
    mutation. Add it to the appropriate existing test file (same file as
    the code under test for unit tests; `tests/` for integration). Run
    `cargo test -p <crate> <test_name>` to confirm it passes on the real
    code. Commit with subject: `test(<crate>): kill mutant — <description>
    at <file>:<line>`."
  - Constraint: "do not modify production code. Tests only. If the only
    way to kill the mutant is to refactor production code, return that as
    a finding instead of doing it."

Wait for all fixers to return. Collect:

- Test names added.
- Commit SHAs.
- Any "refactor needed" findings.

### 8. Re-run cargo-mutants on affected files

After fixers complete, re-run on only the files that were touched:

```bash
cargo mutants \
  --package <crate> \
  --file <file1> --file <file2> ... \
  --no-shuffle \
  --output target/mutants
```

Compute kill-rate delta:

- Before: `survivors_initial / total_initial`
- After: `survivors_after / total_after`

### 9. Final report

Print to the user:

```text
Mutation hunt complete for <crate>.

Initial sweep:
  Total mutants:   124
  Caught:           75 (60.5%)
  Survived:         49 (39.5%)
  Timeout/Unviable:  0

This run:
  Mutants addressed:    20
  Tests added:          18
  Refactor-needed:       2 (see findings below)
  Commits:              <SHA1>, <SHA2>, ..., <SHA18>

Re-sweep on affected files:
  Mutants:              42
  Caught:               40 (95.2%)
  Survived:              2 (4.8%)
  Delta: +34.7pp kill rate on touched files

Remaining survivors (not addressed this run): 29
  Re-invoke /mutation-hunt <crate> to continue.

Findings (refactor needed):
  - crates/sid-core/src/foo.rs:88 — mutant in private impl detail, killing it
    requires extracting a helper. Suggested refactor: ...
  - ...
```

## Tool hints

- `cargo-mutants` output JSON schema lives in `target/mutants/outcomes.json`.
  When the cargo-mutants version updates and the schema shifts, read the
  file and adapt — don't assume the layout above is current.
- Group fixers by file. Parallel fixers on the *same* test file will race
  on writes. Sequential per file, parallel across files.
- Pre-existing failing tests poison the sweep. Confirm green first.
- Sometimes the only way to kill a mutant is to refactor production code
  (e.g., the mutated expression is in a private helper with no public
  observation point). Subagents should return that as a finding instead
  of forcing a brittle test. Surface these findings to the user.
- `--in-diff` is useful for "only test what I changed in this branch", but
  this skill targets a whole crate by default. Pass `--in-diff` only if the
  user explicitly asked for diff-scoped mutation testing.
- Watch for `Timeout` outcomes — they usually mean a mutated infinite loop.
  Not a survivor, not a kill. Mention separately in the report.

## Constraints

- **Tests only.** Fixers must not modify production code. If a kill requires
  refactoring, return a finding.
- **One commit per mutant.** Keeps the history readable and revertable.
- **Cap at 20 per invocation.** Mutation hunt sessions get long; predictable
  scope beats heroic runs.
- **Don't suppress survivors.** Never edit `.mutants.toml` to skip a mutant
  to make the kill rate look better. If a mutant is genuinely unkillable
  (e.g., logging, debug-only code), surface that as a finding and let the
  user decide whether to add it to the skip list manually.

## Example invocation

Input:

```text
/mutation-hunt sid-core
```

Behavior:

1. Check `cargo-mutants` is installed. Yes.
2. `cargo test -p sid-core` — green, takes 8s.
3. Estimate: ~40 functions × 8s/run = roughly 6 minutes. Print estimate.
4. Run `cargo mutants --package sid-core --no-shuffle --output
   target/mutants --jobs 4`. Stream output.
5. Parse `target/mutants/outcomes.json`. 12 survivors.
6. 12 < 20 cap, so address all.
7. Group: 4 in `tab.rs`, 5 in `action/mod.rs`, 3 in `keybind.rs`. Dispatch
   3 fixer subagents in parallel (one per file). Each handles its survivors
   sequentially within the file.
8. Wait for fixers. 11 tests added, 1 "refactor needed" finding.
9. Re-run cargo-mutants on the three files. 1 survivor remains (the one the
   subagent flagged as needing refactor).
10. Print final report.

## Verification before declaring done

- [ ] `cargo-mutants` prerequisite checked.
- [ ] Baseline `cargo test -p <crate>` green (or explicitly acknowledged).
- [ ] Initial mutation sweep ran to completion.
- [ ] Survivors parsed from `target/mutants/outcomes.json`.
- [ ] Capped at 20.
- [ ] Fixers grouped by file (no parallel writes to the same test file).
- [ ] Each fixer returned a test that compiles and passes on real code.
- [ ] Each fixer's test was verified to fail under the mutation.
- [ ] One commit per killed mutant, with the prescribed commit subject
      format.
- [ ] Re-sweep ran on affected files only.
- [ ] Kill-rate delta computed and reported.
- [ ] Remaining survivors (if any) listed for the next invocation.
- [ ] Refactor-needed findings (if any) surfaced separately.
