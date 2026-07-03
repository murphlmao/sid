//! Interactive shell channel — wraps a russh `Channel` behind the split
//! `SshShellReader`/`SshShellWriter` traits.
//!
//! The channel is `split()` so a background pump task owns the read half outright and
//! blocks in its own `wait().await` without holding any lock the write path needs — a
//! shared `Arc<Mutex<Channel>>` would starve interactive writes whenever the reader is
//! parked waiting on an idle session. Splitting `SshShellReader` from `SshShellWriter`
//! at the trait level (not just internally) goes one step further: it means a caller
//! can no longer accidentally reintroduce that hazard one layer up by putting both
//! sides behind one shared mutex (which is exactly what happened before this split —
//! see `sid-core/src/ssh.rs`'s doc comment on `SshShellReader`).

use std::sync::Arc;

use async_trait::async_trait;
use russh::{Channel, ChannelMsg, ChannelWriteHalf, CryptoVec, client::Msg};
use sid_core::ssh::{SshError, SshShellReader, SshShellWriter};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;

use crate::client::map_russh_error;

/// The read half: a background pump task feeds channel data into `buffer`;
/// `try_read` just drains it. Owns the pump task's `JoinHandle` and aborts it on
/// drop so it can never outlive this reader, no matter how the reader itself goes
/// out of scope (an explicit `SshShellWriter::close()` also asks it to stop first
/// via `shutdown_tx` — see [`split`] — but this `Drop` is the backstop for every
/// other path: the entity dropping without a clean disconnect, a panic, etc.).
pub struct RusshShellReader {
    buffer: Arc<Mutex<Vec<u8>>>,
    pump: JoinHandle<()>,
}

/// The write half: input, PTY resize, and close. Meant to live behind its own
/// lock, independent of the reader's — see the module doc comment.
pub struct RusshShellWriter {
    write_half: ChannelWriteHalf<Msg>,
    closed: bool,
    /// Tells the reader's background pump task to stop. A `oneshot`, not a lock:
    /// the pump's `select!` observes the send without ever contending with
    /// `try_read`'s buffer lock, so `close()` can't block behind — or be blocked
    /// by — a concurrent read.
    shutdown: Option<oneshot::Sender<()>>,
}

/// Split a freshly-opened PTY channel into its reader/writer halves and start the
/// reader's background pump task.
pub(crate) fn split(channel: Channel<Msg>) -> (RusshShellReader, RusshShellWriter) {
    let (mut read_half, write_half) = channel.split();
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let buffer_for_task = buffer.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let pump = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_rx => break,
                msg = read_half.wait() => {
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
            }
        }
    });
    (
        RusshShellReader { buffer, pump },
        RusshShellWriter {
            write_half,
            closed: false,
            shutdown: Some(shutdown_tx),
        },
    )
}

#[async_trait]
impl SshShellReader for RusshShellReader {
    async fn try_read(&mut self) -> Result<Vec<u8>, SshError> {
        let mut buf = self.buffer.lock().await;
        Ok(std::mem::take(&mut *buf))
    }
}

impl Drop for RusshShellReader {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

#[async_trait]
impl SshShellWriter for RusshShellWriter {
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
        // Ask the reader's pump task to stop before tearing down the channel —
        // it can't observe `Close`/`Eof` any faster than the round-trip below
        // takes anyway, so there's no reason to wait for that.
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = self.write_half.close().await;
        Ok(())
    }
}
