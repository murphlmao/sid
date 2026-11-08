//! Edit-in-place for remote files: download over SFTP to a private temp dir, open it in
//! the user's editor, and push saves back - refusing to clobber a remote that changed
//! underneath.
//!
//! The flow is driven by the caller (no background task is owned here):
//!
//! 1. [`RemoteEdit::open`] - stat + size/binary checks + download.
//! 2. [`RemoteEdit::launch`] - hand the local copy to an [`EditorLauncher`].
//! 3. Repeatedly (e.g. every second) [`RemoteEdit::sync`] - cheap mtime check, content
//!    hash only when the mtime moved; uploads when the content really changed.
//!    [`RemoteEditError::Conflict`] means the remote file differs from what was
//!    downloaded; the UI may then call [`RemoteEdit::force_upload`] or discard.
//! 4. When [`RemoteEdit::editor_state`] reports exit (or the user presses "done" for a
//!    non-awaitable `xdg-open` launch): one last `sync`, then [`RemoteEdit::close`].
//!
//! The temp dir lives under the user cache dir (`$XDG_CACHE_HOME/sid/remote-edit`, not
//! the RAM-backed `/tmp`), is mode 0700, the file 0600.
//!
//! Never lose edits: every exit goes through [`RemoteEdit::close_or_preserve`]. If the
//! working copy differs from the last successful upload it is COPIED into a durable
//! backups directory ([`RemoteEditConfig::backups_root`], in the app data dir, not the
//! cache) and only then is the temp dir removed; if the backup cannot be written the temp
//! dir is kept and its path reported. An unchanged copy is simply removed.
//!
//! Upload strategy: [`UploadStrategy::Overwrite`] (default) rewrites the remote in place,
//! preserving mode, owner and links, but a disconnect mid-write truncates it.
//! [`UploadStrategy::TmpRename`] (opt-in) writes an exclusively-created, randomly named
//! `.<name>.sid-<hex>` created with the target's permission bits (setuid/setgid/sticky
//! dropped) in the OPEN request, and renames over the target, so a disconnect mid-upload
//! cannot truncate it. It is taken only when the target (lstat) is a regular, non-symlink
//! file whose uid/gid equal the temp file's; otherwise, or when exclusive create /
//! rename-over is unsupported (plain SFTP rename usually refuses to replace, and
//! `russh-sftp` does not expose `posix-rename@openssh.com`), it rewrites in place and
//! reports `Uploaded { atomic: false }`. It cannot see hard links (SFTP v3 has no link
//! count), which a rename would detach - hence opt-in.
//!
//! Conflict detection compares a SHA-256 of the remote's current bytes with the hash of
//! what was downloaded / last uploaded, so a same-size, same-second edit is still caught.

use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sid_core::editor::{EditorError, EditorExit, EditorHandle, EditorLauncher, LaunchPolicy};
use sid_core::ssh::{SftpSession, SshError};

/// Default refusal threshold: 5 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// How an upload replaces the remote file.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UploadStrategy {
    /// Rewrite in place (keeps mode/owner/links, but a disconnect mid-write truncates).
    #[default]
    Overwrite,
    /// Write `.<name>.sid-<hex>` with the target's mode, rename over the target; fall
    /// back to overwrite whenever the target is not a plain same-owner regular file or the
    /// server can't. Opt-in: hard links cannot be detected over SFTP v3.
    TmpRename,
}

/// Settings for one edit session.
#[derive(Clone, Debug)]
pub struct RemoteEditConfig {
    /// Parent of the per-edit private directory.
    pub cache_root: PathBuf,
    /// Durable home of preserved local edits (`<root>/<host>/<name>.local-<ts>`).
    pub backups_root: PathBuf,
    /// Refuse files larger than this.
    pub max_bytes: u64,
    /// Upload strategy.
    pub strategy: UploadStrategy,
}

impl RemoteEditConfig {
    /// Defaults rooted at [`default_cache_root`].
    pub fn new() -> Self {
        Self::with_root(default_cache_root())
    }
    /// Defaults with an explicit cache root.
    pub fn with_root(cache_root: PathBuf) -> Self {
        Self {
            cache_root,
            backups_root: default_backups_root(),
            max_bytes: DEFAULT_MAX_BYTES,
            strategy: UploadStrategy::default(),
        }
    }
}

impl Default for RemoteEditConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// `$XDG_CACHE_HOME/sid/remote-edit`, else `~/.cache/sid/remote-edit`.
pub fn default_cache_root() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("sid").join("remote-edit")
}

/// `$XDG_DATA_HOME/sid/remote-edit-backups`, else `~/.local/share/sid/remote-edit-backups`.
/// The app overrides this with its own data dir.
pub fn default_backups_root() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("sid").join("remote-edit-backups")
}

/// The remote file's identity as seen by SFTP `stat`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteSig {
    /// Modification time, seconds.
    pub mtime_secs: i64,
    /// Size in bytes.
    pub size: u64,
}

/// Why the remote no longer matches the downloaded baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictKind {
    /// mtime/size differ from the download.
    RemoteChanged {
        /// What it is now.
        now: RemoteSig,
    },
    /// The remote file disappeared.
    RemoteDeleted,
}

/// Remote-edit failures.
#[derive(Debug)]
pub enum RemoteEditError {
    /// Larger than the configured cap.
    TooLarge {
        /// Actual size.
        size: u64,
        /// The cap.
        max: u64,
    },
    /// Looks binary (NUL byte in the first 8 KiB).
    Binary,
    /// The path is a directory or otherwise not a regular file.
    NotAFile(String),
    /// The remote changed since download; nothing was uploaded.
    Conflict(ConflictKind),
    /// SFTP failure.
    Ssh(SshError),
    /// Local filesystem failure.
    Io(io::Error),
    /// Editor launch failure.
    Editor(EditorError),
}

