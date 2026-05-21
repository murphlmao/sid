---
name: mc-dc-audit
description: Audit a Rust file or crate for MC/DC (Modified Condition / Decision Coverage). Walks every boolean decision (if, match guards, &&/|| expressions), maps each condition to existing tests that exercise its true/false values, identifies gaps where independence isn't demonstrated, and generates test stubs to close them. Use when you want SQLite-tier rigor on boolean decision logic, or before declaring a critical-path module fully tested. Args: file path or crate directory.
---

# MC/DC Audit

## Overview

Audit a Rust file or crate for **Modified Condition / Decision Coverage**: the
avionics-grade criterion that demands every individual condition inside every
boolean decision be shown to *independently* affect the decision's outcome.
Branch coverage answers "did we evaluate this `if` both ways?". MC/DC answers
"for `if a && b`, did we show `a` flipping alone changes the result, and `b`
flipping alone changes the result?".

This skill does not run coverage instrumentation. It performs a structured
audit:

1. Enumerate every boolean decision in the source.
2. Decompose each decision into individual conditions.
3. Inspect existing tests and map them against each condition's truth values.
4. Report gaps where independence is not demonstrated.
5. Generate concrete Rust test stubs that would close each gap.

Output is a report under `docs/coverage/YYYY-MM-DD-<scope>-mcdc.md` plus a
one-line summary on stdout. **Do not commit the report.** Leave it for the
user to review.

## When to use

- Before declaring a critical-path module "fully tested" (anything in
  `sid-store`, the `Store` trait, session-state code, persistence, auth).
- After a feature lands and `cargo test` passes — to verify the boolean
  logic was actually probed, not merely executed.
- When investigating a logic bug that slipped past the suite — MC/DC gaps
  are where logic bugs live.

## Args

Single positional argument, `$ARGUMENTS`:

| Form | Behavior |
|---|---|
| `.rs` file path | audit that file |
| crate directory (e.g., `crates/sid-core` or `sid-core`) | audit all `src/**/*.rs` in the crate |
| empty | default to the most recently modified `.rs` file in the workspace |

## Process

### 1. Scope resolution

Parse `$ARGUMENTS`:

- If it ends in `.rs` and exists, scope = that file.
- If it is a directory under `crates/` or a bare crate name resolving to one,
  scope = `find <crate>/src -name '*.rs' -type f`.
- If empty, scope = the most recently modified `.rs` file:
  ```bash
  find crates -name '*.rs' -type f -printf '%T@ %p\n' | sort -n | tail -1 | cut -d' ' -f2
  ```

Report the scope back to the user before doing any heavy work.

### 2. Decision enumeration

For each file in scope, enumerate boolean decisions. Use `rg` (ripgrep) with
the patterns below. **Record file:line for every hit.**

```bash
# if statements with short-circuiting boolean operators
rg --type rust -n 'if .*(\&\&|\|\|)' <file_or_dir>

# match guards
rg --type rust -n 'if .+ =>' <file_or_dir>

# bare if without operators (still a decision — one condition)
rg --type rust -n '^\s*if [^{]+\{' <file_or_dir>

# while loops with conditions
rg --type rust -n 'while .+(\&\&|\|\|)' <file_or_dir>

# boolean assignments and combinators
rg --type rust -n '\.filter\(|\.any\(|\.all\(|\.take_while\(|\.skip_while\(' <file_or_dir>

# let-else and let-chains
rg --type rust -n 'let .+ else\b|let .+ = .+ (\&\&|\|\|)' <file_or_dir>
```

For complex match patterns (nested, slice patterns, or-patterns), flag as
`manual review` and move on. Do not try to mechanically derive conditions
from them — the false positive rate is too high.

If macros are obscuring decisions and the file uses non-trivial macros, run
`cargo expand --lib -p <crate>` and re-scan the expanded output. Only do this
when there's reason to suspect macro-generated branches (e.g., `tokio::select!`,
`async-trait`, derive macros generating `match`).

