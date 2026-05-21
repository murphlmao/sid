//! Verifies the SshClient trait is dyn-compatible.

use async_trait::async_trait;
use sid_core::adapters::ssh::{
    ExecResult, SftpEntry, SftpSession, SshAuth, SshClient, SshError, SshHostSpec, SshShell,
};

struct MockClient {
    connected: bool,
}

#[async_trait]
impl SshClient for MockClient {
    async fn connect(&mut self, _host: &SshHostSpec, _auth: &SshAuth) -> Result<(), SshError> {
        self.connected = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<(), SshError> {
        self.connected = false;
        Ok(())
    }
    fn is_connected(&self) -> bool {
        self.connected
    }
    async fn exec(&mut self, _cmd: &str) -> Result<ExecResult, SshError> {
        Ok(ExecResult {
            stdout: b"ok\n".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
        })
    }
    async fn open_shell(
        &mut self,
        _term: &str,
        _rows: u16,
        _cols: u16,
    ) -> Result<Box<dyn SshShell>, SshError> {
        Err(SshError::Other("mock has no shell".into()))
    }
    async fn open_sftp(&mut self) -> Result<Box<dyn SftpSession>, SshError> {
        Err(SshError::Other("mock has no sftp".into()))
    }
}

#[tokio::test]
async fn client_is_dyn_compatible() {
    let mut c: Box<dyn SshClient> = Box::new(MockClient { connected: false });
    let host = SshHostSpec {
        host: "example.com".into(),
        port: 22,
        user: "test".into(),
    };
    c.connect(&host, &SshAuth::None).await.unwrap();
    assert!(c.is_connected());
    let r = c.exec("echo").await.unwrap();
    assert_eq!(r.exit_code, 0);
    c.disconnect().await.unwrap();
    assert!(!c.is_connected());
}

#[test]
fn client_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn SshClient>>();
    assert_send_sync::<Box<dyn SftpSession>>();
    assert_send_sync::<Box<dyn SshShell>>();
}

#[test]
fn ssh_auth_variants_exist() {
    let _ = SshAuth::None;
    let _ = SshAuth::Password("x".into());
    let _ = SshAuth::Key {
        path: std::path::PathBuf::from("/k"),
        passphrase: None,
    };
    let _ = SshAuth::Agent;
}

#[test]
fn sftp_entry_construction() {
    let e = SftpEntry {
        name: "foo.txt".into(),
        is_dir: false,
        size: 42,
        mtime_secs: 0,
        mode: 0o644,
    };
    assert_eq!(e.name, "foo.txt");
    assert!(!e.is_dir);
}

#[test]
fn ssh_host_spec_default_port_is_22_in_constructor() {
    let s = SshHostSpec::new("h", "u");
    assert_eq!(s.port, 22);
    assert_eq!(s.host, "h");
    assert_eq!(s.user, "u");
}
