//! `RusshClient` core — connect/disconnect/exec/open_shell/open_sftp.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use russh::{
    Preferred,
    client::{Config, Handle, Handler},
    keys::{Algorithm, PublicKeyOrCertificate},
};
use sid_core::ssh::{
    ExecResult, SftpSession, SshAuth, SshClient, SshError, SshHostSpec, SshShellReader,
    SshShellWriter,
};

use crate::auth::authenticate;
use crate::known_hosts;

/// How long [`RusshClient::connect`] may spend on the TCP dial plus the SSH
/// transport handshake before it gives up.
///
/// Without an app-level bound the dial rides the kernel's SYN-retry budget —
/// ~127 s on Linux for a blackholed address — during which the UI can only say
/// "Connecting…". `russh`'s `Config::inactivity_timeout` does not help: it is a
/// *post-handshake* idle timer. Ten seconds is generous for a real host on a
/// slow link and short enough that a black hole is reported as a normal connect
/// error while the user is still watching.
pub const DEFAULT_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// How often an idle connection is probed (SSH keepalive request).
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// Unanswered probes tolerated before the transport is torn down.
pub const KEEPALIVE_MAX: usize = 3;

/// Factory carrying the app-level known_hosts path and dial timeout; per-host
/// clients are produced by `new_client`. The caller supplies
/// `<data_dir>/known_hosts` — the adapter contains no XDG logic.
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
    dial_timeout: Duration,
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
        Self {
            app_known_hosts,
            dial_timeout: DEFAULT_DIAL_TIMEOUT,
        }
    }

    /// Override the dial+handshake timeout every client from this factory uses
    /// (see [`DEFAULT_DIAL_TIMEOUT`]). The value enters as configuration rather
    /// than being pinned at the `russh` call site, so tests can bound the dial
    /// in milliseconds and a future settings knob has somewhere to land.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::{path::PathBuf, time::Duration};
    /// use sid_ssh::RusshClientFactory;
    /// let f = RusshClientFactory::new(PathBuf::from("/tmp/sid/known_hosts"))
    ///     .with_dial_timeout(Duration::from_secs(3));
    /// let _c = f.new_client();
    /// ```
    pub fn with_dial_timeout(mut self, dial_timeout: Duration) -> Self {
        self.dial_timeout = dial_timeout;
        self
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
            dial_timeout: self.dial_timeout,
            approved_host_key: None,
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
    /// Deadline for the dial + handshake in `connect` (see [`DEFAULT_DIAL_TIMEOUT`]).
    dial_timeout: Duration,
    /// The unknown host key the user confirmed (`SHA256:` fingerprint), consumed by the
    /// next `connect` — see [`SshClient::approve_host_key`].
    approved_host_key: Option<String>,
}

/// The user's read-only `~/.ssh/known_hosts`, if a home directory is known.
fn user_known_hosts_path() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir().map(|h| h.join(".ssh").join("known_hosts"))
}

/// OpenSSH's `order_hostkeyalgs`: `recorded` algorithms first, in their
/// recorded order, then the remaining `defaults` in their original order.
/// An algorithm recorded for the host that the client doesn't itself
/// support/offer is dropped rather than force-added.
fn order_hostkeyalgs(recorded: &[Algorithm], defaults: &[Algorithm]) -> Vec<Algorithm> {
    let mut ordered: Vec<Algorithm> = recorded
        .iter()
        .filter(|algo| defaults.contains(algo))
        .cloned()
        .collect();
    for algo in defaults {
        if !ordered.contains(algo) {
            ordered.push(algo.clone());
        }
    }
    ordered
}

