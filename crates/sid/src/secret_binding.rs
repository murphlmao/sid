//! Endpoint-bound secrets (security review H2).
//!
//! A record's `secret_ref` is an opaque keyring id, and a workspace-scope record is
//! committed where every collaborator can read and edit it. Before this module, connect
//! fetched whatever id a record named and sent it: a commit that changed `host`/`port`/
//! `dsn` but kept `secret_ref` — or another repo that copied the ref — made the victim's
//! next Connect hand their stored password to the new endpoint.
//!
//! # The model
//!
//! Every stored secret now has a [`SecretBinding`] in the **global, uncommitted** redb
//! store, keyed by its keyring key: the layer it was saved for and the endpoint it was
//! saved to be sent to (an [`ssh_endpoint`] / [`db_endpoint`] fingerprint — never the
//! secret). [`fetch`] only hands a secret back when the record asking for it still has
//! that layer and that endpoint; otherwise it reports [`Bound::Withheld`] and the
//! connect flow asks for the password again, saying why. Typing it again ([`save`])
//! rebinds it — the user's own act is the only thing that moves a binding.
//!
//! Workspace-scope secrets are also keyed per workspace ([`key_for`] →
//! [`SecretId::for_workspace`]), so a ref copied into another repo names a different,
//! empty keyring entry.
//!
//! # Trust on first use, for bindings — never across layers
//!
//! Secrets saved before bindings existed have none. [`fetch`] binds such a secret to the
//! record asking for it the first time it is used (atomically —
//! [`GlobalStore::bind_secret_if_unbound`] refuses to overwrite) **only when the key
//! belongs to that record's own layer**: a global record's bare key, or a workspace
//! record's own namespaced key. The trade-off: if a global record was already tampered
//! with *before* the first post-upgrade connect, that connect still sends the password
//! to the tampered endpoint, exactly as every connect did before this module; every later
//! change is caught. (Global records live in the local redb, not in a committed file, so
//! "tampered before the upgrade" means local access, which owns the keyring anyway.)
//!
//! A workspace record never reaches a bare key at all — not even to read it. Workspace
//! secrets saved before keys were namespaced lived under bare keys, but so do *global*
//! secrets, and a hostile committed `.sid/config.toml` could name one: falling back to
//! the bare key and first-use-binding it would hand the victim's global password to the
//! committed endpoint (security review finding on this very fallback). Such a record gets
//! [`Bound::Unsaved`] instead, and the user enters the password once more; [`save`] then
//! stores it under the workspace's own key, bound. The pre-namespacing bare entry is left
//! in the keyring untouched (see [`forget`] for why sid never deletes it either).

use sid_secrets::{SecretId, SecretStore};
use sid_store::{AuthMethod, DbConnection, GlobalStore, Host, Scope, SecretBinding, StoreError};

/// What [`fetch`] found for a record's `secret_ref`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Bound {
    /// Verified against this record's layer and endpoint (or first-use bound to them):
    /// safe to send.
    Found(Vec<u8>),
    /// Nothing is stored under this ref for this (global) record — a dangling ref.
    Missing,
    /// A workspace record whose own namespaced key holds nothing. Its ref may name a
    /// pre-namespacing bare key — or a global secret a hostile config is fishing for — so
    /// the bare key is deliberately never read. `notice` explains the re-prompt.
    Unsaved { notice: String },
    /// A secret is stored, but it was saved for a different endpoint or layer (or the
    /// ref is forged in sid's reserved key spelling). It was **not** returned; `notice`
    /// says why, for the re-prompt.
    Withheld { notice: String },
}

/// [`Bound::Unsaved`]'s explanation.
const UNSAVED_NOTICE: &str = "no password is saved for this connection in this workspace — \
     enter it to save it here. (Passwords saved for workspace connections before sid kept \
     them per workspace have to be entered once more.)";

/// The explanation for a committed ref spelled like one of sid's namespaced keys.
const FORGED_NOTICE: &str = "the saved password was not looked up: this connection's \
     secret_ref is spelled like one of sid's own per-workspace keys, which a config file \
     can only do to reach another workspace's password. Enter the password only if you \
     trust this connection.";

