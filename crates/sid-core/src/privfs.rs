//! Privileged (root) file access port + supporting domain types. Implementations live
//! in `sid-privfs`.
//!
//! The System tab's config-file editor pins and edits files like `/etc/fstab` and
//! `/etc/ssh/sshd_config`. Those are root-owned, so the unprivileged process can at best
//! read them and at worst not even that. This port is the seam through which the editor
//! asks for an *elevated* read/write without ever learning how elevation happens —
//! CLAUDE.md rule 1: an OS-integration point is a trait owned by the core, and the
//! concrete mechanism (`sudo`, `pkexec`, a setuid helper, a polkit action) lives in its
//! own crate.
//!
//! # The secret invariant
//!
//! Elevation needs a secret (the user's sudo password). [`Passphrase`] is the only way
//! to hand one across this port, and it is deliberately impoverished:
//!
//! - no `Serialize`/`Deserialize`, so it cannot reach the store or a config file;
//! - no `Display`, and a [`Debug`](fmt::Debug) impl that prints `<redacted>`, so it
//!   cannot land in a log line or a `{:?}` dump;
//! - [`Zeroizing`] backing, so the plaintext is wiped when the last clone drops.
//!
//! Everything else about secret handling is the adapter's contract (see `sid-privfs`):
//! the plaintext never appears in a process argument list or an environment variable —
//! both are world-readable through `/proc` — and it is never written to disk.
//!
//! # The failure view
//!
//! [`PrivError`] is deliberately as varied as the real failure modes, because the caller
//! reacts differently to each: [`PrivError::AuthFailed`] means *re-prompt* (the user
//! mistyped), [`PrivError::NotPermitted`] means *stop asking* (no password will ever
//! work here), and the rest are ordinary operational errors to surface.

use std::fmt;
use std::path::Path;

use async_trait::async_trait;
use zeroize::Zeroizing;

/// How the **current, unprivileged** process can reach a path — the question the editor
/// asks before deciding whether to offer an "unlock with sudo" affordance at all.
///
/// # Examples
///
/// ```
/// use sid_core::privfs::Access;
/// assert!(!Access::ReadWrite.needs_elevation());
/// assert!(Access::ReadOnly.needs_elevation());
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    /// Readable and writable as-is — no elevation needed for anything.
    ReadWrite,
    /// Readable but not writable: the editor can show the content, but saving needs
    /// elevation.
    ReadOnly,
    /// Not even readable: the editor has nothing to show until it elevates.
    Denied,
    /// The path does not exist. Elevation cannot conjure it — this is a dead end, not a
    /// permission problem.
    Missing,
}

impl Access {
    /// Whether *any* part of an edit (open or save) needs elevated privileges.
    ///
    /// [`Access::Missing`] is `false`: a nonexistent file is not an elevation problem,
    /// and offering "unlock with sudo" for one would be a lie.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::privfs::Access;
    /// assert!(Access::Denied.needs_elevation());
    /// assert!(!Access::Missing.needs_elevation());
    /// ```
    pub fn needs_elevation(self) -> bool {
        matches!(self, Self::ReadOnly | Self::Denied)
    }
}

/// Classify an access level from three independent observations of a path. Pure — the
/// probing I/O is the adapter's job, the *meaning* of what it observed is the domain's.
///
/// `readable` is checked before `writable`: a path this process cannot read is
/// [`Access::Denied`] whatever its write bit says (a write-only file is real, but there
/// is nothing an *editor* can do with one — it must read before it replaces).
///
/// # Examples
///
/// ```
/// use sid_core::privfs::{Access, classify_access};
/// assert_eq!(classify_access(true, true, true), Access::ReadWrite);
/// assert_eq!(classify_access(true, true, false), Access::ReadOnly);
/// assert_eq!(classify_access(false, false, false), Access::Missing);
/// ```
pub fn classify_access(exists: bool, readable: bool, writable: bool) -> Access {
    if !exists {
        Access::Missing
    } else if !readable {
        Access::Denied
    } else if writable {
        Access::ReadWrite
    } else {
        Access::ReadOnly
    }
}

