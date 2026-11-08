---
name: sid-worktree
description: sid's worktree, commit, merge and push conventions for agents and humans - where worktrees live, branch naming, commit style (no trailers), push after every commit, merging main in, and machine etiquette. Use when starting a branch of work, committing, pushing, or finishing.
---

# sid-worktree

## Where
- Every branch of work lives in its own worktree: `~/vcs/sid-wt/<branch>` (or the
  harness-provided one under `.claude/worktrees/`). **Never edit `main`'s checkout**
  (`~/vcs/sid`) from an agent. **Never** put a worktree or a cargo target under `/tmp`
  (16GB RAM-backed tmpfs - a cargo target evicts RAM).
- Create: `git worktree add ~/vcs/sid-wt/<branch> -b <branch> origin/main`.
  Confirm with `git rev-parse --show-toplevel` before the first edit.
- Branch names: `night/<topic>` for overnight agent work, otherwise `feat/`/`fix/`/`docs/`.
- Leave no worktree with uncommitted changes. Remove merged ones: `git worktree remove <path>`.

## Commit
- Conventional style matching `git log --oneline`: `feat(sid-ui): ...`, `fix(ssh): ...`,
  `docs(sid): ...`, `build: ...`. The body explains *why*.
- **No trailers of any kind** - no `Co-Authored-By`, no "Generated with". Check the last ten
  commits before inventing a convention.

## Durability: push after EVERY commit
`git push -u origin HEAD` (sessions die; origin is the only safe place). Never force-push,
never push to `main` from an agent branch. Push only gate-green units (`sid-gate` skill).

## Finish
1. `git fetch origin && git merge origin/main` (merge, NOT rebase). Resolve, re-run the gate.
2. Push the branch. The orchestrator/Murphy merges to `main`.
3. Report: branch, commits, gate result, what was verified visually, what is open.

## Machine etiquette
Murphy uses this box. Every cargo call: `nice -n 10 cargo <cmd> -j 3`. Iterate with
`-p <crate>`; run the full gate once near the end. Docker fixtures: unique
`COMPOSE_PROJECT_NAME=sid-<topic>`, tear down after. UI checks via `scripts/sid-cap.sh`
only (`sid-capture` skill).
