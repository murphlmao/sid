//! Auth methods — key, password, ssh-agent. Used by `RusshClient::connect`.

use std::path::Path;
use std::sync::Arc;

use russh::client::Handle;
use russh::keys::PrivateKeyWithHashAlg;
use sid_core::adapters::ssh::{SshAuth, SshError};

use crate::client::ClientHandler;

pub async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    auth: &SshAuth,
) -> Result<(), SshError> {
    match auth {
        SshAuth::None => {
            let r = handle
                .authenticate_none(user)
                .await
                .map_err(|e| SshError::AuthFailed(format!("{e}")))?;
            if !r.success() {
                return Err(SshError::AuthFailed("none auth rejected".into()));
            }
            Ok(())
        }
        SshAuth::Password(p) => auth_password(handle, user, p).await,
        SshAuth::Key { path, passphrase } => {
            auth_key(handle, user, path, passphrase.as_deref()).await
        }
        SshAuth::Agent => auth_agent(handle, user).await,
    }
}

async fn auth_password(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    password: &str,
) -> Result<(), SshError> {
    let r = handle
        .authenticate_password(user, password)
        .await
        .map_err(|e| SshError::AuthFailed(format!("{e}")))?;
    if !r.success() {
        return Err(SshError::AuthFailed("password rejected".into()));
    }
    Ok(())
}

async fn auth_key(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    path: &Path,
    passphrase: Option<&str>,
) -> Result<(), SshError> {
    let key = russh::keys::load_secret_key(path, passphrase)
        .map_err(|e| SshError::AuthFailed(format!("load key {path:?}: {e}")))?;
    let key_with_hash =
        PrivateKeyWithHashAlg::new(Arc::new(key), Some(russh::keys::HashAlg::Sha512));
    let r = handle
        .authenticate_publickey(user, key_with_hash)
        .await
        .map_err(|e| SshError::AuthFailed(format!("{e}")))?;
    if !r.success() {
        return Err(SshError::AuthFailed("public-key rejected".into()));
    }
    Ok(())
}

async fn auth_agent(handle: &mut Handle<ClientHandler>, user: &str) -> Result<(), SshError> {
    let sock = std::env::var("SSH_AUTH_SOCK")
        .map_err(|_| SshError::AuthFailed("SSH_AUTH_SOCK not set".into()))?;
    let mut agent = russh::keys::agent::client::AgentClient::connect_uds(&sock)
        .await
        .map_err(|e| SshError::AuthFailed(format!("connect agent: {e}")))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| SshError::AuthFailed(format!("agent identities: {e}")))?;
    if identities.is_empty() {
        return Err(SshError::AuthFailed("agent has no identities".into()));
    }
    for identity in identities {
        // russh 0.61 distinguishes plain agent public keys from OpenSSH
        // certificates (`AgentIdentity`); extract the underlying public key so
        // certificate-backed agent identities still authenticate.
        let pubkey = match identity {
            russh::keys::agent::AgentIdentity::PublicKey { key, .. } => key,
            russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                russh::keys::PublicKey::from(certificate.public_key().clone())
            }
        };
        let result = handle
            .authenticate_publickey_with(
                user,
                pubkey,
                Some(russh::keys::HashAlg::Sha512),
                &mut agent,
            )
            .await
            .map_err(|e| SshError::AuthFailed(format!("{e}")))?;
        if result.success() {
            return Ok(());
        }
    }
    Err(SshError::AuthFailed("all agent identities rejected".into()))
}
