//! `coverage_summary` tool — wraps `cargo llvm-cov --summary-only`.
//!
//! This tool is the **slow one**. A cold `cargo llvm-cov` against the
//! workspace takes 30s-2min depending on test runtime. Results are
//! cached at `target/llvm-cov/sid-mcp-cache.json`; pass `fresh: true`
//! to invalidate.
//!
//! Critical-path crates (per CLAUDE.md: sid-store, sid-core, sid-job,
//! sid-secrets) are flagged in the output when they fall below 95%.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::SidMcpError;

/// Per-file coverage row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileCoverage {
    /// Workspace-relative file path.
    pub file: String,
    /// Line coverage as a percentage (0.0..=100.0).
    pub lines_pct: f32,
    /// Branch coverage as a percentage.
    pub branches_pct: Option<f32>,
}

/// Coverage summary for one crate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrateCoverage {
    /// Crate name.
    pub name: String,
    /// Overall line coverage percentage.
    pub lines_pct: f32,
    /// Overall branch coverage percentage.
    pub branches_pct: Option<f32>,
    /// Is this a 95%-critical-path crate?
    pub is_critical_path: bool,
    /// True when `is_critical_path` and `lines_pct < 95.0`.
    pub below_critical_threshold: bool,
}

/// Top-level result for the `coverage_summary` tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoverageSummary {
    /// Per-crate breakdown.
    pub crates: Vec<CrateCoverage>,
    /// Workspace-wide line coverage percentage.
    pub workspace_lines_pct: f32,
    /// True if the workspace is at or above the 80% bar.
    pub workspace_meets_threshold: bool,
    /// Was this result read from cache?
    pub from_cache: bool,
    /// Unix timestamp the underlying data was produced.
    pub generated_unix: u64,
}

const CRITICAL_PATH: &[&str] = &["sid-store", "sid-core", "sid-job", "sid-secrets"];
const CACHE_PATH: &str = "target/llvm-cov/sid-mcp-cache.json";

/// Implementation entry point.
///
/// Strategy:
/// 1. If `fresh == false` and cache exists, deserialize and return.
/// 2. Otherwise run `cargo llvm-cov --workspace --all-features --branch
///    --summary-only --json` and parse the JSON.
/// 3. Write the parsed result to the cache for next time.
/// 4. If `crate_name` is set, filter to that crate.
pub async fn run(
    workspace_root: &Path,
    crate_name: Option<&str>,
    fresh: bool,
) -> Result<CoverageSummary, SidMcpError> {
    let cache = workspace_root.join(CACHE_PATH);
    let mut summary: Option<CoverageSummary> = None;

    if !fresh && cache.exists() {
        let body = tokio::fs::read_to_string(&cache).await?;
        if let Ok(s) = serde_json::from_str::<CoverageSummary>(&body) {
            let mut s = s;
            s.from_cache = true;
            summary = Some(s);
        }
    }

    let mut summary = match summary {
        Some(s) => s,
        None => run_llvm_cov(workspace_root).await?,
    };

    if let Some(c) = crate_name {
        summary.crates.retain(|x| x.name == c);
        if summary.crates.is_empty() {
            return Err(SidMcpError::UnknownCrate(c.to_string()));
        }
    }

    Ok(summary)
}

async fn run_llvm_cov(workspace_root: &Path) -> Result<CoverageSummary, SidMcpError> {
    let output = Command::new("cargo")
        .args([
            "llvm-cov",
            "--workspace",
            "--all-features",
            "--branch",
            "--summary-only",
            "--json",
        ])
        .current_dir(workspace_root)
        .output()
        .await?;
    if !output.status.success() {
        return Err(SidMcpError::Subprocess {
            cmd: "cargo llvm-cov --workspace --branch --summary-only --json".into(),
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let summary = parse_llvm_cov_json(&output.stdout)?;
    let cache = workspace_root.join(CACHE_PATH);
    if let Some(parent) = cache.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&cache, serde_json::to_vec(&summary)?).await;
    Ok(summary)
}

fn parse_llvm_cov_json(bytes: &[u8]) -> Result<CoverageSummary, SidMcpError> {
    // llvm-cov --json emits a `Coverage` JSON shape. We only need the
    // workspace totals + per-file rows; we synthesize crate-level rollups
    // by prefix-matching `crates/<name>/`.
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let totals = v
        .pointer("/data/0/totals/lines/percent")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0) as f32;

    let mut crates_map: std::collections::BTreeMap<String, (f64, u64)> = Default::default();
    if let Some(files) = v.pointer("/data/0/files").and_then(|x| x.as_array()) {
        for f in files {
            let filename = f
                .get("filename")
                .and_then(|x| x.as_str())
                .unwrap_or_default();
            let crate_name = extract_crate(filename).unwrap_or_default();
            if crate_name.is_empty() {
                continue;
            }
            let pct = f
                .pointer("/summary/lines/percent")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0);
            let count = f
                .pointer("/summary/lines/count")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let e = crates_map.entry(crate_name).or_insert((0.0, 0));
            e.0 += pct * count as f64;
            e.1 += count;
        }
    }

    let mut crates = Vec::new();
    for (name, (weighted_pct, total)) in crates_map {
        let pct = if total == 0 {
            0.0
        } else {
            (weighted_pct / total as f64) as f32
        };
        let is_critical = CRITICAL_PATH.contains(&name.as_str());
        crates.push(CrateCoverage {
            name,
            lines_pct: pct,
            branches_pct: None,
            is_critical_path: is_critical,
            below_critical_threshold: is_critical && pct < 95.0,
        });
    }

    Ok(CoverageSummary {
        crates,
        workspace_lines_pct: totals,
        workspace_meets_threshold: totals >= 80.0,
        from_cache: false,
        generated_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    })
}

fn extract_crate(filename: &str) -> Option<String> {
    // Look for the `crates/<name>/` segment.
    let needle = "crates/";
    let idx = filename.find(needle)?;
    let rest = &filename[idx + needle.len()..];
    let end = rest.find('/')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_crate_finds_segment() {
        assert_eq!(
            extract_crate("/home/u/sid/crates/sid-core/src/tab.rs"),
            Some("sid-core".into())
        );
        // boundary: outside crates/ tree returns None.
        assert_eq!(extract_crate("/home/u/sid/src/main.rs"), None);
    }

    #[test]
    fn parse_minimal_json() {
        let body = br#"{
          "data": [{
            "totals": { "lines": { "percent": 85.0 } },
            "files": [
              {
                "filename": "/x/crates/sid-store/src/lib.rs",
                "summary": { "lines": { "percent": 92.5, "count": 100 } }
              }
            ]
          }]
        }"#;
        let s = parse_llvm_cov_json(body).unwrap();
        assert_eq!(s.workspace_lines_pct, 85.0);
        assert!(s.workspace_meets_threshold);
        assert_eq!(s.crates.len(), 1);
        let c = &s.crates[0];
        assert_eq!(c.name, "sid-store");
        assert!(c.is_critical_path);
        // Adversarial: sid-store at 92.5% is below the 95% critical-path bar.
        assert!(c.below_critical_threshold);
    }

    #[test]
    fn parse_empty_json_gives_zero_pct() {
        let body = br#"{ "data": [{ "totals": {}, "files": [] }] }"#;
        let s = parse_llvm_cov_json(body).unwrap();
        assert_eq!(s.workspace_lines_pct, 0.0);
        assert!(!s.workspace_meets_threshold);
    }
}
