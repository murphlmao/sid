//! `sid-privfs` — `sudo`-backed implementation of `sid_core::privfs::PrivilegedFs`.
//!
//! [`SudoPrivilegedFs`] is how sid reads and writes root-owned config files (`/etc/fstab`,
//! `/etc/ssh/sshd_config`, `/etc/sudoers`) after the user has unlocked them with their
//! password. See [`sudo`]'s module doc for the mechanism and the flag-by-flag rationale,
//! including why `sudo -S` rather than `pkexec`, and [`classify`]'s for how an
//! authenticator's stderr becomes a domain error.
//!
//! Stateless, like `sid-svcctl`'s provider: every call spawns its own subprocess and
//! **holds no secret of its own**. The [`Passphrase`](sid_core::privfs::Passphrase) is
//! borrowed for the duration of one operation and never stored here — whoever wants a
//! grace period keeps it in their own memory and drops it (the config editor drops it
//! when the modal closes).
//!
//! # What is *not* here
//!
//! No caching of authentication (`sudo -k` deliberately defeats sudo's own timestamp),
//! no retry loop (a wrong password is the caller's to re-prompt), and no privileged
//! entry point in sid's own binary — the elevated side is coreutils and a constant
//! shell script, so a bug in sid can never become a root bug in sid.

mod classify;
mod sudo;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sid_core::privfs::{Access, Passphrase, PrivError, PrivilegedFs, classify_access, guard_path};

/// `sudo`-backed [`PrivilegedFs`]. Stateless — nothing to lock, nothing to serialize.
#[derive(Debug, Default, Clone, Copy)]
pub struct SudoPrivilegedFs;

impl SudoPrivilegedFs {
    /// Construct the provider. `crates/sid` is the one crate allowed to name this
    /// constructor concretely (mirrors `SvcctlProvider::new()` / `SysinfoProvider::new()`)
    /// — everything after construction goes back out through
    /// [`PrivilegedFs`](sid_core::privfs::PrivilegedFs).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_privfs::SudoPrivilegedFs;
    /// let _fs = SudoPrivilegedFs::new();
    /// ```
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PrivilegedFs for SudoPrivilegedFs {
    async fn probe(&self, path: &Path) -> Access {
        let path = path.to_path_buf();
        // `stat` and `open` are fast but still blocking syscalls, and a config file can
        // live on an unresponsive mount. Off the async worker they go.
        tokio::task::spawn_blocking(move || probe_blocking(&path))
            .await
            // A probe that could not be taken fails *closed*: `Missing` is the one
            // verdict that offers no unlock affordance, so a broken probe asks the user
            // for a password sid could not honour.
            .unwrap_or(Access::Missing)
    }

    async fn read(&self, path: &Path, secret: &Passphrase) -> Result<Vec<u8>, PrivError> {
        sudo::read(guard_path(path)?, secret).await
    }

    async fn write(&self, path: &Path, bytes: &[u8], secret: &Passphrase) -> Result<(), PrivError> {
        sudo::write(guard_path(path)?, bytes, secret).await
    }
}

/// The three observations behind [`classify_access`], taken with ordinary unprivileged
/// syscalls. Blocking; called on a blocking worker.
///
/// `write(true)` — never `truncate` — is how writability is tested without modifying the
/// file, the same probe the unprivileged editor path already uses.
fn probe_blocking(path: &PathBuf) -> Access {
    let exists = match fs::metadata(path) {
        Ok(_) => true,
        // The file is there; this process may not even `stat` it (an unreadable parent
        // directory, a 0000 file). That is precisely what elevation fixes, so it must not
        // be reported as "missing".
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Access::Denied,
        Err(_) => false,
    };
    let readable = fs::OpenOptions::new().read(true).open(path).is_ok();
    let writable = fs::OpenOptions::new().write(true).open(path).is_ok();
    classify_access(exists, readable, writable)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety: the config editor holds this behind `Arc<dyn PrivilegedFs>`.
    #[allow(dead_code)]
    fn assert_object_safe(_p: &dyn PrivilegedFs) {}

    #[test]
    fn the_provider_constructs_behind_the_port() {
        let fs: std::sync::Arc<dyn PrivilegedFs> = std::sync::Arc::new(SudoPrivilegedFs::new());
        assert_object_safe(&*fs);
    }

    // ---- probe_blocking: real filesystem, no elevation involved -------------------

    #[test]
    fn an_ordinary_file_of_ours_probes_read_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.toml");
        fs::write(&path, "x").unwrap();
        assert_eq!(probe_blocking(&path), Access::ReadWrite);
    }

    #[cfg(unix)]
    #[test]
    fn a_mode_444_file_probes_read_only_the_case_that_offers_an_unlock() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        fs::write(&path, "UUID=… / ext4 defaults 0 1\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        assert_eq!(probe_blocking(&path), Access::ReadOnly);
    }

    #[cfg(unix)]
    #[test]
    fn a_mode_000_file_probes_denied_not_missing() {
        // /etc/sudoers is the real instance of this: it exists, and an unprivileged
        // process cannot read a byte of it. Reporting `Missing` would hide the file
        // behind a "does not exist" notice instead of offering to unlock it.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sudoers");
        fs::write(&path, "root ALL=(ALL:ALL) ALL\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let access = probe_blocking(&path);
        // A test run as root can read anything, so this file is ReadWrite there. Either
        // verdict is correct for the identity running the test; `Missing` never is.
        assert_ne!(access, Access::Missing, "an existing file is never Missing");
        if access != Access::ReadWrite {
            assert_eq!(access, Access::Denied);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_file_we_cannot_even_stat_probes_denied_not_missing() {
        // The `PermissionDenied` branch: an untraversable parent directory makes
        // `metadata` itself fail, so existence cannot be observed. Elevation is exactly
        // what fixes this, so reporting `Missing` would hide a file that is really there.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let closed = dir.path().join("closed");
        fs::create_dir(&closed).unwrap();
        let path = closed.join("secret.conf");
        fs::write(&path, "x").unwrap();
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();

        let access = probe_blocking(&path);

        // Reopen the directory before `dir` drops, or the cleanup cannot recurse into it.
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o700)).unwrap();
        // Root traverses anything, so a test run as root legitimately sees ReadWrite.
        if access != Access::ReadWrite {
            assert_eq!(access, Access::Denied);
        }
    }

    #[test]
    fn a_nonexistent_file_probes_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(probe_blocking(&dir.path().join("nope")), Access::Missing);
    }

    // ---- the path guard is enforced at the port, before any subprocess -------------

    #[tokio::test]
    async fn a_relative_path_is_refused_without_ever_spawning_sudo() {
        // If this reached `sudo`, the test would hang or prompt. It must fail on the
        // domain precondition instead.
        let fs = SudoPrivilegedFs::new();
        let secret = Passphrase::new("irrelevant".into());
        let err = fs.read(Path::new("etc/fstab"), &secret).await.unwrap_err();
        assert!(matches!(err, PrivError::UnsafePath(_)), "got {err:?}");

        let err = fs
            .write(Path::new("etc/fstab"), b"x", &secret)
            .await
            .unwrap_err();
        assert!(matches!(err, PrivError::UnsafePath(_)), "got {err:?}");
    }
}
