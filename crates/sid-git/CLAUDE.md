# sid-git (git2 adapter)

`git2`-backed `sid_core::git::GitProvider`; the only crate that names `git2`.

## Surface
`Git2Provider` (`factory()` yields a per-repo handle; zero-sized). Modules in `src/`: `diff`,
`history`, `remote`, `status`, `write` (stage/commit/stash/branch ops) and the public `cli`.
A non-repo path maps to the typed `NotARepo` error. `cli.rs` runs fetch/push for network
remotes by spawning the `git` CLI, because `git2` is built without ssh/https transports;
local/file remotes stay on libgit2 (`is_network_url`).

## Invariants
- No git2 types escape.
- Read paths must tolerate bare repos, detached HEAD, and empty repos.
- CLI path: argv only (names after `--`), non-interactive env (`GIT_TERMINAL_PROMPT=0`, ssh
  `BatchMode`), 90 s timeout that kills the whole process group, output capped at 64 KiB. It
  honours repo config and runs hooks, unlike libgit2 — hardening is an open review item
  (`docs/design/2026-11-07-night-sprint-handoff.md`).

## Test
`nice -n 10 cargo test -p sid-git -j 3` (builds throwaway repos in tempdirs).

## Landmines
- The Umbrella fleet view opens ~40 repos: keep `open`/`summary` cheap; callers run them in
  background tasks, never in `render`.
- libgit2 links native libs; CI images need them.