/// A one-shot elevation secret, held in memory for the duration of an operation.
///
/// See this module's doc comment for why it has no `Serialize`, no `Display`, and a
/// redacting `Debug`. Cloning is allowed — a spawned task needs its own copy — and every
/// clone zeroizes its own buffer on drop.
///
/// # Examples
///
/// ```
/// use sid_core::privfs::Passphrase;
/// let p = Passphrase::new("hunter2".to_string());
/// assert_eq!(p.expose(), "hunter2");
/// assert!(!format!("{p:?}").contains("hunter2"));
/// ```
#[derive(Clone)]
pub struct Passphrase(Zeroizing<String>);

impl Passphrase {
    /// Take ownership of a plaintext secret. The caller's `String` is moved in, so there
    /// is no second copy left behind to forget about.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::privfs::Passphrase;
    /// let _ = Passphrase::new(String::from("s3cret"));
    /// ```
    pub fn new(plaintext: String) -> Self {
        Self(Zeroizing::new(plaintext))
    }

    /// Borrow the plaintext. Named `expose` rather than `as_str` so every use site reads
    /// as the deliberate act it is — the only legitimate destination is the
    /// authenticator's stdin.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::privfs::Passphrase;
    /// assert_eq!(Passphrase::new("x".into()).expose(), "x");
    /// ```
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is empty — the one thing a caller may ask without exposing it.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::privfs::Passphrase;
    /// assert!(Passphrase::new(String::new()).is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Redacting `Debug`: the length is not printed either, since that is itself a hint
/// about the secret.
impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passphrase(<redacted>)")
    }
}

/// Domain-shaped privileged-access error. The concrete impl maps its authenticator's
/// exit status and stderr into this — see `sid_privfs`'s classifier.
///
/// # Examples
///
/// ```
/// use sid_core::privfs::PrivError;
/// assert!(PrivError::AuthFailed.is_retryable());
/// assert!(!PrivError::NotPermitted("not a sudoer".into()).is_retryable());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PrivError {
    /// The secret was rejected. The user mistyped — ask again.
    #[error("authentication failed — wrong password")]
    AuthFailed,
    /// This user may not elevate at all (not a sudoer, or not permitted to run the
    /// helper). No password will ever work — stop asking.
    #[error("not permitted to elevate: {0}")]
    NotPermitted(String),
    /// The elevation mechanism itself is unusable: the helper is missing, or it refuses
    /// to read a secret from anywhere sid can supply one.
    #[error("elevation unavailable: {0}")]
    Unavailable(String),
    /// The elevation attempt did not finish in time and was killed.
    #[error("elevation timed out")]
    Timeout,
    /// The path is not of a shape this port will act on — see [`guard_path`]. A
    /// precondition failure, never the result of an attempt.
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    /// The file is larger than the caller was willing to hold in memory. Carried as
    /// numbers rather than prose so the caller can phrase it in its own words, and
    /// distinct from [`PrivError::Io`] because it is a *refusal*, not a failure: nothing
    /// went wrong, the answer was simply too big to accept.
    #[error("{bytes} bytes exceeds the {max_bytes}-byte limit")]
    TooLarge { bytes: u64, max_bytes: u64 },
    /// The privileged operation authenticated and ran, then failed on its own terms (no
    /// such file, read-only filesystem, ...).
    #[error("privileged file operation failed: {0}")]
    Io(String),
}

impl PrivError {
    /// Whether re-prompting for the secret could plausibly succeed. This is the
    /// decision that keeps a password prompt alive versus tearing it down, so it lives
    /// with the taxonomy rather than being re-derived at each call site.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::privfs::PrivError;
    /// assert!(PrivError::AuthFailed.is_retryable());
    /// assert!(!PrivError::Timeout.is_retryable());
    /// ```
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::AuthFailed)
    }
}

