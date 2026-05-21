# sid-testing

Project-local Claude Code plugin holding two specialized testing skills for the
`sid` repo. Lives in `.claude/plugins/sid-testing/` so the rigor travels with
the code rather than relying on every contributor remembering to enable the
right global plugin.

## Why this plugin exists

`sid` is a daily-use tool. The repo-wide `CLAUDE.md` says tests are the price
of trusting your own tool, sets coverage targets of 80% workspace / 95%
critical-path, and demands adversarial tests in the same commit as features.
The two skills here are the *teeth* on that policy:

- Standard `cargo test` and `cargo llvm-cov --branch` answer *"was this line
  executed?"* — they cannot tell you whether a boolean decision's individual
  conditions each independently affect the outcome.
- Branch coverage does not catch mutations that survive — tests that *touch*
  code without *verifying* it.

`mc-dc-audit` and `mutation-hunt` close those two gaps.

## Skills

### `/mc-dc-audit <path>`

Audit a Rust file or crate for **Modified Condition / Decision Coverage**
(the avionics-grade boolean coverage criterion used by DO-178C Level A
software). Walks every `if`, match guard, and short-circuiting boolean
expression, decomposes each into its individual conditions, maps existing
tests against them, and reports gaps where a condition's independent
contribution to the decision is not demonstrated. Emits Rust test stubs that
would close each gap, written to `docs/coverage/`.

Use before declaring a critical-path module (anything in `sid-store`,
session-state code, the `Store` trait surface, persistence boundaries)
"fully tested".

### `/mutation-hunt <crate>`

Run `cargo-mutants` on the target crate, parse surviving mutants (mutations
the test suite failed to catch), and dispatch fixer-subagents that each
write the minimal test that would have killed one mutant. Reports kill-rate
delta and the SHAs of the resulting commits.

Use after the unit-test pass when you want **behavior coverage**, not just
code coverage. A surviving mutant is the test suite confessing that some
piece of logic isn't actually verified by anything.

## Prerequisites

Both skills assume a green baseline. Mutation testing in particular only
makes sense on a fully-passing test suite — survivors are meaningful only
when you can rule out flaky tests.

Install the underlying CLI tools once per machine:

```bash
cargo install cargo-llvm-cov   # branch coverage (used by mc-dc-audit when available)
cargo install cargo-mutants    # mutation testing (required by mutation-hunt)
```

## Invocation

After cloning the repo, run `/reload-plugins` in Claude Code to pick up the
local plugin, then:

```text
/mc-dc-audit crates/sid-core/src/tab.rs
/mc-dc-audit crates/sid-store
/mutation-hunt sid-core
/mutation-hunt crates/sid-store
```

Both commands accept either a path or a crate name. With no argument,
`mc-dc-audit` defaults to the most recently modified `.rs` file in the
workspace; `mutation-hunt` defaults to "all crates in the workspace" (slow —
expect minutes per crate).
