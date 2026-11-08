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

/// Per-stream capture cap; output beyond it is dropped and a marker appended.
const MAX_CAPTURE: usize = 1024 * 1024;

/// Read `r` to EOF, keeping at most [`MAX_CAPTURE`] bytes (the rest is drained and dropped).
async fn read_capped<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> String {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = MAX_CAPTURE.saturating_sub(buf.len());
                if room < n {
                    truncated = true;
                }
                buf.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
    let mut s = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        s.push_str("\n[output truncated at 1 MiB]");
    }
    s
}

/// Run `program args…` in `cwd` with a hard timeout. The child runs in its own process
/// group; on timeout the whole group is killed (so grandchildren such as sway/sid/rustc
/// are not orphaned). Captured output is capped at 1 MiB per stream.
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
    #[cfg(unix)]
    cmd.process_group(0);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot spawn `{program}`: {e}"))?;
    let pid = child.id();
    let out = child.stdout.take().map(read_capped);
    let err = child.stderr.take().map(read_capped);
    let work = async {
        let (so, se, st) = tokio::join!(
            async {
                match out {
                    Some(f) => f.await,
                    None => String::new(),
                }
            },
            async {
                match err {
                    Some(f) => f.await,
                    None => String::new(),
                }
            },
            child.wait()
        );
        (so, se, st)
    };
    match tokio::time::timeout(timeout, work).await {
        Ok((stdout, stderr, Ok(status))) => Ok(CmdOut {
            code: status.code(),
            stdout,
            stderr,
            timed_out: false,
        }),
        Ok((_, _, Err(e))) => Err(format!("`{program}` failed: {e}")),
        Err(_) => {
            kill_group(pid);
            Ok(CmdOut {
                code: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: true,
            })
        }
    }
}

/// SIGKILL the process group led by `pid` (the child was spawned with `process_group(0)`).
fn kill_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .status();
    }
    #[cfg(not(unix))]
    let _ = pid;
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

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_grandchildren_and_output_is_capped() {
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("alive");
        // Grandchild keeps touching a marker; the group kill must stop it.
        let script = format!(
            "(while true; do date +%s%N > {m}; sleep 0.1; done) & wait",
            m = marker.display()
        );
        let out = run(
            "bash",
            &["-c".into(), script],
            d.path(),
            &[],
            Duration::from_millis(600),
        )
        .await
        .unwrap();
        assert!(out.timed_out);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let a = std::fs::read_to_string(&marker).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            a,
            std::fs::read_to_string(&marker).unwrap(),
            "grandchild survived"
        );

        let big = run(
            "bash",
            &["-c".into(), "head -c 3000000 /dev/zero | tr '\\0' x".into()],
            d.path(),
            &[],
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(big.stdout.len() < 1024 * 1024 + 100);
        assert!(big.stdout.ends_with("[output truncated at 1 MiB]"));
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