/// Precondition on every path crossing this port: it must be **absolute**.
///
/// Two reasons, one domain and one defensive. Domain: a privileged operation runs in
/// another process whose working directory is not the caller's, so a relative path does
/// not denote a stable file — it is ambiguous, and ambiguity plus root is how the wrong
/// file gets overwritten. Defensive: an absolute path cannot begin with `-`, which is
/// what an argument-injecting path would need to be mistaken for a flag. (The adapter
/// separately passes `--` before every path, so the two defenses are independent.)
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use sid_core::privfs::guard_path;
/// assert!(guard_path(Path::new("/etc/fstab")).is_ok());
/// assert!(guard_path(Path::new("etc/fstab")).is_err());
/// assert!(guard_path(Path::new("--reference=/etc/shadow")).is_err());
/// ```
pub fn guard_path(path: &Path) -> Result<&Path, PrivError> {
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        // A NUL cannot survive `exec`, so the spawn would fail anyway — but it would fail
        // as "elevation unavailable", which tells the user their machine cannot elevate
        // when in fact their path was malformed. A precondition failure is classified
        // here, where it is one.
        return Err(PrivError::UnsafePath(format!(
            "{} contains a NUL byte",
            path.display()
        )));
    }
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(PrivError::UnsafePath(format!(
            "{} is not an absolute path",
            path.display()
        )))
    }
}

/// The most an elevated read will pull into memory when the caller expresses no opinion
/// — see [`PrivilegedFs::read`].
///
/// 1 MiB, which is the config editor's own load gate: a port default larger than the one
/// consumer's limit would only buy a bigger allocation before the same refusal.
///
/// # Examples
///
/// ```
/// use sid_core::privfs::DEFAULT_READ_LIMIT;
/// assert_eq!(DEFAULT_READ_LIMIT, 1024 * 1024);
/// ```
pub const DEFAULT_READ_LIMIT: u64 = 1024 * 1024;

/// Elevated file access needed by the config-file editor. Implementations live in
/// `sid-privfs`.
///
/// Every method is `async` because elevation is an out-of-process round trip that may
/// block on a PAM conversation; none of it may run on a render thread.
///
/// # Object safety
///
/// `#[async_trait]` boxes the returned futures, so `Arc<dyn PrivilegedFs>` works despite
/// the `async fn`s.
#[async_trait]
pub trait PrivilegedFs: Send + Sync {
    /// Classify how the current, unprivileged process can reach `path`. Needs no
    /// secret and must never prompt for one — this is the question asked *before*
    /// deciding whether a prompt is warranted.
    async fn probe(&self, path: &Path) -> Access;

    /// Read `path` with elevated privileges, authenticating with `secret`, refusing
    /// anything over `max_bytes` with [`PrivError::TooLarge`].
    ///
    /// The cap is a *port* concern rather than the caller's own post-hoc check because
    /// an elevated read reaches places an unprivileged one cannot: a multi-gigabyte
    /// root-owned log, a character device, a `/proc` file whose `stat` reports 0 bytes
    /// and then streams forever. By the time a caller could measure such an answer it
    /// would already be in memory. An implementation must therefore bound what it
    /// *fetches*, not merely what it returns.
    ///
    /// Encoding policy stays the caller's (the editor also requires valid UTF-8).
    async fn read_capped(
        &self,
        path: &Path,
        max_bytes: u64,
        secret: &Passphrase,
    ) -> Result<Vec<u8>, PrivError>;

    /// [`read_capped`](Self::read_capped) at the port's own [`DEFAULT_READ_LIMIT`] — the
    /// shape a caller with no opinion about size should use.
    async fn read(&self, path: &Path, secret: &Passphrase) -> Result<Vec<u8>, PrivError> {
        self.read_capped(path, DEFAULT_READ_LIMIT, secret).await
    }

    /// Replace `path`'s contents with `bytes` with elevated privileges, authenticating
    /// with `secret`.
    ///
    /// Two guarantees, both unconditional — they are the invariant, not an option:
    /// the replacement is **atomic** (a reader sees either the whole old file or the
    /// whole new one, never a truncated one), and the file's **mode and ownership are
    /// preserved** (a 0600 root-owned file comes back 0600 root-owned).
    async fn write(&self, path: &Path, bytes: &[u8], secret: &Passphrase) -> Result<(), PrivError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- classify_access ---------------------------------------------------------

