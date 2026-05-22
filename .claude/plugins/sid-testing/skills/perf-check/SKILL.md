---
name: perf-check
description: Compare current criterion benchmark results against the saved baseline and flag any regression over the CLAUDE.md threshold (default 10%). Wraps the sid-mcp `criterion_compare` tool. Use after any change touching hot paths (render loop, store hot reads, action fuzzy filter, JobQueue handoff) and before declaring a perf-sensitive task done. Args: optional crate name and threshold percentage.
---

# Perf Check

## Overview

Compare every criterion benchmark's latest run against its saved baseline
and surface regressions over the CLAUDE.md threshold (10% by default).
Reads structured comparison data from the sid-mcp `criterion_compare`
tool and presents an actionable per-bench breakdown.

CLAUDE.md is explicit: **fail CI if a benchmark regresses ≥10% vs baseline.**
This skill is how you know whether your change is about to do that.

## When to use

- After any change touching the render loop, `RedbStore::recent_queries`,
  `ActionRegistry::fuzzy`, `JobQueue` handoff, or `App::handle_event`.
- Before declaring a perf-sensitive plan task done.
- After a dependency bump (especially ratatui / tokio / redb) — runtime
  behaviour can shift even when API signatures don't.
- Periodically, to catch slow drift.

This skill **does not run benchmarks**. It reads the comparison from
disk. Run `cargo bench` first to refresh `target/criterion/`.

## Args

| Form | Behaviour |
|---|---|
| empty | check every bench in every crate |
| crate name (e.g., `sid-core`) | scope to one crate |
| `--threshold <pct>` | override the 10% regression threshold (use higher to silence noise on small benches) |

## Process

### 1. Confirm benches have been run

```bash
ls target/criterion 2>/dev/null
```

If empty/absent, abort with a clear message:

```text
target/criterion is empty. Run `cargo bench -p <crate>` to populate
the baseline, then re-invoke /perf-check.
```

If only a `new/` exists but no `base/`, the user hasn't established a
baseline yet. Recommend `cargo bench --save-baseline initial` to seed
one, then re-run after the change.

### 2. Call the MCP tool

For workspace-wide:

```text
mcp__sid__criterion_compare({})
```

For a single crate:

```text
mcp__sid__criterion_compare({ "crate_name": "sid-core" })
```

With a custom threshold (e.g., 5% for very tight benches):

```text
mcp__sid__criterion_compare({ "threshold_pct": 5.0 })
```

The tool returns a `CriterionResult`:

```jsonc
{
  "benches": [
    {
      "name": "sid-job/job_queue/handoff",
      "baseline_ns": 142.3,
      "latest_ns": 167.8,
      "delta_pct": 17.9,
      "regressed": true
    },
    ...
  ],
  "threshold_pct": 10.0,
  "regression_count": 1
}
```

### 3. Pull recent commits for context

If `regression_count > 0`, call:

```text
mcp__sid__recent_commits({ "crate_name": "<crate-with-regression>", "count": 5 })
```

The user wants to know which recent change probably caused the regression.

### 4. Render the report

```text
PERF CHECK — sid criterion vs baseline

Threshold: 10.0% (per CLAUDE.md)
Benches compared: 12
Regressions: 1

  BENCH                                       BASELINE     LATEST     DELTA    STATUS
  sid-core/app/handle_event_noop              0.84 µs      0.87 µs    +3.5%    ✓ within threshold
  sid-job/job_queue/handoff                   142.3 ns     167.8 ns   +17.9%   ✗ REGRESSED
  sid-store/recent_queries/reverse_scan       12.4 µs      11.9 µs    -4.0%    ✓ improvement
  sid-core/action/fuzzy/200_actions           41.1 µs      42.3 µs    +2.9%    ✓ within threshold
  ...

REGRESSIONS (1):

  sid-job/job_queue/handoff — +17.9% (142.3ns → 167.8ns)
    Likely suspect commits (sid-job in the last 5):
      8fbd34c  feat(bin): SSH actions + wizard + help drawer
      c85a3e1  feat(widgets): bordered panes with titles
      3b606b8  feat(widgets): Settings Animation sub-view
      aac0fb5  feat(bin): integrate supernovae
      23a78aa  feat(bin): Phase 4+5 — in-TUI CRUD modals

    None of these obviously touch sid-job. Possibilities:
      1. A dep update changed allocator behaviour.
      2. Noise (this bench has high variance — verify with --threshold 5
         and `cargo bench -p sid-job -- --measurement-time 10`).
      3. A genuine change in an upstream dependency.

    Investigation steps:
      a) Re-run: cargo bench -p sid-job -- handoff
      b) Check if regression is stable across 3 runs
      c) If stable: git bisect against the baseline commit

Workspace perf gate: ✗ NOT GREEN — 1 regression must be addressed or justified.
```

For a green run:

```text
PERF CHECK — sid criterion vs baseline

Threshold: 10.0% (per CLAUDE.md)
Benches compared: 12
Regressions: 0

(per-bench table)

Workspace perf gate: ✓ GREEN.
```

## Baseline management

Criterion stores baselines under `target/criterion/<bench>/base/`. Best
practice for sid:

- **Per-feature baseline.** Before starting a perf-sensitive change,
  run `cargo bench -p <crate> -- --save-baseline before-change`. After:
  `cargo bench -p <crate> -- --baseline before-change`.
- **Committed baseline (future).** If/when sid gets a "perf baseline"
  CI gate, commit `target/criterion/<bench>/base/estimates.json` files
  per the CLAUDE.md performance section.

This skill reads whatever is on disk; it doesn't manage baselines.

## Tool hints

- A bench that didn't change usually shows ±2-3% delta from run-to-run
  variance. Don't chase these; the 10% threshold accounts for it.
- For very fast benches (sub-µs), variance is higher. Consider running
  with `--threshold 15.0` for those or using criterion's
  `--measurement-time 10` to reduce noise.
- If `criterion_compare` returns `benches: []`, the user hasn't run
  `cargo bench` since cleaning `target/`. Tell them to run benches first.

## Anti-patterns

| Pattern | Why it's wrong |
|---|---|
| Reporting "passed" on a +9.5% regression | The threshold is binding — 9.5% is within tolerance, but flag for attention (it's drift heading to a regression) |
| Skipping the recent-commits correlation | Without it, the user has no actionable next step |
| Treating every +10% on a sub-µs bench as a real regression | Sub-µs benches are noisy; recommend a re-run before bisecting |
| Recommending the user "fix the regression" without naming likely suspect commits | The whole point of the recent-commits correlation is to point at the cause |

## Verification before declaring done

- [ ] `target/criterion/` was checked for population.
- [ ] `mcp__sid__criterion_compare` was called.
- [ ] If regressions exist, `mcp__sid__recent_commits` was called for context.
- [ ] Per-bench table includes baseline, latest, delta, status.
- [ ] Regression section names specific suspect commits if applicable.
- [ ] Final "perf gate" line is honest about green vs not-green.
