//! `EncryptedFileStore` — a dependency-less, passphrase-protected secret vault.
//!
//! The whole point of this backend is that it needs **nothing installed**: no OS
//! keyring daemon, no system crypto library. Every crypto primitive is a pure-Rust
//! RustCrypto crate (`argon2`, `chacha20poly1305`, `zeroize`, `getrandom`) — see this
//! crate's `Cargo.toml` for the exact versions. `CLAUDE.md`'s adapter rule holds here
//! too: nothing outside this module names those crates.
//!
//! # On-disk format
//!
//! A vault is a single file (`$XDG_DATA_HOME/sid/secrets.vault`), laid out as:
//!
//! ```text
//! offset  size  field
//! 0       4     magic: b"SIDV"
//! 4       1     format version (currently 1)
//! 5       4     argon2id m_cost, u32 little-endian (KiB)
//! 9       4     argon2id t_cost, u32 little-endian (iterations)
//! 13      4     argon2id p_cost, u32 little-endian (lanes)
//! 17      16    argon2id salt — random, fixed for the vault's lifetime
//! 33      24    XChaCha20-Poly1305 nonce — fresh on every write, NEVER reused
//! 57      ..    AEAD ciphertext of the postcard-encoded `BTreeMap<String, Vec<u8>>`
//!               (the RustCrypto `aead` crate appends the 16-byte Poly1305 tag to the
//!               end of the ciphertext it returns, so it travels as part of this tail)
//! ```
//!
//! The salt and KDF params are generated once at [`EncryptedFileStore::create`] and
//! never change (changing them would require re-deriving the key from the passphrase,
//! which is not retained in memory). The nonce is regenerated fresh on **every**
//! mutation — reusing a nonce with the same key would break XChaCha20-Poly1305's
//! confidentiality/integrity guarantees outright, so [`write_vault`] always draws a new
//! one from the OS CSPRNG before encrypting.
//!
//! # Unlock is the passphrase check
//!
//! There is no separate header MAC: decrypting the AEAD ciphertext *is* the passphrase
//! verification. A wrong key fails the Poly1305 tag check and [`EncryptedFileStore::unlock`]
//! reports [`SecretError::Backend("bad passphrase")`](SecretError::Backend) — the vault
//! never partially decrypts or leaks plaintext on a wrong guess.
//!
//! # Lazy unlock, whole-map rewrite
//!
//! The [`SecretStore`] trait stays `&self`-sync, so a locked vault's `put`/`get`/
//! `delete`/`list_ids` return [`SecretError::Locked`] rather than blocking for a
//! passphrase prompt; the caller (the app) shows the unlock modal and does not retry
//! until [`unlock`](EncryptedFileStore::unlock) succeeds. Once unlocked, every mutation
//! re-reads the whole map from disk, applies the change, and re-encrypts the whole map
//! under a fresh nonce — `// ponytail:` this is O(n) per write, which is fine for the
//! handful of secrets sid manages; it would need revisiting only if that ever changed
//! by orders of magnitude. The derived key is cached in memory (`Zeroizing`, wiped on
//! drop) so a session's worth of operations only pays the expensive argon2id hash once.
//!
//! `// TODO(keyctl-session-cache)`: cache the derived key in the Linux kernel session
//! keyring (`keyctl`, no daemon) so a relaunch within the same login session can skip
//! the passphrase prompt. `Unlocked` below is deliberately just "the key plus the
//! header metadata needed to re-encrypt" so that cache can slot in without restructuring
//! this module — e.g. a future `unlock_from_session()` populating the same `Unlocked`
//! shape from a `keyctl` blob instead of an argon2id run.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::{SecretError, SecretId, SecretStore};

const MAGIC: &[u8; 4] = b"SIDV";
/// Current on-disk format version. If the layout ever changes, branch on this byte the
/// same way `sid-store`'s versioned-postcard entities do (bump, add a migration arm) —
/// don't overwrite the meaning of an already-shipped version.
const FORMAT_VERSION: u8 = 1;

