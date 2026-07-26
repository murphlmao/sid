//! Linux GPU pre-flight adapter (`sid_core::gpu::GpuPreflight` impl).
//!
//! Strategy: because the renderer's init failure is an unrecoverable panic deep
//! in the frontend's windowing stack, all probing happens in a *subprocess* —
//! sid re-exec'd as `sid --gpu-probe`, which attempts the real renderer init
//! and exits 0 (healthy) or dies (broken). Driver crashes can never take down
//! the parent. On failure the remediation ladder retries the probe once per
//! installed driver manifest (`VK_DRIVER_FILES` pinned, hardware first,
//! software rasterizers last) and persists the winning pin; when nothing works,
//! a classifier turns the evidence into a `Diagnosis` (see `diagnose`).
//!
//! The probe verdict is cached in a stamp file keyed by a fingerprint of the
//! GPU environment (driver manifests + kernel module versions + the loader/display
//! environment + sid version), so healthy machines pay zero steady-state cost.
//! The stamp lives under `$XDG_STATE_HOME/sid` as a plain file — deliberately not
//! the redb store, so the probe subprocess can never contend for the store's
//! single-writer lock. A crash marker (set by the frontend around the real init)
//! invalidates the cache when reality broke without the fingerprint changing.
//!
//! This crate never LINKS or CALLS the renderer or windowing library — the probe
//! is a process boundary, which is what keeps the adapter seam swappable. It does,
//! however, depend on two OBSERVABLE details of the *current* renderer, both of
//! which must be updated if the renderer is swapped:
//!   1. the log target name in `RUST_LOG=blade_graphics=info`, set on the probe
//!      child (`probe::run_probe`); and
//!   2. the `Adapter: "..."` stderr line format that yields the adapter name and
//!      thus the hardware/software verdict (`probe::parse_adapter`, plus the
//!      `blade_graphics` mentions in `diagnose`'s error-shape arms).
//!
//! Nothing else in the crate knows what draws the window.
//!
//! It also never spawns threads (subprocess I/O is file+poll — see `probe`): the
//! frontend calls `std::env::set_var` right after `ensure_renderable` returns, and
//! edition 2024 makes that unsound with concurrent threads.

use sid_core::gpu::{Diagnosis, GpuPreflight, PreflightOk, RenderPath};
use std::path::PathBuf;
use std::time::Duration;

mod diagnose;
mod fingerprint;
mod icd;
mod ladder;
mod notify;
mod probe;
mod stamp;

/// File names under `state_dir`.
const STAMP_FILE: &str = "gpu-preflight.toml";

/// Prefix of the per-instance crash markers (`launch-attempt-<instance id>`).
///
/// Instance-scoped rather than one shared file: with a single fixed name, a
/// healthy-but-slow instance clearing "the" marker erased a *different*
/// instance's crash record, so the "distrust the stamp after a mid-init death"
/// guarantee could be defeated just by launching sid twice. Each instance now
/// sets and clears only its own marker, and ANY marker present at startup — ours
/// cannot exist yet — means some launch died mid-init, which is the signal.
const MARKER_PREFIX: &str = "launch-attempt";

/// What `sid --gpu-report` produced: the human-readable text, plus whether any
/// rung could actually render. The flag exists so the CLI can exit nonzero on a
/// machine that cannot start sid — a scriptable health check ("does sid run
/// here?") rather than a report that always claims success.
pub struct Report {
    pub text: String,
    pub renderable: bool,
}

/// The Linux pre-flight implementation. Construct via [`Self::from_env`] in
/// production; tests build the struct directly with fake roots/commands.
pub struct LinuxGpuPreflight {
    /// Directory for the stamp file, crash marker, and probe stderr capture.
    pub state_dir: PathBuf,
    /// The probe command line (argv[0] + args). Production: the current
    /// executable + `--gpu-probe`. Tests substitute e.g. `/bin/sh -c ...`.
    pub probe_cmd: Vec<String>,
    /// How long one probe attempt may run before it is killed and counted as
    /// a failure (hung drivers are failures too).
    pub timeout: Duration,
    /// Total wall clock the remediation ladder may spend before it stops climbing
    /// to further rungs. See `ladder::LADDER_BUDGET` for the production value and
    /// why a budget exists at all; injected (like `timeout`) so tests can exercise
    /// the cutoff in milliseconds.
    pub ladder_budget: Duration,
    /// Root for `/sys`, `/dev`, `/etc`, `/usr` inspection — `/` in production,
    /// a tempdir in tests.
    pub sys_root: PathBuf,
    /// Override the driver-manifest search directories (tests / introspection).
    /// `None` = the standard Vulkan loader search path.
    pub icd_dirs: Option<Vec<PathBuf>>,
    /// The mismatch oracle for the NVIDIA reboot-needed diagnosis — a real
    /// `nvidia-smi` in production, a scripted stand-in under test.
    pub nvidia_smi_cmd: Vec<String>,
    /// The application version, mixed into the fingerprint so an upgraded sid
    /// re-probes once.
    pub app_version: String,
    /// The loader/display environment the probe child will inherit, as sorted
    /// (key, value) pairs — see `fingerprint::env_snapshot`. Injected rather than
    /// read inside `fingerprint()` so tests stay hermetic without ever calling
    /// `set_var` (unsafe in edition 2024, and this crate mutates no environment).
    pub env_snapshot: Vec<(String, String)>,
    /// Identity of THIS running instance, used to name its crash marker (see
    /// [`MARKER_PREFIX`]).
    ///
    /// Must be unique per process: `from_env` mints one from pid + clock + a
    /// counter, because the pid alone is not unique across PID namespaces (every
    /// sandboxed instance is pid 1). Tests set a literal, which lets two structs
    /// over one state dir stand in for two concurrent processes.
    pub instance_id: String,
}

