---
name: coverage-report
description: Report current sid coverage with critical-path flags. Wraps the sid-mcp `coverage_summary` tool and presents structured per-crate / per-file output with the 80% workspace floor and 95% critical-path floor (sid-store, sid-core, sid-job, sid-secrets) explicitly highlighted. Args: optional crate name to scope.
---

# Coverage Report

## Overview

Present sid coverage in a single readable report, anchored against the
CLAUDE.md thresholds: **80% workspace floor**, **95% critical-path floor**
(sid-store, sid-core, sid-job, sid-secrets). Highlights which crates and
files are below their applicable floor so the user has an actionable
list, not just a pile of numbers.

Reads from `mcp__sid__coverage_summary` (cached when fresh data isn't
needed). Slow path is the first run — `cargo llvm-cov` takes 30s-2min
across the workspace.

## When to use

- Before declaring a critical-path change ready to ship — `cargo test`
  green is necessary but not sufficient; you also need the 95% floor.
- After landing a feature, to see if any file slipped below its
  threshold.
- Periodically (weekly?) to catch slow drift.
- When `/sid-gate` reports a coverage gate failure and you need to
  drill into which files caused it.

This skill is **read-only**. It doesn't write tests; it surfaces gaps.
Pair with the `rust-test-writer` agent to close the gaps it identifies.

## Args

| Form | Behaviour |
|---|---|
| empty | workspace summary; all crates listed |
| crate name (e.g., `sid-store`) | scope to one crate |
| `--fresh` | force a fresh `cargo llvm-cov` run (slow) |

## Process

### 1. Call the MCP tool

If `$ARGUMENTS` is empty:

```text
mcp__sid__coverage_summary({})
```

If a crate is named:

```text
mcp__sid__coverage_summary({ "crate_name": "sid-store" })
```

If `--fresh`:

```text
mcp__sid__coverage_summary({ "fresh": true })
```

The tool returns a `CoverageSummary` JSON:

```jsonc
{
  "crates": [
    {
      "name": "sid-store",
      "lines_pct": 93.2,
      "is_critical_path": true,
      "below_critical_threshold": true
    },
    ...
  ],
  "workspace_lines_pct": 84.1,
  "workspace_meets_threshold": true,
  "from_cache": false,
  "generated_unix": 1747900000
}
```

### 2. Render the report

Output to the user in this exact format:

```text
COVERAGE REPORT — sid workspace

Workspace: 84.1% (✓ meets 80% floor)
Source: cargo llvm-cov, generated 2 min ago (cached)

Per-crate breakdown:

  CRATE             COVERAGE   FLOOR   STATUS
  sid-core          94.2%      80%     ✓ over floor
  sid-store         93.2%      95%     ✗ under critical-path floor (-1.8pp)
  sid-job           96.1%      95%     ✓ over critical-path floor
  sid-secrets       98.7%      95%     ✓ over critical-path floor
  sid-widgets       82.4%      80%     ✓ over floor
  sid-ui            87.0%      80%     ✓ over floor
  sid-git           91.5%      80%     ✓ over floor
  ...

CRITICAL-PATH GAPS (95% floor):
  sid-store: 93.2% — needs 1.8pp to clear the floor.

To close the gap:
  1. Dispatch the rust-test-writer agent at the specific files below
     the floor. Re-run /coverage-report --fresh after to verify.
  2. Or run `/mc-dc-audit crates/sid-store` to find MC/DC gaps that
     correlate with low coverage.

Workspace floor: ✓ green.
Critical-path: ✗ 1 crate below floor.
```

For a single-crate scope, omit the per-crate breakdown and show only
that crate's row plus its top 5 lowest-covered files.

### 3. Decide overall readiness

The report's last two lines (workspace + critical-path) are the
load-bearing summary. Set them honestly:

- Workspace floor green iff `workspace_meets_threshold` is true.
- Critical-path green iff every crate with `is_critical_path: true`
  has `below_critical_threshold: false`.

## Tool hints

- The MCP tool caches results at `target/llvm-cov/sid-mcp-cache.json`.
  Pass `--fresh` to invalidate; otherwise subsequent calls return the
  cached snapshot.
- The first run on a new machine compiles the workspace with
  instrumentation — expect 30s-2min depending on your machine. Warn
  the user before kicking it off.
- If `mcp__sid__coverage_summary` returns an error mentioning
  `cargo-llvm-cov` not installed, surface the fix: `cargo install
  cargo-llvm-cov`.

## Anti-patterns

| Pattern | Why it's wrong |
|---|---|
| Reporting "80% workspace, all good" when a critical-path crate is at 92% | The 95% floor is *binding* on critical-path crates regardless of workspace average |
| Re-running `--fresh` every invocation | Wastes 30-120 seconds; the cache is the right default |
| Listing every file in every crate | Useless wall of numbers; surface only the gaps |
| Recommending generic "add more tests" | The agent companion (rust-test-writer) is more useful — invoke it |

## Verification before declaring done

- [ ] `mcp__sid__coverage_summary` was called.
- [ ] Workspace status line is correct vs the 80% floor.
- [ ] Per-crate critical-path status is correct vs the 95% floor.
- [ ] Gap list (if any) names specific crates, not "things are below the bar".
- [ ] If gaps exist, suggested next action points at `rust-test-writer` or `/mc-dc-audit`.