impl std::fmt::Display for RemoteEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { size, max } => {
                write!(f, "file is {size} bytes; the edit limit is {max}")
            }
            Self::Binary => write!(f, "file looks binary; refusing to edit as text"),
            Self::NotAFile(p) => write!(f, "{p} is not a regular file"),
            Self::Conflict(ConflictKind::RemoteChanged { .. }) => {
                write!(f, "the remote file changed since it was opened")
            }
            Self::Conflict(ConflictKind::RemoteDeleted) => {
                write!(f, "the remote file was deleted since it was opened")
            }
            Self::Ssh(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "local file error: {e}"),
            Self::Editor(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RemoteEditError {}
impl From<SshError> for RemoteEditError {
    fn from(e: SshError) -> Self {
        Self::Ssh(e)
    }
}
impl From<io::Error> for RemoteEditError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<EditorError> for RemoteEditError {
    fn from(e: EditorError) -> Self {
        Self::Editor(e)
    }
}

/// Result of [`RemoteEdit::sync`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncOutcome {
    /// Local content equals the last synced content.
    Unchanged,
    /// Uploaded. `atomic` is true when the tmp+rename path succeeded.
    Uploaded {
        /// Whether the replace was a rename.
        atomic: bool,
    },
}

/// The editor's state as far as it can be known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorState {
    /// [`RemoteEdit::launch`] has not been called.
    NotLaunched,
    /// Still running.
    Running,
    /// Exited.
    Exited(EditorExit),
    /// Hand-off launcher (`xdg-open`): exit is unknowable; end the session explicitly.
    HandOff,
}

type Digest32 = [u8; 32];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LocalSig {
    mtime: Option<SystemTime>,
    len: u64,
    hash: Digest32,
}

fn hash_bytes(b: &[u8]) -> Digest32 {
    Sha256::digest(b).into()
}

/// What [`RemoteEdit::close_or_preserve`] did with the working copy.
#[derive(Debug)]
pub enum Preserved {
    /// Unchanged since the last upload; the temp dir was removed.
    Removed,
    /// Changed and not uploaded; copied here, then the temp dir was removed.
    BackedUp(PathBuf),
    /// Changed, and the backup could not be written: nothing was deleted.
    Failed {
        /// Why the backup failed.
        error: io::Error,
        /// The working copy, left in place.
        kept: PathBuf,
    },
}

/// `<name>.local-<YYYYmmdd-HHMMSS>` (UTC).
fn backup_file_name(name: &str, epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let rem = epoch_secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    format!(
        "{name}.local-{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// A single path component made of `[A-Za-z0-9._-]` only (for host labels in paths).
fn safe_component(s: &str) -> String {
    let c: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let c = c.trim_start_matches(['.', '-']).to_string();
    if c.is_empty() { "host".to_string() } else { c }
}

fn looks_binary(b: &[u8]) -> bool {
    b[..b.len().min(8192)].contains(&0)
}

/// Extensions whose desktop handler would *run* or *render* the file rather than edit it.
const DANGEROUS_EXTS: &[&str] = &[
    "desktop",
    "html",
    "htm",
    "xhtml",
    "svg",
    "sh",
    "bash",
    "zsh",
    "py",
    "pl",
    "rb",
    "js",
    "jar",
    "exe",
    "appimage",
    "run",
    "command",
    "url",
    "webloc",
    "lnk",
    "bat",
    "cmd",
    "ps1",
    "code-workspace",
    "sublime-project",
    "sublime-workspace",
];

const MAX_NAME_CHARS: usize = 100;

/// Bidi overrides/isolates and zero-width/format characters: invisible, so stripped.
fn is_invisible_format(c: char) -> bool {
    matches!(c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{FEFF}')
}

/// The local working-copy name for a remote path. The remote party chooses it, and it ends
/// up in an absolute path handed to an editor (possibly via a terminal emulator's `-e`,
/// which some terminals join into one shell string), so it keeps only `[A-Za-z0-9._-]`:
/// control and bidi characters are stripped, everything else becomes `_`, it never starts
/// with `-` or `.`, is length-capped keeping the extension, and gets `.txt` appended when
/// the extension is one whose handler would execute or render it.
fn safe_local_name(remote_path: &str) -> String {
    let base = remote_path.rsplit('/').next().unwrap_or("");
    let mapped: String = base
        .chars()
        .filter(|c| !c.is_control() && !is_invisible_format(*c))
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let lead = |s: &str| s.trim_start_matches(['.', '-']).to_string();
    let mut name = lead(&mapped);
    let n = name.chars().count();
    if n > MAX_NAME_CHARS {
        // keep the tail so the extension survives
        name = lead(&name.chars().skip(n - MAX_NAME_CHARS).collect::<String>());
    }
    if name.is_empty() {
        return "file.txt".to_string();
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if DANGEROUS_EXTS.contains(&ext.as_str()) {
        format!("{name}.txt")
    } else {
        name
    }
}

/// One remote file being edited locally.
pub struct RemoteEdit {
    remote_path: String,
    dir: tempfile::TempDir,
    local: PathBuf,
    strategy: UploadStrategy,
    max_bytes: u64,
    baseline: RemoteSig,
    /// SHA-256 of the remote bytes as of download / last upload.
    baseline_hash: Digest32,
    backups_root: PathBuf,
    synced: LocalSig,
    handle: Option<Box<dyn EditorHandle>>,
    #[cfg(test)]
    tmp_override: Option<String>,
}

/// POSIX file-type check on an SFTP `permissions` value. Servers that send no type bits
/// (0) are given the benefit of the doubt; anything that says it is not a regular file
/// (FIFO, device, socket, ...) is refused.
fn is_regular(mode: u32) -> bool {
    const S_IFMT: u32 = 0o170000;
    const S_IFREG: u32 = 0o100000;
    mode & S_IFMT == 0 || mode & S_IFMT == S_IFREG
}

/// `<dir>/.<name>.sid-<16 hex>`: same directory (so rename stays on one filesystem),
/// hidden, and unguessable by another user who might plant something there.
fn random_tmp_name(remote_path: &str) -> io::Result<String> {
    let mut r = [0u8; 8];
    getrandom::fill(&mut r).map_err(|e| io::Error::other(e.to_string()))?;
    let hex: String = r.iter().map(|b| format!("{b:02x}")).collect();
    let (dir, name) = match remote_path.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n),
        None => (String::new(), remote_path),
    };
    Ok(format!("{dir}.{name}.sid-{hex}"))
}

fn private_dir_all(p: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(p)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(p)
    }
}

fn write_private(p: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)?.write_all(bytes)
}

