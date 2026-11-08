# sid-git (git2 adapter)

`git2`-backed `sid_core::git::GitProvider`; the only crate that names `git2`.

## Surface
`Git2Provider` (`factory()` yields a per-repo handle; zero-sized). Modules: `open`, `summary`,
`branches`, `checkout`, `status`, `log`. A non-repo path maps to the typed `NotARepo` error.

## Invariants
- No git2 types escape. Mutation ceiling today is checkout only (commit/diff/stash are unbuilt;
  the archived POC `~/vcs/sid-poc/crates/sid-git` is the reference).
- Read paths must tolerate bare repos, detached HEAD, and empty repos.

## Test
`nice -n 10 cargo test -p sid-git -j 3` (builds throwaway repos in tempdirs).

## Landmines
- The Umbrella fleet view opens ~40 repos: keep `open`/`summary` cheap; callers run them in
  background tasks, never in `render`.
- libgit2 links native libs; CI images need them.
