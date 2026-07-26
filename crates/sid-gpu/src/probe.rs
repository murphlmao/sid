//! Subprocess execution for the pre-flight protocol — the half that may crash.
//!
//! HARD CONSTRAINT: no threads, ever. The frontend calls `std::env::set_var`
//! (unsafe in edition 2024) immediately after `ensure_renderable` returns; a
//! reader thread that outlived these functions would make that call unsound.
//! Child output therefore goes to plain files (no pipe to drain, no deadlock,
//! nothing to join) and completion is `try_wait` polling with a kill-on-timeout
//! — a hung GPU driver is a failure, not a hang, for the parent.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// One probe attempt's observable result.
#[derive(Debug, Clone)]
pub(crate) struct ProbeOutcome {
    /// The child exited 0 within the timeout — the GPU context initialized.
    pub ok: bool,
    /// The child was killed at the timeout (hung driver / hung compositor).
    pub timed_out: bool,
    /// Exit code when the child exited normally (`None`: signal or timeout kill).
    pub exit: Option<i32>,
    /// Captured stderr — env_logger-formatted `blade_graphics` records: the
    /// `Adapter:` line on success, the exact failing call on failure. Empty when
    /// nothing could be captured (see [`Self::no_capture`]).
    pub stderr: String,
    /// The adapter name parsed from stderr, when present.
    pub adapter: Option<String>,
    /// The probe never executed at all — we could not even spawn it.
    ///
    /// This is **not evidence about the GPU**, and callers must never diagnose a
    /// GPU failure from it: doing so would let an unrelated local condition stop
    /// a machine that renders perfectly. See `ensure_renderable`'s fail-open
    /// branch, which is the one place allowed to interpret this.
    pub not_run: bool,
    /// The probe ran, but with stderr discarded because nowhere was writable.
    /// The exit-status verdict is still authoritative; only the adapter name and
    /// the failure evidence are unavailable.
    pub no_capture: bool,
}

impl ProbeOutcome {
    /// A probe that could not be started. Deliberately distinct from a probe that
    /// ran and failed — see [`Self::not_run`].
    fn not_run(why: String) -> Self {
        Self {
            ok: false,
            timed_out: false,
            exit: None,
            stderr: why,
            adapter: None,
            not_run: true,
            no_capture: true,
        }
    }
}

/// Run one probe attempt: `cmd` with `pins` exported, stderr captured into a
/// uniquely-named file under `capture_dir` (removed once read back), stdout
/// discarded, killed at `timeout`.
///
/// `RUST_LOG=blade_graphics=info` is set on the child so the adapter/failure
/// log lines actually materialize even if the probe-mode binary forgot to force
/// its own filter (belt and suspenders with the frontend's `--gpu-probe` mode).
pub(crate) fn run_probe(
    cmd: &[String],
    pins: &[(String, String)],
    capture_dir: &Path,
    timeout: Duration,
) -> ProbeOutcome {
    let Some((exe, args)) = cmd.split_first() else {
        return ProbeOutcome::not_run("probe command is empty".into());
    };
    // A capture we cannot open must never become a verdict: fall back to the temp
    // dir, then to discarding output entirely, because the child's EXIT STATUS is
    // the load-bearing signal and it survives either way.
    let (capture, stderr_stdio) = match open_capture(capture_dir, "probe") {
        Some((capture, file)) => (Some(capture), Stdio::from(file)),
        None => (None, Stdio::null()),
    };

    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr_stdio)
        .env("RUST_LOG", "blade_graphics=info");
    for (key, value) in pins {
        command.env(key, value);
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        // `capture` is dropped on the way out, which unlinks it. That matters
        // most HERE: this arm never writes a stamp, so a machine whose probe
        // cannot spawn re-probes on every single launch — an empty capture left
        // behind would accumulate one file per launch, forever, on the one path
        // that never stops re-probing. See [`Capture`].
        Err(e) => return ProbeOutcome::not_run(format!("cannot spawn probe {exe:?}: {e}")),
    };
    let status = wait_with_timeout(&mut child, timeout);
    // The capture reaches memory here; the file itself is unlinked when `capture`
    // drops at the end of this function (or at any early return above).
    let stderr = capture.as_ref().map(Capture::read).unwrap_or_default();
    let adapter = parse_adapter(&stderr);
    let no_capture = capture.is_none();
    match status {
        Some(s) => ProbeOutcome {
            ok: s.success(),
            timed_out: false,
            exit: s.code(),
            stderr,
            adapter,
            not_run: false,
            no_capture,
        },
        None => ProbeOutcome {
            ok: false,
            timed_out: true,
            exit: None,
            stderr,
            adapter,
            not_run: false,
            no_capture,
        },
    }
}

