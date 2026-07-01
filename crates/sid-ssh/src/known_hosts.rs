//! TOFU (trust-on-first-use) host-key verification.
//!
//! Closes the POC's accept-any-key hole. Verification consults two files:
//!   1. the user's `~/.ssh/known_hosts` (read-only — never written by sid), and
//!   2. sid's own app known_hosts file (`<data_dir>/known_hosts`, created 0600).
//!
//! We lean on russh 0.61's `russh::keys::known_hosts` helpers rather than
//! hand-parsing: `check_known_hosts_path` handles both plain and hashed (`|1|`)
//! host entries (HMAC-SHA1 salted match), and `learn_known_hosts_path` writes
//! the `[host]:port key` line with the correct port-qualification. Hand-parsing
//! would have to re-implement the hashed-entry match and is therefore avoided.

use std::path::Path;

use russh::keys::{Error as KeysError, PublicKey};
use sid_core::ssh::SshError;

/// Outcome of checking a server key against the known-hosts files.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// A matching entry was found — trust it.
    Match,
    /// An entry exists for this host but the key differs — hard fail (MITM).
    Mismatch,
    /// No entry in either file — first contact (caller should TOFU-learn).
    Unknown,
}

/// Check `pubkey` for `host:port` against the user file (if present) then the
/// app file. A mismatch in *either* file is fatal.
///
/// `user_known_hosts` is optional and only ever read. `app_known_hosts` is the
/// file that will be appended to on TOFU (by [`learn`]).
pub(crate) fn verify(
    host: &str,
    port: u16,
    pubkey: &PublicKey,
    user_known_hosts: Option<&Path>,
    app_known_hosts: &Path,
) -> Result<Verdict, SshError> {
    let mut seen = false;
    for path in [user_known_hosts, Some(app_known_hosts)]
        .into_iter()
        .flatten()
    {
        match russh::keys::known_hosts::check_known_hosts_path(host, port, pubkey, path) {
            Ok(true) => return Ok(Verdict::Match),
            // `false` = the host is either absent from this file or present with
            // a *different-algorithm* key (not a conflict). Keep looking.
            Ok(false) => {}
            Err(KeysError::KeyChanged { .. }) => return Ok(Verdict::Mismatch),
            Err(e) => {
                return Err(SshError::Other(format!("known_hosts read {path:?}: {e}")));
            }
        }
        // A file that exists (readable) but had no match still counts as "seen"
        // for diagnostics; `Unknown` is returned regardless.
        seen = true;
    }
    let _ = seen;
    Ok(Verdict::Unknown)
}

/// TOFU-learn: append `[host]:port key` to the app file, creating it `0600`.
///
/// Uses russh's `learn_known_hosts_path` (creates parent dirs, appends with the
/// correct port-qualification), then tightens the mode to `0600` since russh
/// does not set it.
pub(crate) fn learn(
    host: &str,
    port: u16,
    pubkey: &PublicKey,
    app_known_hosts: &Path,
) -> Result<(), SshError> {
    russh::keys::known_hosts::learn_known_hosts_path(host, port, pubkey, app_known_hosts)
        .map_err(|e| SshError::Other(format!("known_hosts append {app_known_hosts:?}: {e}")))?;
    tighten_mode(app_known_hosts)?;
    Ok(())
}

/// Set `0600` on the app known_hosts file (owner read/write only).
#[cfg(unix)]
fn tighten_mode(path: &Path) -> Result<(), SshError> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .map_err(|e| SshError::Other(format!("chmod 0600 {path:?}: {e}")))
}

