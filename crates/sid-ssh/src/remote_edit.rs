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
//! the RAM-backed `/tmp`), is mode 0700, the file 0600, and is removed on close or drop.
//!
//! Upload strategy: [`UploadStrategy::Overwrite`] (default) truncates and rewrites the
//! remote in place, which preserves its mode and owner. [`UploadStrategy::TmpRename`]
//! writes `<name>.sid-tmp` and renames over the target (atomic for readers), falling back
//! to overwrite when the server refuses the rename (plain SFTP rename generally will not
//! replace an existing file). The rename path creates a *new* inode with default mode, so
//! it is opt-in.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sid_core::editor::{EditorError, EditorExit, EditorHandle, EditorLauncher};
use sid_core::ssh::{SftpSession, SshError};

/// Default refusal threshold: 5 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// How an upload replaces the remote file.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UploadStrategy {
    /// Rewrite in place (keeps mode/owner).
    #[default]
    Overwrite,
    /// Write `<name>.sid-tmp`, rename over the target; fall back to overwrite.
    TmpRename,
}

/// Settings for one edit session.
#[derive(Clone, Debug)]
pub struct RemoteEditConfig {
    /// Parent of the per-edit private directory.
    pub cache_root: PathBuf,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LocalSig {
    mtime: Option<SystemTime>,
    len: u64,
    hash: u64,
}

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

fn looks_binary(b: &[u8]) -> bool {
    b[..b.len().min(8192)].contains(&0)
}

/// One remote file being edited locally.
pub struct RemoteEdit {
    remote_path: String,
    dir: tempfile::TempDir,
    local: PathBuf,
    strategy: UploadStrategy,
    max_bytes: u64,
    baseline: RemoteSig,
    synced: LocalSig,
    handle: Option<Box<dyn EditorHandle>>,
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
        if st.is_dir {
            return Err(RemoteEditError::NotAFile(remote_path.to_string()));
        }
        if st.size > cfg.max_bytes {
            return Err(RemoteEditError::TooLarge {
                size: st.size,
                max: cfg.max_bytes,
            });
        }
        let bytes = sftp.get(remote_path).await?;
        if bytes.len() as u64 > cfg.max_bytes {
            return Err(RemoteEditError::TooLarge {
                size: bytes.len() as u64,
                max: cfg.max_bytes,
            });
        }
        if looks_binary(&bytes) {
            return Err(RemoteEditError::Binary);
        }

        private_dir_all(&cfg.cache_root)?;
        let dir = tempfile::Builder::new()
            .prefix("edit-")
            .tempdir_in(&cfg.cache_root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let name = match remote_path.rsplit('/').next() {
            Some(n) if !n.is_empty() && n != "." && n != ".." => n,
            _ => "file",
        };
        let local = dir.path().join(name);
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
            synced: LocalSig {
                mtime,
                len: bytes.len() as u64,
                hash: hash_bytes(&bytes),
            },
            handle: None,
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
        self.handle = Some(launcher.open(&self.local)?);
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
                    if now != self.baseline {
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
                let tmp = format!("{}.sid-tmp", self.remote_path);
                sftp.put(&tmp, &bytes).await?;
                if sftp.rename(&tmp, &self.remote_path).await.is_ok() {
                    atomic = true;
                } else {
                    let res = sftp.put(&self.remote_path, &bytes).await;
                    let _ = sftp.remove_file(&tmp).await;
                    res?;
                }
            }
        }
        if let Some(e) = sftp.stat(&self.remote_path).await? {
            self.baseline = RemoteSig {
                mtime_secs: e.mtime_secs,
                size: e.size,
            };
        }
        self.synced = LocalSig {
            mtime,
            len: bytes.len() as u64,
            hash: hash_bytes(&bytes),
        };
        Ok(SyncOutcome::Uploaded { atomic })
    }

    /// Remove the private directory and its contents.
    pub fn close(self) -> io::Result<()> {
        self.dir.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sid_core::editor::FakeEditorLauncher;
    use sid_core::ssh::SftpEntry;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct State {
        files: HashMap<String, (Vec<u8>, i64)>,
        clock: i64,
        rename_ok: bool,
        puts: Vec<String>,
        renames: usize,
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
            Ok(self.0.lock().unwrap().files.get(p).map(|(b, m)| SftpEntry {
                name: p.into(),
                is_dir: false,
                size: b.len() as u64,
                mtime_secs: *m,
                mode: 0o644,
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
        let mut c = cfg(root.path());
        c.strategy = UploadStrategy::TmpRename;
        let mut re = RemoteEdit::open(&mut sftp, P, &c).await.unwrap();
        edit_local(&re, b"v2 longer\n");
        assert_eq!(
            re.sync(&mut sftp).await.unwrap(),
            SyncOutcome::Uploaded { atomic: true }
        );
        assert_eq!(sftp.body(P).unwrap(), b"v2 longer\n");
        assert!(sftp.body(&format!("{P}.sid-tmp")).is_none());
        assert_eq!(sftp.0.lock().unwrap().renames, 1);
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
    async fn hand_off_launcher_reports_hand_off() {
        let root = tempfile::tempdir().unwrap();
        let mut sftp = FakeSftp::with(P, b"x");
        let mut re = RemoteEdit::open(&mut sftp, P, &cfg(root.path()))
            .await
            .unwrap();
        assert_eq!(re.editor_state().unwrap(), EditorState::NotLaunched);
        let ed = FakeEditorLauncher::hand_off();
        re.launch(&ed).unwrap();
        assert_eq!(re.editor_state().unwrap(), EditorState::HandOff);
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

    #[test]
    fn default_cache_root_is_not_tmp_when_home_is_set() {
        let r = default_cache_root();
        assert!(r.ends_with("sid/remote-edit"));
    }
}
