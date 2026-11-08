---
name: sid-gate
description: Run sid's 3-step verification gate (cargo fmt --check, clippy -D warnings, test) niced with -j 3 and print one RESULT line. Use before declaring any task done or pushing. Arg - optional crate name to scope clippy/test (fmt is always workspace-wide).
---

# sid-gate

The gate is non-optional before "done" or a push. Never say work is ready unless the
RESULT line below is GREEN.

## Run

`just gate` does all of it. Without `just`, run the three steps and capture the real exit
code of each (`| tail` hides failures; in zsh read `${pipestatus[1]}`):

```bash
nice -n 10 cargo fmt --all --check
nice -n 10 cargo clippy --workspace --all-targets -j 3 -- -D warnings
nice -n 10 cargo test --workspace -j 3
```

Scoped (arg = crate, e.g. `sid-store`): `just gate-fast sid-store` (fmt stays workspace-wide).
Iterate scoped, run the full gate once near the end - Murphy is using the machine.

## Procedure

1. Run fmt first (fast). If red, `nice -n 10 cargo fmt --all`, then re-check.
2. Run clippy, then test. Redirect long output to a file under `target/` (not `/tmp`) and
   report only the first failing lint/test with `file:line`.
3. One `sid-privfs` sudo test can flake under parallel threads: rerun it alone
   (`nice -n 10 cargo test -p sid-privfs -j 3 -- --test-threads=1`). Any other failure is real.
4. Expected size today: ~51 suites / ~1600 tests. A suite count drop means something stopped compiling.
5. Print exactly one final line:
   `RESULT: GREEN (fmt ok, clippy ok, test N passed)` or
   `RESULT: RED - <step>: <first failure, file:line>`

## Not part of this gate
Docker-backed `#[ignore]`d tests (`just test-ssh`, `just test-db`) and visual checks
(`sid-capture` skill) - run them when the change touches those areas.