impl LinuxGpuPreflight {
    /// Production construction: XDG state dir (`$XDG_STATE_HOME/sid`, falling
    /// back to `~/.local/state/sid`), the current executable as the probe, a
    /// 10-second probe timeout, real system roots.
    pub fn from_env(app_version: &str) -> Self {
        let state_home = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/tmp"));
                home.join(".local/state")
            });
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "sid".to_string());
        Self {
            state_dir: state_home.join("sid"),
            probe_cmd: vec![exe, "--gpu-probe".to_string()],
            timeout: Duration::from_secs(10),
            ladder_budget: ladder::LADDER_BUDGET,
            sys_root: PathBuf::from("/"),
            icd_dirs: None,
            nvidia_smi_cmd: vec!["nvidia-smi".to_string()],
            app_version: app_version.to_string(),
            env_snapshot: fingerprint::env_snapshot(),
            instance_id: probe::unique_token(),
        }
    }

    fn stamp_path(&self) -> PathBuf {
        self.state_dir.join(STAMP_FILE)
    }

    /// This instance's own crash marker — the only one it ever writes or clears.
    fn marker_path(&self) -> PathBuf {
        self.state_dir
            .join(format!("{MARKER_PREFIX}-{}", self.instance_id))
    }

    /// Every launch-attempt marker currently in the state dir.
    ///
    /// Read at startup, before this instance writes its own, so anything found
    /// belongs to a previous (or concurrent) launch. Directory entries count as
    /// markers too: a directory squatting at a marker name is still evidence to
    /// us, and [`Self::sweep_markers`] knows how to remove one — otherwise it
    /// could never be cleared and would force a re-probe on every launch forever.
    fn markers(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.state_dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(MARKER_PREFIX))
            .map(|e| e.path())
            .collect()
    }

    /// Retire markers whose warning has been *answered* by a completed re-probe.
    ///
    /// Only ever called with the list read at startup, and only after a probe
    /// produced a verdict — see the call site for why both halves matter.
    fn sweep_markers(&self, markers: &[PathBuf]) {
        for marker in markers {
            if let Err(e) = remove_marker(marker) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "sid-gpu: could not clear stale launch marker {}: {e}",
                        marker.display()
                    );
                }
            }
        }
    }

    /// Record that a real launch attempt is starting (called immediately before
    /// the renderer is constructed). If this instance's marker is still present at
    /// a later startup, that init died despite a healthy stamp — the stamp is
    /// distrusted and a full re-probe runs. Best-effort: a failure to write it
    /// only costs cache-invalidation coverage, never the launch.
    pub fn mark_launch_attempt(&self) {
        if std::fs::create_dir_all(&self.state_dir)
            .and_then(|_| std::fs::write(self.marker_path(), b""))
            .is_err()
        {
            log::warn!("sid-gpu: could not write the launch-attempt marker");
        }
    }

    /// Clear this instance's crash marker (called once the renderer is alive).
    ///
    /// Strictly its own: clearing anyone else's is what made the marker
    /// defeatable — a healthy instance would erase a crashing instance's record.
    pub fn clear_launch_attempt(&self) {
        if let Err(e) = std::fs::remove_file(self.marker_path()) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("sid-gpu: could not clear the launch-attempt marker: {e}");
            }
        }
    }

    /// `sid --gpu-report`: run the full fingerprint + ladder live and render a
    /// human-readable report plus the verdict (see [`Report`]).
    ///
    /// Never mutates the stamp or the crash markers — the decision state is left
    /// exactly as the next real launch would find it (in particular it never
    /// sweeps a marker, so running the report cannot silently restore trust in a
    /// stamp the next launch is supposed to distrust). It is *not* side-effect free
    /// on disk, though: a uniquely-named probe capture is written (and removed once
    /// read) per rung, and a failing ladder reaches the diagnosis classifier, which
    /// may run (and capture the output of) `nvidia-smi`.
    pub fn report(&self) -> Report {
        let icds = icd::scan(&self.sys_root, self.icd_dirs.as_deref());
        let modules = fingerprint::kernel_modules(&self.sys_root);
        let fp = fingerprint::fingerprint(&icds, &modules, &self.env_snapshot, &self.app_version);

        let mut out = String::from("== fingerprint inputs ==\n");
        if icds.is_empty() {
            out.push_str("driver manifests: (none)\n");
        }
        for icd in &icds {
            let class = if icd.software { "software" } else { "hardware" };
            out.push_str(&format!(
                "driver manifest: {} [{class}] (mtime {})\n",
                icd.path.display(),
                icd.mtime_unix
            ));
        }
        for (name, version) in &modules {
            out.push_str(&format!("kernel module: {name} {version}\n"));
        }
        if self.env_snapshot.is_empty() {
            out.push_str("loader env: (none set)\n");
        }
        for (key, value) in &self.env_snapshot {
            out.push_str(&format!("loader env: {key}={value}\n"));
        }
        out.push_str(&format!(
            "app version: {}\nfingerprint: {fp}\n",
            self.app_version
        ));

        out.push_str("\n== stamp ==\n");
        match stamp::load(&self.stamp_path()) {
            None => out.push_str("absent (or unreadable/stale-version)\n"),
            Some(s) if s.fingerprint == fp => {
                out.push_str(&format!(
                    "matches current fingerprint — cached verdict: {} ({} pins)\n",
                    if s.software {
                        "software rendering"
                    } else {
                        "hardware"
                    },
                    s.pins.len()
                ));
            }
            Some(s) => out.push_str(&format!(
                "stale (stored fingerprint {}, current {fp})\n",
                s.fingerprint
            )),
        }

        out.push_str("\n== probe ladder ==\n");
        let run = ladder::run(self, &icds);
        for attempt in &run.attempts {
            let status = if attempt.outcome.not_run {
                // Checked FIRST and phrased as our failure, not the machine's: a
                // never-spawned probe has `exit: None` and `timed_out: false`, which
                // the arms below would render as "killed by signal" — blaming a
                // healthy GPU for our own inability to start a subprocess.
                format!("probe could not run: {}", attempt.outcome.stderr.trim())
            } else if attempt.outcome.ok {
                match &attempt.outcome.adapter {
                    Some(a) => format!("ok (adapter: {a})"),
                    None if attempt.outcome.no_capture => "ok (adapter unknown: no capture)".into(),
                    None => "ok".to_string(),
                }
            } else if attempt.outcome.timed_out {
                "timed out (killed)".to_string()
            } else {
                match attempt.outcome.exit {
                    Some(code) => format!("failed (exit {code})"),
                    None => "failed (killed by signal)".to_string(),
                }
            };
            out.push_str(&format!("{}: {status}\n", attempt.label));
        }
        // On STDOUT, deliberately: the live cutoff notice is an `eprintln!` in
        // `ladder::run`, and the documented bug-report flow is
        // `sid --gpu-report > report.txt` — which would otherwise show 2 of 4 rungs
        // with nothing to explain the two that are missing.
        if run.skipped_for_budget > 0 {
            out.push_str(&format!(
                "({} of {} attempts skipped: {:?} ladder budget exceeded)\n",
                run.skipped_for_budget,
                run.attempts.len() + run.skipped_for_budget,
                self.ladder_budget
            ));
        }

        // A probe that never ran is not a verdict about this machine, and
        // `ensure_renderable` fails open on it — so the report agrees, or its exit
        // code would contradict the app it exists to explain ("cannot render" on a
        // machine where sid then starts fine).
        let never_ran = run.attempts.first().is_some_and(|a| a.outcome.not_run);
        let renderable = run.winner.is_some() || never_ran;
        if run.winner.is_none() {
            if never_ran {
                out.push_str(
                    "\n== diagnosis ==\nnone — the probe itself could not be started, which is \
                     not evidence about this machine's GPU.\nsid fails open on this: it will \
                     start and attempt hardware rendering.\n",
                );
            } else {
                let rung0 = run.attempts.first();
                let rung0_stderr = rung0.map(|a| a.outcome.stderr.clone()).unwrap_or_default();
                let evidence = diagnose::LadderEvidence {
                    rung0_stderr: &rung0_stderr,
                    rung0_no_capture: rung0.is_some_and(|a| a.outcome.no_capture),
                    all_timed_out: run.all_attempts_timed_out(),
                };
                let d = diagnose::diagnose(self, &icds, &modules, &evidence);
                out.push_str(&format!(
                    "\n== diagnosis ==\n{}\nremedy: {}\n\n{}",
                    d.summary, d.remedy, d.detail
                ));
            }
        }
        Report {
            text: out,
            renderable,
        }
    }

    /// The real flow, with the escape hatch injected so tests never mutate the
    /// process environment (`set_var` is unsafe in edition 2024, tests run
    /// multi-threaded). The trait method reads the env and delegates here.
    fn ensure_renderable_inner(&self, skip_requested: bool) -> Result<PreflightOk, Diagnosis> {
        if skip_requested {
            return Ok(PreflightOk {
                path: RenderPath::Hardware,
                env_pins: Vec::new(),
            });
        }

        let icds = icd::scan(&self.sys_root, self.icd_dirs.as_deref());
        let modules = fingerprint::kernel_modules(&self.sys_root);
        let fp = fingerprint::fingerprint(&icds, &modules, &self.env_snapshot, &self.app_version);

        // ANY surviving marker (this instance has not written its own yet) means
        // some real init died after a green verdict — reality broke without the
        // fingerprint changing (e.g. a compositor/driver state change). Distrust
        // the cache, re-probe. The list is kept for the sweep below, which happens
        // only on the success path.
        let stale_markers = self.markers();
        if !stale_markers.is_empty() {
            log::warn!(
                "sid-gpu: {} launch attempt(s) did not complete; re-probing",
                stale_markers.len()
            );
        } else if let Some(s) = stamp::load(&self.stamp_path()) {
            if s.fingerprint == fp {
                return Ok(remembered(s));
            }
        }

        let run = ladder::run(self, &icds);
        match run.winner {
            Some(ix) => {
                let attempt = &run.attempts[ix];
                let path = ladder::winning_path(attempt, ix == 0);
                let (software, software_reason) = match &path {
                    RenderPath::Software { reason } => (true, Some(reason.clone())),
                    RenderPath::Hardware => (false, None),
                };
                if verdict_is_cacheable(&attempt.outcome, software) {
                    let stamp = stamp::Stamp {
                        version: stamp::STAMP_VERSION,
                        fingerprint: fp,
                        pins: attempt
                            .pins
                            .iter()
                            .map(|(key, value)| stamp::Pin {
                                key: key.clone(),
                                value: value.clone(),
                            })
                            .collect(),
                        software,
                        software_reason,
                        saved_at_unix: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                    };
                    // A failed save only costs the cache — never the launch.
                    if let Err(e) = stamp::save(&self.stamp_path(), &stamp) {
                        log::warn!("sid-gpu: could not save the pre-flight stamp: {e}");
                    }
                } else {
                    log::warn!(
                        "sid-gpu: rendering with a hardware verdict that could not be \
                         corroborated (no probe output was capturable, so no adapter name); \
                         not caching it — the next launch will re-probe"
                    );
                }
                // The markers found at startup have now been answered by a probe
                // that reached a verdict, so retire them. Deliberately NOT swept at
                // detection time: a re-probe that ends in FAILURE must leave them in
                // place, or the launch after it would trust the very stamp the
                // marker exists to distrust. And deliberately only the ones seen
                // then — a marker written during our probe belongs to a concurrent
                // instance whose init we know nothing about.
                self.sweep_markers(&stale_markers);
                Ok(PreflightOk {
                    path,
                    env_pins: attempt.pins.clone(),
                })
            }
            None => {
                // FAIL OPEN when the probe never even started. A missing/unreadable
                // executable, a fork failure under RLIMIT_NPROC, a sandbox that
                // forbids exec — none of these are statements about the GPU, and
                // gating a machine that renders perfectly on an unrelated local
                // condition is a far worse failure than an un-preflighted launch.
                // No stamp is written here (the arm cannot reach the save above),
                // which is the other half of the rule: a verdict we never observed
                // must never be cached.
                if run.attempts.first().is_some_and(|a| a.outcome.not_run) {
                    let why = run.attempts[0].outcome.stderr.trim().to_string();
                    // Failing open must not also throw away the configuration the
                    // stamp already PROVED necessary. Reaching here means the stamp
                    // was distrusted (fingerprint change, or a crash marker) and
                    // then nothing could be verified — but "unverified" is not
                    // "wrong": dropping e.g. a remembered `VK_DRIVER_FILES` pin
                    // launches the hybrid-GPU machine the ladder exists for straight
                    // into the uncatchable renderer panic, which is the exact
                    // failure this crate was written to prevent. Its render path
                    // travels with its pins (see `remembered`).
                    //
                    // Still no stamp write and no marker sweep: nothing was
                    // observed, so nothing may be freshly cached, and the marker's
                    // warning is unanswered — the next launch must distrust it too.
                    if let Some(s) = stamp::load(&self.stamp_path()) {
                        let ok = remembered(s);
                        log::warn!(
                            "sid-gpu: GPU pre-flight could not run ({why}); reusing the last \
                             known-good GPU configuration UNVERIFIED ({} pin(s), {})",
                            ok.env_pins.len(),
                            match &ok.path {
                                RenderPath::Software { .. } => "software rendering",
                                RenderPath::Hardware => "hardware rendering",
                            }
                        );
                        return Ok(ok);
                    }
                    log::warn!(
                        "sid-gpu: GPU pre-flight could not run ({why}); proceeding without it"
                    );
                    return Ok(PreflightOk {
                        path: RenderPath::Hardware,
                        env_pins: Vec::new(),
                    });
                }
                let rung0 = run.attempts.first();
                let rung0_stderr = rung0.map(|a| a.outcome.stderr.clone()).unwrap_or_default();
                let evidence = diagnose::LadderEvidence {
                    rung0_stderr: &rung0_stderr,
                    rung0_no_capture: rung0.is_some_and(|a| a.outcome.no_capture),
                    all_timed_out: run.all_attempts_timed_out(),
                };
                Err(diagnose::diagnose(self, &icds, &modules, &evidence))
            }
        }
    }
}

