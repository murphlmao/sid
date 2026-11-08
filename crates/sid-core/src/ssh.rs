//! SSH client trait + supporting domain types. Implementations live in `sid-ssh`.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Domain-shaped SSH error.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    #[error("connect failed: {0}")]
    ConnectFailed(String),
    #[error("connection closed unexpectedly")]
    Disconnected,
    #[error("operation timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("not connected - call connect() first")]
    NotConnected,
    #[error("remote path not found: {0}")]
    PathNotFound(String),
    #[error(
        "host key for {0} changed — possible MITM; remove the entry from known_hosts to re-trust"
    )]
    HostKeyMismatch(String),
    /// First contact: the server's host key is in no known_hosts file. Nothing has been
    /// trusted or written — the caller must show the user `UnknownHostKey`, and only on
    /// their confirmation retry with [`SshClient::approve_host_key`] (security review M1).
    #[error(
        "{} presented a host key sid has never seen ({} {}) — confirm it before connecting",
        .0.host, .0.key_type, .0.fingerprint
    )]
    HostKeyUnknown(UnknownHostKey),
    #[error("remote file is larger than the {0}-byte limit")]
    TooLarge(u64),
    #[error("ssh operation failed: {0}")]
    Other(String),
}

/// A host key seen for the first time, as the user must be shown it before deciding
/// whether to trust it — the payload of [`SshError::HostKeyUnknown`].
///
/// # Examples
///
/// ```
/// use sid_core::ssh::UnknownHostKey;
/// let k = UnknownHostKey {
///     host: "[db.example]:2222".into(),
///     key_type: "ssh-ed25519".into(),
///     fingerprint: "SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s".into(),
/// };
/// assert!(k.fingerprint.starts_with("SHA256:"));
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnknownHostKey {
    /// The host as known_hosts names it: `host`, or `[host]:port` off port 22.
    pub host: String,
    /// The key algorithm as OpenSSH prints it, e.g. `ssh-ed25519`.
    pub key_type: String,
    /// The SHA-256 fingerprint in OpenSSH's format (`SHA256:<unpadded base64>`), so it
    /// can be compared with `ssh-keygen -lf` output character for character.
    pub fingerprint: String,
}

/// Host + port + user.
///
/// # Examples
///
/// ```
/// use sid_core::ssh::SshHostSpec;
/// let s = SshHostSpec::new("example.com", "alice");
/// assert_eq!(s.port, 22);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshHostSpec {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl SshHostSpec {
    /// Construct with the default SSH port (22).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::ssh::SshHostSpec;
    /// let s = SshHostSpec::new("h", "u");
    /// assert_eq!(s.port, 22);
    /// ```
    pub fn new(host: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: 22,
            user: user.into(),
        }
    }
}

/// Authentication method.
///
/// # Examples
///
/// ```
/// use sid_core::ssh::SshAuth;
/// let _ = SshAuth::Agent;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SshAuth {
    None,
    Password(String),
    Key {
        path: PathBuf,
        passphrase: Option<String>,
    },
    Agent,
}

/// Result of a one-shot remote command.
///
/// # Examples
///
/// ```
/// use sid_core::ssh::ExecResult;
/// let r = ExecResult { stdout: vec![], stderr: vec![], exit_code: 0 };
/// assert_eq!(r.exit_code, 0);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

/// One entry in a remote directory listing.
///
/// # Examples
///
/// ```
/// use sid_core::ssh::SftpEntry;
/// let e = SftpEntry { name: "f".into(), is_dir: false, size: 0, mtime_secs: 0, mode: 0o644 };
/// assert_eq!(e.mode, 0o644);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_secs: i64,
    pub mode: u32,
}

/// The read half of an interactive shell session.
///
/// Split from the write half (see [`SshShellWriter`]) so a caller can hold the
/// two behind independent locks: a single dedicated task owns the reader
/// outright (no lock needed at all — see `sid/src/ui/session.rs`'s read loop),
/// while the writer sits behind its own mutex for `send_input`/`resize`/
/// `disconnect`'s mutual exclusion. Before this split, a single
/// `Arc<Mutex<Box<dyn SshShell>>>` meant a write awaiting SSH flow-control
/// window (e.g. during a large paste on a congested link) held the lock for the
/// whole `.await` and starved the read loop — a real terminal freeze.
#[async_trait]
pub trait SshShellReader: Send + Sync {
    async fn try_read(&mut self) -> Result<Vec<u8>, SshError>;
}

/// The write half of an interactive shell session: input, PTY resize, and close.
#[async_trait]
pub trait SshShellWriter: Send + Sync {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), SshError>;
    async fn resize(&mut self, rows: u16, cols: u16) -> Result<(), SshError>;
    async fn close(&mut self) -> Result<(), SshError>;
}

