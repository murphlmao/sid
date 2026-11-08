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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitCommand {
    pub argv: Vec<OsString>,
    /// Environment overrides applied on top of the inherited environment.
    pub env: BTreeMap<String, String>,
    /// Inherited variables to remove (they would redirect git at another repo/config).
    pub env_remove: Vec<String>,
}

/// Inherited variables that repoint git's repo, object store, config or helper
/// binaries; none may reach the spawned `git`.
pub const SCRUBBED_ENV: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_EXEC_PATH",
];

/// The user's own (global/system) settings that the hardened argv re-adds, because the
/// repo-local config is the untrusted one.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserGit {
    /// `(config key, value)` for every global/system `credential.*helper`, in order.
    pub credential_helpers: Vec<(String, String)>,
    /// The global `core.sshCommand`, if any.
    pub ssh_command: Option<String>,
}

/// The non-interactive environment. `existing_ssh` is the caller's own
/// `GIT_SSH_COMMAND`, if any (kept, made batch-mode); else `global_ssh` (the user's
/// global `core.sshCommand`); else plain `ssh`. Setting `GIT_SSH_COMMAND` always also
/// overrides a repo-local `core.sshCommand`.
pub fn noninteractive_env(
    existing_ssh: Option<&str>,
    global_ssh: Option<&str>,
) -> BTreeMap<String, String> {
    let pick = |o: Option<&str>| {
        o.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let ssh = match pick(existing_ssh).or_else(|| pick(global_ssh)) {
        Some(s) => format!("{s} -o BatchMode=yes"),
        None => "ssh -o BatchMode=yes".to_string(),
    };
    BTreeMap::from([
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("GIT_SSH_COMMAND".into(), ssh),
        ("GCM_INTERACTIVE".into(), "never".into()),
        ("LC_ALL".into(), "C".into()),
        // Overrides every `protocol.*.allow` in config: no `ext::`, no `file`.
        ("GIT_ALLOW_PROTOCOL".into(), "ssh:https".into()),
        ("GIT_ASKPASS".into(), String::new()),
        ("SSH_ASKPASS".into(), String::new()),
        ("GIT_PROXY_COMMAND".into(), String::new()),
    ])
}

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// The `-c key=value` pairs that neutralise repo-configured command execution.
fn hardening_config(user: &UserGit) -> Vec<String> {
    let mut v = vec![
        format!("core.hooksPath={NULL_DEVICE}"),
        "core.fsmonitor=false".to_string(),
        "core.askPass=".to_string(),
        // Reset every inherited helper (incl. repo-local `!cmd` ones), then re-add the
        // user's own global/system ones.
        "credential.helper=".to_string(),
    ];
    v.extend(
        user.credential_helpers
            .iter()
            .map(|(k, val)| format!("{k}={val}")),
    );
    v.extend(
        [
            "submodule.recurse=false",
            "push.gpgSign=false",
            "gc.auto=0",
            "maintenance.auto=false",
        ]
        .map(String::from),
    );
    v
}

fn base(workdir: &Path, user: &UserGit, args: &[&str]) -> GitCommand {
    let mut argv: Vec<OsString> = vec!["-C".into(), workdir.as_os_str().to_owned()];
    for kv in hardening_config(user) {
        argv.push("-c".into());
        argv.push(kv.into());
    }
    argv.extend(args.iter().map(OsString::from));
    GitCommand {
        argv,
        env: noninteractive_env(
            std::env::var("GIT_SSH_COMMAND").ok().as_deref(),
            user.ssh_command.as_deref(),
        ),
        env_remove: SCRUBBED_ENV.map(String::from).to_vec(),
    }
}

/// Read the user's global/system credential helpers and `core.sshCommand` once. The
/// discovery itself runs with the hardened environment, outside any repository.
pub fn user_git() -> &'static UserGit {
    static USER: std::sync::OnceLock<UserGit> = std::sync::OnceLock::new();
    USER.get_or_init(discover_user_git)
}

