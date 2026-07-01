//! Interactive shell channel — wraps a russh `Channel` behind the `SshShell` trait.

use std::sync::Arc;

use async_trait::async_trait;
use russh::{Channel, ChannelMsg, ChannelWriteHalf, CryptoVec, client::Msg};
use sid_core::ssh::{SshError, SshShell};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::client::map_russh_error;

/// PTY-backed shell channel; reader task pumps bytes into an internal buffer.
///
/// The channel is `split()` so the reader task owns the read half outright and
/// blocks in its own `wait().await` without holding any lock the write path
/// needs — a shared `Arc<Mutex<Channel>>` would starve interactive writes
/// whenever the reader is parked waiting on an idle session.
pub struct RusshShell {
    write_half: ChannelWriteHalf<Msg>,
    buffer: Arc<Mutex<Vec<u8>>>,
    reader: JoinHandle<()>,
    closed: bool,
}

impl RusshShell {
    pub(crate) fn new(channel: Channel<Msg>) -> Self {
        let (mut read_half, write_half) = channel.split();
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buffer_for_task = buffer.clone();
        let reader = tokio::spawn(async move {
            loop {
                match read_half.wait().await {
                    Some(ChannelMsg::Data { data }) => {
                        buffer_for_task.lock().await.extend_from_slice(&data);
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        buffer_for_task.lock().await.extend_from_slice(&data);
                    }
                    Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => break,
                    _ => {}
                }
            }
        });
        Self {
            write_half,
            buffer,
            reader,
            closed: false,
        }
    }
}

#[async_trait]
impl SshShell for RusshShell {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), SshError> {
        if self.closed {
            return Err(SshError::Disconnected);
        }
        let data = CryptoVec::from(bytes.to_vec());
        self.write_half
            .data(&data[..])
            .await
            .map_err(|_| SshError::Other("shell write failed".into()))?;
        Ok(())
    }

    async fn try_read(&mut self) -> Result<Vec<u8>, SshError> {
        let mut buf = self.buffer.lock().await;
        let out = std::mem::take(&mut *buf);
        Ok(out)
    }

    async fn resize(&mut self, rows: u16, cols: u16) -> Result<(), SshError> {
        self.write_half
            .window_change(cols as u32, rows as u32, 0, 0)
            .await
            .map_err(map_russh_error)?;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), SshError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let _ = self.write_half.close().await;
        // The reader task can only exit on its own via `Close`/`Eof`/channel
        // drop; abort it explicitly so it never outlives this shell.
        self.reader.abort();
        Ok(())
    }
}

impl Drop for RusshShell {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