### 3. Condition decomposition

For each decision, list its **conditions** — the leaf operands of the boolean
expression after splitting on `&&` and `||`.

Examples:

| Decision | Conditions |
|---|---|
| `if x && y` | `x`, `y` |
| `if a && (b \|\| c)` | `a`, `b`, `c` |
| `if foo(z) > 0 && bar.is_some()` | `foo(z) > 0`, `bar.is_some()` |
| `match v { Some(n) if n > 0 && n < 10 => ...}` | `n > 0`, `n < 10` |
| `xs.iter().filter(\|x\| x.active && !x.stale)` | `x.active`, `!x.stale` |

A single-condition decision (e.g., `if foo()`) still counts — MC/DC degenerates
to branch coverage in that case (need a `true` and a `false` case).

### 4. Test mapping

This is the approximate part. The skill does not instrument or trace; it
inspects test names and bodies.

For each condition, search the crate's test surface:

```bash
# unit tests in the same file
rg --type rust -n '#\[(test|tokio::test)\]' <file>

# unit tests in sibling mod tests
rg --type rust -n '#\[(test|tokio::test)\]' <crate>/src

# integration tests
rg --type rust -n '#\[(test|tokio::test)\]' <crate>/tests

# proptest cases
rg --type rust -n 'proptest!' <crate>
```

Read each candidate test body. For each condition, decide:

- **True covered**: a test passes inputs that make this condition `true`.
- **False covered**: a test passes inputs that make this condition `false`.
- **Independence demonstrated** (MC/DC): there exist two test cases that
  differ only in this condition's value, and the decision's outcome differs
  between them.

If the test name encodes the case (e.g., `test_decide_with_empty_tabs`),
trust it. If unclear from name alone, read the body. If still unclear, mark
the condition `needs manual confirmation` rather than guessing.

For property-based tests (`proptest!`), check whether the strategy spans
both `true` and `false` for this condition. If the strategy is unrestricted
over the condition's input domain, count it as covering both — but flag for
review if the strategy uses `prop_filter` or similar restrictions.

### 5. Gap report

Produce a markdown report. Each decision gets a row in this table:

```markdown
| File:Line | Decision | Conditions | True-cases | False-cases | Independence | Gap |
|---|---|---|---|---|---|---|
| `tab.rs:34` | `if x && y` | `x`, `y` | `x=T,y=T`: `test_next_wraps` | `x=F,y=*`: NONE; `x=*,y=F`: `test_jump_oob` | `x`: NO; `y`: NO | `x` never flips alone; `y` never flips alone |
| `store.rs:88` | `match guard if n > 0 && n < 10` | `n > 0`, `n < 10` | `test_load_valid` (n=5) | `n<=0`: `test_load_zero`; `n>=10`: NONE | `n > 0`: YES; `n < 10`: NO | upper bound never exercised |
```

After the per-decision table, summary stats:

```markdown
## Summary

- **Scope:** crates/sid-core (12 files, 47 decisions, 89 conditions)
- **Independence demonstrated:** 61 / 89 (68.5%)
- **Critical gaps:** 14 conditions on persistence-touching code
- **Manual review needed:** 4 nested match patterns
```

### 6. Stub generation

For each gap, append a Rust test stub to the report under a `## Stubs`
section. The stub must:

- Name the test `mcdc_<file_stem>_line<N>_<condition>_flips_decision`.
- State in a comment which other conditions are held fixed.
- State in a comment which condition is varied.
- Include a `TODO:` line for the user to fill in the actual setup.
- Compile as written (i.e., be a syntactically valid Rust test).

Example:

```rust
#[test]
fn mcdc_tab_line34_x_flips_decision() {
    // MC/DC: hold y = true; flip x from true to false;
    //        assert the decision (if x && y) flips outcome.
    // TODO: instantiate TabManager with controlled state so that
    //       `y` is true and `x` can be set to either value.
    todo!("MC/DC stub from /mc-dc-audit");
}
```

