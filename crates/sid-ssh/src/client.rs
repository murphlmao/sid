//! `RusshClient` core — connect/disconnect/exec/open_shell/open_sftp.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use russh::{
    client::{Config, Handle, Handler},
    keys::PublicKey,
};
use sid_core::ssh::{ExecResult, SftpSession, SshAuth, SshClient, SshError, SshHostSpec, SshShell};

use crate::auth::authenticate;

/// Stateless factory; per-host clients are produced by `new_client`.
///
/// # Examples
///
/// ```
/// use sid_ssh::RusshClientFactory;
/// let f = RusshClientFactory::new();
/// let _c = f.new_client();
/// ```
pub struct RusshClientFactory;

impl RusshClientFactory {
    /// Construct a new factory. Cheap; no I/O.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ssh::RusshClientFactory;
    /// let _f = RusshClientFactory::new();
    /// ```
    pub fn new() -> Self {
        Self
    }

    /// Construct a fresh per-host client. Not yet connected.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ssh::RusshClientFactory;
    /// let f = RusshClientFactory::new();
    /// let _c = f.new_client();
    /// ```
    pub fn new_client(&self) -> RusshClient {
        RusshClient { handle: None }
    }
}

impl Default for RusshClientFactory {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-host SSH client backed by `russh`.
///
/// # Examples
///
/// ```no_run
/// use sid_ssh::RusshClientFactory;
/// let f = RusshClientFactory::new();
/// let _c = f.new_client();
/// ```
pub struct RusshClient {
    pub(crate) handle: Option<Handle<ClientHandler>>,
}

/// Permissive handler: accept any server key.
// B4 replaces this with TOFU known-hosts verification.
pub struct ClientHandler;

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
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
        let mut handle = russh::client::connect(config, addr.as_str(), ClientHandler)
            .await
            .map_err(|e| SshError::ConnectFailed(format!("{e}")))?;
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
        let f = RusshClientFactory::new();
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
