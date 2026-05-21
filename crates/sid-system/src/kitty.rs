//! `KittyTerminalSpawner` — launches `kitty` in a detached child process,
//! cd'd to the requested cwd, running the requested command.

use std::path::{Path, PathBuf};
use std::process::Command;

use sid_core::adapters::terminal_spawner::{SpawnRequest, SpawnerError, TerminalSpawner};

/// Detached kitty-window spawner.
///
/// # Examples
///
/// ```no_run
/// use sid_system::KittyTerminalSpawner;
/// // Construction succeeds only when `kitty` is on PATH.
/// let _ = KittyTerminalSpawner::new();
/// ```
pub struct KittyTerminalSpawner {
    kitty_path: PathBuf,
}

impl KittyTerminalSpawner {
    /// Resolve `kitty` via `which`. Errors with
    /// [`SpawnerError::TerminalMissing`] if absent.
    pub fn new() -> Result<Self, SpawnerError> {
        let kitty_path =
            which::which("kitty").map_err(|_| SpawnerError::TerminalMissing("kitty".into()))?;
        Ok(Self { kitty_path })
    }
}

impl TerminalSpawner for KittyTerminalSpawner {
    fn name(&self) -> &'static str {
        "kitty"
    }

    fn spawn(&self, req: SpawnRequest) -> Result<(), SpawnerError> {
        // --detach backgrounds kitty.
        // --directory sets cwd of the new window.
        // The command runs through `sh -lc` so shell parsing works.
        let cmd_arg = req.cmd;
        let cwd_arg = req.cwd.to_string_lossy().into_owned();
        Command::new(&self.kitty_path)
            .args(["--detach", "--directory", &cwd_arg, "sh", "-lc", &cmd_arg])
            .spawn()
            .map(|_child| ())
            .map_err(|e| SpawnerError::Io(format!("kitty spawn: {e}")))
    }
}

/// Build a [`SpawnRequest`] for opening `file_path` in the user's `$EDITOR`,
/// with the cwd set to the file's parent directory.
///
/// If `opener_override` is `Some`, the command is used as-is (still cd'd into
/// the parent). Otherwise the command is `<editor> <file_name>` with the
/// filename shell-quoted via [`shell_words::quote`].
///
/// # Examples
///
/// ```
/// # // Make the test deterministic by setting EDITOR before invocation.
/// # unsafe { std::env::set_var("EDITOR", "vi"); }
/// use std::path::Path;
/// let r = sid_system::kitty::spawn_request_for_file(
///     Path::new("/etc/nginx/nginx.conf"),
///     None,
/// ).unwrap();
/// assert_eq!(r.cwd.to_string_lossy(), "/etc/nginx");
/// assert!(r.cmd.contains("nginx.conf"));
/// ```
pub fn spawn_request_for_file(
    file_path: &Path,
    opener_override: Option<&str>,
) -> Result<SpawnRequest, SpawnerError> {
    let cwd: PathBuf = file_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    // If the parent is the empty path (e.g. file_path = "foo"), fall back to ".".
    let cwd = if cwd.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cwd
    };
    let cmd = match opener_override {
        Some(c) => c.to_string(),
        None => {
            let editor = crate::env::resolve_editor()?;
            let name = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_else(|| file_path.to_str().unwrap_or(""));
            format!("{editor} {}", shell_words::quote(name))
        }
    };
    Ok(SpawnRequest { cwd, cmd })
}