/// Remove one launch-attempt marker, reporting the error that actually describes
/// the entry.
///
/// A plain file is unlinked with `remove_file`; only a real directory goes through
/// `remove_dir_all` (a squatting directory there would otherwise be permanent, and
/// permanent means "re-probe forever"). Trying the directory removal
/// unconditionally as a fallback *masked the real errno*: an unremovable file in a
/// write-protected state dir was reported as `Not a directory (os error 20)` —
/// `remove_dir_all`'s complaint about a file — instead of the
/// `Permission denied (os error 13)` that tells the user what to fix.
fn remove_marker(marker: &std::path::Path) -> std::io::Result<()> {
    let is_dir = std::fs::symlink_metadata(marker).is_ok_and(|m| m.is_dir());
    if is_dir {
        std::fs::remove_dir_all(marker)
    } else {
        // A symlink (even to a directory) is unlinked, not followed — the same
        // call that created it as evidence can retire it.
        std::fs::remove_file(marker)
    }
}

/// The configuration a stamp remembers, as a pre-flight answer.
///
/// One function for both readers in `ensure_renderable_inner` — the ordinary cache
/// hit and the fail-open reuse — because a stamp's pins and its verdict are one
/// decision and must never be taken apart: pins that select lavapipe carried under
/// a `Hardware` label would mislabel the badge (silent software rendering), and a
/// `Software` label without its pins would claim a fallback the loader was never
/// pointed at.
fn remembered(stamp: stamp::Stamp) -> PreflightOk {
    let path = if stamp.software {
        RenderPath::Software {
            reason: stamp
                .software_reason
                .unwrap_or_else(|| "software rendering".to_string()),
        }
    } else {
        RenderPath::Hardware
    };
    PreflightOk {
        path,
        env_pins: stamp.pins.into_iter().map(|p| (p.key, p.value)).collect(),
    }
}

