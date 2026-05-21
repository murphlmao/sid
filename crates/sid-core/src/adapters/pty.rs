//! PTY provider trait + supporting domain types. Implementations live in `sid-pty`.

use std::collections::HashMap;

/// Domain-shaped PTY error.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("failed to open PTY: {0}")]
    OpenFailed(String),
    #[error("write failed: {0}")]
    WriteFailed(String),
    #[error("read failed: {0}")]
    ReadFailed(String),
    #[error("resize failed: {0}")]
    ResizeFailed(String),
    #[error("child has exited (status {0:?})")]
    ChildExited(Option<i32>),
    #[error("pty operation failed: {0}")]
    Other(String),
}

/// PTY window size (in cells, not pixels).
///
/// # Examples
///
/// ```
/// use sid_core::adapters::pty::PtySize;
/// let s = PtySize::new(24, 80);
/// assert_eq!(s.rows, 24);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl PtySize {
    /// Construct a size.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::pty::PtySize;
    /// let s = PtySize::new(24, 80);
    /// assert_eq!(s.rows, 24);
    /// ```
    pub fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

impl Default for PtySize {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

/// Spec for spawning a child on the PTY slave.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::pty::PtySpawn;
/// let s = PtySpawn::shell();
/// assert!(!s.program.is_empty());
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtySpawn {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub env: HashMap<String, String>,
    pub size: PtySize,
}

impl PtySpawn {
    /// Spawn the user's default shell (`$SHELL` or `/bin/sh` fallback).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::pty::PtySpawn;
    /// let s = PtySpawn::shell();
    /// assert!(!s.program.is_empty());
    /// ```
    pub fn shell() -> Self {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Self {
            program,
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            size: PtySize::default(),
        }
    }

    /// Spawn an arbitrary command.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::pty::PtySpawn;
    /// let s = PtySpawn::command("ls", &["-la"]);
    /// assert_eq!(s.program, "ls");
    /// ```
    pub fn command(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            env: HashMap::new(),
            size: PtySize::default(),
        }
    }
}

/// A live PTY handle — owns the master end + child handle.
pub trait PtyHandle: Send + Sync {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, PtyError>;
    fn try_read(&mut self) -> Result<Vec<u8>, PtyError>;
    fn resize(&mut self, size: PtySize) -> Result<(), PtyError>;
    fn child_alive(&self) -> bool;
    fn size(&self) -> PtySize;
    fn kill(&mut self) -> Result<(), PtyError>;
}

/// Factory for `PtyHandle`s. Implementations live in `sid-pty`.
pub trait PtyProvider: Send + Sync {
    fn open_pty(&self, spec: &PtySpawn) -> Result<Box<dyn PtyHandle>, PtyError>;
}

/// A render-friendly snapshot of a terminal screen. Implementations live in
/// `sid-pty` (e.g. `Vt100Screen`).
pub trait TerminalScreen: Send + Sync {
    /// Feed bytes from the remote (or local PTY) into the screen.
    fn feed(&mut self, bytes: &[u8]);
    /// Resize the screen.
    fn resize(&mut self, rows: u16, cols: u16);
    /// Current size as `(rows, cols)`.
    fn size(&self) -> (u16, u16);
    /// Cursor position as `(row, col)`.
    fn cursor_position(&self) -> (u16, u16);
    /// Current screen contents as one string per row.
    fn lines(&self) -> Vec<String>;
}
