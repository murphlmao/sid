//! The `sudo` invocation: what to run (pure, fully tested) and how to run it (glue).
//!
//! # Why `sudo -S` and not `pkexec`
//!
//! `pkexec` is the other obvious mechanism on a Linux desktop, and it is the wrong one
//! here for a reason that is about product behaviour, not taste: **it insists on owning
//! the prompt**. `pkexec` delegates authentication to whatever polkit agent the session
//! runs, which draws its own dialog, in its own style, on its own schedule — and cannot
//! accept a password from the calling program at all. sid would have to hand the user
//! off to a foreign window mid-edit, and on a session with no polkit agent running it
//! would simply fail with nothing to show. `sid-svcctl` already refuses to let systemd
//! spawn an interactive authentication agent for exactly this reason (`--no-ask-password`
//! in its module doc): sid owns its window, and an external prompt stealing focus is a
//! bug.
//!
//! `sudo -S` reads the secret from standard input, which is the only mechanism that lets
//! sid's own modal collect the password, hold it in memory for the operation, and drop
//! it. It is also the mechanism a CLI-adjacent developer tool is *expected* to use.
//!
//! Every flag is load-bearing:
//!
//! - `-S` — read the password from stdin. Without it sudo wants a tty and there is none.
//! - `-k` — ignore any cached authentication timestamp. This is a security property, not
//!   a convenience: without it, a valid timestamp left by an unrelated terminal would
//!   make *any* password succeed, so sid's "unlock" would be verifying nothing and its
//!   "wrong password" error could never fire. sid also never silently inherits privilege
//!   another program was granted.
//! - `-p ''` — empty prompt, so stderr carries the *result* and nothing else. The
//!   classifier's input stays clean.
//! - `--` — end of sudo's own options, so a path can never be read as one. The domain's
//!   `guard_path` independently requires an absolute path; these two defenses do not
//!   depend on each other.
//!
//! `-n` (non-interactive) is deliberately absent: it makes sudo refuse to read the
//! password at all, which is the one thing this module exists to do.
//!
//! # Why the new content does not travel on stdin
//!
//! The tempting shape for the write path is one invocation with the password on the
//! first line of stdin and the file content after it. It is unsafe: `sudo` reads its
//! password from the shared descriptor with its own buffering, and any over-read past
//! the newline is content the child never sees — silent truncation of the file being
//! saved. On `/etc/fstab` that is an unbootable machine. So stdin carries the secret and
//! nothing else, and the content reaches the elevated helper through a private 0600
//! staging file that root reads. Config-file content is not a secret; the password is,
//! and it is the one thing that never touches disk.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sid_core::privfs::{Passphrase, PrivError};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use zeroize::Zeroizing;

use crate::classify::{classify_spawn_error, classify_stderr};

/// How long an elevation attempt may take before it is killed. Generous enough for a
/// PAM stack that talks to the network or imposes a failed-attempt delay, short enough
/// that a wedged authenticator surfaces as an error instead of a hung modal.
const ELEVATION_TIMEOUT: Duration = Duration::from_secs(30);

/// The atomic, permission-preserving replace, run by the elevated shell.
///
/// This is a **constant**: no caller data is ever interpolated into it. The destination
/// and the staging file arrive as positional parameters (`$1`, `$2`) — argv, not text —
/// so a path containing spaces, quotes, `$`, or a leading `-` is inert. Every expansion
/// is quoted, and every command that takes a path gets `--` first.
///
/// What the steps buy, in order:
/// - `umask 077` — the staging copy is 0600 from the instant it exists, so a secret
///   file's new content is never briefly world-readable under a temp name.
/// - `tmp` beside the destination — same directory means same filesystem, so the final
///   `mv` is a rename: atomic, never a copy a crash could truncate.
/// - `trap` — a failure at any later step removes the temp instead of leaving litter in
///   `/etc`.
/// - `chmod`/`chown --reference` — the destination's own mode and ownership are copied
///   onto the replacement, so a 0600 root:root file comes back 0600 root:root.
/// - `mv -fT` — the rename that publishes the new content in one step. `-T`
///   (`--no-target-directory`) is not decoration: without it, a destination that is a
///   *directory* makes `mv` quietly move the temp file *inside* it and report success,
///   so a save would appear to work while writing nothing. With `-T` that case fails and
///   the trap cleans up.
///
/// GNU coreutils' `--reference` is assumed (CLAUDE.md: Wayland/Linux now). A symlinked
/// destination is replaced by a regular file, matching what the unprivileged save path
/// already does — one behaviour, not two.
const REPLACE_SCRIPT: &str = "\
set -eu
umask 077
dst=\"$1\"
src=\"$2\"
tmp=\"$dst.sid-privfs-tmp.$$\"
trap 'rm -f -- \"$tmp\"' EXIT
cat -- \"$src\" >\"$tmp\"
chmod --reference=\"$dst\" -- \"$tmp\"
chown --reference=\"$dst\" -- \"$tmp\"
mv -fT -- \"$tmp\" \"$dst\"
";

