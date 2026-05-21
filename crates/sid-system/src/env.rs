//! Environment resolution helpers for the System tab.

use sid_core::adapters::terminal_spawner::SpawnerError;

/// Resolve the user's editor: `$EDITOR` if set, else `$VISUAL`, else `vi`
/// if it exists in `$PATH`, else [`SpawnerError::EditorMissing`].
///
/// # Examples
///
/// ```no_run
/// // The exact value depends on the environment.
/// let _ = sid_system::env::resolve_editor();
/// ```
pub fn resolve_editor() -> Result<String, SpawnerError> {
    if let Ok(e) = std::env::var("EDITOR") {
        if !e.trim().is_empty() {
            return Ok(e);
        }
    }
    if let Ok(v) = std::env::var("VISUAL") {
        if !v.trim().is_empty() {
            return Ok(v);
        }
    }
    if which::which("vi").is_ok() {
        return Ok("vi".to_string());
    }
    Err(SpawnerError::EditorMissing)
}
