//! Connect mapping (Plan 3C, C1) — bridges a stored [`Host`] + a resolved secret into
//! the `sid_core::ssh` adapter's connect inputs (`SshHostSpec`/`SshAuth`).
//!
//! Pure, tested logic. The only I/O is the caller-supplied [`SecretStore::get`], done
//! synchronously before any SSH connect attempt — never logged, never written to disk,
//! held only long enough to build the in-memory `SshAuth`.

use std::path::PathBuf;

use sid_core::ssh::{SshAuth, SshHostSpec};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{AuthMethod, Host};

/// Fetch the secret backing `host.secret_ref`, if any.
///
/// - No ref (`None`) → `Ok(None)` — agent auth, or a key with no stored passphrase.
/// - A ref present in the store → `Ok(Some(bytes))`.
/// - A *dangling* ref (recorded but missing from the keyring) → `Err` only when the host
///   requires the secret to authenticate (`Password`); under `Key` auth a dangling ref
///   degrades to "no passphrase" rather than a hard error, since the passphrase was
///   always optional there.
pub fn resolve_secret(secrets: &dyn SecretStore, host: &Host) -> Result<Option<Vec<u8>>, String> {
    let Some(secret_ref) = host.secret_ref.as_ref() else {
        return Ok(None);
    };
    let id = SecretId::new(secret_ref.clone());
    let found = secrets
        .get(&id)
        .map_err(|e| format!("secret lookup for {secret_ref:?} failed: {e}"))?;
    match found {
        Some(bytes) => Ok(Some(bytes)),
        None if matches!(host.auth, AuthMethod::Password) => Err(format!(
            "host {:?} has a dangling secret_ref {secret_ref:?} — password auth cannot proceed \
             without it",
            host.alias
        )),
        None => Ok(None),
    }
}

/// Whether an SSH connect attempt should pause for the connect-time password prompt
/// (round-D §A.4) instead of surfacing [`resolve_secret`]'s error — or [`connect_params`]'
/// "no secret was resolved" error — outright.
///
/// Only `AuthMethod::Password` ever hard-requires a secret to proceed: `Agent` never
/// needs one, and `Key`'s passphrase is optional (a dangling passphrase ref already
/// degrades to `Ok(None)` in `resolve_secret`), so neither ever prompts. `secret` is
/// whatever `resolve_secret` returned for this host — `Err` (a dangling `secret_ref`)
/// and `Ok(None)` (no `secret_ref` recorded at all) both count as "nothing concretely
/// usable"; only `Ok(Some(_))` skips the prompt.
pub fn needs_password_prompt(auth: &AuthMethod, secret: &Result<Option<Vec<u8>>, String>) -> bool {
    matches!(auth, AuthMethod::Password) && !matches!(secret, Ok(Some(_)))
}

/// Map a stored host + its resolved secret into the adapter's connect inputs.
///
/// `AuthMethod::Agent` ignores `secret` entirely (the running ssh-agent supplies keys).
/// `AuthMethod::Password` requires a secret and UTF-8-decodes it. `AuthMethod::Key` takes
/// an optional passphrase (also UTF-8) — a key may have none.
pub fn connect_params(
    host: &Host,
    secret: Option<Vec<u8>>,
) -> Result<(SshHostSpec, SshAuth), String> {
    let spec = SshHostSpec {
        host: host.host.clone(),
        port: host.port,
        user: host.user.clone(),
    };
    let auth = match &host.auth {
        AuthMethod::Agent => SshAuth::Agent,
        AuthMethod::Password => {
            let bytes = secret.ok_or_else(|| {
                format!(
                    "host {:?} uses password auth but no secret was resolved",
                    host.alias
                )
            })?;
            let password = utf8(bytes, "password")?;
            SshAuth::Password(password)
        }
        AuthMethod::Key { path } => {
            let passphrase = secret
                .map(|bytes| utf8(bytes, "key passphrase"))
                .transpose()?;
            SshAuth::Key {
                path: PathBuf::from(path),
                passphrase,
            }
        }
    };
    Ok((spec, auth))
}