/// `$0` for the elevated shell — what shows up in `ps` and in sudo's audit log while the
/// replace runs. Named for the operation so an administrator reading the log can tell
/// what asked for root.
const REPLACE_ARGV0: &str = "sid-privfs-replace";

/// One fully-decided `sudo` call: the argument vector, and the bytes to feed its stdin.
///
/// Splitting the decision from the execution is what makes the security invariant
/// *testable* rather than merely intended — a test can assert that the secret appears in
/// `stdin` and in no element of `args`.
pub(crate) struct SudoInvocation {
    /// Arguments after the `sudo` program name. **Never contains the secret** — argv is
    /// world-readable through `/proc/<pid>/cmdline`.
    pub(crate) args: Vec<OsString>,
    /// The secret, newline-terminated: exactly one line, so sudo reads it and then hits
    /// EOF. Zeroized when the invocation drops.
    pub(crate) stdin: Zeroizing<Vec<u8>>,
}

/// The flags every invocation carries — see the module doc for why each one is
/// load-bearing.
fn sudo_flags() -> Vec<OsString> {
    ["-S", "-k", "-p", "", "--"]
        .iter()
        .map(OsString::from)
        .collect()
}

/// The one place a [`Passphrase`] becomes bytes on a wire. Newline-terminated and
/// nothing more: no trailing content, so sudo's read of one line is followed by EOF.
fn stdin_payload(secret: &Passphrase) -> Zeroizing<Vec<u8>> {
    let mut payload = Vec::with_capacity(secret.expose().len() + 1);
    payload.extend_from_slice(secret.expose().as_bytes());
    payload.push(b'\n');
    Zeroizing::new(payload)
}

/// Read `path` as root: `sudo -S -k -p '' -- cat -- <path>`.
///
/// `cat` gets its own `--` so a path is never mistaken for one of *its* options either.
pub(crate) fn read_invocation(path: &Path, secret: &Passphrase) -> SudoInvocation {
    let mut args = sudo_flags();
    args.push(OsString::from("cat"));
    args.push(OsString::from("--"));
    args.push(path.as_os_str().to_os_string());
    SudoInvocation {
        args,
        stdin: stdin_payload(secret),
    }
}

/// Replace `dst`'s content with `staging`'s, as root, atomically and preserving mode and
/// ownership: `sudo -S -k -p '' -- sh -c <REPLACE_SCRIPT> sid-privfs-replace <dst>
/// <staging>`.
pub(crate) fn write_invocation(dst: &Path, staging: &Path, secret: &Passphrase) -> SudoInvocation {
    let mut args = sudo_flags();
    args.push(OsString::from("sh"));
    args.push(OsString::from("-c"));
    args.push(OsString::from(REPLACE_SCRIPT));
    args.push(OsString::from(REPLACE_ARGV0));
    args.push(dst.as_os_str().to_os_string());
    args.push(staging.as_os_str().to_os_string());
    SudoInvocation {
        args,
        stdin: stdin_payload(secret),
    }
}

/// Run an invocation to completion and return its stdout, or a classified error.
///
/// Deadlock note (the hazard the `sid-gpu` probe documents for its own subprocesses):
/// the secret is a few dozen bytes, so `write_all` completes into the pipe buffer without
/// the child having read anything, and stdin is then closed — the child can never block
/// waiting for more input. Only after that does this drain stdout and stderr, both at
/// once, via `wait_with_output`. A file larger than a pipe buffer therefore cannot wedge
/// the exchange in either direction.
///
/// On timeout the `wait_with_output` future is dropped; `kill_on_drop` reaps the child
/// rather than leaving an orphaned authenticator holding a tty.
async fn run(
    program: &OsStr,
    invocation: SudoInvocation,
    fallback: &str,
    timeout: Duration,
) -> Result<Vec<u8>, PrivError> {
    let mut child = Command::new(program)
        .args(&invocation.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| classify_spawn_error(&e))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PrivError::Unavailable("sudo stdin was not piped".into()))?;
    // Feed the secret once, then close the pipe: EOF is what stops sudo asking again.
    let fed = stdin.write_all(&invocation.stdin).await;
    let flushed = stdin.flush().await;
    drop(stdin);
    if let Err(e) = fed.and(flushed) {
        return Err(PrivError::Unavailable(format!(
            "could not hand the password to sudo: {e}"
        )));
    }

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_elapsed) => return Err(PrivError::Timeout),
        Ok(Err(e)) => return Err(PrivError::Unavailable(format!("waiting on sudo: {e}"))),
        Ok(Ok(output)) => output,
    };

    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(classify_stderr(
            &String::from_utf8_lossy(&output.stderr),
            fallback,
        ))
    }
}

