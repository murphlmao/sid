//! Network git operations (fetch, push) by spawning the `git` CLI.
//!
//! `git2` is built without ssh/https transports (deliberately), so anything that
//! leaves the machine shells out to `git`: that reuses the user's `~/.ssh/config`,
//! ssh-agent and credential helpers for free. The spawn is hardened:
//!
//! - argv only, never a shell; user-controlled names come after a `--`;
//! - non-interactive: `GIT_TERMINAL_PROMPT=0`, ssh in `BatchMode`, stdin closed;
//! - bounded: a wall-clock timeout, after which the whole process group is killed
//!   (git forks `ssh`/helpers, killing only git would leave them running);
//! - output captured but capped, so a chatty remote cannot balloon memory.
//!
//! Local/file remotes keep using libgit2 (see [`is_network_url`]).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sid_core::git::{FetchReport, GitError, PushReport};

use crate::{ahead_behind, map_git2_error};

/// Upper bound for one network operation.
pub const CLI_TIMEOUT: Duration = Duration::from_secs(90);
/// Bytes of stdout / stderr kept (the rest is drained and dropped).
pub const OUTPUT_CAP: usize = 64 * 1024;

/// A fully-specified `git` invocation. Pure data so argv/env construction is
/// unit-testable without spawning anything.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommand {
    pub argv: Vec<OsString>,
    /// Environment overrides applied on top of the inherited environment.
    pub env: BTreeMap<String, String>,
}

/// The non-interactive environment. `existing_ssh` is the caller's own
/// `GIT_SSH_COMMAND`, if any: it is kept and made batch-mode rather than replaced.
pub fn noninteractive_env(existing_ssh: Option<&str>) -> BTreeMap<String, String> {
    let ssh = match existing_ssh.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => format!("{s} -o BatchMode=yes"),
        None => "ssh -o BatchMode=yes".to_string(),
    };
    BTreeMap::from([
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("GIT_SSH_COMMAND".into(), ssh),
        ("GCM_INTERACTIVE".into(), "never".into()),
        ("LC_ALL".into(), "C".into()),
    ])
}

fn base(workdir: &Path, args: &[&str]) -> GitCommand {
    let mut argv: Vec<OsString> = vec!["-C".into(), workdir.as_os_str().to_owned()];
    argv.extend(args.iter().map(OsString::from));
    GitCommand {
        argv,
        env: noninteractive_env(std::env::var("GIT_SSH_COMMAND").ok().as_deref()),
    }
}

/// `git -C <workdir> fetch --no-progress -- <remote>`
pub fn fetch_command(workdir: &Path, remote: &str) -> GitCommand {
    let mut c = base(workdir, &["fetch", "--no-progress", "--"]);
    c.argv.push(remote.into());
    c
}

/// `git -C <workdir> push --no-progress -- <remote> <refspec>`
pub fn push_command(workdir: &Path, remote: &str, refspec: &str) -> GitCommand {
    let mut c = base(workdir, &["push", "--no-progress", "--"]);
    c.argv.push(remote.into());
    c.argv.push(refspec.into());
    c
}

/// Whether `url` needs a network transport (and so the CLI) rather than libgit2.
/// Local paths, `file://` URLs and Windows drive paths are not network.
///
/// # Examples
///
/// ```
/// use sid_git::cli::is_network_url;
/// assert!(is_network_url("git@github.com:murphlmao/sid.git"));
/// assert!(is_network_url("https://example.com/x.git"));
/// assert!(is_network_url("ssh://host/x"));
/// assert!(!is_network_url("/srv/git/x.git"));
/// assert!(!is_network_url("file:///srv/git/x.git"));
/// assert!(!is_network_url("C:\\repos\\x"));
/// assert!(!is_network_url("../sibling"));
/// ```
pub fn is_network_url(url: &str) -> bool {
    let u = url.trim();
    if let Some((scheme, _)) = u.split_once("://") {
        return !scheme.eq_ignore_ascii_case("file");
    }
    // scp-like `[user@]host:path` — a colon before any slash, not a drive letter.
    match u.find(':') {
        Some(i) if i > 1 => !u[..i].contains('/') && !u[..i].contains('\\'),
        _ => false,
    }
}

