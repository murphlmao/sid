//! Connect mapping (Plan 3C, C1) — bridges a stored [`Host`] + a resolved secret into
//! the `sid_core::ssh` adapter's connect inputs (`SshHostSpec`/`SshAuth`).
//!
//! Pure, tested logic. The only I/O is the caller-supplied [`SecretStore::get`] plus its
//! endpoint-binding check against the global store (`crate::secret_binding`), done
//! synchronously before any SSH connect attempt — never logged, never written to disk,
//! held only long enough to build the in-memory `SshAuth`.

use std::path::PathBuf;

use sid_core::ssh::{SshAuth, SshHostSpec};
use sid_secrets::SecretStore;
use sid_store::{AuthMethod, GlobalStore, Host, Scope};

use crate::secret_binding::{self, Bound};

/// What the connect flow has to work with for a host's secret.
#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedSecret {
    /// `Ok(Some(bytes))`: a secret verified for this host. `Ok(None)`: none recorded or
    /// needed — or one was withheld (see `withheld`). `Err`: a dangling ref the host
    /// needs (`Password`), or a lookup that failed.
    pub secret: Result<Option<Vec<u8>>, String>,
    /// Set when a stored secret exists but was **not** returned because it was saved for
    /// a different endpoint or layer (H2 — see [`crate::secret_binding`]). The connect
    /// flow must re-prompt and show this text; it never falls back to sending anyway.
    pub withheld: Option<String>,
}

impl From<Result<Option<Vec<u8>>, String>> for ResolvedSecret {
    fn from(secret: Result<Option<Vec<u8>>, String>) -> Self {
        Self {
            secret,
            withheld: None,
        }
    }
}

/// Fetch the secret backing `host.secret_ref` for a record living in `origin`, through
/// its endpoint binding (see [`crate::secret_binding`]).
///
/// - `Agent` auth never needs a secret, so nothing is looked up (or first-use bound).
/// - No ref → `Ok(None)` — a key with no stored passphrase, or a password to prompt for.
/// - A ref whose secret was saved for this host's `user@host:port` in this layer →
///   `Ok(Some(bytes))`.
/// - A ref whose secret was saved for somewhere else → `withheld`, `Ok(None)`.
/// - A workspace record with nothing under its own key (`Bound::Unsaved` — never a read
///   of a bare legacy key) → for `Password`, `withheld` with that explanation, so the
///   prompt opens and says why; for `Key`, "no passphrase", like a dangling ref.
/// - A *dangling* ref (recorded but missing from the keyring) → `Err` only when the host
///   requires the secret to authenticate (`Password`); under `Key` auth a dangling ref
///   degrades to "no passphrase" rather than a hard error, since the passphrase was
///   always optional there.
pub fn resolve_secret(
    secrets: &dyn SecretStore,
    bindings: &GlobalStore,
    host: &Host,
    origin: &Scope,
) -> ResolvedSecret {
    if matches!(host.auth, AuthMethod::Agent) {
        return Ok(None).into();
    }
    let Some(secret_ref) = host.secret_ref.as_ref() else {
        return Ok(None).into();
    };
    let endpoint = secret_binding::ssh_endpoint(host);
    match secret_binding::fetch(secrets, bindings, origin, secret_ref, &endpoint) {
        Ok(Bound::Found(bytes)) => Ok(Some(bytes)).into(),
        Ok(Bound::Withheld { notice }) => ResolvedSecret {
            secret: Ok(None),
            withheld: Some(notice),
        },
        Ok(Bound::Unsaved { notice }) if matches!(host.auth, AuthMethod::Password) => {
            ResolvedSecret {
                secret: Ok(None),
                withheld: Some(notice),
            }
        }
        Ok(Bound::Unsaved { .. }) => Ok(None).into(),
        Ok(Bound::Missing) if matches!(host.auth, AuthMethod::Password) => Err(format!(
            "host {:?} has a dangling secret_ref {secret_ref:?} — password auth cannot proceed \
             without it",
            host.alias
        ))
        .into(),
        Ok(Bound::Missing) => Ok(None).into(),
        Err(e) => Err(e).into(),
    }
}

