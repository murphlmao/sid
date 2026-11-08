# sid-secrets (secret storage seam)

Owns the `SecretStore` trait (not in `sid-core`) and its backends. Secrets never reach
committed config or redb — records hold only a `SecretId`.

## Surface
`SecretId`, `SecretStore` (also `impl for Arc<T>`), `MemorySecretStore`,
`keyring::{KeyringStore<B>, KeyringBackend, OsKeyringBackend}`, `file::EncryptedFileStore`
(dormant since 2026-07-03), `resolve::{resolve_secret_store, probe_keyring, SecretBackendToggles,
BackendKind, Resolved}`.

## Invariants
- Effective backend = keyring if enabled and the startup durability probe passes, else memory
  (degraded; the status bar says so). `app.rs::open_secrets` is the only caller.
- Never log or `Debug`-print secret bytes.
- No dependency on `sid-core` or GPUI; platform keyring impls sit behind `KeyringBackend` (cfg-gated).

## Test
`nice -n 10 cargo test -p sid-secrets -j 3` — uses `FakeKeyring`; no real keyring needed.
The boundary test is `crates/sid-store/tests/secret_boundary.rs`.

## Landmines
- The OS keyring may exist but not persist (no Secret Service session): that is what the
  probe is for — a successful write does not prove durability.
- `EncryptedFileStore` is intentionally not a resolution candidate; don't wire it back in without a design pass.
