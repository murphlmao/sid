//! `recent_commits` tool — read recent git commits, optionally scoped to a crate.

use std::path::Path;

use serde::Serialize;
use tokio::process::Command;

use crate::error::SidMcpError;

/// One commit summary.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Commit {
    /// Short SHA (7 chars).
    pub sha: String,
    /// Author name.
    pub author: String,
    /// Unix timestamp (seconds since epoch).
    pub unix: u64,
    /// First line of the commit message.
    pub subject: String,
}

/// Implementation entry point.
pub async fn run(
    workspace_root: &Path,
    crate_name: Option<&str>,
    count: u32,
) -> Result<Vec<Commit>, SidMcpError> {
    if let Some(c) = crate_name {
        if !workspace_root.join("crates").join(c).join("Cargo.toml").exists() {
            return Err(SidMcpError::UnknownCrate(c.to_string()));
        }
    }
    let count = count.clamp(1, 200);

    let mut args: Vec<String> = vec![
        "log".to_string(),
        "--pretty=format:%h%x09%an%x09%at%x09%s".to_string(),
        format!("-n{count}"),
    ];
    if let Some(c) = crate_name {
        args.push("--".to_string());
        args.push(format!("crates/{c}"));
    }

    let output = Command::new("git")
        .args(&args)
        .current_dir(workspace_root)
        .output()
        .await?;
    if !output.status.success() {
        return Err(SidMcpError::Subprocess {
            cmd: format!("git {}", args.join(" ")),
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let mut commits = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() != 4 {
            continue;
        }
        commits.push(Commit {
            sha: parts[0].to_string(),
            author: parts[1].to_string(),
            unix: parts[2].parse().unwrap_or(0),
            subject: parts[3].to_string(),
        });
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_crate_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run(tmp.path(), Some("no-such"), 5).await.unwrap_err();
        assert!(matches!(err, SidMcpError::UnknownCrate(_)));
    }

    #[tokio::test]
    async fn reads_real_repo_commits() {
        // Use the real workspace this crate lives in; if git isn't there
        // (e.g., source extracted from a tarball) the test is skipped.
        let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = crate_dir.parent().unwrap().parent().unwrap();
        if !root.join(".git").exists() {
            return;
        }
        let commits = run(root, None, 3).await.unwrap();
        assert!(!commits.is_empty(), "expected at least one commit");
        // boundary: sha looks like a hex string of expected length.
        let s = &commits[0].sha;
        assert!(s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