/// The authenticator to execute.
///
/// Release builds always run `sudo`, resolved from `PATH` — there is no way to redirect
/// where sid sends a password. Debug builds honour `SID_SUDO_PROGRAM` so the capture
/// harness can drive the whole unlock flow against a scripted mock (see
/// `scripts/mock-sudo`); shipping that seam would be a phishing vector, so it is
/// compiled out of a release binary entirely.
fn sudo_program() -> OsString {
    #[cfg(debug_assertions)]
    if let Some(over) = std::env::var_os("SID_SUDO_PROGRAM") {
        return over;
    }
    OsString::from("sudo")
}

/// Read `path` with elevated privileges. See [`read_invocation`].
pub(crate) async fn read(path: &Path, secret: &Passphrase) -> Result<Vec<u8>, PrivError> {
    read_with(&sudo_program(), path, secret, ELEVATION_TIMEOUT).await
}

/// [`read`] against a named authenticator — the seam the protocol tests drive with a
/// scripted mock instead of the real `sudo`.
async fn read_with(
    program: &OsStr,
    path: &Path,
    secret: &Passphrase,
    timeout: Duration,
) -> Result<Vec<u8>, PrivError> {
    let fallback = format!("reading {} as root", path.display());
    run(program, read_invocation(path, secret), &fallback, timeout).await
}

/// Replace `path`'s content with `bytes` with elevated privileges — atomic, mode and
/// ownership preserved. See [`write_invocation`] and the module doc for why the content
/// is staged in a file rather than piped.
pub(crate) async fn write(path: &Path, bytes: &[u8], secret: &Passphrase) -> Result<(), PrivError> {
    write_with(&sudo_program(), path, bytes, secret, ELEVATION_TIMEOUT).await
}

/// [`write`] against a named authenticator — the seam the protocol tests drive with a
/// scripted mock instead of the real `sudo`.
async fn write_with(
    program: &OsStr,
    path: &Path,
    bytes: &[u8],
    secret: &Passphrase,
    timeout: Duration,
) -> Result<(), PrivError> {
    // A private 0600 file in the user's own temp dir, removed when `staging` drops —
    // including on every error path below.
    let staging = stage_content(bytes)?;
    let fallback = format!("writing {} as root", path.display());
    run(
        program,
        write_invocation(path, staging.path(), secret),
        &fallback,
        timeout,
    )
    .await?;
    Ok(())
}

