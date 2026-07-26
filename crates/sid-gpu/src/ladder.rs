//! The remediation ladder: vanilla probe first, then one probe per driver
//! manifest with the loader pinned to it (`VK_DRIVER_FILES`), hardware
//! manifests before software ones. First success wins. This is what turns "one
//! broken driver poisons device enumeration" — the classic hybrid-GPU failure —
//! into a self-healing launch instead of a panic.

use std::time::Instant;

use sid_core::gpu::RenderPath;

use crate::LinuxGpuPreflight;
use crate::icd::IcdManifest;
use crate::probe::{self, ProbeOutcome};

/// One rung's record — kept for `--gpu-report` and the failure diagnosis, not
/// just the winner.
#[derive(Debug, Clone)]
pub(crate) struct RungAttempt {
    /// Display label: "vanilla" or the manifest file name.
    pub label: String,
    /// The env pins this rung exported (empty for vanilla).
    pub pins: Vec<(String, String)>,
    /// Whether the pinned manifest was classified software.
    pub software_icd: bool,
    pub outcome: ProbeOutcome,
}

#[derive(Debug)]
pub(crate) struct LadderRun {
    /// Every rung attempted, in order; rung 0 is always the vanilla probe.
    pub attempts: Vec<RungAttempt>,
    /// Index into `attempts` of the first success, if any.
    pub winner: Option<usize>,
    /// Rungs that existed but were never tried because [`LADDER_BUDGET`] was
    /// already spent (0 whenever the ladder ran to its end).
    ///
    /// Carried rather than merely logged because the cutoff notice goes to
    /// stderr, while `--gpu-report`'s body goes to stdout — and the documented
    /// bug-report flow is `sid --gpu-report > report.txt`. Without this the report
    /// showed 2 of 4 rungs with no explanation for the two missing ones, which
    /// reads as a truncated ladder rather than a deliberate budget cutoff.
    pub skipped_for_budget: usize,
}

impl LadderRun {
    /// Every rung that was attempted was killed at the timeout — *nothing ever
    /// answered*. A diagnosable shape in its own right (a hung compositor or a
    /// wedged driver, not a misconfiguration), so `diagnose` gets told about it
    /// rather than being left to guess from a stderr tail that is, by definition,
    /// truncated mid-init.
    pub fn all_attempts_timed_out(&self) -> bool {
        !self.attempts.is_empty() && self.attempts.iter().all(|a| a.outcome.timed_out)
    }
}

/// Wall clock after which the ladder stops climbing.
///
/// Every remaining rung costs a full `timeout` of *complete silence* in front of
/// a user staring at a window that has not appeared: measured against a mute
/// compositor socket, three manifests cost 40.17s, and six ICDs would cost 70–90s
/// (`timeout × (1 + N_manifests)`). Past this point more rungs are unlikely to win
/// and certain to hurt — a diagnosis now beats a rescue later. (Same machine,
/// same mute socket, with the budget: 21s regardless of manifest count.)
///
/// The FIRST pinned rung is deliberately exempt: routing around one broken
/// driver is the ladder's whole reason to exist, and that rung is what rescues
/// the common hybrid-GPU machine. So the worst case is bounded by
/// `2 × timeout + REAP_GRACE` (~22s with the production 10s timeout) instead of
/// growing with the number of drivers installed.
///
/// Injected as [`LinuxGpuPreflight::ladder_budget`] (like `timeout`) so tests can
/// exercise the cutoff in milliseconds; this is the production value.
pub(crate) const LADDER_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);