/// The keyring key a record in `origin` stores `secret_ref` under. Global records use
/// the bare ref; workspace records are namespaced by their workspace id.
pub(crate) fn key_for(origin: &Scope, secret_ref: &str) -> SecretId {
    match origin {
        Scope::Global => SecretId::new(secret_ref),
        Scope::Workspace(id) => SecretId::for_workspace(id.as_str(), secret_ref),
    }
}

/// Fetch the secret a record in `origin` names as `secret_ref`, but only if it was saved
/// for `endpoint` in `origin` — see the module doc. Exactly one keyring key is ever read:
/// the record's own ([`key_for`]). A store failure while checking the binding is an `Err`
/// (fail closed: the caller prompts or reports, never sends).
pub(crate) fn fetch(
    secrets: &dyn SecretStore,
    bindings: &GlobalStore,
    origin: &Scope,
    secret_ref: &str,
    endpoint: &str,
) -> Result<Bound, String> {
    if SecretId::is_namespaced(secret_ref) {
        return Ok(Bound::Withheld {
            notice: FORGED_NOTICE.into(),
        });
    }
    let key = key_for(origin, secret_ref);
    let found = secrets
        .get(&key)
        .map_err(|e| format!("secret lookup for {secret_ref:?} failed: {e}"))?;
    let Some(bytes) = found else {
        return Ok(match origin {
            Scope::Global => Bound::Missing,
            Scope::Workspace(_) => Bound::Unsaved {
                notice: UNSAVED_NOTICE.into(),
            },
        });
    };
    // `key` was derived for `origin`, so first use may bind it there (and only there).
    let want = binding(&key, origin, endpoint);
    Ok(match verdict(bindings, &want, origin)? {
        None => Bound::Found(bytes),
        Some(notice) => Bound::Withheld { notice },
    })
}

/// `None` when `want` is (or just became) the key's binding; otherwise why not.
fn verdict(
    bindings: &GlobalStore,
    want: &SecretBinding,
    key_scope: &Scope,
) -> Result<Option<String>, String> {
    let read = |bindings: &GlobalStore| {
        bindings
            .secret_binding(&want.key)
            .map_err(|e| format!("secret binding lookup failed: {e}"))
    };
    let existing = match read(bindings)? {
        Some(b) => b,
        None => match bindings.bind_secret_if_unbound(want, key_scope) {
            Ok(()) => return Ok(None),
            // Another first use got there between the read and the write: judge this
            // record against the binding that won, never against nothing.
            Err(StoreError::Conflict(_)) => {
                read(bindings)?.ok_or_else(|| "secret binding vanished mid-check".to_string())?
            }
            Err(e) => return Err(format!("secret binding write failed: {e}")),
        },
    };
    Ok(mismatch_notice(&existing, want))
}

/// Why `existing` does not cover `want`, worded for the password prompt; `None` if it
/// does.
fn mismatch_notice(existing: &SecretBinding, want: &SecretBinding) -> Option<String> {
    if existing.scope != want.scope {
        Some(
            "the saved password was not sent: it was saved for a connection in a different \
             layer (another workspace, or global). Enter it again only if you trust this \
             connection."
                .into(),
        )
    } else if existing.endpoint != want.endpoint {
        Some(format!(
            "the saved password was not sent: this connection's endpoint changed since it \
             was saved (saved for {}, now {}). Enter it again only if you trust the new \
             endpoint.",
            existing.endpoint, want.endpoint
        ))
    } else {
        None
    }
}

