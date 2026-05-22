//! `criterion_compare` tool — compare criterion bench results vs baseline.

use std::path::Path;

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::SidMcpError;

/// One bench result.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BenchComparison {
    /// Bench name (criterion target path).
    pub name: String,
    /// Mean time (ns) for the baseline run.
    pub baseline_ns: f64,
    /// Mean time (ns) for the latest run.
    pub latest_ns: f64,
    /// Percentage change (`(latest - baseline) / baseline * 100`).
    pub delta_pct: f32,
    /// True if `delta_pct >= threshold_pct`.
    pub regressed: bool,
}

/// Top-level result.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CriterionResult {
    /// Per-bench comparisons. Empty if no benchmarks have been run.
    pub benches: Vec<BenchComparison>,
    /// Threshold percentage used for the regression flag.
    pub threshold_pct: f32,
    /// Number of regressions (`benches[i].regressed == true`).
    pub regression_count: usize,
}

/// JSON shape criterion writes for each bench under
/// `target/criterion/<bench>/{base,new}/estimates.json`.
#[derive(Debug, Deserialize)]
struct Estimates {
    mean: Mean,
}
#[derive(Debug, Deserialize)]
struct Mean {
    point_estimate: f64,
}

/// Implementation entry point.
pub async fn run(
    workspace_root: &Path,
    crate_name: Option<&str>,
    threshold_pct: f32,
) -> Result<CriterionResult, SidMcpError> {
    let criterion_dir = workspace_root.join("target/criterion");
    if !criterion_dir.exists() {
        return Ok(CriterionResult {
            benches: Vec::new(),
            threshold_pct,
            regression_count: 0,
        });
    }

    if let Some(c) = crate_name {
        if !workspace_root
            .join("crates")
            .join(c)
            .join("Cargo.toml")
            .exists()
        {
            return Err(SidMcpError::UnknownCrate(c.to_string()));
        }
    }

    let mut benches = Vec::new();
    for entry in WalkDir::new(&criterion_dir).into_iter().flatten() {
        if !entry.file_type().is_dir() {
            continue;
        }
        let base = entry.path().join("base/estimates.json");
        let new = entry.path().join("new/estimates.json");
        if !(base.exists() && new.exists()) {
            continue;
        }
        // The bench name is the path relative to `target/criterion/`.
        let name = entry
            .path()
            .strip_prefix(&criterion_dir)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();
        // crate scope: criterion's path doesn't include the crate name,
        // so we can't filter precisely here without cargo-bench metadata.
        // Filtering by name prefix is a heuristic the caller can refine.
        if let Some(c) = crate_name {
            if !name.starts_with(c) {
                continue;
            }
        }

        let baseline_ns = read_mean_ns(&base).await?;
        let latest_ns = read_mean_ns(&new).await?;
        let delta = if baseline_ns == 0.0 {
            0.0
        } else {
            (((latest_ns - baseline_ns) / baseline_ns) * 100.0) as f32
        };
        benches.push(BenchComparison {
            name,
            baseline_ns,
            latest_ns,
            delta_pct: delta,
            regressed: delta >= threshold_pct,
        });
    }

    let regression_count = benches.iter().filter(|b| b.regressed).count();
    Ok(CriterionResult {
        benches,
        threshold_pct,
        regression_count,
    })
}

async fn read_mean_ns(path: &Path) -> Result<f64, SidMcpError> {
    let body = tokio::fs::read_to_string(path).await?;
    let est: Estimates = serde_json::from_str(&body)?;
    Ok(est.mean.point_estimate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_criterion_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let r = run(tmp.path(), None, 10.0).await.unwrap();
        assert!(r.benches.is_empty());
        assert_eq!(r.regression_count, 0);
    }

    #[tokio::test]
    async fn unknown_crate_errors() {
        let tmp = tempfile::tempdir().unwrap();
        // Need criterion dir to exist for the function to reach the
        // crate-validity check.
        std::fs::create_dir_all(tmp.path().join("target/criterion")).unwrap();
        let err = run(tmp.path(), Some("no-such"), 10.0).await.unwrap_err();
        assert!(matches!(err, SidMcpError::UnknownCrate(_)));
    }

    #[tokio::test]
    async fn detects_regression() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let bench_dir = root.join("target/criterion/example_bench");
        std::fs::create_dir_all(bench_dir.join("base")).unwrap();
        std::fs::create_dir_all(bench_dir.join("new")).unwrap();
        std::fs::write(
            bench_dir.join("base/estimates.json"),
            r#"{"mean":{"point_estimate":100.0}}"#,
        )
        .unwrap();
        std::fs::write(
            bench_dir.join("new/estimates.json"),
            r#"{"mean":{"point_estimate":120.0}}"#,
        )
        .unwrap();

        let r = run(root, None, 10.0).await.unwrap();
        assert_eq!(r.benches.len(), 1);
        let b = &r.benches[0];
        // 20% regression — well over 10% threshold.
        assert!(b.regressed);
        assert!((b.delta_pct - 20.0).abs() < 0.01);
        assert_eq!(r.regression_count, 1);
    }

    #[tokio::test]
    async fn ignores_improvement() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let bench_dir = root.join("target/criterion/faster_bench");
        std::fs::create_dir_all(bench_dir.join("base")).unwrap();
        std::fs::create_dir_all(bench_dir.join("new")).unwrap();
        std::fs::write(
            bench_dir.join("base/estimates.json"),
            r#"{"mean":{"point_estimate":100.0}}"#,
        )
        .unwrap();
        std::fs::write(
            bench_dir.join("new/estimates.json"),
            r#"{"mean":{"point_estimate":80.0}}"#,
        )
        .unwrap();

        let r = run(root, None, 10.0).await.unwrap();
        // -20% improvement; not a regression.
        assert!(!r.benches[0].regressed);
        assert_eq!(r.regression_count, 0);
    }
}
