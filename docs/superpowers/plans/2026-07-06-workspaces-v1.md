# Workspaces v1 — scope (Fable, from the POC + the rebuild's spine)

Status: BUILDING — approved 2026-07-06 ("surprise me. go."); see the BUILD ADDENDUM for the calls.

## What the POC actually built (evidence)

`sid-poc/crates/sid-widgets/src/{workspaces,workspace_detail}.rs` (~3,500 lines) +
`sid-git` (git2 adapter, 555 lines) + render snapshots:

```
┌ Workspaces ──────────┐┌ Branches ────────────────────────────────────────────┐
│> · myrepo            ││(no branches loaded — select a workspace)             │
└──────────────────────┘└──────────────────────────────────────────────────────┘
```

- A workspace = a registered **git repo root** (`WorkspaceKind::Repo`) or an
  **Umbrella** (a directory of sub-repos, rendered as a multi-repo dashboard
  table on Enter).
- Right pane cycles SIX git sub-views per selected workspace:
  **Branches / Status / Log / Diff / Commit / Actions** — a full git cockpit.
  `Actions` = user-defined per-workspace quick commands with captured output.
- Git behind a `GitProvider` trait (open/list_branches/current_branch/status/
  commit_log/diff/checkout_branch/commit + stash/worktree extras), git2-backed,
  per-repo handles cached in `Arc<Mutex<…>>`.
- Checkout refuses on a dirty tree (`GitError::DirtyWorkingTree`).

## What the rebuild changes about the concept

In the GPUI sid, a workspace is not just a repo — it is a **scope layer**: the
committed `.sid/config.toml` attributes hosts/connections to it, the scope
chips switch it, promote/demote move items across it. That machinery exists
and works today; what does NOT exist is any UI to *manage* workspaces
(register/unregister/see what a workspace carries) — the tab is a placeholder
and the scope list only builds at startup (the `reload_scopes` caveat in
HANDOFF). The Workspaces tab is where the two halves meet:

> **A workspace row answers two questions at once: "what state is this repo
> in?" (git) and "what does sid know inside it?" (scope items).**

## Proposed v1 (one round, three tracks)

### §A — git adapter (port, not rewrite)
- `sid-core/src/git.rs`: trait ported from the POC minus what v1 doesn't use —
  `open / list_branches / current_branch / status / commit_log / checkout_branch`
  (+ typed `GitError` incl. `DirtyWorkingTree`). Commit/diff/stash/worktree
  methods deferred with the POC as reference.
- `crates/sid-git`: git2 impl, cribbed from the POC's 555-line crate. git2 is
  named ONLY here (adapter rule). Repo handles cached per root, calls run on
  the shared runtime (never the render thread).

### §B — store + runtime plumbing
- Register/unregister workspaces from the UI: `Store` facade grows
  `unregister_workspace` (register exists); registering derives the id from
  the canonicalized root, validates the path exists, detects git (non-git
  roots allowed — scope still works, git panel shows "not a git repo").
- **Fix the `reload_scopes` caveat**: adding/removing a workspace rebuilds
  `AppState.scopes` at runtime (scope chips update immediately).
- Removing a workspace NEVER touches `.sid/config.toml` (the committed file
  belongs to the repo; unregister = forget the pointer, attributive invariant
  intact). Warn if it was the focused scope → fall back to Global.

### §C — the tab UI (design-system native)
Layout: `[workspace list | detail]` split, db_tab convention (list ~300px,
detail flex). No emojis; theme tokens; hairlines.

- **List rows**: name (double-click rename → meta only), muted root path,
  current branch + a dirty dot (warning color), scope-item count
  (`N hosts · M connections`), focused-scope marker (accent) when active.
  Right-click: Focus scope / Rename / Unregister. Header: `WORKSPACES · N` +
  `+ add` (path input with tilde expansion, like config-file pinning).
- **Detail sub-tabs** (chips, network_tab convention):
  - **Overview** — branch + ahead/behind, dirty summary (staged/unstaged/
    untracked counts), the workspace's scope items (its hosts + connections
    with jump-to-tab affordances and promote/demote via right-click — reuses
    the existing store calls), duplicate-identity warning if any.
  - **Branches** — list (current marked, last-commit subject + age), click →
    checkout with the dirty-tree refusal surfaced inline.
  - **Status** — changed files (staged/unstaged/untracked groups).
  - **Log** — recent commits (subject · author · age), read-only.
- Focus-scope is the row's primary action (the tab is the scope switcher's
  big brother; chips stay for quick flips).

### §D — deferred (explicitly, with the POC as the reference impl)
Commit flow, Diff view, per-workspace Actions (quick commands), Umbrella
multi-repo dashboards, stash/worktree ops, auto-discovery scans. Each is a
clean later slice on top of §A's trait.

## Open questions for Murphy
1. **Mutation ceiling for v1**: is read-everything + `checkout` the right
   line, or do you want commit-from-sid in v1? (POC had a commit draft UI;
   porting it roughly doubles §C.)
2. **Umbrella workspaces**: your `~/vcs` is a natural umbrella. Defer to v2
   (my lean), or is the multi-repo table part of what makes this tab worth
   opening daily for you?
3. **Auto-discovery**: manual `+ add` only (my lean, calm), or also a one-shot
   "scan a directory for repos" helper?

## Test posture (pragmatic mode)
`sid-git` gets real-repo integration tests against a tempdir fixture repo
(init/commit/branch via git2 in-test); pure helpers unit-tested; UI
observation-gated via sid-cap (which can now click/type — checkout flows are
scriptable end-to-end).

---

## BUILD ADDENDUM (Fable's calls — Murphy said "surprise me. go.")

Decisions: (1) mutation ceiling = read + checkout (commit flow is v2);
(2) **Umbrella fleet dashboard SHIPS IN v1** — this is the surprise: register
`~/vcs` and the detail pane is a sortable fleet table (repo · branch · dirty ·
ahead/behind · last-commit age); (3) no separate scan feature — pointing
`+ add` at a directory of repos IS the umbrella.

Foundation already on main (do not redo):
- `sid_core::git` — the v1 trait (`open/list_branches/current_branch/status/
  commit_log/summary/checkout_branch`), typed `GitError` (NotARepo /
  DirtyWorkingTree / …), `RepoSummary` (the one-call fleet rollup).
- `crates/sid-git` — skeleton crate (compiles; every method errors "port in
  progress"); the ONLY crate that may name `git2`.
- `Store::register_workspace_at(path)` (canonicalize, must-be-dir, name from
  file_name, idempotent by id), `Store::unregister_workspace` (never touches
  `.sid/config.toml` — tested), `Store::list_workspaces`.
- `AppState::reload_scopes_runtime` (+ pure `build_scope_choices`) — scope
  chips rebuild at runtime; focused-scope removal falls back to Global.

Umbrella detection (track U, pure + unit-tested): a workspace root that is NOT
itself a git repo but contains ≥1 git repo exactly one level deep (dir entries
with a `.git`) renders the fleet; a git-repo root renders the single-repo
sub-tabs; neither renders the scope-only view with a muted "not a git repo"
note in place of the git panels. Detection = filesystem checks only (cheap,
no git open); fleet rows then `open`+`summary` each repo on the shared runtime.
