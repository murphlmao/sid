//! Auth methods — key, password, ssh-agent. Used by `RusshClient::connect`.
//!
//! # The agent gap (issue #2)
//!
//! `AuthMethod::Agent` is `#[default]`, so every host sid has ever saved without a
//! deliberate choice dials with agent auth. This module used to answer that, on a
//! machine with no agent, with the four words `SSH_AUTH_SOCK not set` — the name of an
//! environment variable, handed to someone who was trying to open a terminal. It is a
//! particularly bad answer for a *desktop* app: a session launched from a `.desktop`
//! entry inherits the environment as it stood at login, so "there is no agent here" is
//! the normal case, not the exotic one. [`agent_unavailable`] is the whole vocabulary
//! for that failure now, and every arm of it names the way out.

use std::{path::Path, sync::Arc};

use russh::{client::Handle, keys::PrivateKeyWithHashAlg};
use sid_core::ssh::{SshAuth, SshError};

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

/// Why agent auth could not carry a connection — the failure view of one auth method,
/// as varied as the ways it actually breaks, because the user's next move differs for
/// each. See [`agent_unavailable`] for the words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentGap<'a> {
    /// `SSH_AUTH_SOCK` is unset: there is no agent in this process's environment at all.
    /// The reporter's case, and the default one for a desktop launch.
    NoSocket,
    /// A socket is named but the agent behind it would not answer.
    Unreachable(&'a str),
    /// The agent is up and holding nothing.
    NoIdentities,
    /// The agent offered keys and the server took none of them.
    AllRejected(usize),
}

/// A sentence a person can act on, for each way agent auth fails.
///
/// Every arm names the alternative that is one click away in the connection form (key
/// auth), because that is the actual remedy — "start an agent" means relaunching sid on
/// a desktop, and telling someone to go set an environment variable is not an answer.
pub fn agent_unavailable(gap: AgentGap<'_>) -> String {
    /// The one-click way out, spelled the same in every arm that has it.
    const USE_A_KEY: &str = "Choose key auth for this connection";
    match gap {
        AgentGap::NoSocket => format!(
            "no SSH agent is available — SSH_AUTH_SOCK is not set, and an app launched \
             from the desktop does not inherit a shell's environment. {USE_A_KEY}, or \
             start an ssh-agent and relaunch sid."
        ),
        AgentGap::Unreachable(why) => format!(
            "no SSH agent is available — SSH_AUTH_SOCK is set, but the agent it points \
             at did not answer ({why}). {USE_A_KEY}, or start an ssh-agent and relaunch \
             sid."
        ),
        AgentGap::NoIdentities => format!(
            "the SSH agent is running but holds no keys — run `ssh-add` to load one, or \
             {}.",
            USE_A_KEY.to_ascii_lowercase()
        ),
        AgentGap::AllRejected(offered) => format!(
            "the SSH agent offered {offered} key(s) and the server accepted none of \
             them — add the right key to the agent with `ssh-add`, or {}.",
            USE_A_KEY.to_ascii_lowercase()
        ),
    }
}

async fn auth_agent(handle: &mut Handle<ClientHandler>, user: &str) -> Result<(), SshError> {
    let sock = std::env::var("SSH_AUTH_SOCK")
        .map_err(|_| SshError::AuthFailed(agent_unavailable(AgentGap::NoSocket)))?;
    let mut agent = russh::keys::agent::client::AgentClient::connect_uds(&sock)
        .await
        .map_err(|e| {
            SshError::AuthFailed(agent_unavailable(AgentGap::Unreachable(&e.to_string())))
        })?;
    let identities = agent.request_identities().await.map_err(|e| {
        SshError::AuthFailed(agent_unavailable(AgentGap::Unreachable(&e.to_string())))
    })?;
    if identities.is_empty() {
        return Err(SshError::AuthFailed(agent_unavailable(
            AgentGap::NoIdentities,
        )));
    }
    let offered = identities.len();
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
    Err(SshError::AuthFailed(agent_unavailable(
        AgentGap::AllRejected(offered),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the four words used to be, kept here as the thing every message must not be.
    const THE_OLD_ANSWER: &str = "SSH_AUTH_SOCK not set";

    #[test]
    fn a_missing_agent_says_so_in_words_before_it_names_a_variable() {
        let msg = agent_unavailable(AgentGap::NoSocket);
        assert!(msg.starts_with("no SSH agent is available"), "{msg}");
    }

    #[test]
    fn a_missing_agent_explains_why_a_desktop_launch_has_none() {
        // The reporter had no agent running at all, and a desktop-launched sid could not
        // have seen one anyway — a message that only names the variable leaves someone
        // hunting for a shell that has it set.
        let msg = agent_unavailable(AgentGap::NoSocket);
        assert!(msg.contains("desktop"), "{msg}");
        assert!(msg.contains("does not inherit"), "{msg}");
    }

    #[test]
    fn a_missing_agent_offers_key_auth() {
        let msg = agent_unavailable(AgentGap::NoSocket);
        assert!(msg.contains("Choose key auth for this connection"), "{msg}");
    }

    #[test]
    fn an_unreachable_agent_keeps_the_underlying_reason() {
        // Legible does not mean lossy: the socket-level reason is what distinguishes a
        // dead agent from a permissions problem.
        let msg = agent_unavailable(AgentGap::Unreachable("Connection refused"));
        assert!(msg.contains("Connection refused"), "{msg}");
        assert!(msg.contains("did not answer"), "{msg}");
    }

    #[test]
    fn an_empty_agent_is_a_different_problem_with_a_different_fix() {
        // An agent that is *there* needs a key added, not key auth in the form — the two
        // must not read the same.
        let msg = agent_unavailable(AgentGap::NoIdentities);
        assert!(msg.contains("ssh-add"), "{msg}");
        assert_ne!(msg, agent_unavailable(AgentGap::NoSocket));
    }

    #[test]
    fn a_rejected_identity_set_says_how_many_were_tried() {
        let msg = agent_unavailable(AgentGap::AllRejected(3));
        assert!(msg.contains('3'), "{msg}");
    }

    #[test]
    fn no_agent_failure_is_ever_answered_with_the_bare_variable_name() {
        for gap in [
            AgentGap::NoSocket,
            AgentGap::Unreachable("boom"),
            AgentGap::NoIdentities,
            AgentGap::AllRejected(1),
        ] {
            let msg = agent_unavailable(gap);
            assert!(!msg.is_empty(), "{gap:?}: empty");
            assert!(!msg.contains(THE_OLD_ANSWER), "{gap:?}: {msg}");
            assert!(
                msg.contains("key auth") || msg.contains("ssh-add"),
                "{gap:?}: no way forward — {msg}"
            );
        }
    }

    #[test]
    fn the_message_reads_as_a_sentence_after_the_error_prefix() {
        // `SshError::AuthFailed` renders as "authentication failed: {0}", so a message
        // that opened with its own colon produced "failed: no agent: SSH_AUTH_SOCK…".
        let rendered = SshError::AuthFailed(agent_unavailable(AgentGap::NoSocket)).to_string();
        assert!(
            rendered.starts_with("authentication failed: no SSH agent"),
            "{rendered}"
        );
    }
}
