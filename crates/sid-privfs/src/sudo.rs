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
//!
//! # No shell, anywhere
//!
//! **Nothing in this module elevates an interpreter.** Every invocation is an argv — a
//! single `coreutils` program, its flags, `--`, then paths as their own elements. There
//! is no `sh -c`, so there is no string a path could be interpolated into, no quoting
//! rule to get right, and no second language inside the trust boundary. `ps` and the
//! sudo audit log show `cp` and `mv` on named files rather than an opaque script, and a
//! site that wants a narrow `sudoers` rule can write one for `head`/`cp`/`mv`/`rm` —
//! whereas permitting `sh -c` is permitting everything.
//!
//! The cost is that the atomic, permission-preserving replace takes **three
//! authentications instead of one**, because no single `coreutils` program does all
//! three jobs:
//!
//! ```text
//! A. fill    cp -T -- <staging> <tmp>
//! B. stamp   cp --attributes-only --preserve=all --no-preserve=timestamps -T -- <dst> <tmp>
//! C. commit  mv -f -T -- <tmp> <dst>
//! (X. discard rm -f -- <tmp>, only if something after A went wrong)
//! ```
//!
//! Each step earns its place, and together they buy exactly what the `sh -c` script this
//! replaced bought:
//!
//! - **The temp is never world-readable.** `cp` creates it from the 0600 staging file, so
//!   it is 0600 from the instant it exists — the same guarantee the old script got from
//!   `umask 077`, obtained from the source's mode instead of the shell's umask, and not
//!   dependent on what umask `sudo` hands the child.
//! - **The temp is a sibling of the destination**, so it is on the destination's own
//!   filesystem and step C is a `rename(2)`: atomic, never a copy a crash could truncate.
//! - **A failure after A removes the temp** ([`discard`]), so a half-finished save leaves
//!   no litter in `/etc`. This is the one guarantee a step sequence does not get for
//!   free the way a shell `trap` did, so it is explicit — and tested.
//! - **Mode and ownership are cloned off the destination itself**, never read and
//!   re-applied by sid. `--preserve=all` carries ACLs, xattrs and (best-effort) the
//!   SELinux context too, which `chmod --reference` + `chown --reference` did not.
//!   `--no-preserve=timestamps` is deliberate: `--preserve=all` would stamp the *old*
//!   file's mtime onto new content, and a file that lies about when it changed is a file
//!   `rsync`, `etckeeper` and every staleness check will skip.
//! - **`mv -T`** (`--no-target-directory`) is not decoration: without it a destination
//!   that is a *directory* makes `mv` quietly move the temp *inside* it and exit 0 — a
//!   save that reports success while writing nothing where the user asked. `-f` then
//!   removes any question of `mv` wanting to ask about an unwritable destination.
//!
//! ## Content before attributes
//!
//! Step A fills the temp, step B stamps the attributes on. The other order reads better
//! and is wrong: `/etc/sudoers` is `0440` and `/etc/resolv.conf` is often `0444`, so a
//! temp that inherited the destination's mode first would have to be written through a
//! file with no write bit. Real root gets away with that via `CAP_DAC_OVERRIDE`, which is
//! precisely why the bug would survive review and then break every site whose `sudoers`
//! elevates to a non-root user. Filling first works for every identity, and the mode is
//! still in place before the rename, so the destination is never briefly wrong.
//!
//! GNU coreutils is assumed (CLAUDE.md: Wayland/Linux now). A symlinked destination is
//! replaced by a regular file, matching what the unprivileged save path already does —
//! one behaviour, not two.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
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

/// How long the best-effort cleanup of a half-written save may take. Much shorter than
/// [`ELEVATION_TIMEOUT`]: the failure the caller actually cares about has already
/// happened, and a wedged authenticator must not make the *tidying up* hang the editor
/// for a second full budget.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Step A — put the new content in a fresh temp. `-T` so a temp path that somehow named
/// a directory is an error rather than a copy *into* it.
const FILL_FLAGS: &[&str] = &["-T"];

/// Step B — clone the destination's own attributes onto the temp, leaving its data
/// alone. See the module doc for why timestamps are excluded and why this is not first.
const STAMP_FLAGS: &[&str] = &[
    "--attributes-only",
    "--preserve=all",
    "--no-preserve=timestamps",
    "-T",
];

/// Step C — publish. See the module doc on why `-T` is load-bearing.
const COMMIT_FLAGS: &[&str] = &["-f", "-T"];

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

