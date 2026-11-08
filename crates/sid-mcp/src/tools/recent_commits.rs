//! `recent_commits` — compact `git log`.

use std::path::Path;
use std::time::Duration;

const SEP: char = '\u{1f}';

/// argv for `git log`.
pub fn args(n: usize, path: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "log".into(),
        format!("-n{n}"),
        "--date=short".into(),
        format!("--format=%h{SEP}%ad{SEP}%an{SEP}%s"),
    ];
    if let Some(p) = path {
        a.push("--".into());
        a.push(p.into());
    }
    a
}

/// Render `git log` output (our `--format`) as `hash date author subject` lines.
pub fn render(raw: &str) -> String {
    let mut s = String::new();
    let mut n = 0;
    for line in raw.lines() {
        let f: Vec<&str> = line.splitn(4, SEP).collect();
        if f.len() == 4 {
            n += 1;
            s.push_str(&format!(
                "{} {} {:<10} {}\n",
                f[0],
                f[1],
                crate::util::clip(f[2], 10),
                f[3]
            ));
        }
    }
    if n == 0 {
        s.push_str("(no commits)\n");
    }
    s
}

/// Run it.
pub async fn run(root: &Path, n: usize, path: Option<&str>) -> Result<String, String> {
    let n = n.clamp(1, 200);
    let out = crate::util::run("git", &args(n, path), root, &[], Duration::from_secs(15)).await?;
    if out.code != Some(0) {
        return Err(format!("git log failed: {}", out.stderr.trim()));
    }
    Ok(render(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_put_path_after_double_dash() {
        let a = args(5, Some("crates/sid-ui"));
        assert_eq!(&a[a.len() - 2..], ["--", "crates/sid-ui"]);
        assert!(a.contains(&"-n5".to_string()));
        // A path that looks like a flag cannot be read as one.
        let a = args(1, Some("--all"));
        assert_eq!(a[a.len() - 2], "--");
    }

    #[test]
    fn render_formats_rows() {
        let raw = format!("abc123{SEP}2026-11-07{SEP}Murphy{SEP}feat: x\n");
        assert_eq!(render(&raw), "abc123 2026-11-07 Murphy     feat: x\n");
        assert_eq!(render(""), "(no commits)\n");
    }
}