/// Captured result of a bounded run.
#[derive(Debug)]
pub struct RunOutput {
    pub success: bool,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

fn drain<R: Read + Send + 'static>(mut r: R, cap: usize) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(kept.len());
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
            }
        }
        kept
    })
}

/// Run `program` with `cmd`, bounded by `timeout`; each output stream capped at `cap`.
pub fn run(
    program: &str,
    cmd: &GitCommand,
    timeout: Duration,
    cap: usize,
) -> Result<RunOutput, GitError> {
    let mut c = Command::new(program);
    c.args(&cmd.argv)
        .envs(&cmd.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    let mut child = c.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::Other("the `git` command was not found on PATH".into())
        } else {
            GitError::Other(format!("could not start git: {e}"))
        }
    })?;
    let out = child.stdout.take().map(|s| drain(s, cap));
    let err = child.stderr.take().map(|s| drain(s, cap));

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if started.elapsed() >= timeout => {
                timed_out = true;
                kill_group(&mut child);
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(GitError::Other(format!("waiting for git: {e}"))),
        }
    };
    let join = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
        h.and_then(|h| h.join().ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    Ok(RunOutput {
        success: status.is_some_and(|s| s.success()),
        timed_out,
        stdout: join(out),
        stderr: join(err),
    })
}

fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // Negative pid = the whole group we created with `process_group(0)`.
        let pid = child.id() as i32;
        // SAFETY: plain syscall on a pid we spawned; no memory is involved.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

