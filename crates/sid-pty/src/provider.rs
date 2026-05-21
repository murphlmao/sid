//! `PortablePtyProvider` — opens portable-pty master/slave pairs and spawns a
//! child process on the slave end.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{Child, CommandBuilder, PtySize as PortablePtySize, native_pty_system};
use sid_core::adapters::pty::{PtyError, PtyHandle, PtyProvider, PtySize, PtySpawn};

/// Stateless provider; per-PTY handles are produced by `open_pty`.
///
/// # Examples
///
/// ```
/// use sid_pty::PortablePtyProvider;
/// let _p = PortablePtyProvider::new();
/// ```
pub struct PortablePtyProvider;

impl PortablePtyProvider {
    /// Construct a new provider. Cheap; no I/O.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::PortablePtyProvider;
    /// let _p = PortablePtyProvider::new();
    /// ```
    pub fn new() -> Self {
        Self
    }
}

impl Default for PortablePtyProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyProvider for PortablePtyProvider {
    fn open_pty(&self, spec: &PtySpawn) -> Result<Box<dyn PtyHandle>, PtyError> {
        let system = native_pty_system();
        let pair = system
            .openpty(PortablePtySize {
                rows: spec.size.rows,
                cols: spec.size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::OpenFailed(format!("openpty: {e}")))?;
        let mut cmd = CommandBuilder::new(&spec.program);
        for a in &spec.args {
            cmd.arg(a);
        }
        if let Some(c) = &spec.cwd {
            cmd.cwd(c);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::OpenFailed(format!("spawn: {e}")))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::OpenFailed(format!("clone_reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::OpenFailed(format!("take_writer: {e}")))?;
        drop(pair.slave);

        let rx_buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let rx_for_thread = rx_buffer.clone();
        std::thread::spawn(move || {
            let mut tmp = [0u8; 4096];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut buf) = rx_for_thread.lock() {
                            buf.extend_from_slice(&tmp[..n]);
                        } else {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Box::new(PortablePtyHandle {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Arc::new(Mutex::new(child)),
            rx_buffer,
            size: spec.size,
        }))
    }
}

/// Live PTY handle.
pub struct PortablePtyHandle {
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    rx_buffer: Arc<Mutex<Vec<u8>>>,
    size: PtySize,
}

impl PtyHandle for PortablePtyHandle {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, PtyError> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut w = self.writer.lock().map_err(|_| PtyError::WriteFailed("poisoned".into()))?;
        let n = w.write(bytes).map_err(|e| PtyError::WriteFailed(format!("{e}")))?;
        w.flush().map_err(|e| PtyError::WriteFailed(format!("flush: {e}")))?;
        Ok(n)
    }

    fn try_read(&mut self) -> Result<Vec<u8>, PtyError> {
        let mut buf = self
            .rx_buffer
            .lock()
            .map_err(|_| PtyError::ReadFailed("poisoned".into()))?;
        Ok(std::mem::take(&mut *buf))
    }

    fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        self.master
            .lock()
            .map_err(|_| PtyError::ResizeFailed("poisoned".into()))?
            .resize(PortablePtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::ResizeFailed(format!("{e}")))?;
        self.size = size;
        Ok(())
    }

    fn child_alive(&self) -> bool {
        let mut c = match self.child.lock() {
            Ok(c) => c,
            Err(_) => return false,
        };
        matches!(c.try_wait(), Ok(None))
    }

    fn size(&self) -> PtySize {
        self.size
    }

    fn kill(&mut self) -> Result<(), PtyError> {
        let mut c = self.child.lock().map_err(|_| PtyError::Other("poisoned".into()))?;
        let _ = c.kill();
        Ok(())
    }
}