fn discover_user_git() -> UserGit {
    let mut user = UserGit::default();
    let cwd = std::env::temp_dir();
    let q = |scope: &str, args: &[&str]| -> Option<String> {
        let mut c = Command::new("git");
        c.arg("config")
            .arg(scope)
            .args(args)
            .current_dir(&cwd)
            .envs(noninteractive_env(None, None))
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        for k in SCRUBBED_ENV {
            c.env_remove(k);
        }
        let o = c.output().ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    for scope in ["--system", "--global"] {
        if let Some(text) = q(scope, &["--get-regexp", r"^credential\..*helper$"]) {
            user.credential_helpers.extend(parse_helpers(&text));
        }
    }
    // Global wins over system for a single-valued key.
    for scope in ["--global", "--system"] {
        if let Some(t) = q(scope, &["--get", "core.sshCommand"]) {
            let t = t.trim();
            if !t.is_empty() {
                user.ssh_command = Some(t.to_string());
                break;
            }
        }
    }
    user
}

/// Parse `git config --get-regexp` output (`key value` per line).
fn parse_helpers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_end();
            let (k, v) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            (!k.is_empty()).then(|| (k.to_string(), v.trim_start().to_string()))
        })
        .collect()
}

/// `git -C <workdir> <hardening> fetch … -- <remote>`
pub fn fetch_command(workdir: &Path, remote: &str) -> GitCommand {
    fetch_command_with(workdir, remote, user_git())
}

/// [`fetch_command`] with explicit user settings (testable).
pub fn fetch_command_with(workdir: &Path, remote: &str, user: &UserGit) -> GitCommand {
    let mut c = base(
        workdir,
        user,
        &[
            "fetch",
            "--no-progress",
            "--no-recurse-submodules",
            "--no-auto-maintenance",
            "--upload-pack=git-upload-pack",
            "--",
        ],
    );
    c.argv.push(remote.into());
    c
}

/// `git -C <workdir> <hardening> push … -- <remote> <refspec>`
pub fn push_command(workdir: &Path, remote: &str, refspec: &str) -> GitCommand {
    push_command_with(workdir, remote, refspec, user_git())
}

/// [`push_command`] with explicit user settings (testable).
pub fn push_command_with(
    workdir: &Path,
    remote: &str,
    refspec: &str,
    user: &UserGit,
) -> GitCommand {
    let mut c = base(
        workdir,
        user,
        &[
            "push",
            "--no-progress",
            "--no-verify",
            "--no-recurse-submodules",
            "--no-signed",
            "--receive-pack=git-receive-pack",
            "--",
        ],
    );
    c.argv.push(remote.into());
    c.argv.push(refspec.into());
    c
}

/// A `scheme::address` remote-helper URL (`ext::sh -c …`, `fd::`, …).
fn is_helper_url(u: &str) -> bool {
    match u.split_once("::") {
        Some((h, _)) => !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        None => false,
    }
}

/// Whether `url` needs the `git` CLI (and so the hardened spawn): only `ssh://`,
/// `https://` and scp-style `[user@]host:path`. Remote helpers (`ext::`), `file://`,
/// local paths, plain `http://` and `git://` are never CLI.
///
/// # Examples
///
/// ```
/// use sid_git::cli::is_network_url;
/// assert!(is_network_url("git@github.com:murphlmao/sid.git"));
/// assert!(is_network_url("https://example.com/x.git"));
/// assert!(is_network_url("ssh://host/x"));
/// assert!(!is_network_url("http://example.com/x.git"));
/// assert!(!is_network_url("ext::sh -c id"));
/// assert!(!is_network_url("/srv/git/x.git"));
/// assert!(!is_network_url("file:///srv/git/x.git"));
/// assert!(!is_network_url("C:\\repos\\x"));
/// assert!(!is_network_url("../sibling"));
/// ```
pub fn is_network_url(url: &str) -> bool {
    let u = url.trim();
    if is_helper_url(u) || u.starts_with('-') {
        return false;
    }
    if let Some((scheme, _)) = u.split_once("://") {
        return scheme.eq_ignore_ascii_case("ssh") || scheme.eq_ignore_ascii_case("https");
    }
    // scp-like `[user@]host:path` — a colon before any slash, not a drive letter.
    match u.find(':') {
        Some(i) if i > 1 => !u[..i].contains('/') && !u[..i].contains('\\'),
        _ => false,
    }
}