/// `sudo -S -k -p '' -- <command> <flags...> -- <operands...>`, with the secret bound for
/// stdin.
///
/// The only way this module builds an invocation, so the shape is guaranteed rather than
/// remembered: every operand is its own argv element (never text a shell would re-split),
/// and every operand list is fenced off by a `--` of the inner command's own — a second,
/// independent defence beside the domain's absolute-path guard.
pub(crate) fn invocation(
    command: &str,
    flags: &[&str],
    operands: &[&Path],
    secret: &Passphrase,
) -> SudoInvocation {
    let mut args = sudo_flags();
    args.push(OsString::from(command));
    args.extend(flags.iter().map(OsString::from));
    args.push(OsString::from("--"));
    args.extend(operands.iter().map(|p| p.as_os_str().to_os_string()));
    SudoInvocation {
        args,
        stdin: stdin_payload(secret),
    }
}

/// Read `path` as root, bounded by the child itself:
/// `sudo -S -k -p '' -- head -c <max_bytes + 1> -- <path>`.
///
/// `head -c` rather than `cat` is the read cap made real. A `cat` that is handed
/// `/proc/kcore` — or any file whose `stat` size is a lie, which is every `/proc` file —
/// streams until sid runs out of memory, and no check on the *result* can help because
/// the result is what exhausted the machine. One byte over the cap is fetched
/// deliberately: it is what lets the caller distinguish "exactly at the limit" from
/// "truncated at the limit".
pub(crate) fn read_invocation(path: &Path, ceiling: u64, secret: &Passphrase) -> SudoInvocation {
    let ceiling = ceiling.to_string();
    invocation("head", &["-c", &ceiling], &[path], secret)
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
        // sid never asks sudo to fetch the password from anywhere but the pipe below.
        // `SUDO_ASKPASS` names a program sudo will *run* to obtain one, so an inherited
        // value is a foreign binary with a claim on the user's password; removing it
        // means sid's own prompt cannot be displaced by the environment it was launched
        // from.
        .env_remove("SUDO_ASKPASS")
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

/// Read `path` with elevated privileges, refusing anything over `max_bytes`. See
/// [`read_invocation`].
pub(crate) async fn read(
    path: &Path,
    max_bytes: u64,
    secret: &Passphrase,
) -> Result<Vec<u8>, PrivError> {
    read_with(&sudo_program(), path, max_bytes, secret, ELEVATION_TIMEOUT).await
}

/// [`read`] against a named authenticator — the seam the protocol tests drive with a
/// scripted mock instead of the real `sudo`.
async fn read_with(
    program: &OsStr,
    path: &Path,
    max_bytes: u64,
    secret: &Passphrase,
    timeout: Duration,
) -> Result<Vec<u8>, PrivError> {
    // Refuse an oversized file BEFORE spending an authentication on it. A file's *size*
    // needs no privilege — only its contents are protected — so on the ordinary
    // `Access::ReadOnly` path this costs one `stat` and saves the user a password prompt
    // for a file that was never going to open. It is only an optimisation: a `stat` we
    // are not allowed to take, or one that lies (every `/proc` file reports 0), falls
    // through to the child's own `head -c` bound.
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() > max_bytes
    {
        return Err(PrivError::TooLarge {
            bytes: meta.len(),
            max_bytes,
        });
    }

    let fallback = format!("reading {} as root", path.display());
    let ceiling = max_bytes.saturating_add(1);
    let bytes = run(
        program,
        read_invocation(path, ceiling, secret),
        &fallback,
        timeout,
    )
    .await?;

    if bytes.len() as u64 > max_bytes {
        // `head` stopped at the ceiling, so this is a floor on the real size, not the
        // size. The caller only needs to know it is over.
        return Err(PrivError::TooLarge {
            bytes: bytes.len() as u64,
            max_bytes,
        });
    }
    Ok(bytes)
}

/// Replace `path`'s content with `bytes` with elevated privileges — atomic, mode and
/// ownership preserved, no shell involved. See the module doc for the three steps and
/// what each one buys.
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
    let tmp = sibling_temp_path(path);
    let fallback = format!("writing {} as root", path.display());

    let step = async |flags: &[&str], command: &str, operands: [&Path; 2]| {
        run(
            program,
            invocation(command, flags, &operands, secret),
            &fallback,
            timeout,
        )
        .await
        .map(|_stdout| ())
    };

    // A — the new content, into a temp this copy creates, so it is writable whatever mode
    // the destination carries. A `cp` that fails partway still leaves the temp behind, so
    // even this first step's failure has to be swept up.
    if let Err(e) = step(FILL_FLAGS, "cp", [staging.path(), &tmp]).await {
        discard(program, &tmp, secret, timeout, &e).await;
        return Err(e);
    }
    // B — the destination's own attributes, onto the temp, data untouched.
    if let Err(e) = step(STAMP_FLAGS, "cp", [path, &tmp]).await {
        discard(program, &tmp, secret, timeout, &e).await;
        return Err(e);
    }
    // C — publish, in one rename.
    if let Err(e) = step(COMMIT_FLAGS, "mv", [&tmp, path]).await {
        discard(program, &tmp, secret, timeout, &e).await;
        return Err(e);
    }
    Ok(())
}