pub(crate) fn run(pf: &LinuxGpuPreflight, icds: &[IcdManifest]) -> LadderRun {
    // The capture DIRECTORY, not a file: `probe::run_probe` names each attempt's
    // capture uniquely inside it (O_EXCL, pid+nanos+counter) and removes it once
    // read. A shared file name — even a per-pid one, since every process in a
    // fresh PID namespace is pid 1 — let a concurrent instance truncate this
    // one's in-flight capture, so the post-wait read could return the OTHER
    // instance's child output, cross-contaminating `parse_adapter` and persisting
    // a wrong Software/Hardware verdict in the stamp.
    let capture_dir = pf.state_dir.as_path();
    let total_rungs = 1 + icds.len();
    let started = Instant::now();
    let mut attempts = Vec::new();

    progress(1, total_rungs, "vanilla");
    let outcome = probe::run_probe(&pf.probe_cmd, &[], capture_dir, pf.timeout);
    let won = outcome.ok;
    // Nothing was spawned, so no pin can change the answer: climbing would only
    // repeat the same spawn failure N times. `ensure_renderable` reads this shape
    // off rung 0 and fails open — it is not evidence about the GPU.
    let never_ran = outcome.not_run;
    attempts.push(RungAttempt {
        label: "vanilla".into(),
        pins: Vec::new(),
        software_icd: false,
        outcome,
    });
    if won {
        return LadderRun {
            attempts,
            winner: Some(0),
            skipped_for_budget: 0,
        };
    }
    if never_ran {
        return LadderRun {
            attempts,
            winner: None,
            skipped_for_budget: 0,
        };
    }

    let mut skipped_for_budget = 0;
    for (rung, icd) in icds.iter().enumerate() {
        // Rung 1 (`rung == 0` here) always runs; beyond it, respect the budget.
        if rung > 0 && started.elapsed() >= pf.ladder_budget {
            skipped_for_budget = total_rungs - attempts.len();
            log::warn!(
                "GPU pre-flight: giving up after {:?} with {} of {} rungs tried",
                started.elapsed(),
                attempts.len(),
                total_rungs
            );
            // Live feedback for someone watching the launch; the same fact reaches
            // a redirected `--gpu-report` through `skipped_for_budget` above.
            eprintln!(
                "sid: GPU probing exceeded its {:?} budget after {} of {} attempts — \
                 diagnosing instead of retrying",
                pf.ladder_budget,
                attempts.len(),
                total_rungs
            );
            break;
        }
        let manifest = icd.path.to_string_lossy().into_owned();
        // VK_DRIVER_FILES is the current loader spelling; VK_ICD_FILENAMES the
        // pre-1.3.207 one — export both so the pin works on older loaders too.
        let pins = vec![
            ("VK_DRIVER_FILES".to_string(), manifest.clone()),
            ("VK_ICD_FILENAMES".to_string(), manifest),
        ];
        log::info!(
            "GPU pre-flight: retrying with driver manifest {}",
            icd.file_name()
        );
        progress(rung + 2, total_rungs, &icd.file_name());
        let outcome = probe::run_probe(&pf.probe_cmd, &pins, capture_dir, pf.timeout);
        let won = outcome.ok;
        attempts.push(RungAttempt {
            label: icd.file_name(),
            pins,
            software_icd: icd.software,
            outcome,
        });
        if won {
            let winner = attempts.len() - 1;
            return LadderRun {
                attempts,
                winner: Some(winner),
                skipped_for_budget: 0,
            };
        }
    }
    LadderRun {
        attempts,
        winner: None,
        skipped_for_budget,
    }
}

/// Tell the user what the silence is. Deliberately `eprintln!` and not `log`:
/// this runs before any window exists, the default filter is `warn`, and a
/// progress line that only appears under `RUST_LOG` is not a progress line. Each
/// rung can burn a full probe timeout, so an unannounced ladder is indistinguishable
/// from a hang.
fn progress(rung: usize, total: usize, label: &str) {
    eprintln!("sid: probing GPU (rung {rung}/{total}: {label})…");
}

