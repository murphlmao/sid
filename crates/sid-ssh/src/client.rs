//! `RusshClient` core — connect/disconnect/exec/open_shell/open_sftp.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use russh::{
    client::{Config, Handle, Handler},
    keys::PublicKey,
};
use sid_core::ssh::{ExecResult, SftpSession, SshAuth, SshClient, SshError, SshHostSpec, SshShell};

use crate::auth::authenticate;
use crate::known_hosts::{self, Verdict};

/// Factory carrying the app-level known_hosts path; per-host clients are
/// produced by `new_client`. The caller supplies `<data_dir>/known_hosts` —
/// the adapter contains no XDG logic.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_ssh::RusshClientFactory;
/// let f = RusshClientFactory::new(PathBuf::from("/tmp/sid/known_hosts"));
/// let _c = f.new_client();
/// ```
pub struct RusshClientFactory {
    app_known_hosts: PathBuf,
}

impl RusshClientFactory {
    /// Construct a factory. Cheap; no I/O. `app_known_hosts` is sid's own
    /// known_hosts file (created `0600` on first TOFU write).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_ssh::RusshClientFactory;
    /// let _f = RusshClientFactory::new(PathBuf::from("/tmp/sid/known_hosts"));
    /// ```
    pub fn new(app_known_hosts: PathBuf) -> Self {
        Self { app_known_hosts }
    }

    /// Construct a fresh per-host client. Not yet connected.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_ssh::RusshClientFactory;
    /// let f = RusshClientFactory::new(PathBuf::from("/tmp/sid/known_hosts"));
    /// let _c = f.new_client();
    /// ```
    pub fn new_client(&self) -> RusshClient {
        RusshClient {
            handle: None,
            app_known_hosts: self.app_known_hosts.clone(),
        }
    }
}

/// Per-host SSH client backed by `russh`.
///
/// # Examples
///
/// ```no_run
/// use std::path::PathBuf;
/// use sid_ssh::RusshClientFactory;
/// let f = RusshClientFactory::new(PathBuf::from("/tmp/sid/known_hosts"));
/// let _c = f.new_client();
/// ```
pub struct RusshClient {
    pub(crate) handle: Option<Handle<ClientHandler>>,
    app_known_hosts: PathBuf,
}

/// The user's read-only `~/.ssh/known_hosts`, if a home directory is known.
fn user_known_hosts_path() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir().map(|h| h.join(".ssh").join("known_hosts"))
}

/// TOFU host-key verifying handler. Checks the server key against the user's
/// read-only `~/.ssh/known_hosts` and sid's app file; on first contact it
/// learns the key into the app file. A changed key is recorded in `verdict` and
/// the handshake is aborted so `connect` can surface `HostKeyMismatch`.
pub struct ClientHandler {
    host: String,
    port: u16,
    app_known_hosts: PathBuf,
    user_known_hosts: Option<PathBuf>,
    /// Set when verification fails, so `connect` can distinguish a host-key
    /// problem from a generic transport error (the handler can only return a
    /// `russh::Error`, which erases the reason).
    verdict: Arc<Mutex<Option<SshError>>>,
}

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let outcome = known_hosts::verify(
            &self.host,
            self.port,
            server_public_key,
            self.user_known_hosts.as_deref(),
            &self.app_known_hosts,
        );
        match outcome {
            Ok(Verdict::Match) => Ok(true),
            Ok(Verdict::Unknown) => {
                // First contact — trust and record.
                match known_hosts::learn(
                    &self.host,
                    self.port,
                    server_public_key,
                    &self.app_known_hosts,
                ) {
                    Ok(()) => Ok(true),
                    Err(e) => {
                        *self.verdict.lock().unwrap() = Some(e);
                        Ok(false) // abort the handshake
                    }
                }
            }
            Ok(Verdict::Mismatch) => {
                let host_id = if self.port == 22 {
                    self.host.clone()
                } else {
                    format!("[{}]:{}", self.host, self.port)
                };
                *self.verdict.lock().unwrap() = Some(SshError::HostKeyMismatch(host_id));
                Ok(false) // abort the handshake
            }
            Err(e) => {
                *self.verdict.lock().unwrap() = Some(e);
                Ok(false) // abort the handshake
            }
        }
    }
}

