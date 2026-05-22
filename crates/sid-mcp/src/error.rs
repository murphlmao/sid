//! Error type for sid-mcp.

use std::path::PathBuf;

use thiserror::Error;

/// Errors emitted by sid-mcp tool implementations and by the server's
/// init / run lifecycle.
///
/// All variants are recoverable in the sense that the MCP server keeps
/// running — they translate to error tool results, not process exits.
/// Only `Init` and `Run` (returned from [`crate::run_stdio`]) are fatal,
/// and only because they mean the JSON-RPC transport itself failed.
#[derive(Debug, Error)]
pub enum SidMcpError {
    /// Workspace member crate not found in `Cargo.toml`.
    #[error("crate `{0}` is not a member of the sid workspace")]
    UnknownCrate(String),

    /// A required file or directory was missing.
    #[error("missing path: {0}")]
    MissingPath(PathBuf),

    /// Filesystem error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Subprocess (cargo / git) failed.
    #[error("subprocess `{cmd}` exited {code}: {stderr}")]
    Subprocess {
        /// The shell command that was executed.
        cmd: String,
        /// Exit code returned by the subprocess.
        code: i32,
        /// Captured stderr (trimmed).
        stderr: String,
    },

    /// JSON parse / serialize error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// TOML parse error from the manifest.
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

    /// Server failed to initialize the MCP transport.
    #[error("mcp init: {0}")]
    Init(String),

    /// Server failed during the run loop.
    #[error("mcp run: {0}")]
    Run(String),
}

impl SidMcpError {
    /// Wrap an MCP init error.
    pub fn from_init<E: std::fmt::Display>(e: E) -> Self {
        Self::Init(e.to_string())
    }

    /// Wrap an MCP runtime error.
    pub fn from_run<E: std::fmt::Display>(e: E) -> Self {
        Self::Run(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_crate_display() {
        let e = SidMcpError::UnknownCrate("sid-bogus".into());
        assert!(e.to_string().contains("sid-bogus"));
    }

    #[test]
    fn subprocess_display_includes_code_and_stderr() {
        let e = SidMcpError::Subprocess {
            cmd: "cargo test".into(),
            code: 101,
            stderr: "panic in lib.rs:42".into(),
        };
        let s = e.to_string();
        assert!(s.contains("101"));
        assert!(s.contains("panic in lib.rs:42"));
    }

    #[test]
    fn from_init_and_run_wrap_display() {
        let init = SidMcpError::from_init("transport refused");
        assert!(init.to_string().contains("transport refused"));
        let run = SidMcpError::from_run("disconnected");
        assert!(run.to_string().contains("disconnected"));
    }
}