/// Whether a winning rung's verdict may be *cached* in the stamp.
///
/// The verdict itself is always trusted: it is the child's exit status, which
/// survives everything, including a capture we could not open. The stamp is a
/// different promise — it makes the next launch skip probing entirely — so a
/// cached hardware/software classification that rested on evidence we never got
/// would silently pin a software-only machine to `Hardware` (no badge, wrong
/// path) until the fingerprint changes. That is exactly the corruption an
/// unwritable state dir produced.
///
/// So: cache freely whenever the classification is grounded — the rung pinned a
/// software manifest, or an adapter name was parsed. When it is grounded in
/// nothing (capture lost AND no adapter name AND no software pin), return the
/// verdict and skip the cache; the next launch re-probes, most likely with a
/// writable capture dir, and decides for real. The cost is one probe per launch
/// for as long as the condition lasts.
fn verdict_is_cacheable(outcome: &probe::ProbeOutcome, software: bool) -> bool {
    software || !(outcome.no_capture && outcome.adapter.is_none())
}

impl GpuPreflight for LinuxGpuPreflight {
    fn ensure_renderable(&self) -> Result<PreflightOk, Diagnosis> {
        // `SID_GPU_SKIP_PREFLIGHT`: the break-glass escape hatch (and the guard
        // that keeps any accidental nested launch from probing recursively).
        let skip = std::env::var_os("SID_GPU_SKIP_PREFLIGHT").is_some();
        self.ensure_renderable_inner(skip)
    }
}