#[async_trait]
impl SshClient for RusshClient {
    async fn connect(&mut self, host: &SshHostSpec, auth: &SshAuth) -> Result<(), SshError> {
        let config = Arc::new(Config {
            inactivity_timeout: Some(Duration::from_secs(300)),
            ..Default::default()
        });
        let addr = format!("{}:{}", host.host, host.port);
        let verdict = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            host: host.host.clone(),
            port: host.port,
            app_known_hosts: self.app_known_hosts.clone(),
            user_known_hosts: user_known_hosts_path(),
            verdict: verdict.clone(),
        };
        let mut handle = match russh::client::connect(config, addr.as_str(), handler).await {
            Ok(handle) => handle,
            Err(e) => {
                // If the handshake aborted because of host-key verification,
                // surface that specific reason rather than a generic transport
                // error (the handler could only signal by returning `Ok(false)`).
                if let Some(verdict_err) = verdict.lock().unwrap().take() {
                    return Err(verdict_err);
                }
                return Err(SshError::ConnectFailed(format!("{e}")));
            }
        };
        authenticate(&mut handle, &host.user, auth).await?;
        self.handle = Some(handle);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), SshError> {
        if let Some(h) = self.handle.take() {
            let _ = h
                .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                .await;
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.handle.is_some()
    }

    async fn exec(&mut self, cmd: &str) -> Result<ExecResult, SshError> {
        use russh::ChannelMsg;
        let handle = self.handle.as_mut().ok_or(SshError::NotConnected)?;
        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(map_russh_error)?;
        channel.exec(true, cmd).await.map_err(map_russh_error)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: Option<i32> = None;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
                ChannelMsg::ExtendedData { .. } => {}
                ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = Some(exit_status as i32);
                }
                // EOF means no more stdout/stderr, but the server still sends
                // `exit-status` and then `close` afterwards. Breaking on Eof
                // here would drop the exit status and report -1 (observed
                // against OpenSSH). Keep reading until the channel closes; the
                // `while let Some(..)` also ends naturally when `wait()` returns
                // None, so a server that never sends Close cannot hang us.
                ChannelMsg::Eof => {}
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(ExecResult {
            stdout,
            stderr,
            exit_code: exit_code.unwrap_or(-1),
        })
    }

    async fn open_shell(
        &mut self,
        term: &str,
        rows: u16,
        cols: u16,
    ) -> Result<Box<dyn SshShell>, SshError> {
        let handle = self.handle.as_mut().ok_or(SshError::NotConnected)?;
        let channel = handle
            .channel_open_session()
            .await
            .map_err(map_russh_error)?;
        channel
            .request_pty(true, term, cols as u32, rows as u32, 0, 0, &[])
            .await
            .map_err(map_russh_error)?;
        channel.request_shell(true).await.map_err(map_russh_error)?;
        Ok(Box::new(crate::shell::RusshShell::new(channel)))
    }

    async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError> {
        let handle = self.handle.as_mut().ok_or(SshError::NotConnected)?;
        let channel = handle
            .channel_open_session()
            .await
            .map_err(map_russh_error)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(map_russh_error)?;
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| SshError::Other(format!("sftp init: {e}")))?;
        Ok(Box::new(crate::sftp::RusshSftp::new(sftp)))
    }
}

/// Convert a russh error into the domain `SshError`.
pub(crate) fn map_russh_error(e: russh::Error) -> SshError {
    match e {
        russh::Error::Disconnect => SshError::Disconnected,
        russh::Error::NoAuthMethod => SshError::AuthFailed("no auth method".into()),
        other => SshError::Other(format!("russh: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_new_client_is_disconnected() {
        let f = RusshClientFactory::new(std::path::PathBuf::from("/tmp/sid-test/known_hosts"));
        let c = f.new_client();
        assert!(!c.is_connected());
    }

    #[test]
    fn map_russh_error_table() {
        assert!(matches!(
            map_russh_error(russh::Error::Disconnect),
            SshError::Disconnected
        ));
        assert!(matches!(
            map_russh_error(russh::Error::NoAuthMethod),
            SshError::AuthFailed(_)
        ));
        // Anything else falls through to Other.
        assert!(matches!(
            map_russh_error(russh::Error::RequestDenied),
            SshError::Other(_)
        ));
    }
}
