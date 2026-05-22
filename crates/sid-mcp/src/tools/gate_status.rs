//! `gate_status` tool — read the last cached gate status from disk.

use std::path::Path;

use serde::Serialize;

use crate::error::SidMcpError;

/// Status of one gate at the last cached run.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GateOutcome {
    /// Gate name (`"test"`, `"clippy"`, `"fmt"`, `"deny"`).
    pub name: String,
    /// `"PASS"`, `"FAIL"`, `"WARN"`, or `"SKIPPED"`.
    pub result: String,
    /// Truncated stderr/stdout excerpt (first 20 lines).
    pub excerpt: Option<String>,
    /// Path to the full log under `target/gate-logs/`.
    pub log_path: Option<String>,
}

/// Top-level gate-status result.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GateStatus {
    /// Scope as run (`"workspace"` or a crate name).
    pub scope: String,
    /// Per-gate outcomes. Empty if no gate has been run for this scope.
    pub gates: Vec<GateOutcome>,
    /// Unix timestamp (seconds) of the latest gate log read.
    pub last_run_unix: Option<u64>,
    /// Overall readiness: true iff every gate is PASS.
    pub ready: bool,
}

/// Implementation entry point.
///
/// Reads `target/gate-logs/*.log` and synthesises a structured view. If no
/// logs exist for the scope, returns an empty `gates: []` with `ready: false`
/// — the caller (e.g., `/sid-gate` skill) should run the gate first.
///
/// NOTE: this is a best-effort *read* of cached state. The canonical "run the
/// gate" entry point is the `/sid-gate` skill in the `sid-testing` plugin.
pub async fn run(workspace_root: &Path, crate_name: Option<&str>) -> Result<GateStatus, SidMcpError> {
    let log_dir = workspace_root.join("target/gate-logs");
    let scope = crate_name.unwrap_or("workspace").to_string();

    if !log_dir.exists() {
        return Ok(GateStatus {
            scope,
            gates: Vec::new(),
            last_run_unix: None,
            ready: false,
        });
    }

    // Look for logs named `<scope>-<gate>-<timestamp>.log` and group by gate.
    // Take the freshest for each gate.
    let mut latest_per_gate: std::collections::BTreeMap<String, (u64, std::path::PathBuf)> =
        std::collections::BTreeMap::new();
    let mut latest_unix: Option<u64> = None;

    let mut entries = tokio::fs::read_dir(&log_dir).await?;
    while let Some(e) = entries.next_entry().await? {
        let p = e.path();
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Expected: `<scope>-<gate>-<unix>.log`
        let parts: Vec<&str> = stem.rsplitn(3, '-').collect();
        if parts.len() != 3 {
            continue;
        }
        let unix: u64 = parts[0].parse().unwrap_or(0);
        let gate = parts[1].to_string();
        let log_scope = parts[2].to_string();
        if log_scope != scope {
            continue;
        }
        let prev = latest_per_gate.get(&gate).map(|(u, _)| *u).unwrap_or(0);
        if unix > prev {
            latest_per_gate.insert(gate, (unix, p));
        }
        if latest_unix.map(|u| unix > u).unwrap_or(true) {
            latest_unix = Some(unix);
        }
    }

    let mut gates = Vec::new();
    for (name, (_, path)) in &latest_per_gate {
        let body = tokio::fs::read_to_string(path).await.unwrap_or_default();
        let result = classify_gate_log(name, &body);
        let excerpt: String = body.lines().take(20).collect::<Vec<_>>().join("\n");
        let log_path = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        gates.push(GateOutcome {
            name: name.clone(),
            result,
            excerpt: Some(excerpt),
            log_path: Some(log_path),
        });
    }

    let ready = !gates.is_empty() && gates.iter().all(|g| g.result == "PASS");

    Ok(GateStatus {
        scope,
        gates,
        last_run_unix: latest_unix,
        ready,
    })
}

/// Best-effort classification of a gate's stdout/stderr log.
fn classify_gate_log(gate: &str, body: &str) -> String {
    let lower = body.to_lowercase();
    match gate {
        "fmt" => {
            if body.is_empty() {
                "PASS".into()
            } else if lower.contains("diff") {
                "FAIL".into()
            } else {
                "PASS".into()
            }
        }
        "clippy" => {
            if lower.contains("error:") || lower.contains("error[e") {
                "FAIL".into()
            } else if lower.contains("warning:") {
                "WARN".into()
            } else {
                "PASS".into()
            }
        }
        "test" => {
            if lower.contains("test result: failed") {
                "FAIL".into()
            } else if lower.contains("test result: ok") {
                "PASS".into()
            } else {
                "WARN".into()
            }
        }
        "deny" => {
            if lower.contains("error") {
                "FAIL".into()
            } else {
                "PASS".into()
            }
        }
        _ => "WARN".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_clippy_with_error_is_fail() {
        assert_eq!(classify_gate_log("clippy", "error: unused variable"), "FAIL");
    }
    #[test]
    fn classify_clippy_with_only_warning_is_warn() {
        assert_eq!(classify_gate_log("clippy", "warning: needless_collect"), "WARN");
    }
    #[test]
    fn classify_clippy_empty_is_pass() {
        assert_eq!(classify_gate_log("clippy", ""), "PASS");
    }
    #[test]
    fn classify_test_failed_is_fail() {
        assert_eq!(classify_gate_log("test", "test result: FAILED. 1 passed; 1 failed"), "FAIL");
    }
    #[test]
    fn classify_test_ok_is_pass() {
        assert_eq!(classify_gate_log("test", "test result: ok. 847 passed; 0 failed"), "PASS");
    }
    #[test]
    fn classify_fmt_with_diff_is_fail() {
        assert_eq!(classify_gate_log("fmt", "Diff in src/foo.rs at line 42"), "FAIL");
    }

    #[tokio::test]
    async fn no_log_dir_returns_empty_not_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let s = run(tmp.path(), None).await.unwrap();
        assert_eq!(s.scope, "workspace");
        assert!(s.gates.is_empty());
        assert!(!s.ready);
    }
}
