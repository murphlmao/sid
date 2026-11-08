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
- CLI path (`cli.rs`) treats the repo's `.git/config` and hooks as hostile. Only
  `ssh://`, `https://` and scp-style URLs reach the CLI (`is_network_url`); `ext::`/helper
  URLs, plain `http://` and `git://` are refused (`refusal_reason`); local/`file` remotes
  (fetch AND push) use libgit2, which runs no hooks or `receivepack`.
- Hardening list: env `GIT_ALLOW_PROTOCOL=ssh:https` (beats all `protocol.*.allow`),
  `GIT_ASKPASS`/`SSH_ASKPASS`/`GIT_PROXY_COMMAND` empty, `GIT_TERMINAL_PROMPT=0`,
  `GIT_SSH_COMMAND` always set (beats repo `core.sshCommand`; honours the caller's env or
  the user's GLOBAL `core.sshCommand`, plus `BatchMode`); inherited `GIT_DIR`,
  `GIT_WORK_TREE`, `GIT_INDEX_FILE`, object-dir, `GIT_CONFIG_PARAMETERS/COUNT` and
  `GIT_EXEC_PATH` removed. argv `-c`: `core.hooksPath=/dev/null` (`NUL` on Windows),
  `core.fsmonitor=false`, `core.askPass=`, `credential.helper=` then the user's own
  global/system helpers re-added (read once via `git config --global/--system`),
  `submodule.recurse=false`, `push.gpgSign=false`, `gc.auto=0`, `maintenance.auto=false`;
  fetch `--no-recurse-submodules --no-auto-maintenance --upload-pack=git-upload-pack`,
  push `--no-verify --no-recurse-submodules --no-signed --receive-pack=git-receive-pack`;
  user-controlled names after `--`. NOT set: `GIT_CONFIG_NOSYSTEM` (would drop the macOS
  keychain helper). Known residual: `url.*.insteadOf` cannot be cancelled by `-c`;
  `GIT_ALLOW_PROTOCOL` bounds what it can rewrite to.
- 90 s timeout kills the whole process group; reader threads are joined with a deadline;
  output capped at 64 KiB; server-written stderr is stripped of control/bidi chars and
  capped (`sanitize_remote_text`) before it becomes an error message.
- Hooks are skipped everywhere (libgit2 commits, CLI push): the UI says so.
- `discard_paths` disables pathspec matching (a tracked `a*b` name is literal);
  `remove_untracked` refuses a directory containing `.git` (a nested repo).

## Test
`nice -n 10 cargo test -p sid-git -j 3` (builds throwaway repos in tempdirs).

## Landmines
- The Umbrella fleet view opens ~40 repos: keep `open`/`summary` cheap; callers run them in
  background tasks, never in `render`.
- libgit2 links native libs; CI images need them.