/// An SFTP session.
#[async_trait]
pub trait SftpSession: Send + Sync {
    async fn list(&mut self, path: &str) -> Result<Vec<SftpEntry>, SshError>;
    /// Resolve `path` (e.g. `"."`) to its canonical absolute form. Used once at session
    /// start (Plan 3.5) to discover the login's home directory — SFTP servers resolve
    /// `"."` differently per user, so the caller cannot assume any particular string.
    async fn canonicalize(&mut self, path: &str) -> Result<String, SshError>;
    async fn get(&mut self, path: &str) -> Result<Vec<u8>, SshError>;
    /// Read at most `max_bytes`, stopping early with [`SshError::TooLarge`] as soon as
    /// more arrive - the size a server reports in `stat` is not trustworthy. The default
    /// reads everything then checks; implementations should stream and stop early.
    async fn get_bounded(&mut self, path: &str, max_bytes: u64) -> Result<Vec<u8>, SshError> {
        let b = self.get(path).await?;
        if b.len() as u64 > max_bytes {
            return Err(SshError::TooLarge(max_bytes));
        }
        Ok(b)
    }
    async fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), SshError>;
    /// Create `path` with `bytes`, failing if it already exists (`O_CREAT|O_EXCL`, so a
    /// planted file or symlink is never written through). Default: unsupported, and
    /// callers needing it must fall back to something that does not need a temp file.
    async fn put_exclusive(&mut self, _path: &str, _bytes: &[u8]) -> Result<(), SshError> {
        Err(SshError::Other("exclusive create not supported".into()))
    }
    async fn remove_file(&mut self, path: &str) -> Result<(), SshError>;
    async fn mkdir(&mut self, path: &str) -> Result<(), SshError>;
    async fn stat(&mut self, path: &str) -> Result<Option<SftpEntry>, SshError>;
    /// Rename `from` to `to`. Plain SFTP rename often refuses to replace an existing
    /// `to`; callers needing replace semantics must be ready to fall back. Default:
    /// unsupported, so existing impls compile unchanged.
    async fn rename(&mut self, _from: &str, _to: &str) -> Result<(), SshError> {
        Err(SshError::Other("sftp rename not supported".into()))
    }
    async fn close(&mut self) -> Result<(), SshError>;
}

/// SSH operations needed by the SSH tab.
#[async_trait]
pub trait SshClient: Send + Sync {
    async fn connect(&mut self, host: &SshHostSpec, auth: &SshAuth) -> Result<(), SshError>;
    /// Approve, for the **next** [`Self::connect`] only, the unknown host key the user
    /// just confirmed from a [`SshError::HostKeyUnknown`], named by its `fingerprint`.
    /// If the server then presents that exact key, it is recorded and the connect goes
    /// on; any other key is reported as unknown again (a key swapped between the prompt
    /// and the retry is never trusted). It never overrides a *changed* key
    /// ([`SshError::HostKeyMismatch`]). Default: ignored — an adapter that never reports
    /// `HostKeyUnknown` has nothing to approve.
    fn approve_host_key(&mut self, _fingerprint: &str) {}
    async fn disconnect(&mut self) -> Result<(), SshError>;
    fn is_connected(&self) -> bool;
    async fn exec(&mut self, cmd: &str) -> Result<ExecResult, SshError>;
    /// Open an interactive PTY shell, returning its read half and write half
    /// separately — see [`SshShellReader`]/[`SshShellWriter`] for why they're split.
    async fn open_shell(
        &mut self,
        term: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(Box<dyn SshShellReader>, Box<dyn SshShellWriter>), SshError>;
    async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety: the connect flow (Plan 3C) holds an `SshClient` behind a
    // `Box<dyn ...>`, so all four traits must stay object-safe. These are
    // compile-only checks — if a trait gains a non-dispatchable method this
    // stops compiling.
    #[allow(dead_code)]
    fn assert_object_safe(
        _c: &dyn SshClient,
        _r: &dyn SshShellReader,
        _w: &dyn SshShellWriter,
        _f: &dyn SftpSession,
    ) {
    }

    #[test]
    fn boxed_trait_objects_construct() {
        // Prove the boxed forms name real types (no allocation of a real impl).
        fn takes_client(_: Box<dyn SshClient>) {}
        fn takes_shell_reader(_: Box<dyn SshShellReader>) {}
        fn takes_shell_writer(_: Box<dyn SshShellWriter>) {}
        fn takes_sftp(_: Box<dyn SftpSession>) {}
        let _ = takes_client;
        let _ = takes_shell_reader;
        let _ = takes_shell_writer;
        let _ = takes_sftp;
    }

    #[test]
    fn host_key_mismatch_names_host() {
        let e = SshError::HostKeyMismatch("h.example:2222".into());
        assert!(e.to_string().contains("h.example:2222"));
        assert!(e.to_string().contains("possible MITM"));
    }

    #[test]
    fn an_unknown_host_key_error_shows_everything_the_user_must_compare() {
        let e = SshError::HostKeyUnknown(UnknownHostKey {
            host: "[h.example]:2222".into(),
            key_type: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        });
        let s = e.to_string();
        assert!(s.contains("[h.example]:2222"), "{s}");
        assert!(s.contains("ssh-ed25519"), "{s}");
        assert!(s.contains("SHA256:abc"), "{s}");
    }

    #[test]
    fn spec_default_port_is_22() {
        assert_eq!(SshHostSpec::new("h", "u").port, 22);
    }
}