/// Why sid refuses to talk to `url` at all, if it does (never reaches libgit2 or the CLI).
pub fn refusal_reason(url: &str) -> Option<String> {
    let u = url.trim();
    if is_helper_url(u) {
        return Some("remote-helper URLs (ext::, fd::, …) are not supported".into());
    }
    let scheme = u.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("http") => Some("plain http remotes are not supported — use https".into()),
        Some("git") => Some("the unauthenticated git:// protocol is not supported".into()),
        _ => None,
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

type Reader = std::sync::mpsc::Receiver<Vec<u8>>;

fn drain<R: Read + Send + 'static>(mut r: R, cap: usize) -> Reader {
    let (tx, rx) = std::sync::mpsc::channel();
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
        let _ = tx.send(kept);
    });
    rx
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
    for k in &cmd.env_remove {
        c.env_remove(k);
    }
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
    // A grandchild (ssh control master, a hook) can keep the pipes open after git
    // exits: wait for the readers only within the deadline, then kill the group.
    let grace = if timed_out {
        Duration::from_secs(2)
    } else {
        timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_secs(2))
    };
    let mut leaked = false;
    let mut join = |h: Option<Reader>| match h {
        Some(rx) => match rx.recv_timeout(grace) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(_) => {
                leaked = true;
                String::new()
            }
        },
        None => String::new(),
    };
    let stdout = join(out);
    let stderr = join(err);
    if leaked {
        kill_group(&mut child);
    }
    Ok(RunOutput {
        success: status.is_some_and(|s| s.success()),
        timed_out,
        stdout,
        stderr,
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
    let clean = sanitize_remote_text(&out.stderr, STDERR_DISPLAY_CAP);
    let text = clean.trim();
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

/// Longest server-written text that may reach a toast.
pub const STDERR_DISPLAY_CAP: usize = 600;

/// Characters that must never render as-is: controls (terminal escapes, CR overwrites)
/// and the Unicode bidi controls that reorder display.
pub fn is_deceptive_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// Server-written text made safe for display: deceptive characters dropped (newlines
/// kept, tabs become spaces) and the result capped to `max` chars.
pub fn sanitize_remote_text(s: &str, max: usize) -> String {
    let mut out: String = s
        .chars()
        .filter_map(|c| match c {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            c if is_deceptive_char(c) => None,
            c => Some(c),
        })
        .take(max)
        .collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn remote_refs(repo: &git2::Repository, remote: &str) -> BTreeMap<String, git2::Oid> {
    let mut m = BTreeMap::new();
    if let Ok(refs) = repo.references_glob(&format!("refs/remotes/{remote}/*")) {
        for r in refs.flatten() {
            if let (Some(n), Some(t)) = (r.shorthand().ok(), r.target()) {
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
        Ok(h) if h.is_branch() => match h.shorthand().ok() {
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
    let url = repo
        .find_remote(&remote)
        .ok()
        .and_then(|r| r.url().ok().map(String::from))
        .unwrap_or_default();
    if let Some(why) = refusal_reason(&url) {
        return Err(GitError::Other(format!("refusing to push '{remote}': {why}")));
    }
    if !is_network_url(&url) {
        // Local / file remotes: libgit2 (no hooks, no `receivepack` command from config).
        return push_local(&repo, &remote, &branch, &merge, &refspec);
    }
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

/// Push to a local-path remote through libgit2.
fn push_local(
    repo: &git2::Repository,
    remote: &str,
    branch: &str,
    merge: &str,
    refspec: &str,
) -> Result<PushReport, GitError> {
    let mut r = repo.find_remote(remote).map_err(map_git2_error)?;
    let up_to_date = {
        let local = repo
            .find_reference(&format!("refs/heads/{branch}"))
            .ok()
            .and_then(|x| x.target());
        let tracking = repo
            .find_reference(&format!("refs/remotes/{remote}/{branch}"))
            .ok()
            .and_then(|x| x.target());
        local.is_some() && local == tracking
    };
    let rejected = std::cell::RefCell::new(None::<String>);
    let mut cb = git2::RemoteCallbacks::new();
    cb.push_update_reference(|name, status| {
        if let Some(msg) = status {
            *rejected.borrow_mut() = Some(format!("{name}: {msg}"));
        }
        Ok(())
    });
    let res = {
        let mut opts = git2::PushOptions::new();
        opts.remote_callbacks(cb);
        r.push(&[refspec], Some(&mut opts))
    };
    if let Some(msg) = rejected.take() {
        return Err(GitError::Conflict(format!(
            "the remote rejected the push ({msg}) — fetch and integrate before pushing"
        )));
    }
    res.map_err(|e| {
        let m = e.message().to_ascii_lowercase();
        if m.contains("non-fast-forward") || m.contains("not fast") || m.contains("not present locally") {
            GitError::Conflict(
                "the remote has newer commits — fetch and integrate them before pushing".into(),
            )
        } else {
            map_git2_error(e)
        }
    })?;
    Ok(PushReport {
        remote: remote.to_string(),
        branch: branch.to_string(),
        upstream_ref: merge.to_string(),
        up_to_date,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_is_noninteractive() {
        let e = noninteractive_env(None, None);
        assert_eq!(e["GIT_TERMINAL_PROMPT"], "0");
        assert_eq!(e["GIT_SSH_COMMAND"], "ssh -o BatchMode=yes");
        assert_eq!(e["GIT_ALLOW_PROTOCOL"], "ssh:https");
        for k in ["GIT_ASKPASS", "SSH_ASKPASS", "GIT_PROXY_COMMAND"] {
            assert_eq!(e[k], "", "{k}");
        }
    }

    #[test]
    fn existing_ssh_command_is_kept_and_batched() {
        let e = noninteractive_env(Some("ssh -i /k"), Some("ssh -i /global"));
        assert_eq!(e["GIT_SSH_COMMAND"], "ssh -i /k -o BatchMode=yes");
        assert_eq!(
            noninteractive_env(Some("  "), None)["GIT_SSH_COMMAND"],
            "ssh -o BatchMode=yes"
        );
        assert_eq!(
            noninteractive_env(None, Some("ssh -i /global"))["GIT_SSH_COMMAND"],
            "ssh -i /global -o BatchMode=yes"
        );
    }

    #[test]
    fn argv_is_exact_and_names_follow_double_dash() {
        let user = UserGit {
            credential_helpers: vec![
                ("credential.helper".into(), "store".into()),
                ("credential.https://h.example.helper".into(), "cache".into()),
            ],
            ssh_command: None,
        };
        let c = fetch_command_with(Path::new("/r"), "--upload-pack=evil", &user);
        let a: Vec<_> = c.argv.iter().map(|s| s.to_str().unwrap()).collect();
        let dd = a.iter().position(|x| *x == "--").unwrap();
        assert_eq!(a[dd + 1..], ["--upload-pack=evil"]);
        assert_eq!(a[..2], ["-C", "/r"]);
        let joined = a.join(" ");
        for want in [
            "-c core.hooksPath=",
            "-c core.fsmonitor=false",
            "-c core.askPass=",
            "-c credential.helper= -c credential.helper=store -c credential.https://h.example.helper=cache",
            "-c submodule.recurse=false",
            "-c push.gpgSign=false",
            "-c gc.auto=0",
            "-c maintenance.auto=false",
            "fetch --no-progress --no-recurse-submodules --no-auto-maintenance --upload-pack=git-upload-pack --",
        ] {
            assert!(joined.contains(want), "missing {want}: {joined}");
        }
        for k in SCRUBBED_ENV {
            assert!(c.env_remove.iter().any(|x| x == k), "{k}");
        }
        let p = push_command_with(
            Path::new("/r"),
            "origin",
            "refs/heads/m:refs/heads/m",
            &UserGit::default(),
        );
        let a: Vec<_> = p.argv.iter().map(|s| s.to_str().unwrap()).collect();
        let dd = a.iter().position(|x| *x == "--").unwrap();
        assert_eq!(a[dd + 1..], ["origin", "refs/heads/m:refs/heads/m"]);
        assert_eq!(
            a[dd - 6..dd - 1],
            [
                "push",
                "--no-progress",
                "--no-verify",
                "--no-recurse-submodules",
                "--no-signed"
            ]
        );
        assert_eq!(a[dd - 1], "--receive-pack=git-receive-pack");
    }

    #[test]
    fn helpers_parse() {
        let v = parse_helpers("credential.helper store --file x\ncredential.https://a.b.helper\n");
        assert_eq!(v[0], ("credential.helper".into(), "store --file x".into()));
        assert_eq!(v[1], ("credential.https://a.b.helper".into(), "".into()));
    }

    #[test]
    fn hostile_urls_are_not_cli() {
        for u in [
            "ext::sh -c touch /tmp/x",
            "fd::3",
            "http://h/x",
            "git://h/x",
            "file:///x",
            "--upload-pack=x",
        ] {
            assert!(!is_network_url(u), "{u}");
        }
        assert!(refusal_reason("ext::sh -c id").is_some());
        assert!(refusal_reason("http://h/x").is_some());
        assert!(refusal_reason("/srv/x").is_none());
        assert!(refusal_reason("git@h:x").is_none());
    }

    #[test]
    fn remote_text_is_sanitized_and_capped() {
        let s = sanitize_remote_text("a\u{1b}[31mb\u{202E}c\rd\tz\nq", 100);
        assert_eq!(s, "a[31mbcd z\nq");
        let long = "x".repeat(1000);
        assert_eq!(sanitize_remote_text(&long, 10).chars().count(), 11);
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
            ..Default::default()
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
            ..Default::default()
        };
        let o = run("sh", &cmd, Duration::from_secs(10), 1000).unwrap();
        assert!(o.success);
        assert_eq!(o.stdout.len(), 1000);
    }
}
