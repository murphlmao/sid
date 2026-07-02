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

use russh::keys::{Algorithm, Error as KeysError, PublicKey};
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
///
/// Algorithm-ordering (OpenSSH `order_hostkeyalgs`, Plan 3C): a host recorded
/// only under a non-preferred algorithm on a multi-key server would otherwise
/// negotiate a different algorithm and hard-fail here as `Mismatch`, even
/// though the recorded key is trusted. [`recorded_algorithms`] lets the
/// caller (`client.rs::SshClient::connect`) put the recorded algorithm(s)
/// first in the negotiation preference *before* connecting, so this function
/// sees the key under the algorithm it was actually recorded with.
pub(crate) fn verify(
    host: &str,
    port: u16,
    pubkey: &PublicKey,
    user_known_hosts: Option<&Path>,
    app_known_hosts: &Path,
) -> Result<Verdict, SshError> {
    let mut host_present = false;
    for path in [user_known_hosts, Some(app_known_hosts)]
        .into_iter()
        .flatten()
    {
        match russh::keys::known_hosts::check_known_hosts_path(host, port, pubkey, path) {
            Ok(true) => return Ok(Verdict::Match),
            // `false` = the host is either absent from this file or present
            // with a *different-algorithm* key. Either way this file alone
            // doesn't confirm `pubkey`; probe presence below so a
            // different-algorithm entry still fails closed as `Mismatch`
            // instead of falling through to `Unknown` (which would TOFU-learn
            // an attacker's key for an already-trusted host).
            Ok(false) => {}
            Err(KeysError::KeyChanged { .. }) => return Ok(Verdict::Mismatch),
            Err(e) => {
                return Err(SshError::Other(format!("known_hosts read {path:?}: {e}")));
            }
        }
        // A missing/unreadable file returns an empty vec here (never an
        // error) — the hard-error case for a genuinely unreadable file was
        // already handled by the `check_known_hosts_path` match above.
        if russh::keys::known_hosts::known_host_keys_path(host, port, path)
            .map(|keys| !keys.is_empty())
            .unwrap_or(false)
        {
            host_present = true;
        }
    }
    if host_present {
        Ok(Verdict::Mismatch)
    } else {
        Ok(Verdict::Unknown)
    }
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

/// Algorithms `host:port` is recorded under, across the user file (if given)
/// then the app file, in file/line order, deduped. Empty if the host is
/// absent from both — the caller's negotiation preference is then left at
/// its default order.
///
/// Used to implement OpenSSH's `order_hostkeyalgs`: see [`verify`]'s doc
/// comment above.
pub(crate) fn recorded_algorithms(
    host: &str,
    port: u16,
    user_known_hosts: Option<&Path>,
    app_known_hosts: &Path,
) -> Vec<Algorithm> {
    let mut algorithms = Vec::new();
    for path in [user_known_hosts, Some(app_known_hosts)]
        .into_iter()
        .flatten()
    {
        // A missing/unreadable file returns an empty vec here (never an
        // error) — see the identical comment in `verify` above.
        if let Ok(keys) = russh::keys::known_hosts::known_host_keys_path(host, port, path) {
            for (_line, key) in keys {
                let algo = key.algorithm();
                if !algorithms.contains(&algo) {
                    algorithms.push(algo);
                }
            }
        }
    }
    algorithms
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
    // A second *algorithm* (RSA) for the algorithm-swap regression test, from
    // ssh-key's own test fixtures (`tests/examples/id_rsa_4096.pub`) — not a
    // real service key.
    const KEY_RSA: &str = "AAAAB3NzaC1yc2EAAAADAQABAAACAQC0WRHtxuxefSJhpIxGq4ibGFgwYnESPm8C3JFM88A1JJLoprenklrd7VJ+VH3Ov/bQwZwLyRU5dRmfR/SWTtIPWs7tToJVayKKDB+/qoXmM5ui/0CU2U4rCdQ6PdaCJdC7yFgpPL8WexjWN06+eSIKYz1AAXbx9rRv1iasslK/KUqtsqzVliagI6jl7FPO2GhRZMcso6LsZGgSxuYf/Lp0D/FcBU8GkeOo1Sx5xEt8H8bJcErtCe4Blb8JxcW6EXO3sReb4z+zcR07gumPgFITZ6hDA8sSNuvo/AlWg0IKTeZSwHHVknWdQqDJ0uczE837caBxyTZllDNIGkBjCIIOFzuTT76HfYc/7CTTGk07uaNkUFXKN79xDiFOX8JQ1ZZMZvGOTwWjuT9CqgdTvQRORbRWwOYv3MH8re9ykw3Ip6lrPifY7s6hOaAKry/nkGPMt40m1TdiW98MTIpooE7W+WXu96ax2l2OJvxX8QR7l+LFlKnkIEEJd/ItF1G22UmOjkVwNASTwza/hlY+8DoVvEmwum/nMgH2TwQT3bTQzF9s9DOJkH4d8p4Mw4gEDjNx0EgUFA91ysCAeUMQQyIvuR8HXXa+VcvhOOO5mmBcVhxJ3qUOJTyDBsT0932Zb4mNtkxdigoVxu+iiwk0vwtvKwGVDYdyMP5EAQeEIP1t0w==";

    fn pubkey(b64: &str) -> PublicKey {
        russh::keys::parse_public_key_base64(b64).unwrap()
    }

    fn openssh_line(host_field: &str, b64: &str) -> String {
        format!("{host_field} ssh-ed25519 {b64}\n")
    }

    // The on-disk algorithm label is never parsed (`known_host_keys_path` gets
    // the real algorithm from the key blob itself), but a distinct helper
    // keeps the RSA fixture lines below honest to read.
    fn openssh_rsa_line(host_field: &str, b64: &str) -> String {
        format!("{host_field} ssh-rsa {b64}\n")
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
    fn known_host_different_algorithm_is_mismatch() {
        let fx = fixture();
        // Record an ed25519 key for `localhost`...
        write_file(&fx.app, &openssh_line("localhost", KEY_A));
        // ...then present an *RSA* key for the same host. `check_known_hosts_path`
        // reports this as `Ok(false)` (different algorithm — not a `KeyChanged`
        // conflict by its own logic), which before this fix fell through to
        // `Unknown` and would have silently TOFU-relearned an attacker's key for
        // an already-trusted host. It must instead fail closed as `Mismatch`.
        let v = verify("localhost", 22, &pubkey(KEY_RSA), Some(&fx.user), &fx.app).unwrap();
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

    #[test]
    fn recorded_algorithms_single_entry() {
        let fx = fixture();
        write_file(&fx.app, &openssh_line("localhost", KEY_A));
        let algos = recorded_algorithms("localhost", 22, Some(&fx.user), &fx.app);
        assert_eq!(algos, vec![pubkey(KEY_A).algorithm()]);
    }

    #[test]
    fn recorded_algorithms_multiple_entries_are_file_ordered_and_deduped() {
        let fx = fixture();
        // Two algorithms recorded for the same host — as a real
        // ~/.ssh/known_hosts often has for a server offering both an
        // ed25519 and an RSA host key.
        let contents = openssh_line("localhost", KEY_A) + &openssh_rsa_line("localhost", KEY_RSA);
        write_file(&fx.app, &contents);
        let algos = recorded_algorithms("localhost", 22, Some(&fx.user), &fx.app);
        assert_eq!(
            algos,
            vec![pubkey(KEY_A).algorithm(), pubkey(KEY_RSA).algorithm()]
        );
    }

    #[test]
    fn recorded_algorithms_reads_hashed_entries() {
        let fx = fixture();
        // A `|1|salt|hmac` hashed host entry (HMAC-SHA1), verbatim from
        // russh's own known_hosts test fixtures — proves the algorithm
        // survives the hashed-match code path, not just the plain-host one.
        write_file(
            &fx.app,
            "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= ssh-ed25519 \
             AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF\n",
        );
        let algos = recorded_algorithms("example.com", 22, Some(&fx.user), &fx.app);
        assert_eq!(algos, vec![pubkey(KEY_B).algorithm()]);
    }

    #[test]
    fn recorded_algorithms_empty_when_host_absent() {
        let fx = fixture();
        write_file(&fx.app, &openssh_line("otherhost", KEY_A));
        assert!(recorded_algorithms("localhost", 22, Some(&fx.user), &fx.app).is_empty());
        // Neither file existing at all is also fine — never an error.
        let fx2 = fixture();
        assert!(recorded_algorithms("localhost", 22, Some(&fx2.user), &fx2.app).is_empty());
    }
}
