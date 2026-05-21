//! Integration tests confirming each adapter trait can be implemented on a
//! unit struct. These tests act as compile-time verification that the trait
//! signatures are well-formed and usable by external code.

use std::path::Path;

use sid_core::adapters::clipboard::Clipboard;
use sid_core::adapters::db_client::DbClient;
use sid_core::adapters::git::{
    Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit,
};
use sid_core::adapters::notifier::{Notifier, NotifyLevel};
use sid_core::adapters::pty::{PtyError, PtyHandle, PtyProvider, PtySize, PtySpawn};
use sid_core::adapters::ssh::{
    ExecResult, SftpSession, SshAuth, SshClient, SshError, SshHostSpec, SshShell,
};
use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
};

// ---------------------------------------------------------------------------
// No-op impls for each trait
// ---------------------------------------------------------------------------

struct NoopGit;
impl GitProvider for NoopGit {
    fn open(&self, _path: &Path) -> Result<Box<dyn GitProvider>, GitError> {
        Ok(Box::new(NoopGit))
    }
    fn list_branches(&self) -> Result<Vec<Branch>, GitError> {
        Ok(vec![])
    }
    fn current_branch(&self) -> Result<Option<Branch>, GitError> {
        Ok(None)
    }
    fn status(&self) -> Result<GitStatus, GitError> {
        Ok(GitStatus {
            entries: vec![],
            is_clean: true,
        })
    }
    fn commit_log(
        &self,
        _max: usize,
        _from_oid: Option<&str>,
    ) -> Result<Vec<CommitInfo>, GitError> {
        Ok(vec![])
    }
    fn diff(&self, _staged: bool) -> Result<Vec<DiffEntry>, GitError> {
        Ok(vec![])
    }
    fn checkout_branch(&mut self, _name: &str) -> Result<(), GitError> {
        Ok(())
    }
    fn commit(&mut self, _new: NewCommit<'_>) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

struct NoopSsh;
#[async_trait::async_trait]
impl SshClient for NoopSsh {
    async fn connect(&mut self, _host: &SshHostSpec, _auth: &SshAuth) -> Result<(), SshError> {
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<(), SshError> {
        Ok(())
    }
    fn is_connected(&self) -> bool {
        false
    }
    async fn exec(&mut self, _cmd: &str) -> Result<ExecResult, SshError> {
        Ok(ExecResult {
            stdout: vec![],
            stderr: vec![],
            exit_code: 0,
        })
    }
    async fn open_shell(
        &mut self,
        _term: &str,
        _rows: u16,
        _cols: u16,
    ) -> Result<Box<dyn SshShell>, SshError> {
        Err(SshError::Other("noop".into()))
    }
    async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError> {
        Err(SshError::Other("noop".into()))
    }
}

struct NoopPtyHandle {
    size: PtySize,
}
impl PtyHandle for NoopPtyHandle {
    fn write(&mut self, b: &[u8]) -> Result<usize, PtyError> {
        Ok(b.len())
    }
    fn try_read(&mut self) -> Result<Vec<u8>, PtyError> {
        Ok(vec![])
    }
    fn resize(&mut self, s: PtySize) -> Result<(), PtyError> {
        self.size = s;
        Ok(())
    }
    fn child_alive(&self) -> bool {
        false
    }
    fn size(&self) -> PtySize {
        self.size
    }
    fn kill(&mut self) -> Result<(), PtyError> {
        Ok(())
    }
}

struct NoopPty;
impl PtyProvider for NoopPty {
    fn open_pty(&self, _spec: &PtySpawn) -> Result<Box<dyn PtyHandle>, PtyError> {
        Ok(Box::new(NoopPtyHandle {
            size: PtySize::default(),
        }))
    }
}

struct NoopDb;
#[async_trait::async_trait]
impl DbClient for NoopDb {
    async fn open(
        &self,
        _p: sid_core::adapters::db_client::OpenParams,
    ) -> Result<std::sync::Arc<dyn DbClient>, sid_core::adapters::db_client::DbError> {
        Ok(std::sync::Arc::new(NoopDb))
    }
    async fn close(&self) -> Result<(), sid_core::adapters::db_client::DbError> {
        Ok(())
    }
    async fn execute(
        &self,
        _sql: &str,
    ) -> Result<sid_core::adapters::db_client::ExecResult, sid_core::adapters::db_client::DbError>
    {
        Ok(sid_core::adapters::db_client::ExecResult {
            rows_affected: 0,
            duration_ms: 0,
        })
    }
    async fn query_paged(
        &self,
        _sql: &str,
        _cursor: Option<sid_core::adapters::db_client::PageCursor>,
        _page_size: u32,
    ) -> Result<sid_core::adapters::db_client::QueryPage, sid_core::adapters::db_client::DbError>
    {
        Ok(sid_core::adapters::db_client::QueryPage {
            columns: vec![],
            rows: vec![],
            next_cursor: None,
            duration_ms: 0,
        })
    }
    async fn schema_introspect(
        &self,
    ) -> Result<sid_core::adapters::db_client::SchemaInfo, sid_core::adapters::db_client::DbError>
    {
        Ok(sid_core::adapters::db_client::SchemaInfo { tables: vec![] })
    }
    async fn cancel(&self) -> Result<(), sid_core::adapters::db_client::DbError> {
        Ok(())
    }
    fn kind(&self) -> sid_core::adapters::db_client::DbKind {
        sid_core::adapters::db_client::DbKind::Sqlite
    }
}

struct NoopSys;
impl SysProvider for NoopSys {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        Ok(vec![])
    }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
        Ok(vec![])
    }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
        Ok(vec![])
    }
    fn kill_process(&mut self, _pid: Pid, _sig: Signal) -> Result<(), SysError> {
        Ok(())
    }
}

