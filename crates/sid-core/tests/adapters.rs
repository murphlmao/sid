//! Integration tests confirming each adapter trait can be implemented on a
//! unit struct. These tests act as compile-time verification that the trait
//! signatures are well-formed and usable by external code.

use std::path::Path;

use sid_core::adapters::clipboard::Clipboard;
use sid_core::adapters::db_client::DbClient;
use sid_core::adapters::git::{
    Branch, CommitInfo, DiffEntry, GitError, GitProvider, GitStatus, NewCommit,
};
use sid_core::adapters::notifier::{NotifyLevel, Notifier};
use sid_core::adapters::pty::PtyProvider;
use sid_core::adapters::ssh::SshClient;
use sid_core::adapters::sys::SysProvider;

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
        Ok(GitStatus { entries: vec![], is_clean: true })
    }
    fn commit_log(&self, _max: usize, _from_oid: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
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
impl SshClient for NoopSsh {}

struct NoopPty;
impl PtyProvider for NoopPty {}

struct NoopDb;
impl DbClient for NoopDb {}

struct NoopSys;
impl SysProvider for NoopSys {}

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
