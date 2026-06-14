//! End-to-end SFTP edit-in-place flow via a fake SshClient SftpSession.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sid_core::adapters::ssh::{SftpEntry, SftpSession, SshError};
use sid_widgets::{
    ssh::{SftpEditPhase, SftpEditState},
    workspaces::{EditorRunner, MockEditorRunner},
};
use tempfile::tempdir;

struct FakeSftp {
    files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

#[async_trait]
impl SftpSession for FakeSftp {
    async fn list(&mut self, _path: &str) -> Result<Vec<SftpEntry>, SshError> {
        Ok(vec![])
    }
    async fn get(&mut self, path: &str) -> Result<Vec<u8>, SshError> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| SshError::PathNotFound(path.into()))
    }
    async fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), SshError> {
        self.files
            .lock()
            .unwrap()
            .insert(path.into(), bytes.to_vec());
        Ok(())
    }
    async fn remove_file(&mut self, _p: &str) -> Result<(), SshError> {
        Ok(())
    }
    async fn mkdir(&mut self, _p: &str) -> Result<(), SshError> {
        Ok(())
    }
    async fn stat(&mut self, _p: &str) -> Result<Option<SftpEntry>, SshError> {
        Ok(None)
    }
    async fn close(&mut self) -> Result<(), SshError> {
        Ok(())
    }
}

#[tokio::test]
async fn full_edit_in_place_flow_round_trips_modified_bytes() {
    let files = Arc::new(Mutex::new(std::collections::HashMap::from([(
        "/remote/foo.txt".to_string(),
        b"original\n".to_vec(),
    )])));
    let mut sftp: Box<dyn SftpSession> = Box::new(FakeSftp {
        files: files.clone(),
    });

    let tmp = tempdir().unwrap();
    let local = tmp.path().join("foo.txt");
    let editor: Box<dyn EditorRunner> = Box::new(MockEditorRunner::new("modified content".into()));

    let mut state = SftpEditState::default();

    // Phase 1: download
    state.begin_download("/remote/foo.txt".into(), local.clone());
    let bytes = sftp.get("/remote/foo.txt").await.unwrap();
    std::fs::write(&local, &bytes).unwrap();
    state.mark_download_complete();
    assert_eq!(state.phase(), SftpEditPhase::Editing);

    // Phase 2: editor
    let new_content = editor.run_editor().unwrap();
    std::fs::write(&local, new_content.as_bytes()).unwrap();
    state.mark_editor_done(true);
    assert_eq!(state.phase(), SftpEditPhase::Uploading);

    // Phase 3: upload
    let modified = std::fs::read(&local).unwrap();
    sftp.put("/remote/foo.txt", &modified).await.unwrap();
    state.mark_upload_complete();
    assert_eq!(state.phase(), SftpEditPhase::Done);

    let server_now = files
        .lock()
        .unwrap()
        .get("/remote/foo.txt")
        .cloned()
        .unwrap();
    assert_eq!(server_now, b"modified content");
}

#[tokio::test]
async fn edit_in_place_failed_editor_does_not_upload() {
    let files = Arc::new(Mutex::new(std::collections::HashMap::from([(
        "/remote/x.txt".to_string(),
        b"v1".to_vec(),
    )])));
    let mut sftp: Box<dyn SftpSession> = Box::new(FakeSftp {
        files: files.clone(),
    });
    let tmp = tempdir().unwrap();
    let local = tmp.path().join("x.txt");
    let editor: Box<dyn EditorRunner> =
        Box::new(MockEditorRunner::failing("user cancelled".into()));

    let mut state = SftpEditState::default();
    state.begin_download("/remote/x.txt".into(), local.clone());
    std::fs::write(&local, sftp.get("/remote/x.txt").await.unwrap()).unwrap();
    state.mark_download_complete();

    let r = editor.run_editor();
    assert!(r.is_err());
    state.mark_editor_done(false);
    assert_eq!(state.phase(), SftpEditPhase::Failed);

    assert_eq!(files.lock().unwrap().get("/remote/x.txt").unwrap(), b"v1");
}