const SALT_LEN: usize = 16;
/// XChaCha20's extended nonce — long enough to generate randomly per write with a
/// negligible collision chance, unlike plain ChaCha20Poly1305's 12-byte nonce (which
/// would need a counter to be reuse-safe under this store's random-per-write scheme).
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
/// magic(4) + version(1) + m/t/p(4*3) + salt(16) + nonce(24) = 57 bytes.
const HEADER_LEN: usize = 4 + 1 + 12 + SALT_LEN + NONCE_LEN;

/// Argon2id parameters for newly created vaults: the `argon2` crate's own
/// [`Params::DEFAULT`] (19 MiB, 2 passes, 1 lane) — OWASP's floor for interactive use.
/// A passphrase vault unlocked once per app launch doesn't need to be exotic here.
/// Existing vaults keep whatever params they were created with (read from the file
/// header on unlock), so bumping this only affects vaults created from now on.
fn default_params() -> Params {
    Params::DEFAULT
}

/// The in-memory state of an unlocked vault: the derived key plus the header metadata
/// (salt, KDF params) needed to re-encrypt on every mutation without re-deriving the
/// key from the passphrase (which is never retained).
struct Unlocked {
    key: Zeroizing<[u8; KEY_LEN]>,
    salt: [u8; SALT_LEN],
    params: Params,
}

/// A dependency-less, passphrase-protected [`SecretStore`]. See the module docs for the
/// on-disk format and the unlock/lazy-decrypt model.
pub struct EncryptedFileStore {
    path: PathBuf,
    /// `None` = locked (vault absent, or present but not yet unlocked this session).
    state: Mutex<Option<Unlocked>>,
}

impl EncryptedFileStore {
    /// A handle to the vault at `path`. Does no I/O — the file is only ever touched by
    /// [`create`](Self::create), [`unlock`](Self::unlock), or a `put`/`delete` mutation.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            state: Mutex::new(None),
        }
    }

    /// The vault file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a vault file exists on disk (regardless of unlock state).
    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    /// Whether this handle currently holds a derived key in memory.
    pub fn is_unlocked(&self) -> bool {
        self.state.lock().expect("vault state poisoned").is_some()
    }

    /// Create a brand-new vault protected by `passphrase`. Errs if a vault already
    /// exists at this path — call [`unlock`](Self::unlock) instead; this never
    /// overwrites an existing vault.
    pub fn create(&self, passphrase: &str) -> Result<(), SecretError> {
        if self.exists() {
            return Err(SecretError::Backend(
                "a vault already exists at this path — unlock it instead of creating a new one"
                    .into(),
            ));
        }
        let mut salt = [0u8; SALT_LEN];
        getrandom::fill(&mut salt)
            .map_err(|e| SecretError::Backend(format!("random salt: {e}")))?;
        let params = default_params();
        let key = derive_key(passphrase, &salt, &params)?;
        let unlocked = Unlocked { key, salt, params };
        write_vault(&self.path, &unlocked, &BTreeMap::new())?;
        *self.state.lock().expect("vault state poisoned") = Some(unlocked);
        Ok(())
    }

    /// Unlock an existing vault with `passphrase`. Fails closed on a wrong passphrase
    /// (the AEAD tag check fails — see the module docs) and on a corrupted/truncated
    /// file; never partially succeeds or leaks plaintext on failure.
    pub fn unlock(&self, passphrase: &str) -> Result<(), SecretError> {
        let bytes =
            fs::read(&self.path).map_err(|e| SecretError::Backend(format!("read vault: {e}")))?;
        let header = parse_header(&bytes)?;
        let params = Params::new(header.m_cost, header.t_cost, header.p_cost, None)
            .map_err(|e| SecretError::Backend(format!("vault params: {e}")))?;
        let key = derive_key(passphrase, &header.salt, &params)?;
        // Decrypting IS the passphrase check — see the module docs on why there is no
        // separate header MAC. The plaintext is discarded (and zeroized) immediately;
        // `unlock` only proves the key works, it doesn't cache the map.
        decrypt_map(&key, &header.nonce, header.ciphertext)?;
        *self.state.lock().expect("vault state poisoned") = Some(Unlocked {
            key,
            salt: header.salt,
            params,
        });
        Ok(())
    }

    /// Drop the in-memory key (does not touch the file). Not yet wired to any UI
    /// affordance — exposed for tests and for a future "lock now" action.
    pub fn lock(&self) {
        *self.state.lock().expect("vault state poisoned") = None;
    }

    fn with_unlocked<T>(
        &self,
        f: impl FnOnce(&Unlocked) -> Result<T, SecretError>,
    ) -> Result<T, SecretError> {
        let guard = self.state.lock().expect("vault state poisoned");
        let unlocked = guard.as_ref().ok_or(SecretError::Locked)?;
        f(unlocked)
    }

    /// Read + decrypt the current map. A vault that was just created in this process
    /// but has no file yet (shouldn't happen — `create` writes immediately — but this
    /// is defensive) reads as empty rather than erroring.
    fn read_map(&self, unlocked: &Unlocked) -> Result<BTreeMap<String, Vec<u8>>, SecretError> {
        if !self.exists() {
            return Ok(BTreeMap::new());
        }
        let bytes =
            fs::read(&self.path).map_err(|e| SecretError::Backend(format!("read vault: {e}")))?;
        let header = parse_header(&bytes)?;
        decrypt_map(&unlocked.key, &header.nonce, header.ciphertext)
    }

    fn write_map(
        &self,
        unlocked: &Unlocked,
        map: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(), SecretError> {
        write_vault(&self.path, unlocked, map)
    }
}

