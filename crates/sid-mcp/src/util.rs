//! Shared helpers: workspace-root discovery and bounded subprocess execution.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Find the sid workspace root: `$SID_ROOT`, else the nearest ancestor of the current
/// directory whose `Cargo.toml` declares `[workspace]` and which has `crates/sid`.
pub fn find_root() -> Result<PathBuf, String> {
    if let Some(r) = std::env::var_os("SID_ROOT") {
        let p = PathBuf::from(r);
        return if is_root(&p) {
            Ok(p)
        } else {
            Err(format!(
                "SID_ROOT={} is not a sid workspace root",
                p.display()
            ))
        };
    }
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    cwd.ancestors()
        .find(|p| is_root(p))
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("no sid workspace found above {}", cwd.display()))
}

/// Whether `p` looks like the sid workspace root.
pub fn is_root(p: &Path) -> bool {
    p.join("crates/sid").is_dir()
        && std::fs::read_to_string(p.join("Cargo.toml"))
            .map(|t| t.contains("[workspace]"))
            .unwrap_or(false)
}

/// Captured result of a subprocess.
#[derive(Debug)]
pub struct CmdOut {
    /// Exit code (`None` when killed / timed out).
    pub code: Option<i32>,
    /// Captured stdout (lossy UTF-8).
    pub stdout: String,
    /// Captured stderr (lossy UTF-8).
    pub stderr: String,
    /// The command exceeded its timeout and was killed.
    pub timed_out: bool,
}

/// Run `program args…` in `cwd` with a hard timeout. The child is killed on timeout.
pub async fn run(
    program: &str,
    args: &[String],
    cwd: &Path,
    envs: &[(String, String)],
    timeout: Duration,
) -> Result<CmdOut, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("cannot spawn `{program}`: {e}"))?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => Ok(CmdOut {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            timed_out: false,
        }),
        Ok(Err(e)) => Err(format!("`{program}` failed: {e}")),
        // The future (and with it the child) is dropped here; `kill_on_drop` reaps it.
        Err(_) => Ok(CmdOut {
            code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        }),
    }
}

/// Whether `program` is found on `$PATH`.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}

/// Truncate `s` to at most `max` chars, appending an ellipsis marker.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push_str("...");
        t
    }
}

/// Every file under `dir` with extension `ext`, skipping `target/` and dot-dirs, sorted.
pub fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(e.file_type().is_dir() && (n == "target" || n.starts_with('.')))
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == ext))
        .map(|e| e.into_path())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_truncates_on_char_boundary() {
        assert_eq!(clip("abcdef", 3), "abc...");
        assert_eq!(clip("ab", 3), "ab");
        assert_eq!(clip("ééé", 2), "éé...");
    }

    #[tokio::test]
    async fn run_captures_and_times_out() {
        let cwd = std::env::temp_dir();
        let ok = run("echo", &["hi".into()], &cwd, &[], Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(ok.code, Some(0));
        assert_eq!(ok.stdout.trim(), "hi");
        let slow = run(
            "sleep",
            &["5".into()],
            &cwd,
            &[],
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        assert!(slow.timed_out);
        assert!(
            run(
                "definitely-not-a-binary",
                &[],
                &cwd,
                &[],
                Duration::from_secs(1)
            )
            .await
            .is_err()
        );
    }
}