/// Turn a failed run into the most specific [`GitError`].
pub fn classify_failure(what: &str, out: &RunOutput) -> GitError {
    if out.timed_out {
        return GitError::TimedOut(what.to_string());
    }
    let text = out.stderr.trim();
    let lower = text.to_ascii_lowercase();
    const AUTH: [&str; 7] = [
        "permission denied",
        "authentication failed",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "host key verification failed",
        "publickey",
    ];
    if AUTH.iter().any(|m| lower.contains(m)) {
        return GitError::AuthFailed(last_lines(text, 3));
    }
    if lower.contains("non-fast-forward")
        || lower.contains("[rejected]")
        || lower.contains("fetch first")
    {
        return GitError::Conflict(
            "the remote has newer commits — fetch and integrate them before pushing".into(),
        );
    }
    GitError::Other(if text.is_empty() {
        format!("{what} failed")
    } else {
        last_lines(text, 4)
    })
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn remote_refs(repo: &git2::Repository, remote: &str) -> BTreeMap<String, git2::Oid> {
    let mut m = BTreeMap::new();
    if let Ok(refs) = repo.references_glob(&format!("refs/remotes/{remote}/*")) {
        for r in refs.flatten() {
            if let (Some(n), Some(t)) = (r.shorthand(), r.target()) {
                m.insert(n.to_string(), t);
            }
        }
    }
    m
}

/// Fetch `remote` into the repo at `workdir` via the CLI.
pub fn fetch(workdir: &Path, remote: &str, timeout: Duration) -> Result<FetchReport, GitError> {
    let repo = git2::Repository::open(workdir).map_err(map_git2_error)?;
    let before = remote_refs(&repo, remote);
    let out = run("git", &fetch_command(workdir, remote), timeout, OUTPUT_CAP)?;
    if !out.success {
        return Err(classify_failure(&format!("fetching '{remote}'"), &out));
    }
    // Re-open: the CLI wrote refs behind libgit2's back.
    let repo = git2::Repository::open(workdir).map_err(map_git2_error)?;
    let after = remote_refs(&repo, remote);
    let updated_refs = after
        .iter()
        .filter(|(n, oid)| before.get(*n) != Some(*oid))
        .map(|(n, _)| n.clone())
        .collect();
    let (ahead, behind) = match repo.head() {
        Ok(h) if h.is_branch() => match h.shorthand() {
            Some(n) => ahead_behind(&repo, n)?,
            None => (None, None),
        },
        _ => (None, None),
    };
    Ok(FetchReport {
        remote: remote.to_string(),
        // The CLI does not report transfer stats (we run it with --no-progress).
        received_objects: 0,
        received_bytes: 0,
        updated_refs,
        ahead,
        behind,
    })
}

/// Push the current branch to its configured upstream via the CLI.
pub fn push(workdir: &Path, timeout: Duration) -> Result<PushReport, GitError> {
    let repo = git2::Repository::open(workdir).map_err(map_git2_error)?;
    let head = repo.head().map_err(map_git2_error)?;
    if !head.is_branch() {
        return Err(GitError::Other(
            "HEAD is detached — check out a branch to push".into(),
        ));
    }
    let branch = head.shorthand().unwrap_or_default().to_string();
    let cfg = repo.config().map_err(map_git2_error)?;
    let no_upstream = || GitError::Other(format!("'{branch}' has no upstream to push to"));
    let remote = cfg
        .get_string(&format!("branch.{branch}.remote"))
        .map_err(|_| no_upstream())?;
    let merge = cfg
        .get_string(&format!("branch.{branch}.merge"))
        .map_err(|_| no_upstream())?;
    if repo.find_remote(&remote).is_err() {
        return Err(GitError::RemoteNotFound(remote));
    }
    let refspec = format!("refs/heads/{branch}:{merge}");
    let out = run(
        "git",
        &push_command(workdir, &remote, &refspec),
        timeout,
        OUTPUT_CAP,
    )?;
    if !out.success {
        return Err(classify_failure(&format!("pushing '{branch}'"), &out));
    }
    let combined = format!("{}\n{}", out.stdout.trim(), out.stderr.trim());
    Ok(PushReport {
        remote,
        branch,
        upstream_ref: merge,
        up_to_date: combined.to_ascii_lowercase().contains("up-to-date"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_is_noninteractive() {
        let e = noninteractive_env(None);
        assert_eq!(e["GIT_TERMINAL_PROMPT"], "0");
        assert_eq!(e["GIT_SSH_COMMAND"], "ssh -o BatchMode=yes");
    }

    #[test]
    fn existing_ssh_command_is_kept_and_batched() {
        let e = noninteractive_env(Some("ssh -i /k"));
        assert_eq!(e["GIT_SSH_COMMAND"], "ssh -i /k -o BatchMode=yes");
        assert_eq!(
            noninteractive_env(Some("  "))["GIT_SSH_COMMAND"],
            "ssh -o BatchMode=yes"
        );
    }

    #[test]
    fn argv_is_exact_and_names_follow_double_dash() {
        let c = fetch_command(Path::new("/r"), "--upload-pack=evil");
        let a: Vec<_> = c.argv.iter().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(
            a,
            [
                "-C",
                "/r",
                "fetch",
                "--no-progress",
                "--",
                "--upload-pack=evil"
            ]
        );
        let p = push_command(Path::new("/r"), "origin", "refs/heads/m:refs/heads/m");
        let a: Vec<_> = p.argv.iter().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(a[2..5], ["push", "--no-progress", "--"]);
        assert_eq!(a[5..], ["origin", "refs/heads/m:refs/heads/m"]);
    }

    #[test]
    fn url_classification() {
        assert!(is_network_url("user@host:repo.git"));
        assert!(!is_network_url("/tmp/a:b"));
        assert!(!is_network_url("C:/x"));
    }

    fn out(stderr: &str, timed_out: bool) -> RunOutput {
        RunOutput {
            success: false,
            timed_out,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn failures_classify() {
        assert!(matches!(
            classify_failure("x", &out("", true)),
            GitError::TimedOut(_)
        ));
        assert!(matches!(
            classify_failure("x", &out("git@h: Permission denied (publickey).", false)),
            GitError::AuthFailed(_)
        ));
        assert!(matches!(
            classify_failure("x", &out("! [rejected] a -> a (non-fast-forward)", false)),
            GitError::Conflict(_)
        ));
        assert!(matches!(
            classify_failure("x", &out("fatal: boom", false)),
            GitError::Other(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_the_process_group() {
        let cmd = GitCommand {
            argv: vec!["-c".into(), "sleep 30 & wait".into()],
            env: BTreeMap::new(),
        };
        let t = Instant::now();
        let o = run("sh", &cmd, Duration::from_millis(200), 1024).unwrap();
        assert!(o.timed_out && !o.success);
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn output_is_capped() {
        let cmd = GitCommand {
            argv: vec!["-c".into(), "head -c 100000 /dev/zero | tr '\\0' a".into()],
            env: BTreeMap::new(),
        };
        let o = run("sh", &cmd, Duration::from_secs(10), 1000).unwrap();
        assert!(o.success);
        assert_eq!(o.stdout.len(), 1000);
    }
}
