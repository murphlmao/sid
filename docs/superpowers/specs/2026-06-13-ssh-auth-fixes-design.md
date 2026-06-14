# SSH Auth Fixes — Design

**Date:** 2026-06-13  **Priority:** HIGH (owner: "it was bullshit, hardly worked")
**Verified facts:** pi `raspberrypi@10.1.1.93` reachable, offers `publickey,password`, password "raspberry" works via `sshpass`. `sshpass` present at `/usr/bin/sshpass`.

## Root causes (verified in code)
1. **Connect ignores password auth** — `wire.rs:5021-5026`: `SshAuthKind::Password => SshAuth::Agent` (a TODO stub). The real `SshAuth::Password(String)` variant exists and `sid-ssh`'s `auth_password` is implemented but **unreachable**. Primary blocker.
2. **Key import / copy-id can't use a password** — `wire.rs:6271` spawns `ssh-copy-id` with no TTY/stdin/password path → fails on password-only hosts.
3. **Generate-key step 3** (copy to host) fails for the same reason.
4. **Agent auth** fails hard with no clear message when `SSH_AUTH_SOCK` is unset.
5. **Add connection**: code path looks sound (validate → `upsert_ssh_host` → `refresh_ssh_widget` → close form); verify the auth `Choice` field is collected and add tests.

## Decisions (owner)
- **Password handling:** prompt at connect via a modal; checkbox optionally saves to the OS keyring. Saved hosts connect silently thereafter.
- **Key copy:** `sshpass -p <pw> ssh-copy-id ...` using the same password (from keyring or prompt).

## Types in play
- `SshAuth::Password(String)` / `Key{path,passphrase}` / `Agent` (`sid-core/src/adapters/ssh.rs:71`).
- `SecretStore` (`sid_core::adapters::ns`): `put(&SecretId,&[u8])`, `get(&SecretId)->Option<Vec<u8>>`, `delete`, `list_ids`. `SecretId::new(...)`.
- Secret key convention (mirror DB's `db.connection.{id}.password`): **`ssh.host.{alias}.password`**.
- `sid_app.secrets: Arc<dyn SecretStore>` already on SidApp.

## Design

### A. Password auth at connect (`wire.rs` connect flow)
`drain_pending_ssh_connect` / `spawn_ssh_connect_task`: when `host.auth_kind == Password`:
1. Load `secrets.get(SecretId::new("ssh.host.{alias}.password"))`. If `Some(pw)` → spawn connect with `SshAuth::Password(pw)` (silent).
2. If `None` → push a **password modal** (id `ssh.password:{alias}`): one masked field `password`, one bool toggle `save` ("Save to keyring"). Do NOT spawn the connect yet.
3. On modal submit (`submit_ssh_password`): spawn connect with `SshAuth::Password(entered)`. If `save` → `secrets.put(SecretId::new("ssh.host.{alias}.password"), entered.as_bytes())`.
- `Agent`/`Key` paths unchanged. Remove the `Password => Agent` stub.

### B. Agent fallback message
When `Agent` selected and `std::env::var("SSH_AUTH_SOCK").is_err()`, fail the connect with a clear message: "no ssh-agent (SSH_AUTH_SOCK unset) — use password or key auth in the host's settings", surfaced as a Failed outcome → log.

### C. Key copy via sshpass (`run_ssh_copy_id`)
- Resolve the password the same way (keyring, else prompt — reuse the modal; for the keygen wizard step 3 the connect modal flow already has it).
- If a password is available: `sshpass -p <pw> ssh-copy-id -i <pub_key> -p <port> {user}@{host}` (with `-o StrictHostKeyChecking=accept-new`).
- If host is key/agent auth: current `ssh-copy-id -i ... {alias}`.
- Pre-flight: if `sshpass` not on PATH, return a clear error. Never log the password (mask in any error/log).

### D. Add-connection verification
- Confirm the form's auth `Choice` (agent/key/password) maps to `SshAuthKind` correctly on submit. Add tests for `submit_ssh_new_from_form` with each auth kind producing the right persisted `SshHost`.

## Security constraints (binding — critical path)
- Password NEVER stored in the `SshHost` record or logs; only in the keyring, only on opt-in.
- Masked in the modal; redacted in any error string.
- `delete` the keyring entry when the host is removed (extend the host-remove path).

## Tests (scoped: `cargo test -p sid ssh`, `-p sid-ssh`, `-p sid-secrets`)
- connect routes `Password` host → `SshAuth::Password` (mock SshClient asserts the auth it received).
- keyring round-trip: save on submit → next connect loads silently; no save → prompts again.
- password never appears in the persisted `SshHost` nor in a Failed error string.
- agent-unset → clear error.
- `run_ssh_copy_id` builds the `sshpass ... ssh-copy-id` argv correctly (password host) vs plain (key host); missing-sshpass error.
- host removal deletes the keyring secret.
- `submit_ssh_new_from_form` for agent/key/password.

## Out of scope (future)
- Keyboard-interactive multi-prompt auth; per-connection passphrase caching for keys.