struct NoopClipboard;
impl Clipboard for NoopClipboard {
    fn copy(&self, _text: &str) {}
}

struct NoopNotifier;
impl Notifier for NoopNotifier {
    fn notify(&self, _level: NotifyLevel, _message: &str) {}
}

// ---------------------------------------------------------------------------
// Acceptance functions — ensure trait objects work (dyn dispatch)
// ---------------------------------------------------------------------------

fn accept_git(_: &dyn GitProvider) {}
fn accept_ssh(_: &dyn SshClient) {}
fn accept_pty(_: &dyn PtyProvider) {}
fn accept_db(_: &dyn DbClient) {}
fn accept_sys(_: &dyn SysProvider) {}
fn accept_clipboard(_: &dyn Clipboard) {}
fn accept_notifier(_: &dyn Notifier) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn git_provider_noop_impl_compiles_and_dispatches() {
    let g = NoopGit;
    accept_git(&g);
}

#[test]
fn ssh_client_noop_impl_compiles_and_dispatches() {
    let s = NoopSsh;
    accept_ssh(&s);
}

#[test]
fn pty_provider_noop_impl_compiles_and_dispatches() {
    let p = NoopPty;
    accept_pty(&p);
}

#[test]
fn db_client_noop_impl_compiles_and_dispatches() {
    let d = NoopDb;
    accept_db(&d);
}

#[test]
fn sys_provider_noop_impl_compiles_and_dispatches() {
    let s = NoopSys;
    accept_sys(&s);
}

#[test]
fn clipboard_noop_impl_compiles_copy_is_callable() {
    let c = NoopClipboard;
    c.copy("test text");
    accept_clipboard(&c);
}

#[test]
fn notifier_noop_impl_compiles_notify_is_callable() {
    let n = NoopNotifier;
    n.notify(NotifyLevel::Info, "info");
    n.notify(NotifyLevel::Warn, "warning");
    n.notify(NotifyLevel::Error, "error");
    accept_notifier(&n);
}

// ---------------------------------------------------------------------------
// NotifyLevel
// ---------------------------------------------------------------------------

#[test]
fn notify_level_clone_works() {
    let l = NotifyLevel::Info;
    let _cloned = l.clone();
}

#[test]
fn notify_level_debug_formats_all_variants() {
    let variants = [
        (NotifyLevel::Info, "Info"),
        (NotifyLevel::Warn, "Warn"),
        (NotifyLevel::Error, "Error"),
    ];
    for (level, expected) in variants {
        assert_eq!(format!("{level:?}"), expected);
    }
}

// ---------------------------------------------------------------------------
// Send + Sync — verify the trait objects can cross thread boundaries.
// The static assertions below are compile-time only.
// ---------------------------------------------------------------------------

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn adapter_trait_objects_are_send_sync() {
    assert_send_sync::<NoopGit>();
    assert_send_sync::<NoopSsh>();
    assert_send_sync::<NoopPty>();
    assert_send_sync::<NoopDb>();
    assert_send_sync::<NoopSys>();
    assert_send_sync::<NoopClipboard>();
    assert_send_sync::<NoopNotifier>();
}
