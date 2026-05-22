//! `plan_status` tool — parse `docs/superpowers/plans/*.md` for task completion.

use std::path::Path;

use serde::Serialize;

use crate::error::SidMcpError;

/// Per-plan task progress.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PlanProgress {
    /// File path relative to workspace root.
    pub file: String,
    /// Plan title (first `# ` heading, if any).
    pub title: Option<String>,
    /// Number of `- [x]` checked items.
    pub done: usize,
    /// Number of `- [ ]` unchecked items.
    pub pending: usize,
    /// `done / (done + pending)` as a percentage. 0.0 if no tasks.
    pub completion_pct: f32,
}

/// Implementation entry point.
pub async fn run(workspace_root: &Path) -> Result<Vec<PlanProgress>, SidMcpError> {
    let dir = workspace_root.join("docs/superpowers/plans");
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let mut entries = tokio::fs::read_dir(&dir).await?;
    while let Some(e) = entries.next_entry().await? {
        let p = e.path();
        if p.extension().map(|x| x != "md").unwrap_or(true) {
            continue;
        }
        let body = tokio::fs::read_to_string(&p).await?;
        out.push(parse_plan(&p, &body, workspace_root));
    }
    // Stable order by filename.
    out.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(out)
}

fn parse_plan(path: &Path, body: &str, workspace_root: &Path) -> PlanProgress {
    let mut done = 0usize;
    let mut pending = 0usize;
    let mut title: Option<String> = None;
    for line in body.lines() {
        let t = line.trim_start();
        if title.is_none() && t.starts_with("# ") {
            title = Some(t.trim_start_matches('#').trim().to_string());
        }
        // Match list items that are tasks.
        if let Some(rest) = t.strip_prefix("- [") {
            if let Some(c) = rest.chars().next() {
                match c {
                    'x' | 'X' => done += 1,
                    ' ' => pending += 1,
                    _ => {}
                }
            }
        }
    }
    let total = done + pending;
    let pct = if total == 0 {
        0.0
    } else {
        (done as f32 / total as f32) * 100.0
    };
    PlanProgress {
        file: path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string(),
        title,
        done,
        pending,
        completion_pct: pct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_counts_checked_and_unchecked() {
        let body = "# Plan X\n\n- [ ] One\n- [x] Two\n- [X] Three\n- [ ] Four\n";
        let p = parse_plan(Path::new("p.md"), body, Path::new(""));
        assert_eq!(p.done, 2);
        assert_eq!(p.pending, 2);
        assert_eq!(p.title.as_deref(), Some("Plan X"));
        assert!((p.completion_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn parse_no_tasks_returns_zero_pct() {
        let p = parse_plan(Path::new("p.md"), "# Just a title\n", Path::new(""));
        assert_eq!(p.done, 0);
        assert_eq!(p.pending, 0);
        assert_eq!(p.completion_pct, 0.0);
    }

    #[test]
    fn parse_handles_no_title() {
        let p = parse_plan(Path::new("p.md"), "- [x] Done\n", Path::new(""));
        assert_eq!(p.title, None);
        assert_eq!(p.done, 1);
    }

    #[tokio::test]
    async fn missing_plans_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let v = run(tmp.path()).await.unwrap();
        assert!(v.is_empty());
    }
}