fn utf8(bytes: Vec<u8>, what: &str) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("{what} secret is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_secrets::MemorySecretStore;

    fn host(auth: AuthMethod, secret_ref: Option<&str>) -> Host {
        Host {
            alias: "box1".into(),
            user: "alice".into(),
            host: "example.com".into(),
            port: 2222,
            secret_ref: secret_ref.map(str::to_string),
            auth,
            folder: None,
        }
    }

    #[test]
    fn agent_auth_needs_no_secret() {
        let h = host(AuthMethod::Agent, None);
        let (spec, auth) = connect_params(&h, None).unwrap();
        assert_eq!(spec.host, "example.com");
        assert_eq!(spec.port, 2222);
        assert_eq!(spec.user, "alice");
        assert_eq!(auth, SshAuth::Agent);
    }

    #[test]
    fn password_auth_with_secret_decodes_utf8() {
        let h = host(AuthMethod::Password, Some("ssh.box1.password"));
        let (_, auth) = connect_params(&h, Some(b"hunter2".to_vec())).unwrap();
        assert_eq!(auth, SshAuth::Password("hunter2".into()));
    }

    #[test]
    fn password_auth_missing_secret_is_err() {
        let h = host(AuthMethod::Password, Some("ssh.box1.password"));
        assert!(connect_params(&h, None).is_err());
    }

    #[test]
    fn key_auth_with_passphrase() {
        let h = host(
            AuthMethod::Key {
                path: "/home/alice/.ssh/id_ed25519".into(),
            },
            Some("ssh.box1.passphrase"),
        );
        let (_, auth) = connect_params(&h, Some(b"correct-horse".to_vec())).unwrap();
        assert_eq!(
            auth,
            SshAuth::Key {
                path: PathBuf::from("/home/alice/.ssh/id_ed25519"),
                passphrase: Some("correct-horse".into()),
            }
        );
    }

    #[test]
    fn key_auth_without_passphrase() {
        let h = host(
            AuthMethod::Key {
                path: "/home/alice/.ssh/id_ed25519".into(),
            },
            None,
        );
        let (_, auth) = connect_params(&h, None).unwrap();
        assert_eq!(
            auth,
            SshAuth::Key {
                path: PathBuf::from("/home/alice/.ssh/id_ed25519"),
                passphrase: None,
            }
        );
    }

    #[test]
    fn dangling_password_ref_errors() {
        let store = MemorySecretStore::new();
        let h = host(AuthMethod::Password, Some("ssh.box1.password"));
        let err = resolve_secret(&store, &h).unwrap_err();
        assert!(err.contains("dangling"));
    }

    #[test]
    fn dangling_key_passphrase_ref_degrades_to_none() {
        let store = MemorySecretStore::new();
        let h = host(
            AuthMethod::Key {
                path: "/home/alice/.ssh/id_ed25519".into(),
            },
            Some("ssh.box1.passphrase"),
        );
        assert_eq!(resolve_secret(&store, &h).unwrap(), None);
    }

    #[test]
    fn resolve_secret_none_ref_is_none() {
        let store = MemorySecretStore::new();
        let h = host(AuthMethod::Agent, None);
        assert_eq!(resolve_secret(&store, &h).unwrap(), None);
    }

    #[test]
    fn resolve_secret_present_ref_returns_bytes() {
        let store = MemorySecretStore::new();
        let id = SecretId::new("ssh.box1.password");
        store.put(&id, b"s3cret").unwrap();
        let h = host(AuthMethod::Password, Some("ssh.box1.password"));
        assert_eq!(
            resolve_secret(&store, &h).unwrap(),
            Some(b"s3cret".to_vec())
        );
    }

    // ---- needs_password_prompt (round-D §A.4) ------------------------------------

    #[test]
    fn password_auth_with_a_concrete_secret_never_prompts() {
        assert!(!needs_password_prompt(
            &AuthMethod::Password,
            &Ok(Some(b"hunter2".to_vec()))
        ));
    }

    #[test]
    fn password_auth_with_no_secret_at_all_needs_a_prompt() {
        assert!(needs_password_prompt(&AuthMethod::Password, &Ok(None)));
    }

    #[test]
    fn password_auth_with_a_dangling_ref_needs_a_prompt() {
        let err: Result<Option<Vec<u8>>, String> = Err("dangling secret_ref".into());
        assert!(needs_password_prompt(&AuthMethod::Password, &err));
    }

    #[test]
    fn agent_auth_never_prompts_regardless_of_secret() {
        assert!(!needs_password_prompt(&AuthMethod::Agent, &Ok(None)));
        assert!(!needs_password_prompt(
            &AuthMethod::Agent,
            &Err("whatever".into())
        ));
    }

    #[test]
    fn key_auth_never_prompts_regardless_of_secret() {
        let key = AuthMethod::Key {
            path: "~/.ssh/id_ed25519".into(),
        };
        assert!(!needs_password_prompt(&key, &Ok(None)));
        assert!(!needs_password_prompt(&key, &Err("whatever".into())));
    }
}