impl SecretStore for EncryptedFileStore {
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
        self.with_unlocked(|unlocked| {
            let mut map = self.read_map(unlocked)?;
            map.insert(id.0.clone(), value.to_vec());
            self.write_map(unlocked, &map)
        })
    }

    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
        self.with_unlocked(|unlocked| Ok(self.read_map(unlocked)?.get(&id.0).cloned()))
    }

    fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
        self.with_unlocked(|unlocked| {
            let mut map = self.read_map(unlocked)?;
            if map.remove(&id.0).is_none() {
                // Nothing to do — and nothing to re-encrypt or burn a nonce on.
                return Ok(());
            }
            self.write_map(unlocked, &map)
        })
    }

    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
        self.with_unlocked(|unlocked| {
            Ok(self.read_map(unlocked)?.into_keys().map(SecretId).collect())
        })
    }
}

// ---------------------------------------------------------------------------
// KDF / AEAD helpers
// ---------------------------------------------------------------------------

/// Derive the 32-byte vault key from `passphrase` via argon2id.
fn derive_key(
    passphrase: &str,
    salt: &[u8; SALT_LEN],
    params: &Params,
) -> Result<Zeroizing<[u8; KEY_LEN]>, SecretError> {
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::default(), params.clone());
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key[..])
        .map_err(|e| SecretError::Backend(format!("argon2id key derivation: {e}")))?;
    Ok(key)
}

fn encrypt_map(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    map: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>, SecretError> {
    let plaintext = Zeroizing::new(
        postcard::to_stdvec(map).map_err(|e| SecretError::Backend(format!("encode vault: {e}")))?,
    );
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| SecretError::Backend(format!("cipher init: {e}")))?;
    let nonce = XNonce::from(*nonce);
    let nonce = &nonce;
    cipher
        .encrypt(nonce, plaintext.as_slice())
        .map_err(|_| SecretError::Backend("vault encrypt failed".into()))
}

/// Decrypt `ciphertext` and decode the postcard-encoded map. An AEAD tag failure (wrong
/// key, or a corrupted/tampered file) reports as `"bad passphrase"` — see the module
/// docs on why unlock has no separate header MAC. This same path also covers a vault
/// corrupted *after* a successful unlock (e.g. truncated on disk between calls); the
/// wording is a slight misnomer there, but the failure mode (fail closed, no partial
/// data) is identical either way.
fn decrypt_map(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
) -> Result<BTreeMap<String, Vec<u8>>, SecretError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| SecretError::Backend(format!("cipher init: {e}")))?;
    let nonce = XNonce::from(*nonce);
    let nonce = &nonce;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretError::Backend("bad passphrase".into()))?,
    );
    postcard::from_bytes(&plaintext)
        .map_err(|e| SecretError::Backend(format!("corrupt vault: {e}")))
}

