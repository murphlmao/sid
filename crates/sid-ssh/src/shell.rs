//! Interactive shell channel — wraps a russh `Channel` behind the `SshShell` trait.

use std::sync::Arc;

use async_trait::async_trait;
use russh::client::Msg;
use russh::{Channel, ChannelMsg, CryptoVec};
use sid_core::adapters::ssh::{SshError, SshShell};
use tokio::sync::Mutex;

use crate::client::map_russh_error;

/// PTY-backed shell channel; reader task pumps bytes into an internal buffer.
pub struct RusshShell {
    channel: Arc<Mutex<Channel<Msg>>>,
    buffer: Arc<Mutex<Vec<u8>>>,
    closed: bool,
}

impl RusshShell {
    pub(crate) fn new(channel: Channel<Msg>) -> Self {
        let channel = Arc::new(Mutex::new(channel));
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let channel_for_task = channel.clone();
        let buffer_for_task = buffer.clone();
        tokio::spawn(async move {
            loop {
                let msg = { channel_for_task.lock().await.wait().await };
                match msg {
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
            channel,
            buffer,
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
        let ch = self.channel.lock().await;
        ch.data(&data[..])
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
        self.channel
            .lock()
            .await
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
        let _ = self.channel.lock().await.close().await;
        Ok(())
    }
}
