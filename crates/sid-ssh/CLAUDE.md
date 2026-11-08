# sid-ssh (russh adapter)

`russh`-backed impl of `sid_core::ssh::SshClient`; the only crate that names russh/russh_sftp.

## Surface
`RusshClient`, `RusshClientFactory::new(known_hosts_path)` -> `new_client()`, `DEFAULT_DIAL_TIMEOUT`;
modules `auth`, `known_hosts`, `keyscan` (`LocalIdentities: IdentityScan`), `shell`, `sftp`
(`RusshSftp: SftpSession`). Depends on `sid-core`, `sid-term`.

## Invariants
- Known-hosts handling fails closed (a changed key for a trusted host is `Mismatch`).
- First contact is never learned silently: `connect` returns `SshError::HostKeyUnknown`
  (key type + `SHA256:` fingerprint) and learns the key only on a retry after
  `approve_host_key(<that fingerprint>)` — see `known_hosts::decide`.
- The shell reader/writer split is deadlock-free; don't hold one lock across both halves.
- SFTP transfers are traversal-guarded.
- `remote_edit` (SFTP edit-in-place, driven by `crates/sid/src/ui/session/edit.rs`): downloads are bounded (`DEFAULT_MAX_BYTES` 5 MiB) and stat-checked; the working copy lives in a 0700 dir under the cache dir (file 0600), created exclusively; local names are shell-inert (`[A-Za-z0-9._-]` only, never leading `-` or `.`, dangerous extensions get `.txt`); conflicts compare a SHA-256 of the remote bytes (mtime only gates the hash); `TmpRename` (opt-in) creates `.<name>.sid-<hex>` exclusively with the target's mode set in the OPEN request and renames only over a regular, non-symlink, same-owner target, else rewrites in place; every exit goes through `close_or_preserve`, which copies unsaved edits to a backups dir under the data dir (not the cache) before removing anything and keeps the temp dir if the backup fails.
- No russh types escape the crate.

## Test
`nice -n 10 cargo test -p sid-ssh -j 3` (unit). Docker sshd: `scripts/test-ssh.sh` (`just test-ssh`)
runs `docker_sshd_key_auth_exec_and_sftp_round_trip` from `tests/live_sshd_smoke.rs`. The other
`#[ignore]`d test there (`live_sshd_agent_exec_shell_sftp`) needs a real ssh-agent + localhost
sshd — manual only.

## Landmines
- In the app, everything async runs on `ui::session::ssh_runtime()`, not gpui executors.
- Docker fixtures: set a unique `COMPOSE_PROJECT_NAME` and tear down.
