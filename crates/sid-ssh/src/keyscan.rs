//! `LocalIdentities` — the `~/.ssh` + `SSH_AUTH_SOCK` implementation of
//! [`sid_core::keys::IdentityScan`].
//!
//! Every decision this module makes is borrowed from `sid_core::keys`; all it does is
//! *gather facts* and hand them over (adapters translate, never decide). The facts are
//! deliberately shallow — a directory listing, a file mode, whether a `.pub` sibling
//! exists — because of the port's privacy invariant: **a scan never opens a private
//! key**. `keys()` will happily report a key it has no permission to read, which is the
//! observable proof that it did not try.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sid_core::keys::{
    IdentityScan, KeyCandidate, KeyEvidence, KeyScanError, is_key_candidate, sort_candidates,
};

/// The private keys and agent socket this machine actually has.
pub struct LocalIdentities {
    dir: PathBuf,
    agent_sock: Option<PathBuf>,
}

impl LocalIdentities {
    /// The real machine: `$HOME/.ssh`, and whatever `SSH_AUTH_SOCK` currently points at.
    ///
    /// Reading the environment here rather than at every call is deliberate — a
    /// desktop-launched process's environment is fixed at launch, and pretending
    /// otherwise would just hide that fact one layer deeper. Whether the socket is
    /// *live* is still re-checked per call; see [`IdentityScan::agent_available`].
    pub fn from_env() -> Result<Self, KeyScanError> {
        let home = std::env::var_os("HOME").ok_or(KeyScanError::NoHome)?;
        Ok(Self::new(
            Path::new(&home).join(".ssh"),
            std::env::var_os("SSH_AUTH_SOCK")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
        ))
    }

    /// An explicit directory + agent socket — what the tests drive, and the seam a
    /// future `--ssh-dir` would arrive through.
    pub fn new(dir: impl Into<PathBuf>, agent_sock: Option<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            agent_sock,
        }
    }
}

impl IdentityScan for LocalIdentities {
    fn key_dir(&self) -> PathBuf {
        self.dir.clone()
    }

    fn keys(&self) -> Result<Vec<KeyCandidate>, KeyScanError> {
        let names = list_dir(&self.dir)?;
        let present: HashSet<&str> = names.iter().map(|e| e.name.as_str()).collect();
        let mut found: Vec<KeyCandidate> = names
            .iter()
            .filter_map(|entry| {
                let has_public_sibling = present.contains(format!("{}.pub", entry.name).as_str());
                let evidence = KeyEvidence {
                    name: &entry.name,
                    is_file: entry.is_file,
                    owner_only: entry.owner_only,
                    has_public_sibling,
                };
                is_key_candidate(&evidence).then(|| KeyCandidate {
                    path: self.dir.join(&entry.name),
                    name: entry.name.clone(),
                    has_public_sibling,
                })
            })
            .collect();
        sort_candidates(&mut found);
        Ok(found)
    }

    fn agent_available(&self) -> bool {
        // `exists()` rather than a connect: this is asked while rendering a form, and a
        // Unix-socket handshake is not something a paint should block on. A socket that
        // exists but refuses a connection still fails at connect time, where the
        // message now says so plainly (see `auth::agent_unavailable`).
        self.agent_sock.as_deref().is_some_and(|p| p.exists())
    }
}

/// One directory entry, reduced to the three facts the port's decision needs.
struct Entry {
    name: String,
    is_file: bool,
    owner_only: bool,
}

/// List `dir`, gathering [`Entry`] facts. Never opens a file.
fn list_dir(dir: &Path) -> Result<Vec<Entry>, KeyScanError> {
    if !dir.is_dir() {
        return Err(KeyScanError::NoKeyDir(dir.to_path_buf()));
    }
    let read = std::fs::read_dir(dir).map_err(|e| KeyScanError::Unreadable {
        path: dir.to_path_buf(),
        reason: e.to_string(),
    })?;
    let mut entries = Vec::new();
    for entry in read.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // `metadata()` follows symlinks (a key symlinked in from a password manager's
        // mount is still a key); a dangling link simply reports no metadata and is
        // skipped by `is_file: false`.
        let meta = entry.metadata().ok();
        entries.push(Entry {
            name,
            is_file: meta.as_ref().is_some_and(|m| m.is_file()),
            owner_only: meta.as_ref().is_some_and(owner_only),
        });
    }
    Ok(entries)
}