/// Remove a temp a failed save may have left in a directory only root can write.
///
/// Best-effort by construction: the caller is already returning the failure that matters,
/// and a cleanup error that displaced it would be a worse outcome than a stray file. It
/// is skipped entirely when `cause` proves nothing ever ran as root — a wrong password
/// costs a PAM failure delay and an audit-log line, and spending a second one tidying up
/// after a command that was never executed is pure harm.
async fn discard(
    program: &OsStr,
    tmp: &Path,
    secret: &Passphrase,
    timeout: Duration,
    cause: &PrivError,
) {
    let ran_as_root = matches!(cause, PrivError::Io(_) | PrivError::Timeout);
    if !ran_as_root {
        return;
    }
    let fallback = format!("removing {} after a failed save", tmp.display());
    let outcome = run(
        program,
        invocation("rm", &["-f"], &[tmp], secret),
        &fallback,
        timeout.min(CLEANUP_TIMEOUT),
    )
    .await;
    if let Err(e) = outcome {
        log::warn!(
            "sid-privfs: could not remove {} after a failed save: {e}",
            tmp.display()
        );
    }
}

/// Where the replacement is assembled: `.<name>.sid-privfs-tmp.<token>`, **beside** the
/// destination.
///
/// Beside, not in `/tmp`, because the last step is a rename and a rename is only atomic
/// within one filesystem — `/etc` and `/tmp` are routinely different ones. Hidden, so a
/// crash that outruns [`discard`] leaves something that at least does not clutter a
/// listing of `/etc`.
///
/// The token is unpredictable rather than the process id. The temp is created by an
/// elevated `cp`, which follows symlinks, so anyone able to guess the name *and* write to
/// the destination's directory could pre-plant a link and have root write through it. On
/// a root-owned `/etc` that attacker is already root; for a config in a directory someone
/// else can write, a name they cannot guess is what closes it.
fn sibling_temp_path(dst: &Path) -> PathBuf {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let dir = dst.parent().unwrap_or_else(|| Path::new("/"));
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    // `RandomState` is seeded from the OS at first use; that is the unpredictability, and
    // the pid and clock only keep two saves in one process from colliding.
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    dir.join(format!(".{name}.sid-privfs-tmp.{:016x}", hasher.finish()))
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

    /// The program `sudo` is being asked to run: the element just past the `--` that ends
    /// sudo's own options.
    ///
    /// Asked this way rather than by scanning the whole argv for suspicious strings,
    /// because a flag can legitimately look like one — `head -c` is a byte count, not
    /// `sh -c`. The command word is the thing that is actually inside the trust boundary.
    fn elevated_command(inv: &SudoInvocation) -> String {
        let args = args_of(inv);
        let end_of_flags = args
            .iter()
            .position(|a| a == "--")
            .expect("sudo's own options must be terminated");
        args[end_of_flags + 1].clone()
    }

    /// Interpreters. Elevating any of these hands root an entire language, so the set of
    /// things that could go wrong stops being enumerable.
    const SHELLS: &[&str] = &["sh", "bash", "dash", "zsh", "ksh", "env", "eval", "python3"];

    /// Every invocation this module can build, so "no shell anywhere" is a claim about
    /// the whole module rather than about the two calls a test remembered to check.
    fn every_invocation() -> Vec<SudoInvocation> {
        let dst = Path::new("/etc/ssh/sshd_config");
        let tmp = Path::new("/etc/.sshd_config.sid-privfs-tmp.dead");
        let staging = Path::new("/tmp/sid-privfs-stage-abc");
        vec![
            read_invocation(dst, 1024, &secret()),
            invocation("cp", FILL_FLAGS, &[staging, tmp], &secret()),
            invocation("cp", STAMP_FLAGS, &[dst, tmp], &secret()),
            invocation("mv", COMMIT_FLAGS, &[tmp, dst], &secret()),
            invocation("rm", &["-f"], &[tmp], &secret()),
        ]
    }

    #[test]
    fn nothing_this_module_can_build_elevates_a_shell() {
        for inv in every_invocation() {
            let command = elevated_command(&inv);
            assert!(
                !SHELLS.contains(&command.as_str()),
                "`{command}` was elevated: an interpreter inside the trust boundary is a \
                 whole extra language root can be talked into"
            );
        }
    }

    // ---- the secret's only channel ----------------------------------------------

    #[test]
    fn the_secret_is_one_newline_terminated_line() {
        let inv = read_invocation(Path::new("/etc/fstab"), 1024, &secret());
        assert_eq!(inv.stdin.as_slice(), b"hunter2\n");
    }

    #[test]
    fn the_secret_never_appears_in_any_argument_vector() {
        // argv is world-readable through /proc/<pid>/cmdline. This is the invariant.
        for inv in every_invocation() {
            for arg in args_of(&inv) {
                assert!(
                    !arg.contains(SECRET),
                    "the password leaked into argv: {arg:?}"
                );
            }
        }
    }

    #[test]
    fn an_empty_secret_still_terminates_the_line() {
        // sudo must reach EOF after one line even when the user submitted nothing, or it
        // waits forever instead of reporting "no password was provided".
        let inv = read_invocation(
            Path::new("/etc/fstab"),
            1024,
            &Passphrase::new(String::new()),
        );
        assert_eq!(inv.stdin.as_slice(), b"\n");
    }

    #[test]
    fn every_invocation_asks_sudo_for_stdin_auth_and_a_fresh_timestamp() {
        // `-k` is the one that is a security property rather than a convenience: without
        // it a timestamp left by an unrelated terminal makes *any* password succeed, so
        // "unlock" would be verifying nothing.
        for inv in every_invocation() {
            let args = args_of(&inv);
            assert_eq!(
                &args[..5],
                &["-S", "-k", "-p", "", "--"],
                "sudo's own flags are not negotiable: {args:?}"
            );
        }
    }

    // ---- the read invocation ------------------------------------------------------

    #[test]
    fn a_read_is_bounded_by_the_child_itself_not_by_a_check_on_the_answer() {
        // `cat` would stream a lying /proc file until sid ran out of memory, and no check
        // on the result can help when the result is what exhausted the machine.
        let inv = read_invocation(Path::new("/etc/fstab"), 1_048_577, &secret());
        assert_eq!(
            args_of(&inv),
            vec![
                "-S",
                "-k",
                "-p",
                "",
                "--",
                "head",
                "-c",
                "1048577",
                "--",
                "/etc/fstab"
            ]
        );
    }

    #[test]
    fn a_read_puts_a_double_dash_before_the_path_for_both_programs() {
        // Two independent guards: sudo's own `--`, and `head`'s. Neither program can read
        // the path as an option even if the domain guard were ever relaxed.
        let args = args_of(&read_invocation(Path::new("/etc/fstab"), 1024, &secret()));
        let head = args.iter().position(|a| a == "head").expect("head in argv");
        let path = args
            .iter()
            .position(|a| a == "/etc/fstab")
            .expect("path in argv");
        assert_eq!(args[head - 1], "--", "sudo's options must be terminated");
        assert_eq!(args[path - 1], "--", "head's options must be terminated");
    }

    #[test]
    fn a_read_passes_an_awkward_path_as_one_intact_argument() {
        let inv = read_invocation(Path::new("/etc/my configs/a$b'c\"d"), 1024, &secret());
        let args = args_of(&inv);
        assert_eq!(args.last().unwrap(), "/etc/my configs/a$b'c\"d");
    }

    // ---- the write invocations -----------------------------------------------------

    /// The argv of the three steps of one save, in order.
    fn save_steps(dst: &Path, staging: &Path, tmp: &Path) -> Vec<Vec<String>> {
        vec![
            args_of(&invocation("cp", FILL_FLAGS, &[staging, tmp], &secret())),
            args_of(&invocation("cp", STAMP_FLAGS, &[dst, tmp], &secret())),
            args_of(&invocation("mv", COMMIT_FLAGS, &[tmp, dst], &secret())),
        ]
    }

    #[test]
    fn a_save_is_three_argv_and_not_one_program_text() {
        let steps = save_steps(
            Path::new("/etc/ssh/sshd_config"),
            Path::new("/tmp/sid-privfs-stage-abc"),
            Path::new("/etc/.sshd_config.sid-privfs-tmp.dead"),
        );
        assert_eq!(
            steps[0],
            vec![
                "-S",
                "-k",
                "-p",
                "",
                "--",
                "cp",
                "-T",
                "--",
                "/tmp/sid-privfs-stage-abc",
                "/etc/.sshd_config.sid-privfs-tmp.dead",
            ]
        );
        assert_eq!(
            steps[1],
            vec![
                "-S",
                "-k",
                "-p",
                "",
                "--",
                "cp",
                "--attributes-only",
                "--preserve=all",
                "--no-preserve=timestamps",
                "-T",
                "--",
                "/etc/ssh/sshd_config",
                "/etc/.sshd_config.sid-privfs-tmp.dead",
            ]
        );
        assert_eq!(
            steps[2],
            vec![
                "-S",
                "-k",
                "-p",
                "",
                "--",
                "mv",
                "-f",
                "-T",
                "--",
                "/etc/.sshd_config.sid-privfs-tmp.dead",
                "/etc/ssh/sshd_config",
            ]
        );
    }

    #[test]
    fn the_content_is_filled_before_the_attributes_are_stamped() {
        // The ordering bug, pinned in the argv as well as in behaviour (see
        // `a_0440_file_saves_through_a_helper_that_holds_no_privilege_at_all`): a temp
        // that inherited a 0440 destination's mode before it was filled could only be
        // written by an identity holding CAP_DAC_OVERRIDE.
        let steps = save_steps(
            Path::new("/etc/sudoers"),
            Path::new("/tmp/stage"),
            Path::new("/etc/.sudoers.sid-privfs-tmp.dead"),
        );
        let fill = steps[0].iter().position(|a| a == "/tmp/stage");
        assert!(fill.is_some(), "the first step must be the content fill");
        assert!(
            steps[1].contains(&"--attributes-only".to_string()),
            "the second step must be the attribute stamp, got {:?}",
            steps[1]
        );
    }

    #[test]
    fn every_save_step_fences_its_paths_behind_a_double_dash() {
        // Two independent guards on every operand: sudo's `--`, then the inner command's.
        // Neither program can read a path as an option even if the domain's absolute-path
        // guard were ever relaxed.
        for step in save_steps(
            Path::new("/etc/fstab"),
            Path::new("/tmp/stage"),
            Path::new("/etc/.fstab.sid-privfs-tmp.dead"),
        ) {
            let last_fence = step.iter().rposition(|a| a == "--").expect("a `--`");
            let first_fence = step.iter().position(|a| a == "--").expect("a `--`");
            assert!(
                last_fence > first_fence,
                "the inner command's options are not terminated: {step:?}"
            );
            for operand in &step[last_fence + 1..] {
                assert!(operand.starts_with('/'), "unfenced operand in {step:?}");
            }
        }
    }

    #[test]
    fn a_save_passes_an_awkward_destination_as_one_intact_argument() {
        let dst = "/etc/my configs/a$b'c\"d; rm -rf /";
        for step in save_steps(Path::new(dst), Path::new("/tmp/stage"), Path::new("/etc/t")) {
            // Present verbatim as its own element in the two steps that name it, and
            // never spliced into a longer string anywhere.
            for arg in &step {
                assert!(
                    arg == dst || !arg.contains("rm -rf /"),
                    "caller data was interpolated into `{arg}`"
                );
            }
        }
    }

    // ---- the sibling temp ---------------------------------------------------------

    #[test]
    fn the_temp_is_a_hidden_sibling_of_the_destination() {
        // Cross-filesystem staging would silently turn the atomic commit into a copy.
        let tmp = sibling_temp_path(Path::new("/etc/fstab"));
        assert_eq!(tmp.parent(), Some(Path::new("/etc")));
        let name = tmp.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(".fstab.sid-privfs-tmp."), "{name}");
    }

    #[test]
    fn the_temp_name_is_not_guessable_from_the_destination_alone() {
        // An elevated `cp` follows symlinks, so a predictable name in a directory someone
        // else can write is a way to have root write through a pre-planted link.
        let a = sibling_temp_path(Path::new("/etc/fstab"));
        let b = sibling_temp_path(Path::new("/etc/fstab"));
        assert_ne!(a, b, "two saves of one file must not reuse a temp name");
        assert!(
            !a.to_string_lossy()
                .ends_with(&std::process::id().to_string()),
            "the pid is public: {}",
            a.display()
        );
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
    use std::sync::OnceLock;

    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// The cap the config editor uses, for tests that are not about the cap.
    const CAP: u64 = 1024 * 1024;

    fn mock_sudo() -> OsString {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/mock-sudo")
            .into()
    }

    /// A wrapper that forces one of `mock-sudo`'s failure verdicts. Written as a script
    /// rather than set on this process's environment: `set_var` is unsafe in edition 2024
    /// and would race every other test in this binary.
    ///
    /// Every wrapper this binary will ever need is written ONCE, before any test can spawn
    /// anything, and never rewritten. Writing one per test made the suite flaky with
    /// `ETXTBSY` ("Text file busy"): these tests run concurrently, and Linux refuses to
    /// `exec` a file that any process holds open for writing. A `fork` in one thread
    /// momentarily inherits every descriptor another thread has open — including a wrapper
    /// mid-`write` — so a script written on thread A could be un-executable for as long as
    /// thread B's unrelated child took to reach its own `exec`. One write per verdict,
    /// completed before the first spawn, removes the window instead of narrowing it.
    fn mock_sudo_with(verdict: &str) -> OsString {
        wrappers().join(format!("mock-sudo-{verdict}")).into()
    }

    /// Every verdict `mock-sudo` understands, so the eager write below is total.
    const VERDICTS: [&str; 6] = [
        "notsudoer",
        "wrongpass",
        "hang",
        "innerfail",
        "attrfail",
        "partialfill",
    ];

    /// How many independent argv-logging wrappers exist — see [`argv_log`]. One per test
    /// that needs one, because a shared log would interleave concurrent tests.
    const ARGV_LOG_SLOTS: usize = 2;

    /// The wrapper directory — see [`argv_log`] for the logging ones.
    fn wrappers() -> &'static Path {
        static SCRIPTS: OnceLock<tempfile::TempDir> = OnceLock::new();
        SCRIPTS
            .get_or_init(|| {
                let dir = tempfile::tempdir().expect("a temp dir for the mock-sudo wrappers");
                let mock = mock_sudo();
                let write = |name: String, body: String| {
                    let path = dir.path().join(name);
                    std::fs::write(&path, body).expect("write a mock-sudo wrapper");
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                            .expect("make a mock-sudo wrapper executable");
                    }
                };
                for v in VERDICTS {
                    write(
                        format!("mock-sudo-{v}"),
                        format!(
                            "#!/bin/sh\nexport MOCK_SUDO_VERDICT={v}\nexec {} \"$@\"\n",
                            Path::new(&mock).display()
                        ),
                    );
                }
                // The argv-logging wrappers: ordinary (password-checking) mocks that also
                // record what they were asked to run, one private log each.
                for slot in 0..ARGV_LOG_SLOTS {
                    write(
                        format!("mock-sudo-logged-{slot}"),
                        format!(
                            "#!/bin/sh\nexport MOCK_SUDO_ARGV_LOG={}\nexec {} \"$@\"\n",
                            dir.path().join(format!("argv-{slot}.log")).display(),
                            Path::new(&mock).display()
                        ),
                    );
                }
                dir
            })
            .path()
    }

    /// An argv-logging mock, and the file it appends to.
    ///
    /// Each slot is a private wrapper with a private log, and a slot may be claimed once:
    /// these tests run concurrently, so two tests sharing a log would interleave their
    /// invocations into each other's assertions. The claim is enforced rather than
    /// documented, because "only one test uses this" is not a property that survives
    /// somebody adding a test.
    fn argv_log(slot: usize) -> (OsString, PathBuf) {
        static CLAIMED: [std::sync::atomic::AtomicBool; ARGV_LOG_SLOTS] =
            [const { std::sync::atomic::AtomicBool::new(false) }; ARGV_LOG_SLOTS];
        assert!(
            !CLAIMED[slot].swap(true, std::sync::atomic::Ordering::SeqCst),
            "argv log slot {slot} has a second user — claim a fresh slot"
        );
        let dir = wrappers();
        (
            dir.join(format!("mock-sudo-logged-{slot}")).into(),
            dir.join(format!("argv-{slot}.log")),
        )
    }

    /// The command word of every invocation recorded in an argv log, in order — see
    /// `scripts/mock-sudo` for the record format.
    fn elevated_commands(log: &Path) -> Vec<String> {
        let logged = std::fs::read_to_string(log).expect("the mock recorded its argv");
        logged
            .split("=== invocation ===\n")
            .filter(|record| !record.is_empty())
            .map(|record| {
                let args: Vec<&str> = record
                    .lines()
                    .filter_map(|l| l.strip_prefix("arg:"))
                    .collect();
                let end_of_flags = args
                    .iter()
                    .position(|a| *a == "--")
                    .unwrap_or_else(|| panic!("no `--` in a recorded argv: {args:?}"));
                args[end_of_flags + 1].to_string()
            })
            .collect()
    }

    fn good_secret() -> Passphrase {
        Passphrase::new("correct".to_string())
    }

    #[tokio::test]
    async fn an_accepted_password_reads_the_file_back_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "UUID=abc / ext4 defaults 0 1\n").unwrap();

        let got = read_with(&mock_sudo(), &path, CAP, &good_secret(), TEST_TIMEOUT)
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
            CAP,
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
        let program = mock_sudo_with("notsudoer");

        let err = read_with(&program, &path, CAP, &good_secret(), TEST_TIMEOUT)
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
        let program = mock_sudo_with("hang");

        let started = std::time::Instant::now();
        let err = read_with(
            &program,
            &path,
            CAP,
            &good_secret(),
            Duration::from_millis(300),
        )
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
            CAP,
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
            CAP,
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
        let got = read_with(&mock_sudo(), &path, CAP, &good_secret(), TEST_TIMEOUT)
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

    /// Every temp this crate could have left beside `path`.
    fn leftovers_beside(path: &Path) -> Vec<String> {
        std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("sid-privfs-tmp"))
            .collect()
    }

    #[tokio::test]
    async fn the_elevated_command_set_is_exactly_the_coreutils_this_crate_documents() {
        // The real "no shell" assertion, taken from the far side of the process boundary:
        // not "the argv looks fine" but "these, and only these, are the programs sid ever
        // asks root to run". A shell would show up here as `sh` and nothing else could
        // hide it.
        let (program, log) = argv_log(0);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "UUID=abc / ext4 defaults 0 1\n").unwrap();

        read_with(&program, &path, CAP, &good_secret(), TEST_TIMEOUT)
            .await
            .unwrap();
        write_with(
            &program,
            &path,
            b"UUID=def / ext4 defaults 0 1\n",
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap();

        assert_eq!(
            elevated_commands(&log),
            vec!["head", "cp", "cp", "mv"],
            "an unexpected program was elevated"
        );
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(
            !logged.contains(SECRET) && !logged.contains("correct"),
            "the password reached the helper's argv: {logged}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_0440_file_saves_through_a_helper_that_holds_no_privilege_at_all() {
        // The attribute-ordering regression. `/etc/sudoers` is 0440: if the replacement's
        // attributes were cloned from the target BEFORE its content was written, the temp
        // would be 0440 too and the content write would then need CAP_DAC_OVERRIDE. Real
        // root has it and the bug hides; the mock helper — which elevates nothing — does
        // not, so this test fails loudly on the wrong order.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sudoers");
        std::fs::write(&path, "root ALL=(ALL:ALL) ALL\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o440)).unwrap();

        write_with(
            &mock_sudo(),
            &path,
            b"root ALL=(ALL:ALL) ALL\nmurphy ALL=(ALL:ALL) NOPASSWD: ALL\n",
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .expect("a 0440 target must be saveable without CAP_DAC_OVERRIDE");

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "root ALL=(ALL:ALL) ALL\nmurphy ALL=(ALL:ALL) NOPASSWD: ALL\n"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o440,
            "the mode must survive"
        );
        assert!(leftovers_beside(&path).is_empty());
    }

    #[tokio::test]
    async fn a_destination_that_is_a_directory_fails_instead_of_swallowing_the_save() {
        // Without `-T`, `mv` moves the temp *into* a destination directory and exits 0 —
        // a save that reports success while writing nothing where the user asked. The
        // failure has to be visible, and the directory has to be left alone.
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
        let swallowed: Vec<_> = std::fs::read_dir(&target)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            swallowed.is_empty(),
            "the save was moved INSIDE the destination directory: {swallowed:?}"
        );
        assert!(leftovers_beside(&target).is_empty(), "left a temp behind");
    }

    #[tokio::test]
    async fn a_rejected_password_costs_exactly_one_authentication() {
        // Cleanup is skipped when the failure proves nothing ever ran as root. It matters
        // because a wrong password is not free: every attempt costs the user a PAM
        // failure delay and the machine an audit-log line, and spending a second one
        // tidying up after a command that was never executed is pure harm — twice the
        // wait, and a log that suggests sid retried behind the user's back.
        let (program, log) = argv_log(1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "original\n").unwrap();

        let err = write_with(
            &program,
            &path,
            b"vandalized\n",
            &Passphrase::new("wrong".into()),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, PrivError::AuthFailed), "got {err:?}");
        assert_eq!(
            elevated_commands(&log),
            vec!["cp"],
            "a rejected password must not buy a second trip through PAM"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
    }

    #[tokio::test]
    async fn the_commit_step_refuses_a_directory_rather_than_moving_the_temp_inside_it() {
        // `-T` on the commit, in isolation. The full-pipeline test above never reaches
        // `mv` — `cp` rejects a directory source at the attribute step first — so without
        // this the flag could be dropped and every test would still pass, right up until
        // a destination turned into a directory between the stamp and the rename.
        // Without `-T`, `mv` moves the temp INSIDE the directory and exits 0: a save that
        // reports success while writing nothing where the user asked.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("fstab");
        std::fs::create_dir(&target).unwrap();
        let tmp = dir.path().join(".fstab.sid-privfs-tmp.dead");
        std::fs::write(&tmp, "REPLACEMENT\n").unwrap();

        let outcome = run(
            &mock_sudo(),
            invocation("mv", COMMIT_FLAGS, &[&tmp, &target], &good_secret()),
            "committing",
            TEST_TIMEOUT,
        )
        .await;

        assert!(
            outcome.is_err(),
            "the commit reported success against a directory destination"
        );
        let swallowed: Vec<_> = std::fs::read_dir(&target)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            swallowed.is_empty(),
            "the save was moved INSIDE the destination directory: {swallowed:?}"
        );
    }

    #[tokio::test]
    async fn a_fill_that_dies_halfway_through_still_removes_its_own_temp() {
        // The guarantee the shell `trap` used to give for free. `cp` that fails partway —
        // a full disk, an I/O error — has already created the destination, so "the first
        // step failed, there is nothing to clean up" is wrong, and being wrong here means
        // an orphaned `.fstab.sid-privfs-tmp.*` in /etc after every failed save.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "ORIGINAL\n").unwrap();
        let program = mock_sudo_with("partialfill");

        let err = write_with(
            &program,
            &path,
            b"REPLACEMENT\n",
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, PrivError::Io(_)), "got {err:?}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "ORIGINAL\n",
            "a save that never reached the rename must not have touched the target"
        );
        assert!(
            leftovers_beside(&path).is_empty(),
            "left a temp behind: {:?}",
            leftovers_beside(&path)
        );
    }

    #[tokio::test]
    async fn an_attribute_step_that_fails_leaves_the_original_intact_and_no_temp() {
        // The mid-save failure: the temp exists and is full of the new content, and the
        // step that would have made it wearable failed. Publishing it anyway would hand
        // back /etc/sudoers with the wrong owner.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sudoers");
        std::fs::write(&path, "ORIGINAL\n").unwrap();
        let program = mock_sudo_with("attrfail");

        let err = write_with(
            &program,
            &path,
            b"REPLACEMENT\n",
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, PrivError::Io(_)), "got {err:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ORIGINAL\n");
        assert!(
            leftovers_beside(&path).is_empty(),
            "left a temp behind: {:?}",
            leftovers_beside(&path)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_save_does_not_backdate_the_file_it_just_changed() {
        // `--preserve=all` on its own would stamp the destination's OLD mtime onto the
        // new content. A file that claims it has not changed since 2020 is one `rsync`,
        // `etckeeper` and every staleness check will skip — a silent way to lose a save
        // that actually landed.
        use std::time::{Duration as StdDuration, SystemTime};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fstab");
        std::fs::write(&path, "old\n").unwrap();
        let long_ago = SystemTime::now() - StdDuration::from_secs(400 * 24 * 60 * 60);
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        write_with(&mock_sudo(), &path, b"new\n", &good_secret(), TEST_TIMEOUT)
            .await
            .unwrap();

        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(
            modified.duration_since(long_ago).unwrap() > StdDuration::from_secs(60),
            "the save kept the old mtime: the file lies about having changed"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cp_derives_a_fresh_temps_mode_from_its_source_whatever_the_umask() {
        // The `umask 077` guarantee, relocated. The old shell script set the umask itself;
        // the temp is now created by `cp` from the 0600 staging file, so "never briefly
        // world-readable" rests on coreutils deriving a new destination's mode from the
        // source rather than from the process umask. That is the assumption, so it is the
        // test — run directly, with no elevation involved.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let staging = stage_content(b"PermitRootLogin no\n").unwrap();
        let tmp = dir.path().join(".sshd_config.sid-privfs-tmp.dead");

        let status = tokio::process::Command::new("cp")
            .arg("-T")
            .arg("--")
            .arg(staging.path())
            .arg(&tmp)
            .status()
            .await
            .unwrap();

        assert!(status.success());
        assert_eq!(
            std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777,
            0o600,
            "the temp was world-readable for the life of the save"
        );
    }

    // ---- the read cap ---------------------------------------------------------------

    #[tokio::test]
    async fn an_oversized_file_is_refused_before_the_user_is_asked_for_anything() {
        // The helper cannot be spawned, so reaching it at all would surface as
        // `Unavailable`. `TooLarge` proves the size check ran first — and a password
        // prompt for a file the editor was never going to open is a prompt not shown.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.conf");
        std::fs::write(&path, vec![b'a'; 4096]).unwrap();

        let err = read_with(
            &OsString::from(dir.path().join("no-such-sudo")),
            &path,
            1024,
            &good_secret(),
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            PrivError::TooLarge {
                bytes: 4096,
                max_bytes: 1024
            }
        );
        assert!(!err.is_retryable(), "no password makes a file smaller");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_file_whose_size_cannot_be_trusted_is_still_capped_by_the_child() {
        // Every file under /proc reports 0 bytes from `stat` and then streams anyway, so
        // the pre-flight size check waves it straight through. The only thing left
        // between sid and an unbounded read is `head -c` in the child — which is why the
        // cap is an argument to the helper and not a check on the answer. A real /proc
        // file, because a fake one cannot reproduce the lie.
        let path = Path::new("/proc/self/status");
        assert_eq!(
            std::fs::metadata(path).unwrap().len(),
            0,
            "this test needs a file whose metadata under-reports it"
        );

        let err = read_with(&mock_sudo(), path, 64, &good_secret(), TEST_TIMEOUT)
            .await
            .unwrap_err();

        // 65 bytes came back — one over the cap — which is exactly how the caller knows
        // it was truncated rather than exactly at the limit.
        assert_eq!(
            err,
            PrivError::TooLarge {
                bytes: 65,
                max_bytes: 64
            }
        );
    }

    #[tokio::test]
    async fn a_file_exactly_at_the_cap_is_read_whole() {
        // The off-by-one that would otherwise refuse a 1 MiB file the editor can open.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exact.conf");
        std::fs::write(&path, vec![b'x'; 64]).unwrap();

        let got = read_with(&mock_sudo(), &path, 64, &good_secret(), TEST_TIMEOUT)
            .await
            .unwrap();

        assert_eq!(got.len(), 64);
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