/// Copy `src` to a fresh file under `dir` (created 0700), named `file_name`, or
/// `file_name-1`, `-2`, ... if taken. Never overwrites. Fsynced before returning.
fn write_backup(src: &Path, dir: &Path, file_name: &str) -> io::Result<PathBuf> {
    private_dir_all(dir)?;
    let bytes = std::fs::read(src)?;
    for i in 0..1000 {
        let name = if i == 0 {
            file_name.to_string()
        } else {
            format!("{file_name}-{i}")
        };
        let dest = dir.join(name);
        match write_private(&dest, &bytes) {
            Ok(()) => {
                std::fs::File::open(&dest)?.sync_all()?;
                return Ok(dest);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("too many backups with the same name"))
}

impl RemoteEdit {
    /// Stat, vet and download `remote_path` into a fresh private directory.
    pub async fn open(
        sftp: &mut dyn SftpSession,
        remote_path: &str,
        cfg: &RemoteEditConfig,
    ) -> Result<Self, RemoteEditError> {
        // Stat BEFORE reading: a change during the read then shows up as a conflict
        // (safe) rather than being silently adopted as the baseline.
        let st = sftp
            .stat(remote_path)
            .await?
            .ok_or_else(|| SshError::PathNotFound(remote_path.to_string()))?;
        if st.is_dir || !is_regular(st.mode) {
            return Err(RemoteEditError::NotAFile(remote_path.to_string()));
        }
        if st.size > cfg.max_bytes {
            return Err(RemoteEditError::TooLarge {
                size: st.size,
                max: cfg.max_bytes,
            });
        }
        // The size above is the server's claim; the read itself is what is bounded.
        let bytes = match sftp.get_bounded(remote_path, cfg.max_bytes).await {
            Ok(b) => b,
            Err(SshError::TooLarge(max)) => {
                return Err(RemoteEditError::TooLarge {
                    size: max.saturating_add(1),
                    max,
                });
            }
            Err(e) => return Err(e.into()),
        };
        if looks_binary(&bytes) {
            return Err(RemoteEditError::Binary);
        }

        let cache_root = std::path::absolute(&cfg.cache_root)?;
        private_dir_all(&cache_root)?;
        let dir = tempfile::Builder::new()
            .prefix("edit-")
            .tempdir_in(&cache_root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let name = safe_local_name(remote_path);
        let local = dir.path().join(&name);
        write_private(&local, &bytes)?;
        let mtime = std::fs::metadata(&local).and_then(|m| m.modified()).ok();
        Ok(Self {
            remote_path: remote_path.to_string(),
            dir,
            local,
            strategy: cfg.strategy,
            max_bytes: cfg.max_bytes,
            baseline: RemoteSig {
                mtime_secs: st.mtime_secs,
                size: st.size,
            },
            baseline_hash: hash_bytes(&bytes),
            backups_root: cfg.backups_root.clone(),
            synced: LocalSig {
                mtime,
                len: bytes.len() as u64,
                hash: hash_bytes(&bytes),
            },
            handle: None,
            #[cfg(test)]
            tmp_override: None,
        })
    }

    /// Path of the local working copy.
    pub fn local_path(&self) -> &Path {
        &self.local
    }

    /// The remote path being edited.
    pub fn remote_path(&self) -> &str {
        &self.remote_path
    }

    /// Open the working copy in the user's editor.
    pub fn launch(&mut self, launcher: &dyn EditorLauncher) -> Result<(), RemoteEditError> {
        self.handle = Some(launcher.open(&self.local, LaunchPolicy::EditorOnly)?);
        Ok(())
    }

    /// Non-blocking editor status.
    pub fn editor_state(&mut self) -> Result<EditorState, RemoteEditError> {
        let Some(h) = self.handle.as_mut() else {
            return Ok(EditorState::NotLaunched);
        };
        if !h.awaitable() {
            return Ok(EditorState::HandOff);
        }
        Ok(match h.try_wait()? {
            Some(x) => EditorState::Exited(x),
            None => EditorState::Running,
        })
    }

    /// Has the working copy's content changed since the last sync? (mtime+len fast path,
    /// content hash to confirm.)
    pub fn local_changed(&mut self) -> Result<bool, RemoteEditError> {
        let md = std::fs::metadata(&self.local)?;
        let mtime = md.modified().ok();
        if mtime == self.synced.mtime && md.len() == self.synced.len {
            return Ok(false);
        }
        let bytes = std::fs::read(&self.local)?;
        if hash_bytes(&bytes) == self.synced.hash && bytes.len() as u64 == self.synced.len {
            self.synced.mtime = mtime; // touched, not edited
            return Ok(false);
        }
        Ok(true)
    }

    /// Upload the working copy if it changed. Refuses with
    /// [`RemoteEditError::Conflict`] if the remote moved since download/last upload.
    pub async fn sync(
        &mut self,
        sftp: &mut dyn SftpSession,
    ) -> Result<SyncOutcome, RemoteEditError> {
        if !self.local_changed()? {
            return Ok(SyncOutcome::Unchanged);
        }
        self.upload(sftp, false).await
    }

    /// Upload the working copy even though the remote changed (user chose "overwrite").
    /// Uploads regardless of whether the local content changed.
    pub async fn force_upload(
        &mut self,
        sftp: &mut dyn SftpSession,
    ) -> Result<SyncOutcome, RemoteEditError> {
        self.upload(sftp, true).await
    }

    async fn upload(
        &mut self,
        sftp: &mut dyn SftpSession,
        force: bool,
    ) -> Result<SyncOutcome, RemoteEditError> {
        if !force {
            match sftp.stat(&self.remote_path).await? {
                None => return Err(RemoteEditError::Conflict(ConflictKind::RemoteDeleted)),
                Some(e) => {
                    let now = RemoteSig {
                        mtime_secs: e.mtime_secs,
                        size: e.size,
                    };
                    // mtime+size are only a hint (same-size edits within one second slip
                    // through); the content hash decides.
                    let same = match sftp.get_bounded(&self.remote_path, self.max_bytes).await {
                        Ok(b) => hash_bytes(&b) == self.baseline_hash,
                        Err(SshError::TooLarge(_)) => false,
                        Err(SshError::PathNotFound(_)) => {
                            return Err(RemoteEditError::Conflict(ConflictKind::RemoteDeleted));
                        }
                        Err(e) => return Err(e.into()),
                    };
                    if !same {
                        return Err(RemoteEditError::Conflict(ConflictKind::RemoteChanged {
                            now,
                        }));
                    }
                }
            }
        }
        let mtime = std::fs::metadata(&self.local)
            .and_then(|m| m.modified())
            .ok();
        let bytes = std::fs::read(&self.local)?;
        if bytes.len() as u64 > self.max_bytes {
            return Err(RemoteEditError::TooLarge {
                size: bytes.len() as u64,
                max: self.max_bytes,
            });
        }
        let mut atomic = false;
        match self.strategy {
            UploadStrategy::Overwrite => sftp.put(&self.remote_path, &bytes).await?,
            UploadStrategy::TmpRename => {
                atomic = self.upload_tmp_rename(sftp, &bytes).await?;
            }
        }
        if let Some(e) = sftp.stat(&self.remote_path).await? {
            self.baseline = RemoteSig {
                mtime_secs: e.mtime_secs,
                size: e.size,
            };
        }
        self.baseline_hash = hash_bytes(&bytes);
        self.synced = LocalSig {
            mtime,
            len: bytes.len() as u64,
            hash: hash_bytes(&bytes),
        };
        Ok(SyncOutcome::Uploaded { atomic })
    }

    /// Write a temp file next to the target and rename it over. Returns whether the
    /// rename happened; on any doubt it rewrites in place instead (`Ok(false)`).
    ///
    /// Rename-over is only taken for a target that is a plain regular file (lstat: not a
    /// symlink - a rename would replace the link itself) and whose owner/group equal those
    /// of the freshly created temp file (a rename would otherwise change ownership). The
    /// temp file is created with the target's permission bits (minus setuid/setgid/sticky)
    /// in the OPEN request; nothing is chmod'ed by path afterwards. SFTP v3 does not
    /// expose a link count, so a hard-linked target (which a rename would detach from its
    /// other names) cannot be detected - the reason this strategy is opt-in.
    async fn upload_tmp_rename(
        &mut self,
        sftp: &mut dyn SftpSession,
        bytes: &[u8],
    ) -> Result<bool, RemoteEditError> {
        let target = match sftp.lstat_identity(&self.remote_path).await {
            Ok(Some(t)) if t.is_regular && !t.is_symlink && t.uid.is_some() && t.gid.is_some() => t,
            _ => {
                sftp.put(&self.remote_path, bytes).await?;
                return Ok(false);
            }
        };
        let tmp = random_tmp_name(&self.remote_path)?;
        #[cfg(test)]
        let tmp = self.tmp_override.clone().unwrap_or(tmp);
        // Exclusive create: if anything already exists at `tmp` (or exclusive create is
        // unsupported) we do NOT write through it and do NOT remove it; we overwrite the
        // real target in place instead.
        if sftp
            .put_exclusive(&tmp, bytes, target.mode & 0o777)
            .await
            .is_err()
        {
            sftp.put(&self.remote_path, bytes).await?;
            return Ok(false);
        }
        // From here the tmp file is ours, so it is ours to clean up.
        let same_owner = matches!(
            sftp.lstat_identity(&tmp).await,
            Ok(Some(t)) if t.is_regular && !t.is_symlink
                && t.uid == target.uid && t.gid == target.gid
        );
        if same_owner && sftp.rename(&tmp, &self.remote_path).await.is_ok() {
            return Ok(true);
        }
        let res = sftp.put(&self.remote_path, bytes).await;
        let _ = sftp.remove_file(&tmp).await;
        res?;
        Ok(false)
    }

    /// Remove the private directory and its contents, UNCONDITIONALLY. Only for a copy
    /// that is known to be disposable; every user-facing exit uses
    /// [`Self::close_or_preserve`].
    pub fn close(self) -> io::Result<()> {
        self.dir.close()
    }

    /// Has the working copy changed since the last successful upload (or download)? A
    /// working copy that can't be read counts as changed; one that is gone does not.
    pub fn has_unsaved_changes(&mut self) -> bool {
        match self.local_changed() {
            Ok(c) => c,
            Err(RemoteEditError::Io(e)) if e.kind() == io::ErrorKind::NotFound => false,
            Err(_) => true,
        }
    }

    /// The one exit path (disconnect, stop, keep-server-copy, session close). Unchanged
    /// since the last upload: remove the temp dir. Changed: copy the working copy to
    /// `<backups_root>/<host>/<name>.local-<timestamp>` (dirs 0700, file 0600, never
    /// overwriting) and only then remove the temp dir. If the backup fails, NOTHING is
    /// deleted ([`Preserved::Failed`]).
    pub fn close_or_preserve(mut self, host: &str, epoch_secs: i64) -> Preserved {
        if !self.has_unsaved_changes() {
            let _ = self.dir.close();
            return Preserved::Removed;
        }
        let name = self
            .local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file.txt".into());
        let dir = self.backups_root.join(safe_component(host));
        match write_backup(&self.local, &dir, &backup_file_name(&name, epoch_secs)) {
            Ok(dest) => {
                let _ = self.dir.close();
                Preserved::BackedUp(dest)
            }
            Err(error) => {
                let kept = self.local.clone();
                let _ = self.dir.keep();
                Preserved::Failed { error, kept }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sid_core::editor::FakeEditorLauncher;
    use sid_core::ssh::{SftpEntry, SftpIdentity};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct State {
        files: HashMap<String, (Vec<u8>, i64)>,
        clock: i64,
        rename_ok: bool,
        exclusive_ok: bool,
        lie_size: Option<u64>,
        mode: u32,
        puts: Vec<String>,
        renames: usize,
        ident: Option<SftpIdentity>,
        tmp_uid: u32,
        put_modes: Vec<(String, u32)>,
    }

    #[derive(Clone, Default)]
    struct FakeSftp(Arc<Mutex<State>>);

    impl FakeSftp {
        fn with(path: &str, body: &[u8]) -> Self {
            let f = Self::default();
            {
                let mut s = f.0.lock().unwrap();
                s.clock = 1000;
                s.files.insert(path.into(), (body.to_vec(), 1000));
            }
            f
        }
        fn remote_edit_behind_our_back(&self, path: &str, body: &[u8]) {
            let mut s = self.0.lock().unwrap();
            s.clock += 10;
            let c = s.clock;
            s.files.insert(path.into(), (body.to_vec(), c));
        }
        fn body(&self, path: &str) -> Option<Vec<u8>> {
            self.0.lock().unwrap().files.get(path).map(|f| f.0.clone())
        }
    }

    #[async_trait]
    impl SftpSession for FakeSftp {
        async fn list(&mut self, _: &str) -> Result<Vec<SftpEntry>, SshError> {
            Ok(vec![])
        }
        async fn canonicalize(&mut self, p: &str) -> Result<String, SshError> {
            Ok(p.into())
        }
        async fn get(&mut self, p: &str) -> Result<Vec<u8>, SshError> {
            self.0
                .lock()
                .unwrap()
                .files
                .get(p)
                .map(|f| f.0.clone())
                .ok_or_else(|| SshError::PathNotFound(p.into()))
        }
        async fn get_bounded(&mut self, p: &str, max: u64) -> Result<Vec<u8>, SshError> {
            let s = self.0.lock().unwrap();
            let (b, _) = s
                .files
                .get(p)
                .ok_or_else(|| SshError::PathNotFound(p.into()))?;
            // streams until the cap is exceeded, whatever stat claimed
            if b.len() as u64 > max {
                return Err(SshError::TooLarge(max));
            }
            Ok(b.clone())
        }
        async fn put_exclusive(&mut self, p: &str, b: &[u8], mode: u32) -> Result<(), SshError> {
            if !self.0.lock().unwrap().exclusive_ok || self.0.lock().unwrap().files.contains_key(p)
            {
                return Err(SshError::Other("exists or unsupported".into()));
            }
            self.0.lock().unwrap().put_modes.push((p.into(), mode));
            self.put(p, b).await
        }
        async fn put(&mut self, p: &str, b: &[u8]) -> Result<(), SshError> {
            let mut s = self.0.lock().unwrap();
            s.clock += 10;
            let c = s.clock;
            s.puts.push(p.into());
            s.files.insert(p.into(), (b.to_vec(), c));
            Ok(())
        }
        async fn remove_file(&mut self, p: &str) -> Result<(), SshError> {
            self.0.lock().unwrap().files.remove(p);
            Ok(())
        }
        async fn mkdir(&mut self, _: &str) -> Result<(), SshError> {
            Ok(())
        }
        async fn stat(&mut self, p: &str) -> Result<Option<SftpEntry>, SshError> {
            let s = self.0.lock().unwrap();
            let lie = s.lie_size;
            let mode = if s.mode == 0 { 0o644 } else { s.mode };
            Ok(s.files.get(p).map(|(b, m)| SftpEntry {
                name: p.into(),
                is_dir: false,
                size: lie.unwrap_or(b.len() as u64),
                mtime_secs: *m,
                mode,
            }))
        }
        async fn rename(&mut self, from: &str, to: &str) -> Result<(), SshError> {
            let mut s = self.0.lock().unwrap();
            if !s.rename_ok {
                return Err(SshError::Other("no rename".into()));
            }
            s.renames += 1;
            let f = s
                .files
                .remove(from)
                .ok_or_else(|| SshError::PathNotFound(from.into()))?;
            s.files.insert(to.into(), f);
            Ok(())
        }
        async fn lstat_identity(&mut self, p: &str) -> Result<Option<SftpIdentity>, SshError> {
            let s = self.0.lock().unwrap();
            if !s.files.contains_key(p) {
                return Ok(None);
            }
            let base = s.ident.unwrap_or(SftpIdentity {
                is_symlink: false,
                is_regular: true,
                uid: Some(1000),
                gid: Some(1000),
                mode: 0o100644,
            });
            // a temp file is "owned" by tmp_uid (the ssh user); the target by `ident`
            Ok(Some(if p.contains(".sid-") {
                SftpIdentity {
                    uid: Some(s.tmp_uid),
                    is_symlink: false,
                    is_regular: true,
                    ..base
                }
            } else {
                base
            }))
        }
        async fn close(&mut self) -> Result<(), SshError> {
            Ok(())
        }
    }

    const P: &str = "/etc/app/conf.toml";

    fn cfg(root: &Path) -> RemoteEditConfig {
        RemoteEditConfig::with_root(root.join("cache"))
    }

    fn edit_local(re: &RemoteEdit, body: &[u8]) {
        std::fs::write(re.local_path(), body).unwrap();
    }

    #[tokio::test]
    async fn download_edit_save_uploads_and_private_perms_hold() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"a = 1\n");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg(root.path()))
            .await
            .unwrap();
        assert!(re.local_path().starts_with(root.path().join("cache")));
        assert_eq!(re.local_path().file_name().unwrap(), "conf.toml");
        assert_eq!(std::fs::read(re.local_path()).unwrap(), b"a = 1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dm = std::fs::metadata(re.local_path().parent().unwrap()).unwrap();
            assert_eq!(dm.permissions().mode() & 0o777, 0o700);
            let fm = std::fs::metadata(re.local_path()).unwrap();
            assert_eq!(fm.permissions().mode() & 0o777, 0o600);
        }
        let ed = FakeEditorLauncher::new();
        re.launch(&ed).unwrap();
        assert_eq!(ed.opened(), [re.local_path().to_path_buf()]);
        assert_eq!(re.editor_state().unwrap(), EditorState::Running);

        assert_eq!(re.sync(&mut sftp).await.unwrap(), SyncOutcome::Unchanged);
        edit_local(&re, b"a = 22222\n");
        assert_eq!(
            re.sync(&mut sftp).await.unwrap(),
            SyncOutcome::Uploaded { atomic: false }
        );
        assert_eq!(sftp.body(P).unwrap(), b"a = 22222\n");
        // baseline refreshed: an immediate second sync is a no-op, no conflict
        assert_eq!(re.sync(&mut sftp).await.unwrap(), SyncOutcome::Unchanged);
        // and a further edit still uploads cleanly
        edit_local(&re, b"a = 3\n");
        assert!(matches!(
            re.sync(&mut sftp).await,
            Ok(SyncOutcome::Uploaded { .. })
        ));

        ed.finish_all();
        assert!(matches!(re.editor_state().unwrap(), EditorState::Exited(_)));
    }

    #[tokio::test]
    async fn touching_without_changing_content_does_not_upload() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"same\n");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg(root.path()))
            .await
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        edit_local(&re, b"same\n"); // new mtime, same bytes
        assert_eq!(re.sync(&mut sftp).await.unwrap(), SyncOutcome::Unchanged);
        assert!(sftp.0.lock().unwrap().puts.is_empty());
    }

    #[tokio::test]
    async fn remote_change_underneath_is_a_conflict_and_nothing_is_uploaded() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"v1\n");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg(root.path()))
            .await
            .unwrap();
        sftp.remote_edit_behind_our_back(P, b"someone else\n");
        edit_local(&re, b"my edit!\n");
        let err = re.sync(&mut sftp).await.unwrap_err();
        assert!(matches!(
            err,
            RemoteEditError::Conflict(ConflictKind::RemoteChanged { .. })
        ));
        assert_eq!(sftp.body(P).unwrap(), b"someone else\n", "remote untouched");
        // the user chooses to overwrite
        re.force_upload(&mut sftp).await.unwrap();
        assert_eq!(sftp.body(P).unwrap(), b"my edit!\n");
        // and the session is healthy again
        edit_local(&re, b"again\n!");
        assert!(re.sync(&mut sftp).await.is_ok());
    }

    #[tokio::test]
    async fn remote_deleted_is_a_conflict() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"v1\n");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg(root.path()))
            .await
            .unwrap();
        sftp.0.lock().unwrap().files.clear();
        edit_local(&re, b"edited\n");
        assert!(matches!(
            re.sync(&mut sftp).await,
            Err(RemoteEditError::Conflict(ConflictKind::RemoteDeleted))
        ));
    }

    #[tokio::test]
    async fn too_large_and_binary_are_refused_before_anything_is_written() {
        let root = tempfile::tempdir().unwrap();
        let mut c = cfg(root.path());
        c.max_bytes = 10;
        let mut big = FakeSftp::with(P, &[b'x'; 11]);
        assert!(matches!(
            RemoteEdit::open(&mut big, P, &c).await,
            Err(RemoteEditError::TooLarge { size: 11, max: 10 })
        ));
        let mut bin = FakeSftp::with(P, b"ab\0cd");
        assert!(matches!(
            RemoteEdit::open(&mut bin, P, &cfg(root.path())).await,
            Err(RemoteEditError::Binary)
        ));
        assert!(
            !root.path().join("cache").exists() || {
                std::fs::read_dir(root.path().join("cache"))
                    .unwrap()
                    .count()
                    == 0
            }
        );
    }

    #[tokio::test]
    async fn missing_remote_is_path_not_found() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::default();
        assert!(matches!(
            RemoteEdit::open(&mut sftp, P, &cfg(root.path())).await,
            Err(RemoteEditError::Ssh(SshError::PathNotFound(_)))
        ));
    }

    #[tokio::test]
    async fn tmp_rename_strategy_renames_when_supported() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"v1\n");
        sftp.0.lock().unwrap().rename_ok = true;
        sftp.0.lock().unwrap().exclusive_ok = true;
        sftp.0.lock().unwrap().tmp_uid = 1000;
        let mut c = cfg(root.path());
        c.strategy = UploadStrategy::TmpRename;
        let mut re = RemoteEdit::open(&mut sftp, P, &c).await.unwrap();
        edit_local(&re, b"v2 longer\n");
        assert_eq!(
            re.sync(&mut sftp).await.unwrap(),
            SyncOutcome::Uploaded { atomic: true }
        );
        assert_eq!(sftp.body(P).unwrap(), b"v2 longer\n");
        assert_eq!(sftp.0.lock().unwrap().files.len(), 1, "no stray tmp left");
        assert_eq!(sftp.0.lock().unwrap().renames, 1);
        let tmp = sftp.0.lock().unwrap().puts.last().cloned().unwrap();
        assert!(tmp.starts_with("/etc/app/.conf.toml.sid-"), "{tmp}");
        assert_eq!(tmp.len(), "/etc/app/.conf.toml.sid-".len() + 16);
    }

    #[tokio::test]
    async fn tmp_rename_falls_back_to_overwrite_and_cleans_the_tmp() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"v1\n");
        let mut c = cfg(root.path());
        c.strategy = UploadStrategy::TmpRename;
        let mut re = RemoteEdit::open(&mut sftp, P, &c).await.unwrap();
        edit_local(&re, b"v2 longer\n");
        assert_eq!(
            re.sync(&mut sftp).await.unwrap(),
            SyncOutcome::Uploaded { atomic: false }
        );
        assert_eq!(sftp.body(P).unwrap(), b"v2 longer\n");
        assert!(
            sftp.body(&format!("{P}.sid-tmp")).is_none(),
            "tmp cleaned up"
        );
    }

    #[tokio::test]
    async fn editor_state_starts_not_launched() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"x");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg(root.path()))
            .await
            .unwrap();
        assert_eq!(re.editor_state().unwrap(), EditorState::NotLaunched);
    }

    #[tokio::test]
    async fn close_removes_the_private_directory() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"x");
        let re = RemoteEdit::open(&mut sftp, P, &cfg(root.path()))
            .await
            .unwrap();
        let dir = re.local_path().parent().unwrap().to_path_buf();
        assert!(dir.exists());
        re.close().unwrap();
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn a_server_that_lies_about_size_cannot_defeat_the_cap() {
        let root = tempfile::tempdir().unwrap();
        let mut c = cfg(root.path());
        c.max_bytes = 10;
        let mut sftp = FakeSftp::with(P, &[b'x'; 5000]);
        sftp.0.lock().unwrap().lie_size = Some(3); // stat says 3 bytes
        assert!(matches!(
            RemoteEdit::open(&mut sftp, P, &c).await,
            Err(RemoteEditError::TooLarge { max: 10, .. })
        ));
    }

    #[tokio::test]
    async fn non_regular_files_are_refused() {
        let root = tempfile::tempdir().unwrap();
        for mode in [0o010644, 0o020644, 0o060644, 0o140644, 0o120777] {
            let mut sftp = FakeSftp::with(P, b"x");
            sftp.0.lock().unwrap().mode = mode;
            assert!(
                matches!(
                    RemoteEdit::open(&mut sftp, P, &cfg(root.path())).await,
                    Err(RemoteEditError::NotAFile(_))
                ),
                "mode {mode:o}"
            );
        }
        let mut ok = FakeSftp::with(P, b"x");
        ok.0.lock().unwrap().mode = 0o100644;
        assert!(
            RemoteEdit::open(&mut ok, P, &cfg(root.path()))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_preexisting_tmp_path_is_never_written_through_or_removed() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"v1\n");
        let planted = "/etc/app/.conf.toml.sid-planted";
        {
            let mut s = sftp.0.lock().unwrap();
            s.rename_ok = true;
            s.exclusive_ok = true;
            s.files.insert(planted.into(), (b"attacker".to_vec(), 5));
        }
        let mut c = cfg(root.path());
        c.strategy = UploadStrategy::TmpRename;
        let mut re = RemoteEdit::open(&mut sftp, P, &c).await.unwrap();
        re.tmp_override = Some(planted.into());
        edit_local(&re, b"v2 longer\n");
        assert_eq!(
            re.sync(&mut sftp).await.unwrap(),
            SyncOutcome::Uploaded { atomic: false }
        );
        assert_eq!(sftp.body(planted).unwrap(), b"attacker", "left untouched");
        assert_eq!(sftp.body(P).unwrap(), b"v2 longer\n");
        assert_eq!(sftp.0.lock().unwrap().renames, 0);
    }

    #[test]
    fn tmp_names_are_random_and_stay_in_the_directory() {
        let a = random_tmp_name("/srv/x/file.conf").unwrap();
        let b = random_tmp_name("/srv/x/file.conf").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with("/srv/x/.file.conf.sid-"));
        assert!(random_tmp_name("bare").unwrap().starts_with(".bare.sid-"));
    }

    #[test]
    fn local_names_are_sanitized() {
        assert_eq!(safe_local_name("/a/b/conf.toml"), "conf.toml");
        assert_eq!(safe_local_name("/a/evil.desktop"), "evil.desktop.txt");
        assert_eq!(safe_local_name("/a/X.HTML"), "X.HTML.txt");
        assert_eq!(safe_local_name("/a/run.SH"), "run.SH.txt");
        assert_eq!(safe_local_name("/a/p.ps1"), "p.ps1.txt");
        assert_eq!(safe_local_name("/a/app.AppImage"), "app.AppImage.txt");
        assert_eq!(safe_local_name("/a/x.sh.bak"), "x.sh.bak");
        assert_eq!(
            safe_local_name("/a/we\nird\x07na\x1b[me.txt"),
            "weirdna_me.txt"
        );
        assert_eq!(safe_local_name("/a/dir/"), "file.txt");
        assert_eq!(safe_local_name("/a/.."), "file.txt");
        assert_eq!(safe_local_name("/a/-rf"), "rf");
        let long = format!("/a/{}.rs", "n".repeat(300));
        let n = safe_local_name(&long);
        assert!(n.chars().count() <= MAX_NAME_CHARS && n.ends_with(".rs"));
    }

    #[test]
    fn hostile_names_become_shell_inert() {
        let cases = [
            ("/a/$(touch x).sh", "__touch_x_.sh.txt"),
            ("/a/a;b", "a_b"),
            ("/a/`id`", "_id_"),
            ("/a/-rf", "rf"),
            ("/a/.bashrc", "bashrc"),
            ("/a/--help.md", "help.md"),
            ("/a/x\u{202E}gnp.exe", "xgnp.exe.txt"),
            ("/a/a b|c&d>e.txt", "a_b_c_d_e.txt"),
            ("/a/w.code-workspace", "w.code-workspace.txt"),
            ("/a/p.sublime-project", "p.sublime-project.txt"),
            ("/a/p.sublime-workspace", "p.sublime-workspace.txt"),
            ("/a/h\u{e9}llo.rs", "h_llo.rs"),
        ];
        for (input, want) in cases {
            let got = safe_local_name(input);
            assert_eq!(got, want, "{input:?}");
            assert!(
                got.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
                "{got}"
            );
            assert!(!got.starts_with(['-', '.']), "{got}");
        }
    }

    #[tokio::test]
    async fn a_hostile_remote_name_lands_neutralised_and_absolute() {
        let root = tempfile::tempdir().unwrap();
        let hostile = "/srv/-evil.desktop";
        let mut sftp = FakeSftp::with(hostile, b"[Desktop Entry]\n");
        let mut re = RemoteEdit::open(&mut sftp, hostile, &cfg(root.path()))
            .await
            .unwrap();
        assert_eq!(re.local_path().file_name().unwrap(), "evil.desktop.txt");
        assert!(re.local_path().is_absolute());
        let ed = FakeEditorLauncher::new();
        re.launch(&ed).unwrap();
        assert!(ed.opened()[0].is_absolute());
    }

    #[tokio::test]
    async fn remote_edits_refuse_a_hand_off_launcher() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"x");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg(root.path()))
            .await
            .unwrap();
        let err = re.launch(&FakeEditorLauncher::hand_off()).unwrap_err();
        assert!(matches!(
            err,
            RemoteEditError::Editor(EditorError::NoEditorConfigured)
        ));
    }

    #[test]
    fn default_cache_root_is_not_tmp_when_home_is_set() {
        let r = default_cache_root();
        assert!(r.ends_with("sid/remote-edit"));
    }

    fn cfg_b(root: &Path) -> RemoteEditConfig {
        let mut c = cfg(root);
        c.backups_root = root.join("backups");
        c
    }

    #[tokio::test]
    async fn close_or_preserve_removes_an_unchanged_copy() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"x");
        let re = RemoteEdit::open(&mut sftp, P, &cfg_b(root.path()))
            .await
            .unwrap();
        let dir = re.local_path().parent().unwrap().to_path_buf();
        assert!(matches!(
            re.close_or_preserve("web1", 0),
            Preserved::Removed
        ));
        assert!(!dir.exists());
        assert!(!root.path().join("backups").exists());
    }

    #[tokio::test]
    async fn close_or_preserve_moves_unsaved_edits_to_a_private_backup() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"x");
        let re = RemoteEdit::open(&mut sftp, P, &cfg_b(root.path()))
            .await
            .unwrap();
        edit_local(&re, b"unsaved work");
        let dir = re.local_path().parent().unwrap().to_path_buf();
        let Preserved::BackedUp(dest) = re.close_or_preserve("web 1/../x", 1_794_090_605) else {
            panic!("expected a backup");
        };
        assert!(!dir.exists(), "temp dir removed only after the backup");
        assert_eq!(std::fs::read(&dest).unwrap(), b"unsaved work");
        let rel = dest.strip_prefix(root.path().join("backups")).unwrap();
        assert_eq!(
            rel.to_str().unwrap(),
            "web_1_.._x/conf.toml.local-20261107-223005"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(m(&dest), 0o600);
            assert_eq!(m(dest.parent().unwrap()), 0o700);
        }
    }

    #[tokio::test]
    async fn close_or_preserve_never_deletes_after_a_backup_error() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"x");
        let mut c = cfg_b(root.path());
        // A regular file where the backups dir must go: creating it fails.
        std::fs::write(root.path().join("blocker"), b"").unwrap();
        c.backups_root = root.path().join("blocker");
        let re = RemoteEdit::open(&mut sftp, P, &c).await.unwrap();
        edit_local(&re, b"precious");
        let local = re.local_path().to_path_buf();
        match re.close_or_preserve("h", 0) {
            Preserved::Failed { kept, .. } => assert_eq!(kept, local),
            other => panic!("{other:?}"),
        }
        assert_eq!(std::fs::read(&local).unwrap(), b"precious");
    }

    #[tokio::test]
    async fn a_second_backup_of_the_same_name_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let mut got = Vec::new();
        for body in [&b"one"[..], b"two"] {
            let mut sftp = FakeSftp::with(P, b"x");
            let re = RemoteEdit::open(&mut sftp, P, &cfg_b(root.path()))
                .await
                .unwrap();
            edit_local(&re, body);
            let Preserved::BackedUp(d) = re.close_or_preserve("h", 5) else {
                panic!()
            };
            got.push(d);
        }
        assert_ne!(got[0], got[1]);
        assert_eq!(std::fs::read(&got[0]).unwrap(), b"one");
        assert_eq!(std::fs::read(&got[1]).unwrap(), b"two");
    }

    #[tokio::test]
    async fn an_uploaded_edit_counts_as_saved() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"x");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg_b(root.path()))
            .await
            .unwrap();
        edit_local(&re, b"saved");
        re.sync(&mut sftp).await.unwrap();
        assert!(matches!(re.close_or_preserve("h", 0), Preserved::Removed));
    }

    #[tokio::test]
    async fn same_size_same_mtime_remote_change_is_still_a_conflict() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"aaaa");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg_b(root.path()))
            .await
            .unwrap();
        // identical size and mtime, different bytes
        sftp.0
            .lock()
            .unwrap()
            .files
            .insert(P.into(), (b"bbbb".to_vec(), 1000));
        edit_local(&re, b"mine");
        assert!(matches!(
            re.sync(&mut sftp).await,
            Err(RemoteEditError::Conflict(
                ConflictKind::RemoteChanged { .. }
            ))
        ));
        assert_eq!(sftp.body(P).unwrap(), b"bbbb");
    }

    #[tokio::test]
    async fn a_touched_but_identical_remote_is_not_a_conflict() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"same");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg_b(root.path()))
            .await
            .unwrap();
        sftp.remote_edit_behind_our_back(P, b"same");
        edit_local(&re, b"mine!");
        assert!(re.sync(&mut sftp).await.is_ok());
    }

    fn tmp_cfg(root: &Path) -> RemoteEditConfig {
        let mut c = cfg_b(root);
        c.strategy = UploadStrategy::TmpRename;
        c
    }

    async fn rename_fixture(root: &Path, ident: Option<SftpIdentity>) -> (FakeSftp, RemoteEdit) {
        let mut sftp = FakeSftp::with(P, b"v1\n");
        {
            let mut s = sftp.0.lock().unwrap();
            s.rename_ok = true;
            s.exclusive_ok = true;
            s.tmp_uid = 1000;
            s.ident = ident;
        }
        let re = RemoteEdit::open(&mut sftp, P, &tmp_cfg(root))
            .await
            .unwrap();
        edit_local(&re, b"v2 longer\n");
        (sftp, re)
    }

    fn ident(f: impl FnOnce(&mut SftpIdentity)) -> Option<SftpIdentity> {
        let mut i = SftpIdentity {
            is_symlink: false,
            is_regular: true,
            uid: Some(1000),
            gid: Some(1000),
            mode: 0o100640,
        };
        f(&mut i);
        Some(i)
    }

    #[tokio::test]
    async fn overwrite_is_the_default_strategy() {
        assert_eq!(UploadStrategy::default(), UploadStrategy::Overwrite);
    }

    #[tokio::test]
    async fn tmp_rename_sets_the_mode_at_open_and_drops_suid() {
        let root = tempfile::tempdir().unwrap();
        let (mut sftp, mut re) = rename_fixture(
            root.path(),
            ident(|i| i.mode = 0o104755), // setuid
        )
        .await;
        assert_eq!(
            re.sync(&mut sftp).await.unwrap(),
            SyncOutcome::Uploaded { atomic: true }
        );
        let modes = sftp.0.lock().unwrap().put_modes.clone();
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].1, 0o755, "suid bit dropped, set at open");
    }

    async fn assert_in_place(i: Option<SftpIdentity>, tmp_uid: u32) {
        let root = tempfile::tempdir().unwrap();
        let (mut sftp, mut re) = rename_fixture(root.path(), i).await;
        sftp.0.lock().unwrap().tmp_uid = tmp_uid;
        assert_eq!(
            re.sync(&mut sftp).await.unwrap(),
            SyncOutcome::Uploaded { atomic: false }
        );
        assert_eq!(sftp.0.lock().unwrap().renames, 0);
        assert_eq!(sftp.body(P).unwrap(), b"v2 longer\n");
        assert_eq!(sftp.0.lock().unwrap().files.len(), 1, "tmp cleaned up");
    }

    #[tokio::test]
    async fn a_symlink_target_is_rewritten_in_place() {
        assert_in_place(ident(|i| i.is_symlink = true), 1000).await;
    }

    #[tokio::test]
    async fn a_non_regular_target_is_rewritten_in_place() {
        assert_in_place(ident(|i| i.is_regular = false), 1000).await;
    }

    #[tokio::test]
    async fn an_owner_mismatch_is_rewritten_in_place() {
        // the temp file would be owned by uid 0 while the target belongs to 1000
        assert_in_place(ident(|_| {}), 0).await;
    }

    #[tokio::test]
    async fn unknown_ownership_is_rewritten_in_place() {
        assert_in_place(ident(|i| i.uid = None), 1000).await;
    }
}