    #[test]
    fn a_readable_writable_path_needs_no_elevation() {
        assert_eq!(classify_access(true, true, true), Access::ReadWrite);
    }

    #[test]
    fn a_readable_unwritable_path_is_read_only() {
        assert_eq!(classify_access(true, true, false), Access::ReadOnly);
    }

    #[test]
    fn an_unreadable_path_is_denied() {
        assert_eq!(classify_access(true, false, false), Access::Denied);
    }

    #[test]
    fn an_unreadable_but_writable_path_is_still_denied() {
        // An editor must read before it replaces, so the write bit alone buys nothing.
        assert_eq!(classify_access(true, false, true), Access::Denied);
    }

    #[test]
    fn a_nonexistent_path_is_missing_not_denied() {
        assert_eq!(classify_access(false, false, false), Access::Missing);
    }

    #[test]
    fn nonexistence_wins_over_stale_readability_observations() {
        // The three observations are taken independently and could race; existence is
        // the one that decides, because elevation cannot create the file.
        assert_eq!(classify_access(false, true, true), Access::Missing);
    }

    #[test]
    fn only_read_only_and_denied_need_elevation() {
        assert!(Access::ReadOnly.needs_elevation());
        assert!(Access::Denied.needs_elevation());
        assert!(!Access::ReadWrite.needs_elevation());
        assert!(!Access::Missing.needs_elevation());
    }

    // ---- Passphrase: the secret invariants ---------------------------------------

    #[test]
    fn passphrase_exposes_the_plaintext_it_was_given() {
        assert_eq!(Passphrase::new("hunter2".to_string()).expose(), "hunter2");
    }

    #[test]
    fn passphrase_debug_never_prints_the_secret() {
        let p = Passphrase::new("correct horse battery staple".to_string());
        let rendered = format!("{p:?}");
        assert!(
            !rendered.contains("correct"),
            "Debug leaked the secret: {rendered}"
        );
        assert_eq!(rendered, "Passphrase(<redacted>)");
    }

    #[test]
    fn passphrase_debug_never_prints_the_secrets_length() {
        // Length is a hint too — two different secrets must render identically.
        let short = format!("{:?}", Passphrase::new("a".to_string()));
        let long = format!("{:?}", Passphrase::new("aaaaaaaaaaaaaaaaaaaa".to_string()));
        assert_eq!(short, long);
    }

    #[test]
    fn passphrase_reports_emptiness_without_exposing_anything() {
        assert!(Passphrase::new(String::new()).is_empty());
        assert!(!Passphrase::new("x".to_string()).is_empty());
    }

    // ---- PrivError::is_retryable -------------------------------------------------

    #[test]
    fn a_wrong_password_is_worth_re_prompting() {
        assert!(PrivError::AuthFailed.is_retryable());
    }

    #[test]
    fn a_user_who_may_not_elevate_is_never_re_prompted() {
        assert!(!PrivError::NotPermitted("not in the sudoers file".into()).is_retryable());
    }

    #[test]
    fn operational_failures_are_not_re_prompted() {
        // None of these mean "you mistyped", so keeping the prompt open would be a lie
        // about what went wrong.
        assert!(!PrivError::Unavailable("no sudo on PATH".into()).is_retryable());
        assert!(!PrivError::Timeout.is_retryable());
        assert!(!PrivError::UnsafePath("relative".into()).is_retryable());
        assert!(!PrivError::Io("No such file or directory".into()).is_retryable());
    }

    // ---- guard_path --------------------------------------------------------------

    #[test]
    fn guard_path_accepts_an_absolute_path() {
        let p = Path::new("/etc/ssh/sshd_config");
        assert_eq!(guard_path(p).unwrap(), p);
    }

    #[test]
    fn guard_path_rejects_a_relative_path() {
        let err = guard_path(Path::new("etc/fstab")).unwrap_err();
        assert!(matches!(err, PrivError::UnsafePath(_)), "got {err:?}");
    }