/// The winning rung's render path. Software is detected two ways: the pinned
/// manifest's classification, or the adapter name blade logged (catches the
/// vanilla rung succeeding on a lavapipe-only machine, where nothing was
/// pinned). A hardware rung with no parseable adapter line stays Hardware —
/// missing telemetry must not spook users with a false "software" badge.
pub(crate) fn winning_path(attempt: &RungAttempt, is_vanilla_rung: bool) -> RenderPath {
    let software_adapter = attempt
        .outcome
        .adapter
        .as_deref()
        .map(probe::adapter_is_software)
        .unwrap_or(false);
    if attempt.software_icd || software_adapter {
        let reason = if is_vanilla_rung {
            "only a software GPU device is available".to_string()
        } else {
            format!(
                "hardware GPU init failed; falling back to {}",
                attempt.label
            )
        };
        RenderPath::Software { reason }
    } else {
        RenderPath::Hardware
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icd;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn pf(state: &Path, probe_cmd: Vec<String>, icd_dir: &Path) -> LinuxGpuPreflight {
        LinuxGpuPreflight {
            state_dir: state.to_path_buf(),
            probe_cmd,
            timeout: Duration::from_secs(5),
            ladder_budget: LADDER_BUDGET,
            sys_root: PathBuf::from("/nonexistent"),
            icd_dirs: Some(vec![icd_dir.to_path_buf()]),
            nvidia_smi_cmd: vec!["/bin/false".into()],
            app_version: "test".into(),
            env_snapshot: Vec::new(),
            instance_id: "test-instance".into(),
        }
    }

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    #[test]
    fn vanilla_success_stops_at_rung_zero() {
        let dir = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        let pf = pf(dir.path(), sh("exit 0"), icd_dir.path());
        let icds = icd::scan(&pf.sys_root, pf.icd_dirs.as_deref());

        let run = run(&pf, &icds);
        assert_eq!(run.winner, Some(0));
        assert_eq!(
            run.attempts.len(),
            1,
            "no per-ICD rungs after a vanilla win"
        );
        assert!(matches!(
            winning_path(&run.attempts[0], true),
            RenderPath::Hardware
        ));
    }

    #[test]
    fn pin_rescues_a_failing_vanilla_probe() {
        let dir = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        // Succeeds only when the loader pin is exported — the broken-ICD rescue.
        let pf = pf(
            dir.path(),
            sh(r#"[ -n "$VK_DRIVER_FILES" ]"#),
            icd_dir.path(),
        );
        let icds = icd::scan(&pf.sys_root, pf.icd_dirs.as_deref());

        let run = run(&pf, &icds);
        let winner = run.winner.expect("a pinned rung wins");
        assert_eq!(winner, 1);
        let attempt = &run.attempts[winner];
        assert_eq!(attempt.pins.len(), 2, "both loader spellings pinned");
        assert!(attempt.pins.iter().any(|(k, _)| k == "VK_DRIVER_FILES"));
        assert!(matches!(winning_path(attempt, false), RenderPath::Hardware));
    }

    #[test]
    fn software_manifest_win_reports_the_software_path_with_reason() {
        let dir = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("lvp_icd.x86_64.json"), "{}").unwrap();
        let pf = pf(
            dir.path(),
            sh(r#"[ -n "$VK_DRIVER_FILES" ]"#),
            icd_dir.path(),
        );
        let icds = icd::scan(&pf.sys_root, pf.icd_dirs.as_deref());

        let run = run(&pf, &icds);
        let winner = run.winner.expect("the software rung wins");
        match winning_path(&run.attempts[winner], false) {
            RenderPath::Software { reason } => {
                assert!(
                    reason.contains("lvp_icd.x86_64.json"),
                    "reason names the fallback: {reason}"
                );
            }
            RenderPath::Hardware => panic!("a software manifest win must report Software"),
        }
    }

    #[test]
    fn total_failure_records_every_rung() {
        let dir = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("a_icd.json"), "{}").unwrap();
        std::fs::write(icd_dir.path().join("b_icd.json"), "{}").unwrap();
        let pf = pf(dir.path(), sh("exit 1"), icd_dir.path());
        let icds = icd::scan(&pf.sys_root, pf.icd_dirs.as_deref());

        let run = run(&pf, &icds);
        assert_eq!(run.winner, None);
        assert_eq!(run.attempts.len(), 3, "vanilla + one rung per manifest");
    }

    #[test]
    fn probe_captures_land_in_the_state_dir_and_are_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        let pf = pf(dir.path(), sh("echo marker >&2; exit 1"), icd_dir.path());
        let icds = icd::scan(&pf.sys_root, pf.icd_dirs.as_deref());

        let run = run(&pf, &icds);
        // The capture reached memory (so the diagnosis still has its evidence)...
        assert!(run.attempts[0].outcome.stderr.contains("marker"));
        assert!(!run.attempts[0].outcome.no_capture);
        // ...the state dir is treated as a DIRECTORY of per-attempt captures, not
        // as one shared file (which a concurrent sid could truncate mid-capture,
        // contaminating this instance's adapter verdict)...
        assert!(
            !dir.path()
                .join(format!("probe-{}.log", std::process::id()))
                .is_dir()
        );
        // ...and nothing accumulates there.
        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("probe-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "captures must not accumulate: {leftovers:?}"
        );
    }

    #[test]
    fn a_spent_budget_stops_the_ladder_short() {
        let dir = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        for name in ["a_icd.json", "b_icd.json", "c_icd.json"] {
            std::fs::write(icd_dir.path().join(name), "{}").unwrap();
        }
        // The mute-compositor shape: every rung hangs and is killed. Without a
        // budget this is timeout × (1 + 3) of complete silence.
        let mut pf = pf(dir.path(), sh("sleep 30"), icd_dir.path());
        pf.timeout = Duration::from_millis(150);
        pf.ladder_budget = Duration::from_millis(200);
        let icds = icd::scan(&pf.sys_root, pf.icd_dirs.as_deref());

        let started = Instant::now();
        let run = run(&pf, &icds);
        assert_eq!(run.winner, None);
        // vanilla + the always-allowed first pinned rung, then the budget bites.
        assert_eq!(
            run.attempts.len(),
            2,
            "the budget must stop the climb: {:?}",
            run.attempts.iter().map(|a| &a.label).collect::<Vec<_>>()
        );
        assert!(
            run.all_attempts_timed_out(),
            "every rung was killed at the timeout"
        );
        // The cutoff is a FACT THE RUN CARRIES, not just a line on stderr:
        // `--gpu-report`'s body goes to stdout, and a redirected report showed 2 of
        // 4 rungs with nothing to explain the other two.
        assert_eq!(run.skipped_for_budget, 2, "4 rungs exist, 2 were tried");
        // 4 rungs would have cost 600ms+; two cost ~300ms.
        assert!(
            started.elapsed() < Duration::from_millis(550),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_fast_failing_ladder_still_tries_every_rung() {
        // The budget must not cost the self-healing it exists to protect: when
        // rungs fail immediately, all of them run regardless of how many there are.
        let dir = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        for name in ["a_icd.json", "b_icd.json", "c_icd.json"] {
            std::fs::write(icd_dir.path().join(name), "{}").unwrap();
        }
        let mut pf = pf(dir.path(), sh("exit 1"), icd_dir.path());
        pf.ladder_budget = Duration::from_millis(200);
        let icds = icd::scan(&pf.sys_root, pf.icd_dirs.as_deref());

        let run = run(&pf, &icds);
        assert_eq!(run.attempts.len(), 4, "vanilla + one rung per manifest");
        assert!(
            !run.all_attempts_timed_out(),
            "these failed, they did not hang"
        );
        assert_eq!(
            run.skipped_for_budget, 0,
            "nothing was skipped, so the report must claim nothing was"
        );
    }

    #[test]
    fn a_probe_that_cannot_spawn_does_not_climb() {
        // No pin can make an unspawnable command spawn, so repeating it once per
        // manifest is pure latency — and the outcome is not GPU evidence at all
        // (see `ensure_renderable`'s fail-open branch).
        let dir = tempfile::tempdir().unwrap();
        let icd_dir = tempfile::tempdir().unwrap();
        std::fs::write(icd_dir.path().join("a_icd.json"), "{}").unwrap();
        std::fs::write(icd_dir.path().join("b_icd.json"), "{}").unwrap();
        let pf = pf(
            dir.path(),
            vec!["/nonexistent/definitely-not-a-binary".into()],
            icd_dir.path(),
        );
        let icds = icd::scan(&pf.sys_root, pf.icd_dirs.as_deref());

        let run = run(&pf, &icds);
        assert_eq!(run.winner, None);
        assert_eq!(
            run.attempts.len(),
            1,
            "no pinned rungs after a spawn failure"
        );
        assert!(run.attempts[0].outcome.not_run);
        assert!(
            !run.all_attempts_timed_out(),
            "nothing ran, so nothing timed out"
        );
    }

    #[test]
    fn vanilla_win_on_a_software_adapter_is_reported_software() {
        // A lavapipe-only machine: nothing pinned, but blade's Adapter line names
        // llvmpipe — the badge must still appear.
        let attempt = RungAttempt {
            label: "vanilla".into(),
            pins: Vec::new(),
            software_icd: false,
            outcome: ProbeOutcome {
                ok: true,
                timed_out: false,
                exit: Some(0),
                stderr: String::new(),
                adapter: Some("llvmpipe (LLVM 19.1.0, 256 bits)".into()),
                not_run: false,
                no_capture: false,
            },
        };
        match winning_path(&attempt, true) {
            RenderPath::Software { reason } => {
                assert_eq!(reason, "only a software GPU device is available");
            }
            RenderPath::Hardware => panic!("software adapter must be reported"),
        }
    }
}