/// Do this file's mode bits deny group and other everything? `false` off Unix, where
/// the other two pieces of corroboration carry the decision.
#[cfg(unix)]
fn owner_only(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    meta.mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn owner_only(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_core::keys::preferred;
    use std::fs;

    /// A `~/.ssh` with `files` in it. `(name, mode)`; a `None` mode means "leave the
    /// default", which on a tempdir is world-readable — the uncorroborated case.
    fn ssh_dir(files: &[(&str, Option<u32>)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, mode) in files {
            let path = dir.path().join(name);
            fs::write(&path, b"not a real key").expect("write");
            if let Some(mode) = mode {
                set_mode(&path, *mode);
            }
        }
        dir
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    }

    #[cfg(not(unix))]
    fn set_mode(_path: &Path, _mode: u32) {}

    fn names(source: &LocalIdentities) -> Vec<String> {
        source
            .keys()
            .expect("scan")
            .into_iter()
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn a_conventional_keypair_is_found_and_ranked_first() {
        let dir = ssh_dir(&[
            ("id_rsa", Some(0o600)),
            ("id_rsa.pub", None),
            ("id_ed25519", Some(0o600)),
            ("id_ed25519.pub", None),
        ]);
        let source = LocalIdentities::new(dir.path(), None);
        assert_eq!(names(&source), vec!["id_ed25519", "id_rsa"]);
    }

    #[test]
    fn the_public_half_is_never_offered_as_a_key() {
        let dir = ssh_dir(&[("id_ed25519", Some(0o600)), ("id_ed25519.pub", None)]);
        let source = LocalIdentities::new(dir.path(), None);
        assert_eq!(names(&source), vec!["id_ed25519"]);
    }

    #[test]
    fn a_public_sibling_is_recorded_on_the_candidate_it_belongs_to() {
        let dir = ssh_dir(&[
            ("id_ed25519", Some(0o600)),
            ("id_ed25519.pub", None),
            ("orphan", Some(0o600)),
        ]);
        let source = LocalIdentities::new(dir.path(), None);
        let found = source.keys().expect("scan");
        let paired = found.iter().find(|c| c.name == "id_ed25519").expect("pair");
        let orphan = found.iter().find(|c| c.name == "orphan").expect("orphan");
        assert!(paired.has_public_sibling);
        assert!(!orphan.has_public_sibling);
    }

    #[test]
    fn opensshs_own_bookkeeping_is_not_offered_as_a_key() {
        // All three are routinely 0600, so mode bits alone would promote every one of
        // them into the picker.
        let dir = ssh_dir(&[
            ("config", Some(0o600)),
            ("known_hosts", Some(0o600)),
            ("known_hosts.old", Some(0o600)),
            ("authorized_keys", Some(0o600)),
            ("id_ed25519", Some(0o600)),
        ]);
        let source = LocalIdentities::new(dir.path(), None);
        assert_eq!(names(&source), vec!["id_ed25519"]);
    }

    #[test]
    fn a_subdirectory_is_skipped() {
        let dir = ssh_dir(&[("id_ed25519", Some(0o600))]);
        fs::create_dir(dir.path().join("id_backup")).expect("mkdir");
        let source = LocalIdentities::new(dir.path(), None);
        assert_eq!(names(&source), vec!["id_ed25519"]);
    }

    #[test]
    fn the_candidate_path_is_absolute_and_points_at_the_key() {
        let dir = ssh_dir(&[("id_ed25519", Some(0o600))]);
        let source = LocalIdentities::new(dir.path(), None);
        let found = source.keys().expect("scan");
        assert_eq!(found[0].path, dir.path().join("id_ed25519"));
        assert!(found[0].path.is_absolute());
    }

    #[test]
    #[cfg(unix)]
    fn the_scan_never_opens_the_key_it_reports() {
        // The port's privacy invariant, as an observable fact: a key this process has
        // no permission to *read* is still found. If the scan ever starts sniffing
        // headers, this goes red.
        let dir = ssh_dir(&[("id_ed25519", Some(0o000))]);
        let path = dir.path().join("id_ed25519");
        assert!(
            fs::read(&path).is_err(),
            "test setup: key is still readable"
        );
        let source = LocalIdentities::new(dir.path(), None);
        assert_eq!(names(&source), vec!["id_ed25519"]);
        set_mode(&path, 0o600); // so the tempdir can clean itself up
    }

    #[test]
    fn an_empty_key_directory_finds_nothing_without_failing() {
        // Distinct from "no directory at all": the picker's copy differs, and only one
        // of the two is an error.
        let dir = ssh_dir(&[]);
        let source = LocalIdentities::new(dir.path(), None);
        assert!(source.keys().expect("scan").is_empty());
    }

    #[test]
    fn a_missing_key_directory_says_which_one_is_missing() {
        let dir = ssh_dir(&[]);
        let missing = dir.path().join("nope");
        let source = LocalIdentities::new(&missing, None);
        let err = source.keys().expect_err("no dir");
        assert!(
            err.to_string().contains(&missing.display().to_string()),
            "{err}"
        );
    }

    #[test]
    fn the_scan_it_returns_is_already_in_preference_order() {
        // Ties the adapter's output to `sid_core::keys::preferred`, so the form's
        // prefilled path and the picker's first row can never disagree.
        let dir = ssh_dir(&[
            ("vps-1", Some(0o600)),
            ("id_rsa", Some(0o600)),
            ("id_ed25519", Some(0o600)),
        ]);
        let source = LocalIdentities::new(dir.path(), None);
        let found = source.keys().expect("scan");
        assert_eq!(preferred(&found), found.first());
    }

    // ---- the agent half -------------------------------------------------------------

    #[test]
    fn no_agent_socket_at_all_reads_as_no_agent() {
        let dir = ssh_dir(&[]);
        assert!(!LocalIdentities::new(dir.path(), None).agent_available());
    }

    #[test]
    fn a_socket_path_that_does_not_exist_reads_as_no_agent() {
        // The reporter's exact situation, one layer up: an `SSH_AUTH_SOCK` inherited
        // from a login that no longer has an agent behind it.
        let dir = ssh_dir(&[]);
        let stale = dir.path().join("agent.dead");
        let source = LocalIdentities::new(dir.path(), Some(stale));
        assert!(!source.agent_available());
    }

    #[test]
    fn a_socket_that_is_there_reads_as_an_available_agent() {
        let dir = ssh_dir(&[("agent.sock", None)]);
        let source = LocalIdentities::new(dir.path(), Some(dir.path().join("agent.sock")));
        assert!(source.agent_available());
    }
}