#[cfg(not(unix))]
fn tighten_mode(_path: &Path) -> Result<(), SshError> {
    // Cross-platform is accommodated, not solved: mode 0600 is a Unix concept.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    // A stable ed25519 host key (from russh's own known_hosts tests) and a
    // second, distinct key for the mismatch case.
    const KEY_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const KEY_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

    fn pubkey(b64: &str) -> PublicKey {
        russh::keys::parse_public_key_base64(b64).unwrap()
    }

    fn openssh_line(host_field: &str, b64: &str) -> String {
        format!("{host_field} ssh-ed25519 {b64}\n")
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        user: PathBuf,
        app: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        Fixture {
            user: dir.path().join("user_known_hosts"),
            app: dir.path().join("app").join("known_hosts"),
            _dir: dir,
        }
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn app_file_match_is_ok() {
        let fx = fixture();
        write_file(&fx.app, &openssh_line("localhost", KEY_A));
        let v = verify("localhost", 22, &pubkey(KEY_A), Some(&fx.user), &fx.app).unwrap();
        assert_eq!(v, Verdict::Match);
    }

    #[test]
    fn user_file_match_is_ok_and_never_writes() {
        let fx = fixture();
        write_file(&fx.user, &openssh_line("localhost", KEY_A));
        let v = verify("localhost", 22, &pubkey(KEY_A), Some(&fx.user), &fx.app).unwrap();
        assert_eq!(v, Verdict::Match);
        // The app file was never created — the user file satisfied the check.
        assert!(!fx.app.exists());
    }

    #[test]
    fn different_key_is_mismatch() {
        let fx = fixture();
        write_file(&fx.app, &openssh_line("localhost", KEY_A));
        // Same host, different key ⇒ Mismatch (KeyChanged).
        let v = verify("localhost", 22, &pubkey(KEY_B), Some(&fx.user), &fx.app).unwrap();
        assert_eq!(v, Verdict::Mismatch);
    }

    #[test]
    fn mismatch_in_user_file_is_also_fatal() {
        let fx = fixture();
        write_file(&fx.user, &openssh_line("localhost", KEY_A));
        let v = verify("localhost", 22, &pubkey(KEY_B), Some(&fx.user), &fx.app).unwrap();
        assert_eq!(v, Verdict::Mismatch);
    }

    #[test]
    fn unknown_when_absent_from_both() {
        let fx = fixture();
        // Neither file exists.
        let v = verify("localhost", 22, &pubkey(KEY_A), Some(&fx.user), &fx.app).unwrap();
        assert_eq!(v, Verdict::Unknown);
    }

    #[test]
    fn learn_appends_and_second_verify_matches() {
        let fx = fixture();
        // First contact: unknown, then learn.
        assert_eq!(
            verify("localhost", 22, &pubkey(KEY_A), Some(&fx.user), &fx.app).unwrap(),
            Verdict::Unknown
        );
        learn("localhost", 22, &pubkey(KEY_A), &fx.app).unwrap();
        assert!(fx.app.exists());
        // Second contact now matches.
        assert_eq!(
            verify("localhost", 22, &pubkey(KEY_A), Some(&fx.user), &fx.app).unwrap(),
            Verdict::Match
        );
    }

    #[cfg(unix)]
    #[test]
    fn learned_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let fx = fixture();
        learn("localhost", 22, &pubkey(KEY_A), &fx.app).unwrap();
        let mode = std::fs::metadata(&fx.app).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "app known_hosts must be 0600");
    }

    #[test]
    fn port_qualified_entry_distinguished_from_port_22() {
        let fx = fixture();
        // Learn the key for port 2222 only.
        learn("localhost", 2222, &pubkey(KEY_A), &fx.app).unwrap();
        // Port 2222 matches...
        assert_eq!(
            verify("localhost", 2222, &pubkey(KEY_A), Some(&fx.user), &fx.app).unwrap(),
            Verdict::Match
        );
        // ...but port 22 is a different host entry ⇒ still Unknown.
        assert_eq!(
            verify("localhost", 22, &pubkey(KEY_A), Some(&fx.user), &fx.app).unwrap(),
            Verdict::Unknown
        );
    }

    #[test]
    fn learned_line_is_port_qualified_for_nonstandard_port() {
        let fx = fixture();
        learn("h.example", 2222, &pubkey(KEY_A), &fx.app).unwrap();
        let contents = std::fs::read_to_string(&fx.app).unwrap();
        assert!(
            contents.contains("[h.example]:2222 "),
            "expected port-qualified entry, got: {contents:?}"
        );
    }
}
