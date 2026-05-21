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
    #[error("ssh operation failed: {0}")]
    Other(String),
}

/// Host + port + user.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::ssh::SshHostSpec;
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
    /// use sid_core::adapters::ssh::SshHostSpec;
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
/// use sid_core::adapters::ssh::SshAuth;
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
/// use sid_core::adapters::ssh::ExecResult;
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
/// use sid_core::adapters::ssh::SftpEntry;
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

/// An interactive shell session.
#[async_trait]
pub trait SshShell: Send + Sync {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), SshError>;
    async fn try_read(&mut self) -> Result<Vec<u8>, SshError>;
    async fn resize(&mut self, rows: u16, cols: u16) -> Result<(), SshError>;
    async fn close(&mut self) -> Result<(), SshError>;
}

/// An SFTP session.
#[async_trait]
pub trait SftpSession: Send + Sync {
    async fn list(&mut self, path: &str) -> Result<Vec<SftpEntry>, SshError>;
    async fn get(&mut self, path: &str) -> Result<Vec<u8>, SshError>;
    async fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), SshError>;
    async fn remove_file(&mut self, path: &str) -> Result<(), SshError>;
    async fn mkdir(&mut self, path: &str) -> Result<(), SshError>;
    async fn stat(&mut self, path: &str) -> Result<Option<SftpEntry>, SshError>;
    async fn close(&mut self) -> Result<(), SshError>;
}

/// SSH operations needed by the SSH tab.
#[async_trait]
pub trait SshClient: Send + Sync {
    async fn connect(&mut self, host: &SshHostSpec, auth: &SshAuth) -> Result<(), SshError>;
    async fn disconnect(&mut self) -> Result<(), SshError>;
    fn is_connected(&self) -> bool;
    async fn exec(&mut self, cmd: &str) -> Result<ExecResult, SshError>;
    async fn open_shell(
        &mut self,
        term: &str,
        rows: u16,
        cols: u16,
    ) -> Result<Box<dyn SshShell>, SshError>;
    async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError>;
}