/// A capture file that unlinks itself when it goes out of scope.
///
/// RAII rather than a `remove_file` at the bottom of each run function, because
/// the unlink must happen on EVERY exit path and the early ones are exactly where
/// it went missing: the capture is opened before `spawn()`, and a spawn failure
/// returned straight past the cleanup. That path never writes a stamp, so a
/// machine with an unspawnable probe (a missing binary, `RLIMIT_NPROC`, a sandbox
/// forbidding exec) re-probes on every launch and accumulated one 0-byte
/// `probe-*.log` per launch, forever, in the user's state dir — the "temps must
/// not accumulate" violation, reintroduced on the one path that never stops
/// re-probing. A `Drop` impl makes the guarantee structural: a future early
/// return cannot lose it again.
struct Capture {
    path: PathBuf,
}

impl Capture {
    /// Read the child's output back. An unreadable capture reads as empty: the
    /// exit status is the verdict, this text is only the evidence for it.
    fn read(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // Best-effort: an abandoned (unreapable) child may still hold the fd, and
        // on Linux the unlink succeeds anyway.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Open a uniquely-named capture file, preferring `dir` and falling back to the
/// system temp dir. `None` means nowhere was writable — the caller still runs its
/// command and still gets a real verdict, just without the evidence.
///
/// Uniqueness deliberately does NOT rest on the pid alone. Every process in a
/// fresh PID namespace (containers, `unshare`, bubblewrap/Flatpak) is pid 1, so
/// concurrent sandboxed instances truncated and unlinked each other's captures —
/// which loses the `Adapter:` line and thereby silently downgrades a software
/// verdict to `Hardware`. pid + nanoseconds + a process-local counter, created
/// `O_EXCL`, is unique across namespaces as well as within one process.
fn open_capture(dir: &Path, prefix: &str) -> Option<(Capture, std::fs::File)> {
    let _ = std::fs::create_dir_all(dir);
    let temp = std::env::temp_dir();
    for base in [dir, temp.as_path()] {
        for _ in 0..8 {
            let path = base.join(unique_name(prefix));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Some((Capture { path }, file)),
                // Astronomically unlikely, but retry rather than clobber.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                // This directory is unusable (read-only, ENOTDIR, out of inodes):
                // stop retrying it and try the next one.
                Err(_) => break,
            }
        }
    }
    log::warn!(
        "sid-gpu: no writable capture directory ({} or {}); probing with output discarded — \
         the verdict stays valid, the adapter name and failure evidence do not",
        dir.display(),
        temp.display()
    );
    None
}

/// A scratch-file name unique across processes, PID namespaces, and calls.
fn unique_name(prefix: &str) -> String {
    format!("{prefix}-{}.log", unique_token())
}

/// The uniqueness primitive behind every scratch name this crate writes:
/// `pid-nanos-counter`, unique across processes, PID namespaces, and calls.
///
/// Lives here because this is where the reasoning is written down (see
/// [`open_capture`]), but it is deliberately crate-visible: `stamp::save`'s
/// write-then-rename temp has the identical namespace hazard — pid 1 in every
/// sandbox — and must not grow a second, subtly different answer to it. The
/// counter is process-global for that reason: it makes the two callers' names
/// unique against each other as well as against themselves.
pub(crate) fn unique_token() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// How long we keep polling for a killed child's exit status before giving up on
/// reaping it. SIGKILL cannot terminate a process in uninterruptible sleep
/// (D-state) — which is precisely what a wedged kernel GPU driver produces: the
/// probe child stuck in a DRM/nvidia ioctl inside `vkCreateInstance` /
/// `vkEnumeratePhysicalDevices`, or a hung `nvidia-smi` on a dead GPU. A blocking
/// `wait()` there would hang the parent forever, pre-window and silently, in
/// exactly the case the timeout exists to survive. A *killable* child dies in
/// milliseconds, so this grace never costs anything in practice.
const REAP_GRACE: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Threadless wait: poll `try_wait` every 25ms; at `timeout` kill and reap within
/// a bounded grace, then return `None`. (The sleep is on the calling thread —
/// nothing is spawned.)
///
/// INVARIANT: returns within `timeout + REAP_GRACE`, always — never blocks
/// indefinitely. That is what lets `ensure_renderable` classify and report a
/// wedged driver instead of hanging on it.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    kill_and_reap(child);
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            // try_wait errors are unrecoverable for this child; reap and bail.
            Err(_) => {
                kill_and_reap(child);
                return None;
            }
        }
    }
}