/// Best-effort desktop notification for a terminal pre-flight failure (sid may
/// have been launched from a desktop entry where stderr is invisible). Silently
/// a no-op when no notification agent is available.
pub fn send_failure_notification(diagnosis: &Diagnosis) {
    notify::notify_failure(diagnosis);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    fn pf(state: &Path, probe: Vec<String>, icd_dir: &Path) -> LinuxGpuPreflight {
        LinuxGpuPreflight {
            state_dir: state.to_path_buf(),
            probe_cmd: probe,
            timeout: Duration::from_secs(5),
            ladder_budget: ladder::LADDER_BUDGET,
            sys_root: PathBuf::from("/nonexistent"),
            icd_dirs: Some(vec![icd_dir.to_path_buf()]),
            nvidia_smi_cmd: vec!["/bin/true".into()],
            app_version: "test".into(),
            // Hermetic: tests never read (or mutate) the real environment.
            env_snapshot: Vec::new(),
            instance_id: "inst-solo".into(),
        }
    }

    /// The same, standing in for a *different process*: a distinct instance id is
    /// what makes two structs over one state dir behave like two running sids.
    fn pf_as(
        state: &Path,
        probe: Vec<String>,
        icd_dir: &Path,
        instance: &str,
    ) -> LinuxGpuPreflight {
        LinuxGpuPreflight {
            instance_id: instance.into(),
            ..pf(state, probe, icd_dir)
        }
    }

    #[test]
    fn skip_env_short_circuits_without_probing() {
        let state = tempfile::tempdir().unwrap();
        let icds = tempfile::tempdir().unwrap();
        // A probe that would fail loudly if it ran.
        let p = pf(state.path(), sh("exit 1"), icds.path());
        let ok = p
            .ensure_renderable_inner(true)
            .expect("skip always succeeds");
        assert_eq!(ok.path, RenderPath::Hardware);
        assert!(ok.env_pins.is_empty());
    }

    #[test]
    fn first_run_probes_saves_stamp_and_second_run_skips_the_probe() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        // First run: vanilla probe succeeds; a stamp lands on disk.
        let p_ok = pf(state.path(), sh("exit 0"), icd_dir.path());
        let ok = p_ok
            .ensure_renderable_inner(false)
            .expect("vanilla success");
        assert_eq!(ok.path, RenderPath::Hardware);
        assert!(p_ok.stamp_path().exists(), "verdict must be cached");

        // Second run, same fingerprint, but a probe that would FAIL if it ran:
        // the stamp hit must return without probing at all.
        let p_broken_probe = pf(state.path(), sh("exit 1"), icd_dir.path());
        let ok = p_broken_probe
            .ensure_renderable_inner(false)
            .expect("stamp hit skips the probe");
        assert_eq!(ok.path, RenderPath::Hardware);
    }

    #[test]
    fn crash_marker_forces_a_reprobe_despite_a_matching_stamp() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        let p_ok = pf_as(state.path(), sh("exit 0"), icd_dir.path(), "inst-crashed");
        p_ok.ensure_renderable_inner(false).expect("seed the stamp");

        // The frontend marked a launch attempt and never cleared it (init died).
        p_ok.mark_launch_attempt();
        // With the marker present the stamp must be ignored — this probe fails,
        // and with no manifests to pin, the whole ladder fails.
        let p_broken = pf_as(state.path(), sh("exit 1"), icd_dir.path(), "inst-next");
        let err = p_broken
            .ensure_renderable_inner(false)
            .expect_err("marker must force a live re-probe");
        // (Empty fake world → the no-driver arm.)
        assert_eq!(err.cause, sid_core::gpu::FailureCause::NoDriverInstalled);

        // Clearing the marker restores stamp trust.
        p_ok.clear_launch_attempt();
        let ok = p_broken
            .ensure_renderable_inner(false)
            .expect("stamp trusted again once the marker is gone");
        assert_eq!(ok.path, RenderPath::Hardware);
    }

    #[test]
    fn one_instances_success_cannot_erase_anothers_crash_record() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        let a = pf_as(state.path(), sh("exit 0"), icd_dir.path(), "inst-a");
        let b = pf_as(state.path(), sh("exit 0"), icd_dir.path(), "inst-b");
        a.ensure_renderable_inner(false).expect("seed the stamp");

        // Two sids are mid-launch at once, each with its own marker...
        a.mark_launch_attempt();
        b.mark_launch_attempt();
        assert_ne!(a.marker_path(), b.marker_path());
        assert_eq!(a.markers().len(), 2, "markers are per instance, not shared");

        // ...A (healthy, slow) comes up and clears ITS marker. B's crash record
        // must survive — with one shared marker file, A's success erased it, which
        // is how the "distrust the stamp after a mid-init death" guarantee was
        // defeatable by simply launching sid twice.
        a.clear_launch_attempt();
        assert!(!a.marker_path().exists());
        assert!(
            b.marker_path().exists(),
            "only the owner may clear a marker"
        );

        // So the next launch still distrusts the stamp and re-probes live.
        let c = pf_as(state.path(), sh("exit 1"), icd_dir.path(), "inst-c");
        let err = c
            .ensure_renderable_inner(false)
            .expect_err("a foreign marker must force a live re-probe");
        assert_eq!(err.cause, sid_core::gpu::FailureCause::NoDriverInstalled);
        // And a re-probe that FAILED must not retire the warning: the launch after
        // it has to distrust the stamp too, or a crash loop hides behind a cache.
        assert!(
            b.marker_path().exists(),
            "a failed re-probe answers nothing"
        );
    }

    #[test]
    fn a_successful_reprobe_retires_the_markers_it_answered() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        // An instance that died mid-init and can never clear its own marker.
        let dead = pf_as(state.path(), sh("exit 0"), icd_dir.path(), "inst-dead");
        dead.mark_launch_attempt();

        let next = pf_as(state.path(), sh("exit 0"), icd_dir.path(), "inst-next");
        next.ensure_renderable_inner(false)
            .expect("the re-probe succeeds");
        assert!(
            next.markers().is_empty(),
            "an answered warning must be retired, or every launch re-probes forever"
        );
        // Which is the observable point: the launch after it trusts the stamp
        // again (this probe would fail if it ran).
        let after = pf_as(state.path(), sh("exit 1"), icd_dir.path(), "inst-after");
        after
            .ensure_renderable_inner(false)
            .expect("stamp trusted once the marker is retired");
    }

    #[test]
    fn a_directory_squatting_at_a_marker_path_is_still_retired() {
        // `remove_file` cannot unlink a directory, so without the fallback such a
        // squatter would force a full re-probe on every launch, forever.
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        let squatter = state.path().join(format!("{MARKER_PREFIX}-squatter"));
        std::fs::create_dir_all(squatter.join("nested")).unwrap();

        let p = pf(state.path(), sh("exit 0"), icd_dir.path());
        p.ensure_renderable_inner(false)
            .expect("the probe succeeds");
        assert!(
            !squatter.exists(),
            "a directory marker must be removable too"
        );
    }

    #[test]
    fn a_probe_that_cannot_run_fails_open_and_caches_nothing() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        // The unrelated-local-condition shape: the probe binary isn't there (a
        // sandbox forbidding exec or an RLIMIT_NPROC ceiling look the same). Not one
        // bit has been learned about the GPU, so a machine that renders perfectly
        // must still start.
        let p = pf(
            state.path(),
            vec!["/nonexistent/definitely-not-a-binary".into()],
            icd_dir.path(),
        );
        let ok = p
            .ensure_renderable_inner(false)
            .expect("must fail open rather than block the launch");
        assert_eq!(ok.path, RenderPath::Hardware);
        assert!(ok.env_pins.is_empty());
        assert!(
            !p.stamp_path().exists(),
            "a verdict we never observed must never be cached"
        );
    }

    #[test]
    fn repeated_fail_open_launches_leave_no_probe_captures_behind() {
        // Finding 1, end to end: the capture is opened before the spawn attempt, and
        // this path writes no stamp — so it re-probes on EVERY launch. A capture left
        // behind here is one 0-byte `probe-*.log` per launch, forever, on the one
        // path that never stops re-probing.
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        let p = pf(
            state.path(),
            vec!["/nonexistent/definitely-not-a-binary".into()],
            icd_dir.path(),
        );
        for _ in 0..5 {
            p.ensure_renderable_inner(false)
                .expect("fails open every time");
        }
        let leftovers: Vec<String> = std::fs::read_dir(state.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("probe-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "5 launches, 0 leftovers expected: {leftovers:?}"
        );
    }

    #[test]
    fn a_probe_that_cannot_run_reuses_the_last_known_good_configuration() {
        // Finding 7: failing open must not throw away the pins the stamp already
        // PROVED necessary. Without them this launches the hybrid-GPU machine the
        // ladder exists for straight into the uncatchable renderer panic.
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        // Seed a stamp whose pin is load-bearing: this probe succeeds ONLY when
        // `VK_DRIVER_FILES` is exported, i.e. on the pinned rung.
        let seed = pf(
            state.path(),
            sh(r#"[ -n "$VK_DRIVER_FILES" ]"#),
            icd_dir.path(),
        );
        let seeded = seed
            .ensure_renderable_inner(false)
            .expect("the pinned rung wins");
        assert_eq!(seeded.env_pins.len(), 2);
        let stamp_before = stamp::load(&seed.stamp_path()).expect("seeded");

        // Now invalidate the stamp two ways at once — a driver install changes the
        // fingerprint, and a dead instance's marker demands a re-probe...
        std::fs::write(icd_dir.path().join("intel_icd.json"), "{}").unwrap();
        let dead = pf_as(state.path(), sh("exit 0"), icd_dir.path(), "inst-dead");
        dead.mark_launch_attempt();
        // ...and make the re-probe impossible to spawn (RLIMIT_NPROC / sandbox).
        let p = pf(
            state.path(),
            vec!["/nonexistent/definitely-not-a-binary".into()],
            icd_dir.path(),
        );
        let ok = p
            .ensure_renderable_inner(false)
            .expect("must still fail open");
        assert!(
            ok.env_pins
                .iter()
                .any(|(k, v)| k == "VK_DRIVER_FILES" && v.contains("radeon_icd.json")),
            "the remembered pin must be carried: {:?}",
            ok.env_pins
        );
        assert_eq!(ok.path, RenderPath::Hardware, "as the stamp remembered it");

        // Unverified means unrecorded: the stamp is untouched (not re-saved under
        // the new fingerprint)...
        let stamp_after = stamp::load(&p.stamp_path()).expect("still the seeded stamp");
        assert_eq!(stamp_after.fingerprint, stamp_before.fingerprint);
        assert_eq!(stamp_after.saved_at_unix, stamp_before.saved_at_unix);
        // ...and the marker's warning is unanswered, so the next launch distrusts
        // the stamp exactly as this one did.
        assert!(
            dead.marker_path().exists(),
            "a probe that never ran answers nothing"
        );
    }

    #[test]
    fn a_reused_configuration_keeps_its_software_label() {
        // The other half of finding 7: pins that select lavapipe carried under a
        // `Hardware` label would mislabel the badge — silent software rendering.
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("lvp_icd.x86_64.json"), "{}").unwrap();
        let seed = pf(
            state.path(),
            sh(r#"[ -n "$VK_DRIVER_FILES" ]"#),
            icd_dir.path(),
        );
        assert!(matches!(
            seed.ensure_renderable_inner(false)
                .expect("the software rung wins")
                .path,
            RenderPath::Software { .. }
        ));

        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        let p = pf(
            state.path(),
            vec!["/nonexistent/definitely-not-a-binary".into()],
            icd_dir.path(),
        );
        let ok = p.ensure_renderable_inner(false).expect("fails open");
        let RenderPath::Software { reason } = &ok.path else {
            panic!("a remembered software configuration must stay Software: {ok:?}");
        };
        assert!(reason.contains("lvp_icd"), "{reason}");
        assert!(
            ok.env_pins
                .iter()
                .any(|(k, v)| k == "VK_DRIVER_FILES" && v.contains("lvp_icd.x86_64.json"))
        );
    }

    #[test]
    fn a_marker_removal_failure_reports_its_own_errno() {
        // Finding 5: trying `remove_dir_all` as an unconditional fallback masked the
        // real error — a write-protected state dir was reported as
        // "Not a directory (os error 20)" (remove_dir_all complaining about a file)
        // instead of the "Permission denied" that names the actual fault.
        use std::os::unix::fs::PermissionsExt as _;
        let state = tempfile::tempdir().unwrap();
        let marker = state.path().join(format!("{MARKER_PREFIX}-locked"));
        std::fs::write(&marker, b"").unwrap();
        // Unlinking an entry needs write permission on its PARENT directory.
        std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let unrestricted = std::fs::write(state.path().join("root-can-write"), b"").is_ok();
        let result = remove_marker(&marker);
        std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        if unrestricted {
            // Running as root (or with CAP_DAC_OVERRIDE): permissions cannot be made
            // to bite, so there is nothing to assert about the errno.
            return;
        }
        let err = result.expect_err("an unwritable parent cannot be unlinked from");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::PermissionDenied,
            "the error must describe the entry, not the fallback: {err}"
        );
        assert!(!err.to_string().contains("Not a directory"), "{err}");
    }

    #[test]
    fn the_report_body_explains_a_budget_cut_ladder() {
        // Finding 6: the cutoff notice is an `eprintln!`, but the report body goes
        // to stdout — and the documented bug-report flow is
        // `sid --gpu-report > report.txt`, which showed 2 of 4 rungs with no reason.
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        for name in ["a_icd.json", "b_icd.json", "c_icd.json"] {
            std::fs::write(icd_dir.path().join(name), "{}").unwrap();
        }
        // The mute-compositor shape: every rung hangs and is killed.
        let mut p = pf(state.path(), sh("sleep 30"), icd_dir.path());
        p.timeout = Duration::from_millis(150);
        p.ladder_budget = Duration::from_millis(200);

        let r = p.report();
        assert!(
            r.text.contains("2 of 4 attempts skipped"),
            "the report body must account for every rung: {}",
            r.text
        );
        assert!(r.text.contains("ladder budget exceeded"), "{}", r.text);
        assert!(!r.renderable, "nothing rendered");
    }

    #[test]
    fn report_says_the_probe_could_not_run_instead_of_blaming_a_signal() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        let p = pf(
            state.path(),
            vec!["/nonexistent/definitely-not-a-binary".into()],
            icd_dir.path(),
        );
        let r = p.report();
        assert!(r.text.contains("probe could not run"), "{}", r.text);
        // `exit: None` + `!timed_out` used to render as a signal death, blaming the
        // machine for our own inability to spawn a subprocess.
        assert!(!r.text.contains("killed by signal"), "{}", r.text);
        // And the report agrees with `ensure_renderable`, which fails open here —
        // otherwise `--gpu-report` exits nonzero on a machine where sid starts fine.
        assert!(
            r.renderable,
            "the report must not claim sid cannot start here"
        );
    }

    /// The stamp policy in isolation (the both-dirs-unwritable state it guards
    /// against cannot be faked without mutating the environment).
    #[test]
    fn only_grounded_verdicts_are_cacheable() {
        let outcome = |no_capture: bool, adapter: Option<&str>| probe::ProbeOutcome {
            ok: true,
            timed_out: false,
            exit: Some(0),
            stderr: String::new(),
            adapter: adapter.map(str::to_string),
            not_run: false,
            no_capture,
        };
        // Grounded: evidence captured, or an adapter name, or a software pin that
        // decided the classification without needing either.
        assert!(verdict_is_cacheable(&outcome(false, None), false));
        assert!(verdict_is_cacheable(
            &outcome(true, Some("llvmpipe")),
            false
        ));
        assert!(verdict_is_cacheable(&outcome(true, None), true));
        // Grounded in nothing: `Hardware` claimed with no capture and no adapter
        // name is a guess, and caching it pins a software-only box to Hardware.
        assert!(!verdict_is_cacheable(&outcome(true, None), false));
    }

    #[test]
    fn fingerprint_change_invalidates_the_stamp() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        let p_ok = pf(state.path(), sh("exit 0"), icd_dir.path());
        p_ok.ensure_renderable_inner(false).expect("seed the stamp");

        // A driver install appears → fingerprint changes → live probe runs
        // again (observable via a now-failing probe that pins do rescue).
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        let p_pin_only = pf(
            state.path(),
            sh(r#"[ -n "$VK_DRIVER_FILES" ]"#),
            icd_dir.path(),
        );
        let ok = p_pin_only
            .ensure_renderable_inner(false)
            .expect("ladder rescues via the new manifest");
        assert_eq!(
            ok.env_pins.len(),
            2,
            "the winning pin is returned to the caller"
        );
        assert!(ok.env_pins.iter().any(|(k, _)| k == "VK_DRIVER_FILES"));

        // And the rescued verdict is itself cached, pins included.
        let s = stamp::load(&p_pin_only.stamp_path()).expect("stamp rewritten");
        assert_eq!(s.pins.len(), 2);
    }

    #[test]
    fn software_fallback_verdict_round_trips_through_the_stamp() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("lvp_icd.x86_64.json"), "{}").unwrap();
        let p = pf(
            state.path(),
            sh(r#"[ -n "$VK_DRIVER_FILES" ]"#),
            icd_dir.path(),
        );
        let ok = p
            .ensure_renderable_inner(false)
            .expect("software rung wins");
        let RenderPath::Software { reason } = ok.path else {
            panic!("expected the software path");
        };
        assert!(reason.contains("lvp_icd"));

        // Cached: a second run returns the same software verdict from the stamp.
        let p2 = pf(state.path(), sh("exit 1"), icd_dir.path());
        let ok2 = p2
            .ensure_renderable_inner(false)
            .expect("cached software verdict");
        assert!(matches!(ok2.path, RenderPath::Software { .. }));
    }

    #[test]
    fn total_failure_diagnoses_instead_of_panicking() {
        let state = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        let p = pf(state.path(), sh("exit 1"), icd_dir.path());
        let err = p
            .ensure_renderable_inner(false)
            .expect_err("no rung can win");
        assert!(!err.summary.is_empty());
        assert!(!err.remedy.is_empty());
        assert!(!p.stamp_path().exists(), "failures are never cached");
    }
}
