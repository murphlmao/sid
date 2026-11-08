//! `sid-secrets` — the secret-storage adapter seam.
//!
//! Secrets never live in the committed config or the redb store. A config record holds
//! only an opaque [`SecretId`] (`secret_ref`); the actual bytes live behind a
//! [`SecretStore`]. This crate defines that trait plus three implementations:
//! [`MemorySecretStore`] (non-persistent, tests + the runtime fallback),
//! [`keyring::KeyringStore`] (the OS Secret Service), and [`file::EncryptedFileStore`] (a
//! dependency-less passphrase-protected vault). [`resolve::resolve_secret_store`] is the
//! entry point that picks the effective backend per the user's toggle plus a startup
//! keyring durability probe.
//!
//! **2026-07-03 (round D, §A):** `file::EncryptedFileStore` is dormant — reviewed and
//! tested, but not wired into [`resolve::resolve_secret_store`]'s chain since Murphy
//! rejected the passphrase-per-launch UX. The runtime chain is keyring-or-memory only;
//! see that function's module doc for the replacement (a connect-time password prompt).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use thiserror::Error;

pub mod file;
pub mod keyring;
pub mod resolve;

pub use file::EncryptedFileStore;
pub use resolve::{
    BackendKind, KeyringProbe, Resolved, SecretBackendToggles, probe_keyring, resolve_secret_store,
};

/// An opaque reference to a secret, e.g. `"ssh.prod.key"`. This is all that ever appears
/// in committed config.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretId(pub String);

impl SecretId {
    /// Construct from anything string-like.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The keyring key for `secret_ref` as saved by a record in the workspace
    /// `namespace` (its `WorkspaceId`): `sid-ws:<len>:<namespace>:<secret_ref>`.
    ///
    /// A workspace-scope `secret_ref` is committed, so anyone can copy it into another
    /// repo. Namespacing the *key* by workspace means the copy resolves to a different
    /// keyring entry — nothing — instead of the original workspace's password. The
    /// namespace's byte length is spelled out so the encoding is injective: without it,
    /// workspace `/a` + ref `b:c` and workspace `/a:b` + ref `c` would share a key.
    ///
    /// Backend-neutral on purpose: this is the account/user string every keyring
    /// backend (Secret Service, Keychain, Credential Manager) and the memory store see,
    /// so none of them needs to know about workspaces. Global-scope secrets keep the
    /// bare `secret_ref` as their key, as do workspace secrets saved before this existed
    /// (the app still reads those, behind its endpoint-binding check).
    ///
    /// ```
    /// use sid_secrets::SecretId;
    /// let a = SecretId::for_workspace("/home/me/repo", "ssh-db1-1");
    /// assert_eq!(a.as_str(), "sid-ws:13:/home/me/repo:ssh-db1-1");
    /// assert!(SecretId::is_namespaced(a.as_str()));
    /// ```
    pub fn for_workspace(namespace: &str, secret_ref: &str) -> Self {
        Self(format!(
            "{WORKSPACE_KEY_PREFIX}{}:{namespace}:{secret_ref}",
            namespace.len()
        ))
    }

    /// Whether `raw` is already in the [`Self::for_workspace`] key form. A committed
    /// `secret_ref` spelled this way is a forgery aimed at another workspace's key and
    /// must never be used as a key verbatim.
    pub fn is_namespaced(raw: &str) -> bool {
        raw.starts_with(WORKSPACE_KEY_PREFIX)
    }
}

/// The reserved prefix of a workspace-namespaced keyring key — see
/// [`SecretId::for_workspace`].
const WORKSPACE_KEY_PREFIX: &str = "sid-ws:";

/// Errors from a secret backend.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The underlying backend failed.
    #[error("secret backend: {0}")]
    Backend(String),
    /// [`file::EncryptedFileStore`] exists but hasn't been unlocked this session (or
    /// hasn't been created yet). Dormant since round D §A — `file::EncryptedFileStore`
    /// is no longer wired into [`resolve::resolve_secret_store`]'s chain, so nothing in
    /// the `sid` crate produces this variant anymore; kept for `file.rs`'s own tests.
    #[error("secret vault is locked — unlock it first")]
    Locked,
}

/// A backend that stores opaque secret bytes keyed by [`SecretId`].
pub trait SecretStore: Send + Sync {
    /// Store (or overwrite) `value` under `id`.
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError>;
    /// Fetch the bytes for `id`, or `None` if absent.
    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError>;
    /// Delete `id` (no-op if absent).
    fn delete(&self, id: &SecretId) -> Result<(), SecretError>;
    /// List every stored id.
    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError>;
}

/// Delegate through an `Arc`, so a concrete store can be shared (e.g. the app keeps an
/// `Arc<EncryptedFileStore>` to drive the unlock modal's `unlock`/`create` calls) while
/// also being handed out as the common `Box<dyn SecretStore>` every other call site
/// uses — see [`resolve::resolve_secret_store`].
impl<T: SecretStore + ?Sized> SecretStore for Arc<T> {
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
        (**self).put(id, value)
    }
    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
        (**self).get(id)
    }
    fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
        (**self).delete(id)
    }
    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
        (**self).list_ids()
    }
}

/// An in-memory [`SecretStore`] — for tests and as a non-persistent fallback.
#[derive(Default)]
pub struct MemorySecretStore {
    map: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemorySecretStore {
    /// A fresh, empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
        self.map
            .lock()
            .expect("secret map poisoned")
            .insert(id.0.clone(), value.to_vec());
        Ok(())
    }

    fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
        Ok(self
            .map
            .lock()
            .expect("secret map poisoned")
            .get(&id.0)
            .cloned())
    }

    fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
        self.map.lock().expect("secret map poisoned").remove(&id.0);
        Ok(())
    }

    fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
        Ok(self
            .map
            .lock()
            .expect("secret map poisoned")
            .keys()
            .cloned()
            .map(SecretId)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_delete_roundtrip() {
        let s = MemorySecretStore::new();
        let id = SecretId::new("ssh.prod.key");
        assert_eq!(s.get(&id).unwrap(), None);
        s.put(&id, b"PRIVATE-KEY-BYTES").unwrap();
        assert_eq!(
            s.get(&id).unwrap().as_deref(),
            Some(&b"PRIVATE-KEY-BYTES"[..])
        );
        assert_eq!(s.list_ids().unwrap(), vec![id.clone()]);
        s.delete(&id).unwrap();
        assert_eq!(s.get(&id).unwrap(), None);
    }

    #[test]
    fn workspace_keys_are_injective_across_namespace_and_ref_boundaries() {
        // The collision a plain `{ws}:{ref}` join would allow.
        let a = SecretId::for_workspace("/a", "b:c");
        let b = SecretId::for_workspace("/a:b", "c");
        assert_ne!(a, b);
        // And a copied ref lands on a different key in a different workspace.
        assert_ne!(
            SecretId::for_workspace("/home/me/repo", "ssh-x-1"),
            SecretId::for_workspace("/home/me/fork", "ssh-x-1")
        );
    }

    #[test]
    fn a_workspace_key_never_equals_a_bare_global_ref() {
        let key = SecretId::for_workspace("/w", "ssh-x-1");
        assert_ne!(key, SecretId::new("ssh-x-1"));
        assert!(SecretId::is_namespaced(key.as_str()));
        assert!(!SecretId::is_namespaced("ssh-x-1"));
    }

    #[test]
    fn delete_absent_is_ok() {
        let s = MemorySecretStore::new();
        assert!(s.delete(&SecretId::new("nope")).is_ok());
    }
}