/// Kill the child and poll for its exit status for at most [`REAP_GRACE`].
///
/// Deliberately never calls blocking `wait()`: on grace expiry we just log and
/// let the `Child` drop. Rust's `Child::drop` neither kills nor waits, so it
/// cannot block; a truly D-state process is unkillable by any means and gets
/// reaped by init once we exit. Leaking one zombie beats hanging the launch.
fn kill_and_reap(child: &mut std::process::Child) {
    let _ = child.kill();
    let deadline = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if deadline.elapsed() < REAP_GRACE => std::thread::sleep(POLL_INTERVAL),
            // Grace expired, or try_wait itself is failing: stop waiting.
            _ => {
                log::warn!(
                    "sid-gpu: probe child {} could not be reaped within {:?} after SIGKILL \
                     (likely uninterruptible sleep in a wedged GPU driver); abandoning it",
                    child.id(),
                    REAP_GRACE
                );
                return;
            }
        }
    }
}

/// Run a helper command with stdout+stderr captured into a uniquely-named file
/// under `capture_dir` (removed once read back), killed at `timeout`. Returns
/// (exited-zero, combined output). Used for `nvidia-smi`; the same threadless
/// discipline — and the same [`open_capture`] discipline — as the probe itself.
///
/// THE FLOOR: for the probe, the verdict is the exit status and the capture is
/// only evidence, so losing it costs a little. Here the OUTPUT IS THE ORACLE —
/// `diagnose`'s arm 2 recognizes an NVIDIA driver/library mismatch by grepping
/// this text — so a box where NOTHING is writable (state dir *and* temp dir both
/// fail) loses that arm entirely and gets the "file a bug" fallthrough instead of
/// "reboot". That is accepted: it is the floor of a threadless design (an
/// in-process pipe would need a reader thread, forbidden here — see the module
/// header), and it is strictly narrower than the bug it replaces, where an
/// unwritable STATE DIR alone was enough to silence the oracle.
pub(crate) fn run_captured(
    cmd: &[String],
    capture_dir: &Path,
    prefix: &str,
    timeout: Duration,
) -> (bool, String) {
    let Some((exe, args)) = cmd.split_first() else {
        return (false, String::new());
    };
    // Both streams into ONE file, interleaved as the child writes them: nvidia-smi
    // prints its mismatch banner on stderr and its ordinary output on stdout, and
    // the classifier greps the pair. A failed `try_clone` (out of descriptors)
    // drops the guard — and with it the file — rather than capturing half a story.
    let (capture, streams) = match open_capture(capture_dir, prefix) {
        Some((capture, file)) => match file.try_clone() {
            Ok(dup) => (Some(capture), Some((file, dup))),
            Err(e) => {
                // `open_capture` logs when nowhere is writable; this rarer loss
                // (out of descriptors) would otherwise be silent, and a silently
                // empty oracle reads as "nvidia-smi said nothing".
                log::warn!("sid-gpu: cannot duplicate the {prefix} capture ({e}); output lost");
                (None, None)
            }
        },
        None => (None, None),
    };
    let (stdout, stderr) = match streams {
        Some((out, err)) => (Stdio::from(out), Stdio::from(err)),
        None => (Stdio::null(), Stdio::null()),
    };
    let mut child = match Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
    {
        Ok(c) => c,
        // As in `run_probe`: the guard unlinks on the way out. Reachable whenever
        // the nvidia kernel module is loaded but the binary is absent, which is a
        // permanent condition — so a leak here was one file per diagnosis.
        Err(_) => return (false, String::new()),
    };
    let status = wait_with_timeout(&mut child, timeout);
    let output = capture.as_ref().map(Capture::read).unwrap_or_default();
    (status.map(|s| s.success()).unwrap_or(false), output)
}

