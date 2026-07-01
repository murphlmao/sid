//! SFTP wrapper — bridges russh-sftp's `SftpSession` to the domain `SftpSession` trait.

use async_trait::async_trait;
use russh_sftp::{
    client::{SftpSession as RusshSftpSession, error::Error as RusshSftpError},
    protocol::StatusCode,
};
use sid_core::ssh::{SftpEntry, SftpSession, SshError};

pub struct RusshSftp {
    inner: RusshSftpSession,
}

impl RusshSftp {
    pub(crate) fn new(inner: RusshSftpSession) -> Self {
        Self { inner }
    }
}

fn map_sftp_error(e: RusshSftpError) -> SshError {
    match e {
        RusshSftpError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => SshError::PathNotFound(s.error_message),
            _ => SshError::Other(format!("sftp: {}", s.error_message)),
        },
        other => SshError::Other(format!("sftp: {other}")),
    }
}

#[async_trait]
impl SftpSession for RusshSftp {
    async fn list(&mut self, path: &str) -> Result<Vec<SftpEntry>, SshError> {
        let read_dir = self
            .inner
            .read_dir(path.to_string())
            .await
            .map_err(map_sftp_error)?;
        let mut out = Vec::new();
        for entry in read_dir {
            let md = entry.metadata();
            out.push(SftpEntry {
                name: entry.file_name(),
                is_dir: md.is_dir(),
                size: md.size.unwrap_or(0),
                mtime_secs: md.mtime.unwrap_or(0) as i64,
                mode: md.permissions.unwrap_or(0),
            });
        }
        Ok(out)
    }

    async fn get(&mut self, path: &str) -> Result<Vec<u8>, SshError> {
        self.inner
            .read(path.to_string())
            .await
            .map_err(map_sftp_error)
    }

    async fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), SshError> {
        self.inner
            .write(path.to_string(), bytes)
            .await
            .map_err(map_sftp_error)
    }

    async fn remove_file(&mut self, path: &str) -> Result<(), SshError> {
        self.inner
            .remove_file(path.to_string())
            .await
            .map_err(map_sftp_error)
    }

    async fn mkdir(&mut self, path: &str) -> Result<(), SshError> {
        self.inner
            .create_dir(path.to_string())
            .await
            .map_err(map_sftp_error)
    }

    async fn stat(&mut self, path: &str) -> Result<Option<SftpEntry>, SshError> {
        match self.inner.metadata(path.to_string()).await {
            Ok(md) => Ok(Some(SftpEntry {
                name: path.rsplit('/').next().unwrap_or(path).to_string(),
                is_dir: md.is_dir(),
                size: md.size.unwrap_or(0),
                mtime_secs: md.mtime.unwrap_or(0) as i64,
                mode: md.permissions.unwrap_or(0),
            })),
            Err(e) => match map_sftp_error(e) {
                SshError::PathNotFound(_) => Ok(None),
                other => Err(other),
            },
        }
    }

    async fn close(&mut self) -> Result<(), SshError> {
        let _ = self.inner.close().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_sftp::protocol::{Status, StatusCode};

    fn status_err(code: StatusCode, msg: &str) -> RusshSftpError {
        RusshSftpError::Status(Status {
            id: 0,
            status_code: code,
            error_message: msg.to_string(),
            language_tag: String::new(),
        })
    }

    #[test]
    fn no_such_file_maps_to_path_not_found() {
        match map_sftp_error(status_err(StatusCode::NoSuchFile, "/nope")) {
            SshError::PathNotFound(m) => assert_eq!(m, "/nope"),
            other => panic!("expected PathNotFound, got {other:?}"),
        }
    }

    #[test]
    fn other_status_maps_to_other() {
        match map_sftp_error(status_err(StatusCode::PermissionDenied, "denied")) {
            SshError::Other(m) => assert!(m.contains("denied")),
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