// ---------------------------------------------------------------------------
// Header parse / vault write
// ---------------------------------------------------------------------------

struct Header<'a> {
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
    ciphertext: &'a [u8],
}

/// Parse a vault file's header. Never panics on truncated/garbage input — every
/// malformed case (too short, bad magic, unsupported version, empty ciphertext) is a
/// plain [`SecretError`].
fn parse_header(bytes: &[u8]) -> Result<Header<'_>, SecretError> {
    if bytes.len() < HEADER_LEN {
        return Err(SecretError::Backend(
            "vault file too short (truncated?)".into(),
        ));
    }
    if &bytes[0..4] != MAGIC {
        return Err(SecretError::Backend(
            "not a sid secret vault (bad magic)".into(),
        ));
    }
    let version = bytes[4];
    if version != FORMAT_VERSION {
        return Err(SecretError::Backend(format!(
            "unsupported vault format version {version}"
        )));
    }
    let m_cost = u32::from_le_bytes(bytes[5..9].try_into().expect("4-byte slice"));
    let t_cost = u32::from_le_bytes(bytes[9..13].try_into().expect("4-byte slice"));
    let p_cost = u32::from_le_bytes(bytes[13..17].try_into().expect("4-byte slice"));
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bytes[17..17 + SALT_LEN]);
    let nonce_start = 17 + SALT_LEN;
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&bytes[nonce_start..nonce_start + NONCE_LEN]);
    let ciphertext = &bytes[HEADER_LEN..];
    if ciphertext.is_empty() {
        return Err(SecretError::Backend(
            "vault file has no ciphertext (truncated?)".into(),
        ));
    }
    Ok(Header {
        m_cost,
        t_cost,
        p_cost,
        salt,
        nonce,
        ciphertext,
    })
}

/// Encrypt `map` under a **fresh** nonce and atomically replace the vault file at
/// `path`. Writes to a sibling `.tmp` file (created with `0600` perms from the start —
/// no separate chmod, no race window where the secrets briefly have looser
/// permissions) and renames it into place, so a crash mid-write never leaves a
/// half-written vault and secrets are never world/group-readable even momentarily.
fn write_vault(
    path: &Path,
    unlocked: &Unlocked,
    map: &BTreeMap<String, Vec<u8>>,
) -> Result<(), SecretError> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| SecretError::Backend(format!("random nonce: {e}")))?;
    let ciphertext = encrypt_map(&unlocked.key, &nonce, map)?;

    let mut buf = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    buf.extend_from_slice(MAGIC);
    buf.push(FORMAT_VERSION);
    buf.extend_from_slice(&unlocked.params.m_cost().to_le_bytes());
    buf.extend_from_slice(&unlocked.params.t_cost().to_le_bytes());
    buf.extend_from_slice(&unlocked.params.p_cost().to_le_bytes());
    buf.extend_from_slice(&unlocked.salt);
    buf.extend_from_slice(&nonce);
    buf.extend_from_slice(&ciphertext);

    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);
    {
        let mut f = create_0600(&tmp_path)
            .map_err(|e| SecretError::Backend(format!("write vault tmp: {e}")))?;
        f.write_all(&buf)
            .map_err(|e| SecretError::Backend(format!("write vault tmp: {e}")))?;
        // Best-effort durability: a failure here doesn't corrupt anything already on
        // disk (the rename below hasn't happened yet), so it isn't fatal to the write.
        let _ = f.sync_all();
    }
    fs::rename(&tmp_path, path)
        .map_err(|e| SecretError::Backend(format!("finalize vault: {e}")))?;
    Ok(())
}

