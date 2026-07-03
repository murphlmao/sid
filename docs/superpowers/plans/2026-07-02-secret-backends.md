# Secret backends — chain, encrypted-file vault, toggles

**Goal (Murphy's ask):** sid should not *need* an OS keyring. Ship a **dependency-less
encrypted-file** backend as a first-class peer to the keyring, let the user **toggle each
backend on/off** (with honest consequence warnings), **recommend installing a keyring** when
none is present, and keep **memory as the unconditional final fallback**. All behind the
existing `SecretStore` trait — pure adapter pattern.

## Backends (all impl `sid_secrets::SecretStore`)
1. **`KeyringStore`** (exists) — OS Secret Service. Best on macOS/Windows later (built-in);
   on Linux depends on a provider (gnome-keyring/kwallet). Zero config when present.
2. **`EncryptedFileStore`** (NEW, the dependency-less path) — an authenticated-encryption
   vault file in the global data dir. **argon2id** (passphrase → 32-byte key) + **XChaCha20-
   Poly1305** AEAD over the serialized secret map. Pure-Rust RustCrypto crates (library deps,
   NOT system deps): `argon2`, `chacha20poly1305`, `zeroize`. Master key held in memory,
   `Zeroizing`, wiped on drop. Passphrase entered via a masked sid `TextInput` modal — never
   `rpassword`/CLI, never an env var, never on disk.
3. **`MemorySecretStore`** (exists) — non-persistent final fallback.

## Selection (`auto` chain + explicit toggles)
New `Settings` fields (mirror how `file_browser_side`/`default_scope` are versioned — bump
`SETTINGS_VERSION`, add a legacy→new decode arm):
- `secret_keyring_enabled: bool` (default `true`)
- `secret_file_enabled: bool` (default `true`)
- (memory is always the last resort; not toggleable)

`resolve_secret_store(cfg, probe) -> Resolved { store, effective: BackendKind, warning: Option<String>, recommendation: Option<String> }`:
- Walk **keyring (if enabled & startup probe passes) → encrypted-file (if enabled) → memory**.
- **Toggle semantics (Murphy):** keyring off + file on ⇒ file only. Both off ⇒ memory only.
- **Warnings (be honest about consequences):**
  - effective == memory ⇒ `"secrets will not persist across restarts"` (existing message).
  - both persistent backends disabled ⇒ warn that's why it's memory-only.
  - keyring enabled but **no provider present** (probe failed for "no Secret Service" reason,
    not a transient error) ⇒ `recommendation`: suggest installing one
    (`sudo pacman -S gnome-keyring` / distro equiv) OR switching to the encrypted-file backend.
- Startup surfaces `effective` + any warning/recommendation in the app's status line
  (Murphy is happy being told which backend is live — informational, not an error).

## Lazy unlock (encrypted-file)
The trait stays `&self` sync. `EncryptedFileStore` holds `Mutex<Option<Zeroizing<Key>>>`:
- Vault file **exists** but not yet unlocked ⇒ `get/put/delete/list_ids` return a new
  `SecretError::Locked`. App catches `Locked`, shows the **unlock modal** (new
  `crates/sid/src/ui/secret_unlock.rs`), calls `store.unlock(passphrase)` (verifies by
  decrypting the header/MAC — wrong passphrase ⇒ `SecretError::Backend("bad passphrase")`),
  retries the op.
- Vault file **absent** + a `put` arrives ⇒ modal in **create** mode (enter + confirm a new
  passphrase), then write the first encrypted vault.
- Acceptable v1 simplification: prompt for unlock **at startup** when encrypted-file is the
  effective backend, rather than threading lazy retries through every call site — pick
  whichever is cleaner given how `open_secrets`/call-sites are structured; `// ponytail:` note it.
- **Future (do NOT build now):** cache the derived key in the Linux **kernel session keyring**
  (`keyctl`, no daemon) so relaunches within a login session unlock silently. Structure the
  vault so this can slot in later; leave a `// TODO(keyctl-session-cache)` marker.

## Vault file format (document inline)
`$XDG_DATA_HOME/sid/secrets.vault`: magic + format-version byte + argon2id params (m/t/p) +
16-byte salt + 24-byte XChaCha20 nonce + AEAD ciphertext of the `postcard`-encoded
`BTreeMap<String, Vec<u8>>`. Whole-map re-encrypt on each mutation (few secrets — fine;
`// ponytail:` note the O(n) rewrite ceiling). File perms `0o600`.

## Tests — pragmatic but crypto is load-bearing (Fable reviews this)
- **Vault round-trip:** put/get/delete/list through a real temp-file vault; reopen with the
  same passphrase reads secrets back; **wrong passphrase fails closed** (no partial/plaintext
  leak); a truncated/corrupted vault errors, never panics.
- **No plaintext on disk:** after writing a known secret, assert the raw file bytes do NOT
  contain the plaintext.
- **`resolve_secret_store` selection matrix:** every (keyring on/off × file on/off × probe
  pass/fail) combo picks the right backend and emits the right warning/recommendation.
- **Zeroization:** key wiped on drop (best-effort assert; at least that `Zeroizing` is used).
- No exhaustive KDF-parameter permutations.

## Ownership (this agent)
`crates/sid-secrets/**`, `crates/sid-store/src/{entities,global}.rs` (Settings fields only),
`crates/sid/src/app.rs` (`open_secrets` → selection + startup message; keep hunks tiny),
new `crates/sid/src/ui/secret_unlock.rs` + one line in `crates/sid/src/ui/mod.rs`, and the
crypto crates added to `[workspace.dependencies]` + `sid-secrets/Cargo.toml`.