/// Run a command fully silenced (both streams to null), killed at `timeout`.
/// Used for `notify-send`, where only best-effort delivery matters.
pub(crate) fn run_silenced(cmd: &[String], timeout: Duration) -> bool {
    let Some((exe, args)) = cmd.split_first() else {
        return false;
    };
    let mut child = match Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    wait_with_timeout(&mut child, timeout)
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Extract the adapter name from blade's `Adapter: "..."` log line. The line
/// arrives env_logger-prefixed (`[... INFO blade_graphics::vulkan::init] Adapter:
/// "Intel(R) Graphics"`), so this searches within each line rather than anchoring.
///
/// The LAST match wins, not the first: blade logs `Adapter:` at the *top* of its
/// per-device inspection, before every rejection check (device-id filter, API
/// < 1.1, missing extensions, inline-uniform/timeline-semaphore/dynamic-rendering
/// features), and inspects devices in a `find_map` — so every REJECTED device
/// also logs a line and the ACCEPTED one is logged last. Taking the first would
/// misattribute on multi-GPU machines (a rejected discrete GPU logged first,
/// lavapipe accepted last → a software fallback reported as `Hardware`, no badge,
/// and the wrong verdict persisted in the stamp).
pub(crate) fn parse_adapter(stderr: &str) -> Option<String> {
    const NEEDLE: &str = "Adapter: \"";
    let mut found = None;
    for line in stderr.lines() {
        if let Some(pos) = line.find(NEEDLE) {
            let rest = &line[pos + NEEDLE.len()..];
            if let Some(end) = rest.find('"') {
                found = Some(rest[..end].to_string());
            }
        }
    }
    found
}

/// Whether an adapter name identifies a software rasterizer (Mesa's llvmpipe/
/// lavapipe, Google's SwiftShader, classic softpipe).
pub(crate) fn adapter_is_software(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["llvmpipe", "lavapipe", "swiftshader", "softpipe"]
        .iter()
        .any(|n| lower.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    /// Every scratch file `dir` still holds with the given capture prefix.
    fn captures(dir: &Path, prefix: &str) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with(prefix))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn exit_zero_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_probe(&sh("exit 0"), &[], dir.path(), Duration::from_secs(5));
        assert!(out.ok);
        assert!(!out.timed_out);
        assert_eq!(out.exit, Some(0));
    }

    #[test]
    fn nonzero_exit_is_failure_with_code() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_probe(&sh("exit 3"), &[], dir.path(), Duration::from_secs(5));
        assert!(!out.ok);
        assert!(!out.timed_out);
        assert_eq!(out.exit, Some(3));
    }

    #[test]
    fn stderr_is_captured_and_adapter_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_probe(
            &sh(
                r#"echo '[2026-07-08T00:00:00Z INFO  blade_graphics::vulkan::init] Adapter: "llvmpipe (LLVM 19.1.0, 256 bits)"' >&2"#,
            ),
            &[],
            dir.path(),
            Duration::from_secs(5),
        );
        assert!(out.ok);
        assert_eq!(
            out.adapter.as_deref(),
            Some("llvmpipe (LLVM 19.1.0, 256 bits)")
        );
        assert!(adapter_is_software(out.adapter.as_deref().unwrap()));
    }

    #[test]
    fn pins_reach_the_child_environment() {
        let dir = tempfile::tempdir().unwrap();
        let pins = vec![("VK_DRIVER_FILES".to_string(), "/tmp/x.json".to_string())];
        let out = run_probe(
            &sh(r#"[ "$VK_DRIVER_FILES" = "/tmp/x.json" ]"#),
            &pins,
            dir.path(),
            Duration::from_secs(5),
        );
        assert!(out.ok);
    }

    #[test]
    fn hung_child_is_killed_at_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = run_probe(&sh("sleep 5"), &[], dir.path(), Duration::from_millis(200));
        assert!(!out.ok);
        assert!(out.timed_out);
        assert_eq!(out.exit, None);
        // The whole point: a hung driver costs the timeout, not 5 seconds.
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn spawn_failure_is_flagged_not_run_rather_than_a_gpu_failure() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_probe(
            &["/nonexistent/definitely-not-a-binary".to_string()],
            &[],
            dir.path(),
            Duration::from_secs(1),
        );
        assert!(!out.ok);
        assert!(!out.timed_out, "nothing ran, so nothing hung");
        // The load-bearing bit: a probe that never executed is NOT evidence about
        // the GPU, and `ensure_renderable` fails open on exactly this flag.
        assert!(out.not_run);
        assert!(out.stderr.contains("cannot spawn"));
        // And the capture opened BEFORE the spawn attempt is unlinked on the way
        // out. This path never writes a stamp, so it re-probes on every launch:
        // a leak here is one 0-byte log per launch, forever.
        assert!(
            captures(dir.path(), "probe").is_empty(),
            "a spawn failure must leave no capture: {:?}",
            captures(dir.path(), "probe")
        );
    }

    #[test]
    fn repeated_spawn_failures_do_not_accumulate_captures() {
        // The process-level repro's shape (`prlimit --nproc=1 -- sid --gpu-report`
        // five times over one state dir), reduced to the function that leaked.
        let dir = tempfile::tempdir().unwrap();
        for _ in 0..5 {
            let out = run_probe(
                &["/nonexistent/definitely-not-a-binary".to_string()],
                &[],
                dir.path(),
                Duration::from_secs(1),
            );
            assert!(out.not_run);
        }
        assert!(
            captures(dir.path(), "probe").is_empty(),
            "one file per launch, forever: {:?}",
            captures(dir.path(), "probe")
        );
    }

    #[test]
    fn run_captured_returns_both_streams_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let (ok, output) = run_captured(
            &sh("echo on-stdout; echo on-stderr >&2; exit 12"),
            dir.path(),
            "nvidia-smi",
            Duration::from_secs(5),
        );
        assert!(!ok, "exit 12 is not success");
        // Both streams land in the one capture: nvidia-smi's mismatch banner is on
        // stderr, its ordinary output on stdout, and the classifier greps the pair.
        assert!(output.contains("on-stdout"), "{output}");
        assert!(output.contains("on-stderr"), "{output}");
        assert!(
            captures(dir.path(), "nvidia-smi").is_empty(),
            "read, then dropped: {:?}",
            captures(dir.path(), "nvidia-smi")
        );
    }

    #[test]
    fn run_captured_falls_back_to_the_temp_dir_when_the_state_dir_is_unusable() {
        // The NVIDIA mismatch oracle is the OUTPUT, not the exit status, so a
        // `File::create` failure in the state dir used to silence the whole
        // diagnosis arm — a post-driver-update box told "file a bug" instead of
        // "reboot". The temp-dir fallback is what keeps the arm alive.
        let (ok, output) = run_captured(
            &sh("echo 'Failed to initialize NVML: Driver/library version mismatch' >&2; exit 12"),
            Path::new("/dev/null/nope"),
            "nvidia-smi",
            Duration::from_secs(5),
        );
        assert!(!ok);
        assert!(
            output.to_lowercase().contains("mismatch"),
            "the oracle must survive an unwritable state dir: {output:?}"
        );
    }

    #[test]
    fn a_helper_that_cannot_spawn_leaves_no_capture_behind() {
        // Reachable for real: the nvidia kernel module is loaded (so the arm runs)
        // but `nvidia-smi` itself is not installed.
        let dir = tempfile::tempdir().unwrap();
        let (ok, output) = run_captured(
            &["/nonexistent/definitely-not-a-binary".to_string()],
            dir.path(),
            "nvidia-smi",
            Duration::from_secs(1),
        );
        assert!(!ok);
        assert!(output.is_empty());
        assert!(
            captures(dir.path(), "nvidia-smi").is_empty(),
            "a spawn failure must leave no capture: {:?}",
            captures(dir.path(), "nvidia-smi")
        );
    }

    #[test]
    fn an_unusable_capture_dir_still_yields_a_verdict_with_evidence() {
        // The finding-1 repro shape: a state dir that cannot be written (here an
        // impossible path — ENOTDIR). The verdict must come from the child's exit
        // status, never from our ability to open a log file, and the temp-dir
        // fallback keeps the evidence that a diagnosis would otherwise lose.
        let out = run_probe(
            &sh("echo hi >&2; exit 0"),
            &[],
            Path::new("/dev/null/nope"),
            Duration::from_secs(5),
        );
        assert!(out.ok);
        assert!(
            !out.not_run,
            "it ran; only the preferred capture dir was unusable"
        );
        assert!(!out.no_capture, "the temp-dir fallback captured it");
        assert!(out.stderr.contains("hi"));
    }

    #[test]
    fn adapter_parse_ignores_lines_without_the_marker() {
        assert_eq!(parse_adapter("no adapters here\nnothing"), None);
        // Hardware names are parsed but not classified as software.
        let s = r#"[INFO blade] Adapter: "Intel(R) Arc(TM) A770 Graphics""#;
        let name = parse_adapter(s).unwrap();
        assert!(!adapter_is_software(&name));
    }

    #[test]
    fn adapter_parse_takes_the_last_line_the_accepted_device() {
        // blade logs `Adapter:` before its rejection checks, so a rejected
        // discrete GPU appears FIRST and the accepted device LAST. Taking the
        // first here would report software rendering as hardware.
        let stderr = concat!(
            "[INFO  blade_graphics::vulkan::init] Adapter: \"NVIDIA GeForce RTX 4090\"\n",
            "[WARN  blade_graphics::vulkan::init] \tRejected for API version 4198400\n",
            "[INFO  blade_graphics::vulkan::init] Adapter: \"llvmpipe (LLVM 19.1.0, 256 bits)\"\n",
        );
        let name = parse_adapter(stderr).expect("an adapter line is present");
        assert_eq!(name, "llvmpipe (LLVM 19.1.0, 256 bits)");
        assert!(
            adapter_is_software(&name),
            "the accepted device is the software one"
        );
    }

    #[test]
    fn unkillable_wait_still_returns_within_timeout_plus_grace() {
        // A child that ignores SIGTERM is still SIGKILL-able, so this only
        // exercises the fast path of the bounded reap; the point of the assert is
        // the INVARIANT — wait_with_timeout returns bounded, never blocking.
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = run_probe(
            &sh("trap '' TERM; sleep 30"),
            &[],
            dir.path(),
            Duration::from_millis(200),
        );
        assert!(out.timed_out);
        assert!(
            start.elapsed() < Duration::from_millis(200) + REAP_GRACE,
            "must return within timeout + REAP_GRACE, took {:?}",
            start.elapsed()
        );
    }
}