#[cfg(unix)]
fn create_0600(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// Non-Unix perms are an empty slot for now (`CLAUDE.md`: cross-platform is
/// accommodated, not solved — Wayland/Linux now, keep the seams).
#[cfg(not(unix))]
fn create_0600(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("secrets.vault")
    }

    #[test]
    fn locked_before_create_or_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::at(vault_path(&dir));
        assert!(!store.exists());
        assert!(!store.is_unlocked());
        assert!(matches!(
            store.get(&SecretId::new("x")),
            Err(SecretError::Locked)
        ));
        assert!(matches!(
            store.put(&SecretId::new("x"), b"v"),
            Err(SecretError::Locked)
        ));
        assert!(matches!(
            store.delete(&SecretId::new("x")),
            Err(SecretError::Locked)
        ));
        assert!(matches!(store.list_ids(), Err(SecretError::Locked)));
    }

    #[test]
    fn create_unlocks_immediately_and_writes_0600() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::at(vault_path(&dir));
        store.create("hunter2-passphrase").unwrap();
        assert!(store.exists());
        assert!(store.is_unlocked());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path()).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "vault must not be group/world readable"
            );
        }
    }

    #[test]
    fn create_twice_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::at(vault_path(&dir));
        store.create("p1").unwrap();
        assert!(store.create("p2").is_err());
    }

    #[test]
    fn put_get_delete_list_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::at(vault_path(&dir));
        store.create("correct horse battery staple").unwrap();

        let id = SecretId::new("ssh.prod.key");
        assert_eq!(store.get(&id).unwrap(), None);
        store.put(&id, b"PRIVATE-KEY-BYTES").unwrap();
        assert_eq!(
            store.get(&id).unwrap().as_deref(),
            Some(&b"PRIVATE-KEY-BYTES"[..])
        );
        assert_eq!(store.list_ids().unwrap(), vec![id.clone()]);
        store.delete(&id).unwrap();
        assert_eq!(store.get(&id).unwrap(), None);
        assert!(store.list_ids().unwrap().is_empty());
    }

    #[test]
    fn delete_absent_is_ok_and_does_not_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::at(vault_path(&dir));
        store.create("p").unwrap();
        let before = fs::read(store.path()).unwrap();
        assert!(store.delete(&SecretId::new("nope")).is_ok());
        let after = fs::read(store.path()).unwrap();
        assert_eq!(
            before, after,
            "a no-op delete must not burn a nonce/rewrite"
        );
    }

    #[test]
    fn reopen_with_same_passphrase_reads_secrets_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = vault_path(&dir);
        {
            let store = EncryptedFileStore::at(&path);
            store.create("correct horse battery staple").unwrap();
            store.put(&SecretId::new("a"), b"one").unwrap();
            store.put(&SecretId::new("b"), b"two").unwrap();
        }
        // Fresh handle — simulates a relaunch. Locked until explicitly unlocked.
        let reopened = EncryptedFileStore::at(&path);
        assert!(!reopened.is_unlocked());
        assert!(matches!(
            reopened.get(&SecretId::new("a")),
            Err(SecretError::Locked)
        ));
        reopened.unlock("correct horse battery staple").unwrap();
        assert_eq!(
            reopened.get(&SecretId::new("a")).unwrap().as_deref(),
            Some(&b"one"[..])
        );
        assert_eq!(
            reopened.get(&SecretId::new("b")).unwrap().as_deref(),
            Some(&b"two"[..])
        );
    }

    #[test]
    fn wrong_passphrase_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = vault_path(&dir);
        {
            let store = EncryptedFileStore::at(&path);
            store.create("the-right-one").unwrap();
            store.put(&SecretId::new("a"), b"secret-bytes").unwrap();
        }
        let reopened = EncryptedFileStore::at(&path);
        let err = reopened.unlock("totally-wrong").unwrap_err();
        match err {
            SecretError::Backend(msg) => assert!(msg.contains("bad passphrase"), "{msg}"),
            other => panic!("expected Backend(\"bad passphrase\"), got {other:?}"),
        }
        // A failed unlock must not leave the store usable.
        assert!(!reopened.is_unlocked());
        assert!(matches!(
            reopened.get(&SecretId::new("a")),
            Err(SecretError::Locked)
        ));
    }

    #[test]
    fn no_plaintext_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::at(vault_path(&dir));
        store.create("passphrase-for-the-vault").unwrap();
        let secret = b"super-secret-marker-xyz-do-not-leak";
        // A long, distinctive id — a 1-2 byte id would risk a false-positive match by
        // sheer chance against the file's own random salt/nonce bytes.
        let id = "distinctive-secret-id-marker-abcdef";
        store.put(&SecretId::new(id), secret).unwrap();

        let raw = fs::read(store.path()).unwrap();
        // Neither the secret value nor the id it's stored under should appear as a
        // contiguous byte run anywhere in the file.
        assert!(
            !contains_subslice(&raw, secret),
            "raw vault bytes must not contain the plaintext secret"
        );
        assert!(
            !contains_subslice(&raw, id.as_bytes()),
            "raw vault bytes must not contain the plaintext id"
        );
        // The passphrase itself must never be written either.
        assert!(!contains_subslice(&raw, b"passphrase-for-the-vault"));
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || needle.len() > haystack.len() {
            return needle.is_empty();
        }
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn truncated_vault_errors_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = vault_path(&dir);
        {
            let store = EncryptedFileStore::at(&path);
            store.create("p").unwrap();
            store.put(&SecretId::new("a"), b"v").unwrap();
        }
        let full = fs::read(&path).unwrap();
        // Truncate to header-only (no ciphertext) and to a few bytes (not even a full
        // header) — both must error cleanly, never panic.
        fs::write(&path, &full[..HEADER_LEN]).unwrap();
        let store = EncryptedFileStore::at(&path);
        assert!(store.unlock("p").is_err());

        fs::write(&path, &full[..4]).unwrap();
        let store = EncryptedFileStore::at(&path);
        assert!(store.unlock("p").is_err());

        fs::write(&path, b"").unwrap();
        let store = EncryptedFileStore::at(&path);
        assert!(store.unlock("p").is_err());
    }

    #[test]
    fn corrupted_ciphertext_errors_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = vault_path(&dir);
        {
            let store = EncryptedFileStore::at(&path);
            store.create("p").unwrap();
            store.put(&SecretId::new("a"), b"v").unwrap();
        }
        let mut bytes = fs::read(&path).unwrap();
        // Flip a byte in the middle of the ciphertext — must fail the AEAD tag, not
        // panic or silently decode garbage.
        let mid = bytes.len() - 4;
        bytes[mid] ^= 0xFF;
        fs::write(&path, &bytes).unwrap();

        let store = EncryptedFileStore::at(&path);
        assert!(store.unlock("p").is_err());
    }

    #[test]
    fn bad_magic_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = vault_path(&dir);
        {
            let store = EncryptedFileStore::at(&path);
            store.create("p").unwrap();
        }
        let mut bytes = fs::read(&path).unwrap();
        bytes[0..4].copy_from_slice(b"NOPE");
        fs::write(&path, &bytes).unwrap();

        let store = EncryptedFileStore::at(&path);
        let err = store.unlock("p").unwrap_err();
        match err {
            SecretError::Backend(msg) => assert!(msg.contains("bad magic"), "{msg}"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn zeroizing_key_type_is_used() {
        // Best-effort compile-time assertion that the derived key is wrapped in
        // `Zeroizing` (wiped on drop) rather than a plain array — see `Unlocked::key`'s
        // type and `derive_key`'s return type.
        fn assert_zeroizing(_: &Zeroizing<[u8; KEY_LEN]>) {}
        let key = derive_key("p", &[0u8; SALT_LEN], &default_params()).unwrap();
        assert_zeroizing(&key);
    }

    #[test]
    fn each_write_uses_a_fresh_nonce() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::at(vault_path(&dir));
        store.create("p").unwrap();
        let after_create = fs::read(store.path()).unwrap();
        let nonce_range = HEADER_LEN - NONCE_LEN..HEADER_LEN;
        let nonce1 = after_create[nonce_range.clone()].to_vec();

        store.put(&SecretId::new("a"), b"v").unwrap();
        let after_put = fs::read(store.path()).unwrap();
        let nonce2 = after_put[nonce_range].to_vec();

        assert_ne!(nonce1, nonce2, "every rewrite must draw a fresh nonce");
    }
}