/// Whether an SSH connect attempt should pause for the connect-time password prompt
/// (round-D §A.4) instead of surfacing [`resolve_secret`]'s error — or [`connect_params`]'
/// "no secret was resolved" error — outright.
///
/// `Agent` never needs a secret. `Password` prompts whenever nothing concretely usable
/// came back (`Err`: a dangling `secret_ref`; `Ok(None)`: none recorded). `Key`'s
/// passphrase is optional, so it prompts only when a stored passphrase was **withheld**
/// (H2) — the user has to be told, and decide, rather than meet a confusing key-decrypt
/// failure. Every withheld secret prompts, whatever the auth.
pub fn needs_password_prompt(auth: &AuthMethod, resolved: &ResolvedSecret) -> bool {
    match auth {
        AuthMethod::Agent => false,
        _ if resolved.withheld.is_some() => true,
        AuthMethod::Password => !matches!(resolved.secret, Ok(Some(_))),
        AuthMethod::Key { .. } => false,
    }
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
    use sid_secrets::{MemorySecretStore, SecretId};

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

    struct Bindings {
        _dir: tempfile::TempDir,
        store: GlobalStore,
    }

    fn bindings() -> Bindings {
        let dir = tempfile::tempdir().unwrap();
        let store = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        Bindings { _dir: dir, store }
    }

    #[test]
    fn dangling_password_ref_errors() {
        let store = MemorySecretStore::new();
        let b = bindings();
        let h = host(AuthMethod::Password, Some("ssh.box1.password"));
        let err = resolve_secret(&store, &b.store, &h, &Scope::Global)
            .secret
            .unwrap_err();
        assert!(err.contains("dangling"));
    }

    #[test]
    fn dangling_key_passphrase_ref_degrades_to_none() {
        let store = MemorySecretStore::new();
        let b = bindings();
        let h = host(
            AuthMethod::Key {
                path: "/home/alice/.ssh/id_ed25519".into(),
            },
            Some("ssh.box1.passphrase"),
        );
        let got = resolve_secret(&store, &b.store, &h, &Scope::Global);
        assert_eq!(got, Ok(None).into());
    }

    #[test]
    fn resolve_secret_none_ref_is_none() {
        let store = MemorySecretStore::new();
        let b = bindings();
        let h = host(AuthMethod::Agent, None);
        assert_eq!(
            resolve_secret(&store, &b.store, &h, &Scope::Global),
            Ok(None).into()
        );
    }

    #[test]
    fn resolve_secret_present_ref_returns_bytes() {
        let store = MemorySecretStore::new();
        let b = bindings();
        let id = SecretId::new("ssh.box1.password");
        store.put(&id, b"s3cret").unwrap();
        let h = host(AuthMethod::Password, Some("ssh.box1.password"));
        assert_eq!(
            resolve_secret(&store, &b.store, &h, &Scope::Global),
            Ok(Some(b"s3cret".to_vec())).into()
        );
    }

    /// H2 end to end at this layer: the same record, re-pointed at another host after
    /// its password was saved, gets no password — and a reason that forces the prompt.
    #[test]
    fn a_host_re_pointed_after_its_password_was_saved_gets_it_withheld() {
        let store = MemorySecretStore::new();
        let b = bindings();
        let origin = Scope::Workspace(sid_store::WorkspaceId("/home/me/repo".into()));
        let saved = host(AuthMethod::Password, Some("ssh-box1-1"));
        secret_binding::save(
            &store,
            &b.store,
            &origin,
            "ssh-box1-1",
            &secret_binding::ssh_endpoint(&saved),
            b"hunter2",
        )
        .unwrap();
        assert_eq!(
            resolve_secret(&store, &b.store, &saved, &origin).secret,
            Ok(Some(b"hunter2".to_vec()))
        );

        let mut tampered = saved.clone();
        tampered.host = "attacker.example".into();
        let got = resolve_secret(&store, &b.store, &tampered, &origin);
        assert_eq!(got.secret, Ok(None), "the password is never handed back");
        assert!(got.withheld.is_some());
        assert!(needs_password_prompt(&tampered.auth, &got));
        // Port-only and user-only changes count too.
        for change in [
            |h: &mut Host| h.port = 2200,
            |h: &mut Host| h.user = "root".into(),
        ] {
            let mut h = saved.clone();
            change(&mut h);
            assert!(
                resolve_secret(&store, &b.store, &h, &origin)
                    .withheld
                    .is_some()
            );
        }
    }

    /// Review finding: a committed workspace record naming a bare key (here a global
    /// secret) gets nothing and is re-prompted under password auth; under key auth it
    /// degrades to "no passphrase", like any dangling ref.
    #[test]
    fn a_workspace_record_naming_a_bare_key_gets_nothing_sent() {
        let store = MemorySecretStore::new();
        let b = bindings();
        store
            .put(&SecretId::new("ssh-box1-1"), b"global-pw")
            .unwrap();
        let origin = Scope::Workspace(sid_store::WorkspaceId("/w".into()));

        let pw = host(AuthMethod::Password, Some("ssh-box1-1"));
        let got = resolve_secret(&store, &b.store, &pw, &origin);
        assert_eq!(got.secret, Ok(None));
        assert!(got.withheld.is_some());
        assert!(needs_password_prompt(&pw.auth, &got));

        let key = host(AuthMethod::Key { path: "k".into() }, Some("ssh-box1-1"));
        let got = resolve_secret(&store, &b.store, &key, &origin);
        assert_eq!(got, Ok(None).into());
        assert_eq!(b.store.secret_binding("ssh-box1-1").unwrap(), None);
    }

    #[test]
    fn agent_auth_looks_nothing_up_even_with_a_ref() {
        let store = MemorySecretStore::new();
        let b = bindings();
        store.put(&SecretId::new("r"), b"pw").unwrap();
        let h = host(AuthMethod::Agent, Some("r"));
        assert_eq!(
            resolve_secret(&store, &b.store, &h, &Scope::Global),
            Ok(None).into()
        );
        assert_eq!(
            b.store.secret_binding("r").unwrap(),
            None,
            "no first-use binding for a record that never sends the secret"
        );
    }

    // ---- needs_password_prompt (round-D §A.4) ------------------------------------

    fn resolved(secret: Result<Option<Vec<u8>>, String>) -> ResolvedSecret {
        secret.into()
    }

    fn withheld() -> ResolvedSecret {
        ResolvedSecret {
            secret: Ok(None),
            withheld: Some("endpoint changed".into()),
        }
    }

    #[test]
    fn password_auth_with_a_concrete_secret_never_prompts() {
        assert!(!needs_password_prompt(
            &AuthMethod::Password,
            &resolved(Ok(Some(b"hunter2".to_vec())))
        ));
    }

    #[test]
    fn password_auth_with_no_secret_at_all_needs_a_prompt() {
        assert!(needs_password_prompt(
            &AuthMethod::Password,
            &resolved(Ok(None))
        ));
    }

    #[test]
    fn password_auth_with_a_dangling_ref_needs_a_prompt() {
        let err = resolved(Err("dangling secret_ref".into()));
        assert!(needs_password_prompt(&AuthMethod::Password, &err));
    }

    #[test]
    fn agent_auth_never_prompts_regardless_of_secret() {
        assert!(!needs_password_prompt(
            &AuthMethod::Agent,
            &resolved(Ok(None))
        ));
        assert!(!needs_password_prompt(
            &AuthMethod::Agent,
            &resolved(Err("whatever".into()))
        ));
        assert!(!needs_password_prompt(&AuthMethod::Agent, &withheld()));
    }

    #[test]
    fn key_auth_never_prompts_unless_a_passphrase_was_withheld() {
        let key = AuthMethod::Key {
            path: "~/.ssh/id_ed25519".into(),
        };
        assert!(!needs_password_prompt(&key, &resolved(Ok(None))));
        assert!(!needs_password_prompt(
            &key,
            &resolved(Err("whatever".into()))
        ));
        assert!(needs_password_prompt(&key, &withheld()));
    }

    #[test]
    fn a_withheld_password_always_prompts() {
        assert!(needs_password_prompt(&AuthMethod::Password, &withheld()));
    }
}