/// Store `bytes` as the secret a record in `origin` names as `secret_ref`, bound to
/// `endpoint` — the user just typed it for this record (a form save, a password prompt),
/// which is what makes this endpoint trusted. The binding is written **first**, so a
/// failure never leaves an unbound secret behind for a later first use to claim.
pub(crate) fn save(
    secrets: &dyn SecretStore,
    bindings: &GlobalStore,
    origin: &Scope,
    secret_ref: &str,
    endpoint: &str,
    bytes: &[u8],
) -> Result<(), String> {
    if SecretId::is_namespaced(secret_ref) {
        return Err(format!(
            "refusing secret_ref {secret_ref:?}: that spelling is reserved for sid's own keys"
        ));
    }
    let key = key_for(origin, secret_ref);
    bindings
        .rebind_secret(&binding(&key, origin, endpoint))
        .map_err(|e| format!("couldn't record what the secret is for: {e}"))?;
    secrets.put(&key, bytes).map_err(|e| e.to_string())
}

/// Delete the secret (and its binding) a record in `origin` names as `secret_ref` — its
/// own key only. A workspace record's pre-namespacing bare entry is **not** deleted: a
/// bare key is indistinguishable from a global secret, and a hostile committed record
/// naming the victim's global ref must not be a way to delete it. The cost is that such
/// a legacy entry stays in the OS keyring (still behind the keyring's own protection)
/// until the user removes it there.
pub(crate) fn forget(
    secrets: &dyn SecretStore,
    bindings: &GlobalStore,
    origin: &Scope,
    secret_ref: &str,
) -> Result<(), String> {
    if SecretId::is_namespaced(secret_ref) {
        return Ok(());
    }
    let key = key_for(origin, secret_ref);
    secrets.delete(&key).map_err(|e| e.to_string())?;
    bindings
        .unbind_secret(key.as_str())
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// An edit kept the record's secret (`SecretPlan::Keep`) but changed its endpoint: carry
/// the binding along — but only if it still described the record as it was *before* the
/// edit. A record tampered with before the user opened the form keeps its stale binding,
/// so saving the form for an unrelated reason (a folder rename) cannot launder the
/// tampered endpoint into a trusted one. An unbound (legacy) secret is left for first
/// use to bind.
pub(crate) fn carry_over(
    bindings: &GlobalStore,
    origin: &Scope,
    secret_ref: &str,
    old_endpoint: &str,
    new_endpoint: &str,
) -> Result<(), String> {
    if old_endpoint == new_endpoint || SecretId::is_namespaced(secret_ref) {
        return Ok(());
    }
    let key = key_for(origin, secret_ref);
    let existing = bindings
        .secret_binding(key.as_str())
        .map_err(|e| e.to_string())?;
    if existing.as_ref() == Some(&binding(&key, origin, old_endpoint)) {
        bindings
            .rebind_secret(&binding(&key, origin, new_endpoint))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A record moved between layers (promote/demote): move its secret with it, if — and
/// only if — the secret was verified for that record in its old layer. Anything else is
/// left where it is; the next connect from the new layer finds nothing and prompts.
pub(crate) fn relocate(
    secrets: &dyn SecretStore,
    bindings: &GlobalStore,
    from: &Scope,
    to: &Scope,
    secret_ref: &str,
    endpoint: &str,
) -> Result<(), String> {
    if from == to {
        return Ok(());
    }
    if let Bound::Found(bytes) = fetch(secrets, bindings, from, secret_ref, endpoint)? {
        save(secrets, bindings, to, secret_ref, endpoint, &bytes)?;
        forget(secrets, bindings, from, secret_ref)?;
    }
    Ok(())
}

fn binding(key: &SecretId, origin: &Scope, endpoint: &str) -> SecretBinding {
    SecretBinding {
        key: key.as_str().to_string(),
        scope: origin.clone(),
        endpoint: endpoint.to_string(),
    }
}

/// Where an SSH host's secret goes: `user@host:port`, plus which slot it fills. The slot
/// is part of the endpoint because switching a record from key auth to password auth
/// would otherwise send a key passphrase to the server as a login password.
pub(crate) fn ssh_endpoint(host: &Host) -> String {
    let slot = match host.auth {
        AuthMethod::Password => "password",
        AuthMethod::Key { .. } => "key passphrase",
        AuthMethod::Agent => "agent",
    };
    format!("{}@{}:{} ({slot})", host.user, host.host, host.port)
}

/// Where a DB connection's secret goes: its engine plus its **whole** DSN with any
/// embedded password removed.
///
/// Deliberately the whole DSN rather than a parsed host/port/database/user: libpq reads
/// far more than those four — `hostaddr` overrides `host` for the actual dial, a host
/// list fails over to later entries, `sslmode=disable` turns a TLS login into a
/// plaintext one, `service`/`passfile` redirect lookups elsewhere. Any of them can send
/// the password somewhere new while host/port/db/user stay identical, so any DSN change
/// re-prompts. The cost is one re-prompt after a legitimate DSN edit made outside the
/// form (an edit made *in* the form carries the binding over — see [`carry_over`]).
pub(crate) fn db_endpoint(conn: &DbConnection) -> String {
    format!("{} ({:?})", redact_dsn(&conn.dsn), conn.kind)
}

/// `dsn` with any password removed: a URL's `user:password@` userinfo and `password=`
/// query parameter, or a key/value DSN's `password=` pair (quoted values included). The
/// DSNs sid writes never hold one; a hand-edited one might, and the binding is no place
/// to copy it to.
fn redact_dsn(dsn: &str) -> String {
    let dsn = dsn.trim();
    if let Some((scheme, rest)) = dsn.split_once("://") {
        let (main, query) = match rest.split_once('?') {
            Some((m, q)) => (m, Some(q)),
            None => (rest, None),
        };
        let auth_end = main.find('/').unwrap_or(main.len());
        let (authority, path) = main.split_at(auth_end);
        let authority = match authority.rsplit_once('@') {
            Some((userinfo, hosts)) => {
                let user = userinfo.split(':').next().unwrap_or("");
                format!("{user}@{hosts}")
            }
            None => authority.to_string(),
        };
        let query = query.map(|q| {
            q.split('&')
                .filter(|p| !p.to_ascii_lowercase().starts_with("password="))
                .collect::<Vec<_>>()
                .join("&")
        });
        return match query {
            Some(q) if !q.is_empty() => format!("{scheme}://{authority}{path}?{q}"),
            _ => format!("{scheme}://{authority}{path}"),
        };
    }
    // libpq allows `password=v`, `password= v` and `password = v`; drop the key and its
    // value in every spelling.
    let mut tokens = kv_tokens(dsn).into_iter().peekable();
    let mut kept = Vec::new();
    while let Some(t) = tokens.next() {
        let Some(rest) = t
            .to_ascii_lowercase()
            .strip_prefix("password")
            .map(str::to_owned)
        else {
            kept.push(t);
            continue;
        };
        let mut rest = rest;
        if rest.is_empty()
            && let Some(eq) = tokens.next_if(|n| n.starts_with('='))
        {
            rest = eq;
        }
        if rest == "=" {
            tokens.next(); // the value is the next token
        }
    }
    kept.join(" ")
}

/// Split a libpq key/value DSN on unquoted whitespace, keeping `'quoted values'` (with
/// `\` escapes) inside their token.
fn kv_tokens(dsn: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = dsn.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '\'' => {
                quoted = !quoted;
                cur.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_core::db::DbKind;
    use sid_secrets::{MemorySecretStore, SecretError};
    use sid_store::WorkspaceId;

    struct Fixture {
        _dir: tempfile::TempDir,
        bindings: GlobalStore,
        secrets: MemorySecretStore,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let bindings = GlobalStore::open(&dir.path().join("sid.redb")).unwrap();
        Fixture {
            _dir: dir,
            bindings,
            secrets: MemorySecretStore::new(),
        }
    }

    fn ws(id: &str) -> Scope {
        Scope::Workspace(WorkspaceId(id.into()))
    }

    const GOOD: &str = "alice@good.example:22 (password)";
    const EVIL: &str = "alice@evil.example:22 (password)";

    fn withheld(b: &Bound) -> bool {
        matches!(b, Bound::Withheld { .. })
    }

    fn unsaved(b: &Bound) -> bool {
        matches!(b, Bound::Unsaved { .. })
    }

    /// A `SecretStore` that records every key it is asked to read.
    #[derive(Default)]
    struct Recording {
        inner: MemorySecretStore,
        gets: std::sync::Mutex<Vec<String>>,
    }

    impl SecretStore for Recording {
        fn put(&self, id: &SecretId, value: &[u8]) -> Result<(), SecretError> {
            self.inner.put(id, value)
        }
        fn get(&self, id: &SecretId) -> Result<Option<Vec<u8>>, SecretError> {
            self.gets.lock().unwrap().push(id.as_str().to_string());
            self.inner.get(id)
        }
        fn delete(&self, id: &SecretId) -> Result<(), SecretError> {
            self.inner.delete(id)
        }
        fn list_ids(&self) -> Result<Vec<SecretId>, SecretError> {
            self.inner.list_ids()
        }
    }

    // ---- the attack ----------------------------------------------------------------

    /// H2 itself: a commit changes the endpoint and keeps the ref. The stored password
    /// must not come back for the new endpoint.
    #[test]
    fn a_secret_is_withheld_once_its_record_points_somewhere_else() {
        let f = fixture();
        let repo = ws("/home/me/repo");
        save(&f.secrets, &f.bindings, &repo, "ssh-db-1", GOOD, b"hunter2").unwrap();

        let ok = fetch(&f.secrets, &f.bindings, &repo, "ssh-db-1", GOOD).unwrap();
        assert_eq!(ok, Bound::Found(b"hunter2".to_vec()));

        let moved = fetch(&f.secrets, &f.bindings, &repo, "ssh-db-1", EVIL).unwrap();
        let Bound::Withheld { notice } = moved else {
            panic!("expected Withheld, got {moved:?}");
        };
        assert!(notice.contains("endpoint changed"), "{notice}");
        assert!(
            notice.contains("evil.example"),
            "the prompt names the new endpoint"
        );
        assert!(!notice.contains("hunter2"));
    }

    /// The copied-ref half: another repo reusing a committed ref resolves to its own,
    /// empty, namespaced key — never the original workspace's password.
    #[test]
    fn a_ref_copied_into_another_workspace_finds_nothing() {
        let f = fixture();
        save(
            &f.secrets,
            &f.bindings,
            &ws("/w/a"),
            "ssh-db-1",
            GOOD,
            b"pw",
        )
        .unwrap();
        let other = fetch(&f.secrets, &f.bindings, &ws("/w/b"), "ssh-db-1", GOOD).unwrap();
        assert!(unsaved(&other), "{other:?}");
        // Nor from the global layer.
        let global = fetch(&f.secrets, &f.bindings, &Scope::Global, "ssh-db-1", GOOD).unwrap();
        assert_eq!(global, Bound::Missing);
    }

    /// Review finding: a hostile committed workspace record naming a bare (global-format)
    /// key — the victim's global secret, or a pre-namespacing workspace one — must not
    /// first-use-bind it and send it. The bare key is never even read, nothing is bound,
    /// and the record re-prompts.
    #[test]
    fn a_workspace_record_never_reads_or_binds_a_bare_key() {
        let f = fixture();
        let secrets = Recording::default();
        secrets
            .inner
            .put(&SecretId::new("ssh-db-1"), b"global-pw")
            .unwrap();

        let got = fetch(&secrets, &f.bindings, &ws("/w"), "ssh-db-1", EVIL).unwrap();
        assert!(unsaved(&got), "{got:?}");
        assert_eq!(
            *secrets.gets.lock().unwrap(),
            vec![SecretId::for_workspace("/w", "ssh-db-1").0],
            "only the workspace's own key is read"
        );
        assert_eq!(f.bindings.secret_binding("ssh-db-1").unwrap(), None);

        // The global record that really owns it still first-use-binds normally.
        let own = fetch(&secrets, &f.bindings, &Scope::Global, "ssh-db-1", GOOD).unwrap();
        assert_eq!(own, Bound::Found(b"global-pw".to_vec()));
        assert_eq!(
            f.bindings
                .secret_binding("ssh-db-1")
                .unwrap()
                .unwrap()
                .scope,
            Scope::Global
        );
    }

    /// Re-entering the password for such a record stores it under the workspace's own
    /// key, bound; the bare entry is left exactly as it was.
    #[test]
    fn re_entering_an_unsaved_workspace_password_mints_a_namespaced_bound_key() {
        let f = fixture();
        f.secrets
            .put(&SecretId::new("ssh-db-1"), b"legacy")
            .unwrap();
        save(
            &f.secrets,
            &f.bindings,
            &ws("/w"),
            "ssh-db-1",
            GOOD,
            b"typed",
        )
        .unwrap();
        assert_eq!(
            fetch(&f.secrets, &f.bindings, &ws("/w"), "ssh-db-1", GOOD).unwrap(),
            Bound::Found(b"typed".to_vec())
        );
        assert_eq!(
            f.secrets.get(&SecretId::new("ssh-db-1")).unwrap(),
            Some(b"legacy".to_vec())
        );
    }

    /// A committed ref spelled like a namespaced key would, as a bare key, name another
    /// workspace's entry directly. It is withheld without a lookup.
    #[test]
    fn a_ref_forged_as_another_workspaces_key_is_withheld_unread() {
        let f = fixture();
        let secrets = Recording::default();
        save(&secrets, &f.bindings, &ws("/w/a"), "ssh-db-1", GOOD, b"pw").unwrap();
        let forged = SecretId::for_workspace("/w/a", "ssh-db-1").0;
        for origin in [ws("/w/b"), Scope::Global] {
            let got = fetch(&secrets, &f.bindings, &origin, &forged, GOOD).unwrap();
            assert!(withheld(&got), "{origin:?}: {got:?}");
        }
        assert!(secrets.gets.lock().unwrap().is_empty(), "no keyring read");
        assert!(save(&secrets, &f.bindings, &ws("/w/b"), &forged, GOOD, b"x").is_err());
        forget(&secrets, &f.bindings, &ws("/w/b"), &forged).unwrap();
        let own = fetch(&secrets, &f.bindings, &ws("/w/a"), "ssh-db-1", GOOD).unwrap();
        assert_eq!(own, Bound::Found(b"pw".to_vec()), "the real entry survives");
    }

    // ---- trust on first use, re-entry ----------------------------------------------

    #[test]
    fn an_unbound_global_secret_binds_on_first_use_and_then_holds() {
        let f = fixture();
        f.secrets.put(&SecretId::new("ssh-db-1"), b"pw").unwrap();
        let first = fetch(&f.secrets, &f.bindings, &Scope::Global, "ssh-db-1", GOOD).unwrap();
        assert_eq!(first, Bound::Found(b"pw".to_vec()));
        let bound = f.bindings.secret_binding("ssh-db-1").unwrap().unwrap();
        assert_eq!(bound.endpoint, GOOD);
        assert_eq!(bound.scope, Scope::Global);
        let later = fetch(&f.secrets, &f.bindings, &Scope::Global, "ssh-db-1", EVIL).unwrap();
        assert!(withheld(&later));
    }

    #[test]
    fn re_entering_the_password_rebinds_it_to_the_new_endpoint() {
        let f = fixture();
        save(&f.secrets, &f.bindings, &Scope::Global, "r", GOOD, b"old").unwrap();
        assert!(withheld(
            &fetch(&f.secrets, &f.bindings, &Scope::Global, "r", EVIL).unwrap()
        ));
        save(&f.secrets, &f.bindings, &Scope::Global, "r", EVIL, b"new").unwrap();
        assert_eq!(
            fetch(&f.secrets, &f.bindings, &Scope::Global, "r", EVIL).unwrap(),
            Bound::Found(b"new".to_vec())
        );
        assert!(withheld(
            &fetch(&f.secrets, &f.bindings, &Scope::Global, "r", GOOD).unwrap()
        ));
    }

    #[test]
    fn workspace_secrets_are_written_under_the_namespaced_key() {
        let f = fixture();
        save(&f.secrets, &f.bindings, &ws("/w"), "r", GOOD, b"pw").unwrap();
        assert_eq!(f.secrets.get(&SecretId::new("r")).unwrap(), None);
        let key = SecretId::for_workspace("/w", "r");
        assert_eq!(f.secrets.get(&key).unwrap(), Some(b"pw".to_vec()));
        assert!(f.bindings.secret_binding(key.as_str()).unwrap().is_some());
    }

    #[test]
    fn a_dangling_ref_finds_nothing_and_binds_nothing() {
        let f = fixture();
        let global = fetch(&f.secrets, &f.bindings, &Scope::Global, "gone", GOOD).unwrap();
        assert_eq!(global, Bound::Missing);
        let workspace = fetch(&f.secrets, &f.bindings, &ws("/w"), "gone", GOOD).unwrap();
        assert!(unsaved(&workspace), "{workspace:?}");
        assert_eq!(f.bindings.secret_binding("gone").unwrap(), None);
        let key = SecretId::for_workspace("/w", "gone");
        assert_eq!(f.bindings.secret_binding(key.as_str()).unwrap(), None);
    }

    // ---- edits, moves, deletes ------------------------------------------------------

    #[test]
    fn a_form_edit_of_a_verified_record_carries_the_binding_to_the_new_endpoint() {
        let f = fixture();
        save(&f.secrets, &f.bindings, &ws("/w"), "r", GOOD, b"pw").unwrap();
        carry_over(&f.bindings, &ws("/w"), "r", GOOD, EVIL).unwrap();
        assert_eq!(
            fetch(&f.secrets, &f.bindings, &ws("/w"), "r", EVIL).unwrap(),
            Bound::Found(b"pw".to_vec())
        );
    }

    /// The laundering guard: the record was already tampered with (binding says GOOD,
    /// record says EVIL) when the user opened the form and saved for another reason.
    #[test]
    fn a_form_edit_cannot_launder_an_endpoint_that_was_already_tampered() {
        let f = fixture();
        save(&f.secrets, &f.bindings, &ws("/w"), "r", GOOD, b"pw").unwrap();
        const ELSEWHERE: &str = "alice@elsewhere:22 (password)";
        carry_over(&f.bindings, &ws("/w"), "r", EVIL, ELSEWHERE).unwrap();
        assert!(withheld(
            &fetch(&f.secrets, &f.bindings, &ws("/w"), "r", ELSEWHERE).unwrap()
        ));
        assert_eq!(
            fetch(&f.secrets, &f.bindings, &ws("/w"), "r", GOOD).unwrap(),
            Bound::Found(b"pw".to_vec()),
            "the original binding is untouched"
        );
    }

    #[test]
    fn promoting_a_verified_record_moves_its_secret_and_binding() {
        let f = fixture();
        save(&f.secrets, &f.bindings, &ws("/w"), "r", GOOD, b"pw").unwrap();
        relocate(
            &f.secrets,
            &f.bindings,
            &ws("/w"),
            &Scope::Global,
            "r",
            GOOD,
        )
        .unwrap();
        assert_eq!(
            fetch(&f.secrets, &f.bindings, &Scope::Global, "r", GOOD).unwrap(),
            Bound::Found(b"pw".to_vec())
        );
        assert!(
            unsaved(&fetch(&f.secrets, &f.bindings, &ws("/w"), "r", GOOD).unwrap()),
            "nothing is left behind in the old layer"
        );
    }

    #[test]
    fn relocating_an_unverified_record_moves_nothing() {
        let f = fixture();
        save(&f.secrets, &f.bindings, &ws("/w"), "r", GOOD, b"pw").unwrap();
        relocate(
            &f.secrets,
            &f.bindings,
            &ws("/w"),
            &Scope::Global,
            "r",
            EVIL,
        )
        .unwrap();
        assert_eq!(
            fetch(&f.secrets, &f.bindings, &Scope::Global, "r", EVIL).unwrap(),
            Bound::Missing
        );
    }

    #[test]
    fn forgetting_removes_the_records_own_secret_and_binding() {
        let f = fixture();
        save(&f.secrets, &f.bindings, &ws("/w"), "r", GOOD, b"pw").unwrap();
        forget(&f.secrets, &f.bindings, &ws("/w"), "r").unwrap();
        let key = SecretId::for_workspace("/w", "r");
        assert_eq!(f.secrets.get(&key).unwrap(), None);
        assert_eq!(f.bindings.secret_binding(key.as_str()).unwrap(), None);
    }

    /// A workspace record naming a bare key — a global secret, say — must not be a way
    /// to delete it.
    #[test]
    fn forgetting_a_workspace_record_never_deletes_a_bare_key() {
        let f = fixture();
        save(
            &f.secrets,
            &f.bindings,
            &Scope::Global,
            "r",
            GOOD,
            b"global",
        )
        .unwrap();
        forget(&f.secrets, &f.bindings, &ws("/w"), "r").unwrap();
        assert_eq!(
            fetch(&f.secrets, &f.bindings, &Scope::Global, "r", GOOD).unwrap(),
            Bound::Found(b"global".to_vec())
        );
    }

    // ---- fingerprints ----------------------------------------------------------------

    fn host(auth: AuthMethod) -> Host {
        Host {
            alias: "db".into(),
            user: "alice".into(),
            host: "db.example".into(),
            port: 2222,
            secret_ref: None,
            auth,
            folder: None,
        }
    }

    #[test]
    fn an_ssh_endpoint_names_user_host_port_and_the_secret_slot() {
        assert_eq!(
            ssh_endpoint(&host(AuthMethod::Password)),
            "alice@db.example:2222 (password)"
        );
        // Swapping key auth for password auth is an endpoint change: a passphrase must
        // not be sent as a login password.
        assert_ne!(
            ssh_endpoint(&host(AuthMethod::Password)),
            ssh_endpoint(&host(AuthMethod::Key { path: "k".into() }))
        );
    }

    fn conn(dsn: &str) -> DbConnection {
        DbConnection {
            id: "c".into(),
            dsn: dsn.into(),
            secret_ref: None,
            kind: DbKind::Postgres,
            name: "c".into(),
            folder: None,
        }
    }

    #[test]
    fn a_db_endpoint_is_the_whole_dsn_so_a_tls_downgrade_counts_as_a_change() {
        let base = db_endpoint(&conn("postgres://alice@db:5432/app"));
        assert_eq!(base, "postgres://alice@db:5432/app (Postgres)");
        assert_ne!(
            base,
            db_endpoint(&conn("postgres://alice@db:5432/app?sslmode=disable"))
        );
        assert_ne!(
            db_endpoint(&conn("host=db user=alice dbname=app")),
            db_endpoint(&conn("host=db hostaddr=203.0.113.9 user=alice dbname=app"))
        );
    }

    #[test]
    fn a_db_endpoint_never_carries_a_password() {
        for (dsn, want) in [
            (
                "postgres://alice:s3cret@db:5432/app",
                "postgres://alice@db:5432/app (Postgres)",
            ),
            (
                "postgres://alice@db/app?sslmode=require&password=s3cret",
                "postgres://alice@db/app?sslmode=require (Postgres)",
            ),
            (
                "host=db password='s3 cret' user=alice",
                "host=db user=alice (Postgres)",
            ),
            ("host=db PASSWORD=s3cret", "host=db (Postgres)"),
            (
                "host=db password = 's3 cret' user=alice",
                "host=db user=alice (Postgres)",
            ),
            ("password= s3cret host=db", "host=db (Postgres)"),
        ] {
            let got = db_endpoint(&conn(dsn));
            assert_eq!(got, want, "{dsn}");
            assert!(!got.contains("s3"), "{got}");
        }
    }
}