    #[test]
    fn guard_path_rejects_a_flag_shaped_path() {
        // The argument-injection shape: a "path" that a careless argv would hand to the
        // helper as an option. It is also not absolute, which is exactly why requiring
        // absoluteness is the whole guard.
        for candidate in ["-rf", "--reference=/etc/shadow", "-"] {
            let err = guard_path(Path::new(candidate)).unwrap_err();
            assert!(
                matches!(err, PrivError::UnsafePath(_)),
                "{candidate} should be rejected, got {err:?}"
            );
        }
    }

    #[test]
    fn guard_path_rejects_an_empty_path() {
        assert!(guard_path(Path::new("")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn guard_path_rejects_an_interior_nul_before_it_reaches_exec() {
        // A NUL cannot survive being handed to `exec`, so without this the failure
        // surfaces from the spawn as "elevation unavailable" — which reads as "this
        // machine cannot elevate" and is a lie about a bad path. It is a precondition
        // failure and must be classified as one.
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let hostile = Path::new(OsStr::from_bytes(b"/etc/fs\0tab"));
        let err = guard_path(hostile).unwrap_err();
        assert!(matches!(err, PrivError::UnsafePath(_)), "got {err:?}");
    }

    #[test]
    fn guard_paths_error_names_the_offending_path() {
        let err = guard_path(Path::new("relative/thing")).unwrap_err();
        assert!(
            err.to_string().contains("relative/thing"),
            "unhelpful message: {err}"
        );
    }

    // ---- the port itself ---------------------------------------------------------

    /// In-memory [`PrivilegedFs`] for domain and caller-side tests: a seeded access
    /// level, a seeded content, and one accepted secret. Lives here, beside the port, so
    /// there is exactly one fake to keep honest (T7) rather than one per consumer.
    struct FakePrivilegedFs {
        access: Access,
        content: std::sync::Mutex<Vec<u8>>,
        accepted_secret: &'static str,
        /// The `max_bytes` of the last `read_capped`, so a test can prove which cap the
        /// defaulted [`PrivilegedFs::read`] passed down.
        last_cap: std::sync::Mutex<Option<u64>>,
    }

    impl FakePrivilegedFs {
        fn new(access: Access, content: &str) -> Self {
            Self {
                access,
                content: std::sync::Mutex::new(content.as_bytes().to_vec()),
                accepted_secret: "correct",
                last_cap: std::sync::Mutex::new(None),
            }
        }

        fn authenticate(&self, secret: &Passphrase) -> Result<(), PrivError> {
            if secret.expose() == self.accepted_secret {
                Ok(())
            } else {
                Err(PrivError::AuthFailed)
            }
        }
    }

    #[async_trait]
    impl PrivilegedFs for FakePrivilegedFs {
        async fn probe(&self, _path: &Path) -> Access {
            self.access
        }

        async fn read_capped(
            &self,
            path: &Path,
            max_bytes: u64,
            secret: &Passphrase,
        ) -> Result<Vec<u8>, PrivError> {
            *self.last_cap.lock().unwrap() = Some(max_bytes);
            guard_path(path)?;
            self.authenticate(secret)?;
            let content = self.content.lock().unwrap().clone();
            if content.len() as u64 > max_bytes {
                return Err(PrivError::TooLarge {
                    bytes: content.len() as u64,
                    max_bytes,
                });
            }
            Ok(content)
        }

        async fn write(
            &self,
            path: &Path,
            bytes: &[u8],
            secret: &Passphrase,
        ) -> Result<(), PrivError> {
            guard_path(path)?;
            self.authenticate(secret)?;
            *self.content.lock().unwrap() = bytes.to_vec();
            Ok(())
        }
    }

    // Object-safety: the config editor holds this behind `Arc<dyn PrivilegedFs>`.
    // Compile-only.
    #[allow(dead_code)]
    fn assert_object_safe(_p: &dyn PrivilegedFs) {}

    /// A tiny blocking executor so the port's contract can be exercised without pulling
    /// a whole async runtime into `sid-core`'s dev-dependencies. The fake never yields,
    /// so a single poll always completes it.
    fn block_on<F: Future>(future: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
                return out;
            }
        }
    }

    #[test]
    fn the_port_is_object_safe_behind_an_arc() {
        let fs: std::sync::Arc<dyn PrivilegedFs> =
            std::sync::Arc::new(FakePrivilegedFs::new(Access::ReadOnly, ""));
        assert_object_safe(&*fs);
    }

    #[test]
    fn a_correct_secret_reads_the_content() {
        let fs = FakePrivilegedFs::new(Access::Denied, "PermitRootLogin no\n");
        let got = block_on(fs.read(
            Path::new("/etc/ssh/sshd_config"),
            &Passphrase::new("correct".into()),
        ))
        .unwrap();
        assert_eq!(String::from_utf8(got).unwrap(), "PermitRootLogin no\n");
    }

    #[test]
    fn a_wrong_secret_fails_authentication_and_reads_nothing() {
        let fs = FakePrivilegedFs::new(Access::Denied, "secret\n");
        let err = block_on(fs.read(Path::new("/etc/shadow"), &Passphrase::new("wrong".into())))
            .unwrap_err();
        assert!(matches!(err, PrivError::AuthFailed), "got {err:?}");
    }

    #[test]
    fn a_write_is_observable_by_a_later_read() {
        let fs = FakePrivilegedFs::new(Access::ReadOnly, "old\n");
        let secret = Passphrase::new("correct".into());
        let path = Path::new("/etc/fstab");
        block_on(fs.write(path, b"new\n", &secret)).unwrap();
        let got = block_on(fs.read(path, &secret)).unwrap();
        assert_eq!(got, b"new\n");
    }

    #[test]
    fn the_path_guard_applies_to_every_privileged_operation() {
        let fs = FakePrivilegedFs::new(Access::ReadOnly, "x");
        let secret = Passphrase::new("correct".into());
        let relative = Path::new("fstab");
        assert!(matches!(
            block_on(fs.read(relative, &secret)).unwrap_err(),
            PrivError::UnsafePath(_)
        ));
        assert!(matches!(
            block_on(fs.write(relative, b"y", &secret)).unwrap_err(),
            PrivError::UnsafePath(_)
        ));
    }

    #[test]
    fn the_defaulted_read_passes_the_ports_own_cap_down() {
        // The seam that keeps `config_editor.rs` compiling unchanged: the old two-argument
        // `read` still exists, and it is not uncapped — it is `read_capped` at
        // DEFAULT_READ_LIMIT.
        let fs = FakePrivilegedFs::new(Access::Denied, "x");
        block_on(fs.read(Path::new("/etc/fstab"), &Passphrase::new("correct".into()))).unwrap();
        assert_eq!(*fs.last_cap.lock().unwrap(), Some(DEFAULT_READ_LIMIT));
    }

    #[test]
    fn a_caller_with_an_opinion_gets_the_cap_it_asked_for() {
        let fs = FakePrivilegedFs::new(Access::Denied, "0123456789");
        let secret = Passphrase::new("correct".into());
        let err = block_on(fs.read_capped(Path::new("/etc/fstab"), 4, &secret)).unwrap_err();
        assert_eq!(
            err,
            PrivError::TooLarge {
                bytes: 10,
                max_bytes: 4
            }
        );
        // And the same file under a cap that fits is an ordinary read.
        assert_eq!(
            block_on(fs.read_capped(Path::new("/etc/fstab"), 10, &secret)).unwrap(),
            b"0123456789"
        );
    }

    #[test]
    fn an_oversized_file_is_never_a_reason_to_re_prompt() {
        assert!(
            !PrivError::TooLarge {
                bytes: 2,
                max_bytes: 1
            }
            .is_retryable()
        );
    }

    #[test]
    fn probing_needs_no_secret() {
        // Expressed as a signature fact: `probe` takes no `Passphrase`, so a caller
        // cannot be made to hold one before it knows whether elevation is even needed.
        let fs = FakePrivilegedFs::new(Access::ReadOnly, "x");
        assert_eq!(
            block_on(fs.probe(Path::new("/etc/fstab"))),
            Access::ReadOnly
        );
    }
}
