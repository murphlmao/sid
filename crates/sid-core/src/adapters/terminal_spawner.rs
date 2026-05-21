//! `TerminalSpawner` trait — abstraction over "launch an external terminal
//! window running this command in this directory".
//!
//! v1 ships [`sid_system::KittyTerminalSpawner`] in `sid-system`. Users on
//! iTerm/wezterm/tmux can swap in their own impl via the binary's wire layer.

use std::path::PathBuf;

/// Error type for terminal spawners.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::terminal_spawner::SpawnerError;
/// let e = SpawnerError::TerminalMissing("kitty".into());
/// assert!(format!("{e}").contains("kitty"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum SpawnerError {
    /// The configured terminal binary is missing on PATH.
    #[error("terminal binary not found in PATH (looked for: {0})")]
    TerminalMissing(String),
    /// Neither `$EDITOR` nor `$VISUAL` is set and the fallback (`vi`) is missing.
    #[error("$EDITOR is not set and no fallback (vi) is available")]
    EditorMissing,
    /// I/O error spawning the child process.
    #[error("io: {0}")]
    Io(String),
    /// Catch-all unexpected error.
    #[error("other: {0}")]
    Other(String),
}

/// Request to spawn a terminal in a given directory running a given command.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_core::adapters::terminal_spawner::SpawnRequest;
/// let r = SpawnRequest { cwd: PathBuf::from("/tmp"), cmd: "ls -la".into() };
/// assert_eq!(r.cwd.to_string_lossy(), "/tmp");
/// ```
#[derive(Clone, Debug)]
pub struct SpawnRequest {
    /// Working directory the spawned terminal cd's into.
    pub cwd: PathBuf,
    /// Shell command line to run (already-rendered; the spawner does not interpolate).
    pub cmd: String,
}

/// Spawn a detached external terminal window. Returns immediately after spawn.
///
/// # Object safety
///
/// All methods take `&self`; `Box<dyn TerminalSpawner>` works.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_core::adapters::terminal_spawner::{SpawnRequest, SpawnerError, TerminalSpawner};
///
/// struct Noop;
/// impl TerminalSpawner for Noop {
///     fn spawn(&self, _req: SpawnRequest) -> Result<(), SpawnerError> { Ok(()) }
///     fn name(&self) -> &'static str { "noop" }
/// }
///
/// let s: Box<dyn TerminalSpawner> = Box::new(Noop);
/// s.spawn(SpawnRequest { cwd: PathBuf::from("/tmp"), cmd: "ls".into() }).unwrap();
/// assert_eq!(s.name(), "noop");
/// ```
pub trait TerminalSpawner: Send + Sync {
    /// Launch a detached external terminal window. Returns immediately after spawn.
    fn spawn(&self, req: SpawnRequest) -> Result<(), SpawnerError>;

    /// Human-readable spawner name (e.g. "kitty", "wezterm", "iterm").
    fn name(&self) -> &'static str;
}