For match guards, generate one stub per condition in the guard. For
iterator combinators (`.filter`, `.any`, ...), the stub asserts on the
collected output rather than on the closure directly.

### 7. Output

Write the full report to:

```text
docs/coverage/<YYYY-MM-DD>-<scope-slug>-mcdc.md
```

where `<scope-slug>` is the basename of the scope (e.g., `tab.rs` becomes
`tab-rs`, `crates/sid-core` becomes `sid-core`). Create the directory if it
doesn't exist:

```bash
mkdir -p docs/coverage
```

**Do not commit the report.** Tell the user where it is and what the top-line
summary is. Example final message:

```text
MC/DC audit complete.
  Report: docs/coverage/2026-05-21-sid-core-mcdc.md
  Independence: 61/89 (68.5%)
  Gaps: 14 (10 in critical-path code under crates/sid-store)
  Manual review: 4 nested match arms
Review the report and run the stubs through the TDD cycle.
```

## Tool hints

- Use `rg --type rust -n ...` for all enumeration. Faster and more accurate
  than `grep`. `--type rust` excludes `target/` and Cargo metadata.
- Use `cargo expand --lib -p <crate>` only when macro-generated branches are
  suspected. It is slow.
- Don't be exhaustive on complex `match` patterns. Flag as `manual review`
  and move on. The cost of a wrong audit is worse than the cost of an
  honest "I didn't analyze this".
- Don't trust test names blindly — but use them as the first signal. When
  the name and body disagree, the body wins.
- For `Result`-returning functions, `CLAUDE.md` requires `Ok` and `Err`
  paths both tested. That's an MC/DC requirement on the implicit decision
  inside `?` or `match result {}`. Audit these explicitly.
- For `Option`-returning, same rule on `Some`/`None`.

## Example invocation

Input:

```text
/mc-dc-audit crates/sid-core/src/tab.rs
```

Behavior:

1. Scope = `crates/sid-core/src/tab.rs`.
2. Run `rg` against the patterns above.
3. For each decision found, decompose conditions.
4. Walk `crates/sid-core/src/tab.rs` `#[cfg(test)] mod tests` plus
   `crates/sid-core/tests/`.
5. Build the table.
6. Generate stubs for each gap.
7. Write to `docs/coverage/2026-05-21-tab-rs-mcdc.md`.
8. Print summary.

Expected output to user:

```text
MC/DC audit complete for crates/sid-core/src/tab.rs.
  Decisions: 6
  Conditions: 11
  Independence demonstrated: 7/11 (63.6%)
  Gaps: 4
  Report: docs/coverage/2026-05-21-tab-rs-mcdc.md
  Stubs generated: 4 (see report)
```

## Anti-patterns

| Pattern | Why it's wrong |
|---|---|
| Counting "any test that touches the function" as covering all conditions | MC/DC requires per-condition independence, not function-level execution |
| Skipping `proptest!` cases because they're hard to analyze | They often *do* cover MC/DC; check the strategy |
| Auto-committing the report | The report belongs to the user; let them decide what's worth tracking |
| Treating `manual review` as 0% | It's not a fail; it's an honest acknowledgment that machine analysis is wrong here |
| Generating stubs that don't compile | A stub the user can't paste-and-fix is dead weight |

## Verification before declaring done

- [ ] Scope was resolved and reported back to the user before scanning.
- [ ] Every file in scope was enumerated.
- [ ] Each decision is listed with file:line, the raw expression, and its
      decomposed conditions.
- [ ] Each condition has a True-cases / False-cases column populated (or
      explicitly `NONE`).
- [ ] Independence column is per-condition, not per-decision.
- [ ] Stubs compile (use `todo!()` for body, not invalid syntax).
- [ ] Report is at `docs/coverage/YYYY-MM-DD-<scope>-mcdc.md`.
- [ ] Report is **not** committed.
- [ ] Top-line summary printed: scope, independence %, gap count.