/// Host-key verifying handler. Checks the server key against the user's read-only
/// `~/.ssh/known_hosts` and sid's app file ([`known_hosts::decide`]). A first-contact
/// key is learned only if the user already confirmed exactly that key (`approved`);
/// otherwise — and for a changed key, always — the reason is recorded in `verdict` and
/// the handshake is aborted so `connect` can surface `HostKeyUnknown` (for the UI to ask)
/// or `HostKeyMismatch`.
pub struct ClientHandler {
    host: String,
    port: u16,
    app_known_hosts: PathBuf,
    user_known_hosts: Option<PathBuf>,
    /// The `SHA256:` fingerprint the user confirmed for this host on the last attempt.
    approved: Option<String>,
    /// Set when verification fails, so `connect` can distinguish a host-key
    /// problem from a generic transport error (the handler can only return a
    /// `russh::Error`, which erases the reason).
    verdict: Arc<Mutex<Option<SshError>>>,
}

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // russh 0.65 hands the host key over as key-or-certificate. sid never
        // advertises a certificate host-key algorithm (`host_key_certificates` stays
        // at its empty default), so a certificate here is unsolicited: refuse it
        // rather than trust it, exactly as before the type grew the variant.
        let PublicKeyOrCertificate::PublicKey {
            key: server_public_key,
            ..
        } = server_key
        else {
            *self.verdict.lock().unwrap() = Some(SshError::ConnectFailed(
                "the server presented a host certificate; sid only trusts plain host keys".into(),
            ));
            return Ok(false);
        };
        match known_hosts::decide(
            &self.host,
            self.port,
            server_public_key,
            self.user_known_hosts.as_deref(),
            &self.app_known_hosts,
            self.approved.as_deref(),
        ) {
            Ok(()) => Ok(true),
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
        // A host recorded only under a non-preferred algorithm (e.g. imported
        // from ~/.ssh/known_hosts) must negotiate *that* algorithm, or the 3B
        // fail-closed check below sees a same-host, different-algorithm entry
        // and hard-fails as a Mismatch (see `known_hosts::verify`'s doc comment).
        let recorded = known_hosts::recorded_algorithms(
            &host.host,
            host.port,
            user_known_hosts_path().as_deref(),
            &self.app_known_hosts,
        );
        let preferred = if recorded.is_empty() {
            Preferred::DEFAULT
        } else {
            Preferred {
                key: order_hostkeyalgs(&recorded, &Preferred::DEFAULT.key).into(),
                ..Preferred::DEFAULT
            }
        };
        let config = Arc::new(Config {
            inactivity_timeout: Some(Duration::from_secs(300)),
            // A server that dies silently (stopped container, pulled cable) never sends
            // a FIN we can see in time; probe it so the transport (and with it every
            // channel) closes within ~keepalive_interval * (keepalive_max + 1).
            keepalive_interval: Some(KEEPALIVE_INTERVAL),
            keepalive_max: KEEPALIVE_MAX,
            preferred,
            ..Default::default()
        });
        let addr = format!("{}:{}", host.host, host.port);
        let verdict = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            host: host.host.clone(),
            port: host.port,
            app_known_hosts: self.app_known_hosts.clone(),
            user_known_hosts: user_known_hosts_path(),
            // One attempt only: an approval is for the key the user just looked at.
            approved: self.approved_host_key.take(),
            verdict: verdict.clone(),
        };
        // Bound the TCP dial *and* the SSH transport handshake. `russh` bounds
        // neither: `Config::inactivity_timeout` above only arms once the
        // connection is up, so without this a blackholed address burns the
        // kernel's SYN-retry budget (~127 s) and a peer that accepts but never
        // sends an identification string hangs forever. The deadline is
        // configuration (`RusshClientFactory::with_dial_timeout`), not a literal
        // at this call site, so tests can bound it in milliseconds.
        //
        // Cancellation: this is a plain future with no background task of its
        // own, so a future "cancel connect" button needs no new adapter API —
        // dropping the `connect` future, or aborting the task it runs in via the
        // `JoinHandle` the UI already holds (`sid/src/ui/session.rs`'s
        // `ssh_runtime().spawn`), tears the dial down immediately.
        let dial = russh::client::connect(config, addr.as_str(), handler);
        let dialed = match tokio::time::timeout(self.dial_timeout, dial).await {
            Ok(dialed) => dialed,
            Err(_elapsed) => return Err(SshError::Timeout(self.dial_timeout)),
        };
        let mut handle = match dialed {
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

    fn approve_host_key(&mut self, fingerprint: &str) {
        self.approved_host_key = Some(fingerprint.to_string());
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
        self.handle.as_ref().is_some_and(|h| !h.is_closed())
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
    ) -> Result<(Box<dyn SshShellReader>, Box<dyn SshShellWriter>), SshError> {
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
        let (reader, writer) = crate::shell::split(channel);
        Ok((Box::new(reader), Box::new(writer)))
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

    /// Bug-hunt 2026-07-26 #5: a connect to a blackholed address hung for ~127 s
    /// (the kernel's SYN-retry budget) with the tab stuck on "Connecting…".
    /// The dial must fail on *our* deadline instead.
    ///
    /// 192.0.2.1 is TEST-NET-1 (RFC 5737): routable-looking, never answers. On a
    /// host with no route to it the dial fails immediately with `ConnectFailed`
    /// instead of timing out — also a pass, since the property under test is the
    /// bounded wait, not which of the two error shapes wins the race.
    #[tokio::test]
    async fn connect_to_a_blackholed_host_fails_within_the_dial_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let dial_timeout = Duration::from_millis(300);
        let factory =
            RusshClientFactory::new(dir.path().join("known_hosts")).with_dial_timeout(dial_timeout);
        let mut client = factory.new_client();
        let spec = SshHostSpec {
            host: "192.0.2.1".into(),
            port: 22,
            user: "nobody".into(),
        };

        let started = std::time::Instant::now();
        let err = client
            .connect(&spec, &SshAuth::None)
            .await
            .expect_err("a blackholed address cannot connect");
        let elapsed = started.elapsed();

        assert!(
            elapsed < dial_timeout + Duration::from_secs(2),
            "dial was not bounded: gave up after {elapsed:?}, expected ~{dial_timeout:?}"
        );
        assert!(
            matches!(err, SshError::Timeout(_) | SshError::ConnectFailed(_)),
            "unexpected error shape: {err:?}"
        );
    }

    /// The other half of the same hang: a peer that completes the TCP handshake
    /// and then never sends its SSH identification string. Hermetic — a local
    /// listening socket that is never accepted from — so this one pins the exact
    /// error mapping (`SshError::Timeout`) that the UI renders.
    #[tokio::test]
    async fn connect_to_a_peer_that_never_sends_a_banner_times_out() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let dial_timeout = Duration::from_millis(300);
        let factory =
            RusshClientFactory::new(dir.path().join("known_hosts")).with_dial_timeout(dial_timeout);
        let mut client = factory.new_client();
        let spec = SshHostSpec {
            host: "127.0.0.1".into(),
            port,
            user: "nobody".into(),
        };

        let started = std::time::Instant::now();
        let err = client
            .connect(&spec, &SshAuth::None)
            .await
            .expect_err("a silent peer cannot complete the SSH handshake");
        let elapsed = started.elapsed();

        assert!(
            matches!(err, SshError::Timeout(d) if d == dial_timeout),
            "expected a Timeout carrying the configured deadline, got {err:?}"
        );
        assert!(
            elapsed < dial_timeout + Duration::from_secs(2),
            "handshake was not bounded: gave up after {elapsed:?}"
        );
    }

    #[test]
    fn order_hostkeyalgs_moves_recorded_first() {
        let defaults = vec![
            Algorithm::Ed25519,
            Algorithm::Ecdsa {
                curve: russh::keys::EcdsaCurve::NistP256,
            },
            Algorithm::Rsa { hash: None },
        ];
        // Recorded under the algorithm that's last in the default order —
        // the exact "multi-key server, non-preferred algorithm" case this
        // fixes (Plan 3C).
        let recorded = vec![Algorithm::Rsa { hash: None }];
        let ordered = order_hostkeyalgs(&recorded, &defaults);
        assert_eq!(ordered[0], Algorithm::Rsa { hash: None });
        assert_eq!(ordered.len(), defaults.len(), "no algorithm gained or lost");
    }

    #[test]
    fn order_hostkeyalgs_ignores_unsupported_recorded_algorithm() {
        // A recorded algorithm the client doesn't offer at all (not in
        // `defaults`) must not be force-added — only reordered among what's
        // already supported.
        let defaults = vec![Algorithm::Ed25519, Algorithm::Rsa { hash: None }];
        let recorded = vec![Algorithm::Dsa];
        assert_eq!(order_hostkeyalgs(&recorded, &defaults), defaults);
    }
}