/// Stage `bytes` in a private temp file for the elevated helper to read. `tempfile`
/// creates it 0600 and unlinks it on drop.
fn stage_content(bytes: &[u8]) -> Result<tempfile::NamedTempFile, PrivError> {
    use std::io::Write as _;
    let mut file = tempfile::Builder::new()
        .prefix("sid-privfs-stage-")
        .tempfile()
        .map_err(|e| PrivError::Io(format!("staging the new content: {e}")))?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|e| PrivError::Io(format!("staging the new content: {e}")))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "hunter2";

    fn secret() -> Passphrase {
        Passphrase::new(SECRET.to_string())
    }

    fn args_of(inv: &SudoInvocation) -> Vec<String> {
        inv.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    // ---- the secret's only channel ----------------------------------------------

    #[test]
    fn the_secret_is_one_newline_terminated_line() {
        let inv = read_invocation(Path::new("/etc/fstab"), &secret());
        assert_eq!(inv.stdin.as_slice(), b"hunter2\n");
    }

    #[test]
    fn the_secret_never_appears_in_a_read_argument_vector() {
        // argv is world-readable through /proc/<pid>/cmdline. This is the invariant.
        let inv = read_invocation(Path::new("/etc/fstab"), &secret());
        for arg in args_of(&inv) {
            assert!(
                !arg.contains(SECRET),
                "the password leaked into argv: {arg:?}"
            );
        }
    }

    #[test]
    fn the_secret_never_appears_in_a_write_argument_vector() {
        let inv = write_invocation(Path::new("/etc/fstab"), Path::new("/tmp/stage"), &secret());
        for arg in args_of(&inv) {
            assert!(
                !arg.contains(SECRET),
                "the password leaked into argv: {arg:?}"
            );
        }
    }

    #[test]
    fn an_empty_secret_still_terminates_the_line() {
        // sudo must reach EOF after one line even when the user submitted nothing, or it
        // waits forever instead of reporting "no password was provided".
        let inv = read_invocation(Path::new("/etc/fstab"), &Passphrase::new(String::new()));
        assert_eq!(inv.stdin.as_slice(), b"\n");
    }

    // ---- the read invocation ------------------------------------------------------

    #[test]
    fn a_read_asks_sudo_for_stdin_auth_a_fresh_timestamp_and_a_silent_prompt() {
        let inv = read_invocation(Path::new("/etc/fstab"), &secret());
        let args = args_of(&inv);
        assert_eq!(
            args,
            vec!["-S", "-k", "-p", "", "--", "cat", "--", "/etc/fstab"]
        );
    }

    #[test]
    fn a_read_puts_a_double_dash_before_the_path_for_both_programs() {
        // Two independent guards: sudo's own `--`, and `cat`'s. Neither program can read
        // the path as an option even if the domain guard were ever relaxed.
        let args = args_of(&read_invocation(Path::new("/etc/fstab"), &secret()));
        let cat = args.iter().position(|a| a == "cat").expect("cat in argv");
        let path = args
            .iter()
            .position(|a| a == "/etc/fstab")
            .expect("path in argv");
        assert_eq!(args[cat - 1], "--", "sudo's options must be terminated");
        assert_eq!(args[path - 1], "--", "cat's options must be terminated");
    }

    #[test]
    fn a_read_passes_an_awkward_path_as_one_intact_argument() {
        let inv = read_invocation(Path::new("/etc/my configs/a$b'c\"d"), &secret());
        let args = args_of(&inv);
        assert_eq!(args.last().unwrap(), "/etc/my configs/a$b'c\"d");
    }

    // ---- the write invocation -----------------------------------------------------

    #[test]
    fn a_write_runs_the_constant_replace_script_with_both_paths_as_parameters() {
        let inv = write_invocation(
            Path::new("/etc/ssh/sshd_config"),
            Path::new("/tmp/sid-privfs-stage-abc"),
            &secret(),
        );
        assert_eq!(
            args_of(&inv),
            vec![
                "-S",
                "-k",
                "-p",
                "",
                "--",
                "sh",
                "-c",
                REPLACE_SCRIPT,
                REPLACE_ARGV0,
                "/etc/ssh/sshd_config",
                "/tmp/sid-privfs-stage-abc",
            ]
        );
    }

    #[test]
    fn the_replace_script_never_contains_the_paths_it_operates_on() {
        // The whole point of positional parameters: no interpolation, so no quoting bug
        // and no injection surface. A path is data, never program text.
        let dst = "/etc/'; rm -rf /; '";
        let inv = write_invocation(Path::new(dst), Path::new("/tmp/stage"), &secret());
        let script = args_of(&inv)
            .into_iter()
            .find(|a| a.contains("mv -f"))
            .expect("the script is in argv");
        assert!(
            !script.contains("rm -rf /"),
            "caller data reached the script text: {script}"
        );
        assert_eq!(script, REPLACE_SCRIPT, "the script must be the constant");
    }

    #[test]
    fn a_write_passes_an_awkward_destination_as_one_intact_argument() {
        let dst = "/etc/my configs/a$b'c\"d";
        let inv = write_invocation(Path::new(dst), Path::new("/tmp/stage"), &secret());
        let args = args_of(&inv);
        assert!(
            args.contains(&dst.to_string()),
            "destination mangled: {args:?}"
        );
    }

    // ---- the replace script's load-bearing steps ---------------------------------

    #[test]
    fn the_replace_script_keeps_every_guarantee_it_promises() {
        // A regression pin, not a tautology: each of these lines is the whole reason the
        // privileged save is as safe as the unprivileged one. Dropping any of them
        // silently downgrades a save on /etc — losing a file's mode, leaving a
        // world-readable temp behind, or turning an atomic rename into a truncation.
        for required in [
            "set -eu",                     // no step is skipped after a failure
            "umask 077",                   // the staging copy is never world-readable
            "$dst.sid-privfs-tmp.$$",      // the temp is beside the destination: rename, not copy
            "trap 'rm -f -- \"$tmp\"'",    // a failure leaves no litter in /etc
            "chmod --reference=\"$dst\"",  // the mode survives
            "chown --reference=\"$dst\"",  // the owner survives
            "mv -fT -- \"$tmp\" \"$dst\"", // publication is one atomic step, never into a directory
        ] {
            assert!(
                REPLACE_SCRIPT.contains(required),
                "the replace script lost `{required}`"
            );
        }
    }

    #[test]
    fn the_replace_script_takes_its_paths_only_from_positional_parameters() {
        assert!(REPLACE_SCRIPT.contains("dst=\"$1\""));
        assert!(REPLACE_SCRIPT.contains("src=\"$2\""));
    }

    // ---- staging -------------------------------------------------------------------

    #[test]
    fn staged_content_round_trips_and_is_private_to_this_user() {
        let staged = stage_content(b"PermitRootLogin no\n").unwrap();
        let read_back = std::fs::read(staged.path()).unwrap();
        assert_eq!(read_back, b"PermitRootLogin no\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(staged.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "staging file must not be readable by others"
            );
        }
    }

    #[test]
    fn staging_an_empty_buffer_is_a_legitimate_truncation_to_nothing() {
        let staged = stage_content(b"").unwrap();
        assert_eq!(std::fs::read(staged.path()).unwrap(), b"");
    }

    // ---- the protocol, end to end, against a scripted authenticator ---------------
    //
    // These drive the REAL `run`/`REPLACE_SCRIPT` machinery — the subprocess, the
    // one-line stdin feed, the EOF, the exit-code branch, and the atomic replace — with
    // `scripts/mock-sudo` standing in for sudo(8). It grants no privilege, so everything
    // below happens on files this test user already owns; what is being proven is the
    // protocol and the script, not the escalation. The escalation itself is the one part
    // that still needs a human at a real machine (see the crate doc).

    /// A quick timeout: no test should ever wait on the production 30s budget.
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    fn mock_sudo() -> OsString {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/mock-sudo")
            .into()
    }

    /// A wrapper that forces one of `mock-sudo`'s failure verdicts. Written as a script
    /// rather than set on this process's environment: `set_var` is unsafe in edition 2024
    /// and would race every other test in this binary.
    fn mock_sudo_with(dir: &Path, verdict: &str) -> OsString {
        let path = dir.join(format!("mock-sudo-{verdict}"));
        let mock = mock_sudo();
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nexport MOCK_SUDO_VERDICT={verdict}\nexec {} \"$@\"\n",
                Path::new(&mock).display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.into()
    }

    fn good_secret() -> Passphrase {
        Passphrase::new("correct".to_string())
    }

    #[tokio::test]
    async fn an_accepted_password_reads_the_file_back_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "UUID=abc / ext4 defaults 0 1\n").unwrap();

        let got = read_with(&mock_sudo(), &path, &good_secret(), TEST_TIMEOUT)
            .await
            .unwrap();

        assert_eq!(got, b"UUID=abc / ext4 defaults 0 1\n");
    }

    #[tokio::test]
    async fn a_rejected_password_is_an_auth_failure_and_yields_no_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shadow");
        std::fs::write(&path, "root:$6$secret\n").unwrap();

        let err = read_with(
            &mock_sudo(),
            &path,
            &Passphrase::new("wrong".into()),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, PrivError::AuthFailed), "got {err:?}");
        assert!(err.is_retryable(), "a mistyped password must re-prompt");
    }

    #[tokio::test]
    async fn a_user_who_is_not_a_sudoer_is_told_so_and_never_re_prompted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "x\n").unwrap();
        let program = mock_sudo_with(dir.path(), "notsudoer");

        let err = read_with(&program, &path, &good_secret(), TEST_TIMEOUT)
            .await
            .unwrap_err();

        assert!(matches!(err, PrivError::NotPermitted(_)), "got {err:?}");
        assert!(!err.is_retryable(), "no password can fix this");
    }

    #[tokio::test]
    async fn a_wedged_authenticator_times_out_instead_of_hanging_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "x\n").unwrap();
        let program = mock_sudo_with(dir.path(), "hang");

        let started = std::time::Instant::now();
        let err = read_with(&program, &path, &good_secret(), Duration::from_millis(300))
            .await
            .unwrap_err();

        assert!(matches!(err, PrivError::Timeout), "got {err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the timeout must fire, not the full budget"
        );
    }

    #[tokio::test]
    async fn a_missing_authenticator_is_unavailable_not_an_auth_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "x\n").unwrap();

        let err = read_with(
            &OsString::from(dir.path().join("no-such-sudo")),
            &path,
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, PrivError::Unavailable(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_missing_file_fails_on_the_inner_command_not_the_password() {
        let dir = tempfile::tempdir().unwrap();

        let err = read_with(
            &mock_sudo(),
            &dir.path().join("nope"),
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, PrivError::Io(_)), "got {err:?}");
        assert!(!err.is_retryable(), "the password was fine");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_replaces_the_content_and_keeps_the_files_mode() {
        // The whole point of the replace script: /etc/ssh/sshd_config must come back
        // with the mode it had, not with whatever the writer's umask would have given it.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sshd_config");
        std::fs::write(&path, "PermitRootLogin no\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        write_with(
            &mock_sudo(),
            &path,
            b"PermitRootLogin yes\n",
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "PermitRootLogin yes\n"
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640, "the mode must survive the replace");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_preserves_even_a_mode_that_forbids_writing() {
        // A 0444 file is exactly the case that sent the editor down this path. Replacing
        // it must not quietly make it writable.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly.conf");
        std::fs::write(&path, "old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

        write_with(&mock_sudo(), &path, b"new\n", &good_secret(), TEST_TIMEOUT)
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o444);
    }

    #[tokio::test]
    async fn a_rejected_password_leaves_the_target_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "original\n").unwrap();

        let err = write_with(
            &mock_sudo(),
            &path,
            b"vandalized\n",
            &Passphrase::new("wrong".into()),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, PrivError::AuthFailed), "got {err:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
    }

    #[tokio::test]
    async fn a_filename_full_of_shell_metacharacters_round_trips_intact() {
        // The injection test. If any of this were interpolated into the script instead of
        // arriving as a positional parameter, this would delete files or write elsewhere.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a b$c'd\"e;f`g|h.conf");
        std::fs::write(&path, "before\n").unwrap();

        write_with(
            &mock_sudo(),
            &path,
            b"after\n",
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap();
        let got = read_with(&mock_sudo(), &path, &good_secret(), TEST_TIMEOUT)
            .await
            .unwrap();

        assert_eq!(got, b"after\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
    }

    #[tokio::test]
    async fn a_replace_that_fails_after_staging_removes_its_own_temp_file() {
        // The trap, exercised for real: the destination is a directory, so the script
        // gets all the way to `mv` — the temp file exists by then — and only then fails.
        // Without the trap, a half-finished save would litter /etc with
        // `.sid-privfs-tmp.*` files nobody ever cleans up.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("fstab");
        std::fs::create_dir(&target).unwrap();

        let err = write_with(
            &mock_sudo(),
            &target,
            b"new\n",
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, PrivError::Io(_)), "got {err:?}");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("sid-privfs-tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "left temp files behind: {leftovers:?}"
        );
    }

    #[tokio::test]
    async fn a_rejected_password_never_even_stages_a_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "original\n").unwrap();

        let _ = write_with(
            &mock_sudo(),
            &path,
            b"new\n",
            &Passphrase::new("wrong".into()),
            TEST_TIMEOUT,
        )
        .await;

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("sid-privfs-tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "left temp files behind: {leftovers:?}"
        );
    }

    #[tokio::test]
    async fn a_write_to_an_unwritable_directory_reports_io_and_leaves_no_litter() {
        // The replace fails at the staging-copy step (the directory denies creation), so
        // the trap is what has to clean up — and the original must survive.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let closed = dir.path().join("closed");
            std::fs::create_dir(&closed).unwrap();
            let path = closed.join("fstab");
            std::fs::write(&path, "original\n").unwrap();
            std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o500)).unwrap();

            let result =
                write_with(&mock_sudo(), &path, b"new\n", &good_secret(), TEST_TIMEOUT).await;

            let entries: Vec<_> = std::fs::read_dir(&closed)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();

            // Root can write into a 0500 directory, so a test run as root legitimately
            // succeeds; what must never happen is a leftover temp file.
            if let Err(e) = result {
                assert!(matches!(e, PrivError::Io(_)), "got {e:?}");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
            }
            assert_eq!(
                entries,
                vec!["fstab".to_string()],
                "a failed replace must leave the directory as it found it"
            );
        }
    }
}
