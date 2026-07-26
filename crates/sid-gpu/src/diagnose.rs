//! The failure classifier: when every ladder rung fails, turn the evidence
//! (probe stderr, manifest scan, kernel modules, `nvidia-smi`) into a
//! `Diagnosis` a human can act on — the difference between an opaque panic and
//! "your driver updated; reboot". Arms are ordered most-specific first; every
//! check reads through `sys_root` / injected commands so tests never touch the
//! real machine.

use sid_core::gpu::{Diagnosis, FailureCause};
use std::path::Path;
use std::time::Duration;

use crate::LinuxGpuPreflight;
use crate::icd::IcdManifest;
use crate::probe;

/// What the ladder observed, reduced to the shapes the classifier reads.
///
/// Bundled rather than passed as loose arguments because every arm below is a
/// statement *about this evidence*, and a bare `bool` at the call sites would say
/// nothing about which shape it describes.
pub(crate) struct LadderEvidence<'a> {
    /// Rung 0's captured stderr — the vanilla probe's own words, where every
    /// recognizable error signature appears. Empty when the capture was lost
    /// (`ProbeOutcome::no_capture`) or when the probe never ran.
    pub rung0_stderr: &'a str,
    /// Rung 0 ran with its output discarded, because nowhere was writable.
    ///
    /// Carried so the evidence bundle can say *why* it has no probe stderr. The
    /// two states are indistinguishable in `rung0_stderr` alone (both are empty),
    /// and printing a bare `(empty)` for the second one hides the actual fault —
    /// a report reader then blames a silent probe rather than an unwritable state
    /// dir and temp dir.
    pub rung0_no_capture: bool,
    /// Every rung that ran was killed at its timeout: nothing ever answered.
    pub all_timed_out: bool,
}

pub(crate) fn diagnose(
    pf: &LinuxGpuPreflight,
    icds: &[IcdManifest],
    modules: &[(String, String)],
    evidence: &LadderEvidence<'_>,
) -> Diagnosis {
    let rung0_stderr = evidence.rung0_stderr;
    let detail = build_detail(icds, modules, rung0_stderr, evidence.rung0_no_capture);

    // Arm 1: the Vulkan loader itself never loaded — blade reports it before any
    // driver is consulted ("Missing Vulkan entry points" / a Loading(...) error).
    if rung0_stderr.contains("Missing Vulkan entry points") || rung0_stderr.contains("Loading(") {
        return Diagnosis {
            cause: FailureCause::NoDriverInstalled,
            summary: "the Vulkan loader is not installed".into(),
            remedy: distro_hint(&pf.sys_root),
            detail,
        };
    }

    // Arm 2: NVIDIA kernel/userspace mismatch — the classic post-update state
    // where every GPU call fails until a reboot loads the matching module.
    // nvidia-smi is the cheapest oracle: on mismatch it exits nonzero printing
    // "Failed to initialize NVML: Driver/library version mismatch".
    if pf.sys_root.join("sys/module/nvidia/version").exists() {
        // Captured through the probe's own capture discipline, which buys two
        // things: a per-call unique name (a shared one lets a concurrent sid
        // truncate this capture mid-write and hand us ITS output instead — same
        // hazard as the probe capture, see `ladder::run` — and a pid-only name is
        // not unique, because every process in a fresh PID namespace is pid 1),
        // and the temp-dir fallback, without which a read-only state dir silenced
        // this oracle and sent a post-update NVIDIA box to "file a bug" instead of
        // "reboot". See `probe::run_captured` for the remaining floor: when
        // nothing at all is writable the output is gone and so is this arm.
        let (ok, output) = probe::run_captured(
            &pf.nvidia_smi_cmd,
            &pf.state_dir,
            "nvidia-smi",
            Duration::from_secs(3),
        );
        if !ok && output.to_lowercase().contains("mismatch") {
            return Diagnosis {
                cause: FailureCause::DriverMismatchRebootNeeded,
                summary: "NVIDIA driver was updated but the running kernel module is older".into(),
                remedy: "Reboot to load the matching kernel module.".into(),
                detail,
            };
        }
    }

    // Arm 3: nothing ever answered — every rung that ran was killed at its
    // timeout. This is not a configuration shape at all: a driver call that never
    // returns means a wedged kernel driver, or a compositor that stopped serving
    // its socket (observed live: a mute Wayland socket hangs every rung
    // identically). No stderr signature can say so, because the child is killed
    // mid-init and its evidence is truncated by definition — which is exactly why
    // this shape used to fall through to "unrecognized reason" after tens of
    // seconds of silence.
    //
    // Classified NoDeviceAccess: devices may well exist, but this session cannot
    // complete an open on any of them, and the remedy is about the session rather
    // than about installing anything.
    if evidence.all_timed_out {
        return Diagnosis {
            cause: FailureCause::NoDeviceAccess,
            summary: "every GPU probe hung and had to be killed — nothing responded".into(),
            remedy: "Check that your graphical session is healthy (compositor still running, \
                     WAYLAND_DISPLAY/DISPLAY pointing at it). If it is, the GPU driver is \
                     most likely wedged — reboot to clear it."
                .into(),
            detail,
        };
    }

    // Arm 4: a loader with nothing to load — no driver manifests anywhere.
    if icds.is_empty() {
        return Diagnosis {
            cause: FailureCause::NoDriverInstalled,
            summary: "no Vulkan driver is installed (the loader found no driver manifests)".into(),
            remedy: distro_hint(&pf.sys_root),
            detail,
        };
    }

    // Arm 5: the loader came up but offers no presentation support at all, so
    // blade fails at instance creation before it ever inspects a device
    // (`Instance extension "VK_KHR_surface" is not supported`, blade's
    // vulkan/init.rs — which then reports the generic NoSupportedDeviceFound).
    // Manifests do exist on disk (arm 4 passed) and the ladder already tried
    // pinning each one, so the usual cause is the loader being told to ignore
    // them: a driver-filtering environment variable, or a loader/driver package
    // pair whose versions were split by a partial upgrade.
    if rung0_stderr.contains("Instance extension") && rung0_stderr.contains("is not supported") {
        return Diagnosis {
            cause: FailureCause::NoDriverInstalled,
            summary: "the Vulkan loader reports no usable driver (no presentation support)".into(),
            remedy: "Unset any driver-filtering environment variables \
                     (VK_LOADER_DRIVERS_DISABLE, VK_LOADER_DRIVERS_SELECT, VK_DRIVER_FILES, \
                     VK_ICD_FILENAMES) and retry. If none are set, reinstall your Vulkan \
                     driver and loader packages together so their versions match."
                .into(),
            detail,
        };
    }

    // Arm 6: devices were found and every one was rejected for lacking a Vulkan
    // feature the renderer requires — blade logs a "Rejected for ..." line per
    // device (API version, a required device extension, inline uniform blocks,
    // timeline semaphores, dynamic rendering). That's hardware or a driver too
    // old for the renderer, not a misconfiguration, so the remedy differs from
    // every arm above: update, or accept software rendering.
    if rung0_stderr.contains("Rejected for") {
        return Diagnosis {
            cause: FailureCause::NoDriverInstalled,
            summary: "this GPU or driver lacks Vulkan features sid's renderer requires".into(),
            remedy: format!(
                "Update your GPU driver and retry. If the hardware itself is too old, install \
                 a software rasterizer instead — sid then falls back to it automatically. \
                 Packages: {}",
                distro_hint(&pf.sys_root)
            ),
            detail,
        };
    }

    // Arm 7: drivers exist but no render node is visible to this session —
    // a VM without 3D acceleration, or device-permission trouble.
    let dri = pf.sys_root.join("dev/dri");
    let has_render_node = std::fs::read_dir(&dri)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        })
        .unwrap_or(false);
    if !has_render_node {
        return Diagnosis {
            cause: FailureCause::NoDeviceAccess,
            summary: "no GPU render node is visible to this session".into(),
            remedy: "In a VM, enable 3D acceleration; on bare metal check that your user can \
                     access /dev/dri (video/render group)."
                .into(),
            detail,
        };
    }

    // Fallthrough: real evidence, unrecognized shape — hand the human the tools.
    Diagnosis {
        cause: FailureCause::Unknown,
        summary: "GPU initialization failed for an unrecognized reason".into(),
        remedy: "Run `sid --gpu-report` and file the output.".into(),
        detail,
    }
}

/// Every diagnosis carries the same evidence bundle: what drivers were visible,
/// what kernel modules were loaded, and what the probe actually said.
///
/// `no_capture` distinguishes "the probe said nothing" from "we could not keep
/// what it said": with no writable directory anywhere, the tail is empty for a
/// reason that has nothing to do with the GPU, and a bare `(empty)` would send
/// the reader hunting for a silent probe instead.
fn build_detail(
    icds: &[IcdManifest],
    modules: &[(String, String)],
    rung0_stderr: &str,
    no_capture: bool,
) -> String {
    let mut d = String::from("driver manifests found:\n");
    if icds.is_empty() {
        d.push_str("  (none)\n");
    }
    for icd in icds {
        let class = if icd.software { "software" } else { "hardware" };
        d.push_str(&format!("  {} [{}]\n", icd.path.display(), class));
    }
    d.push_str("kernel GPU modules:\n");
    if modules.is_empty() {
        d.push_str("  (none with a version file)\n");
    }
    for (name, version) in modules {
        d.push_str(&format!("  {name} {version}\n"));
    }
    d.push_str("probe stderr (tail):\n");
    let lines: Vec<&str> = rung0_stderr.lines().collect();
    let tail_start = lines.len().saturating_sub(30);
    if lines.is_empty() {
        if no_capture {
            d.push_str(
                "  probe output could not be captured (no writable directory — state dir and \
                 temp dir both failed)\n",
            );
        } else {
            d.push_str("  (empty)\n");
        }
    }
    for line in &lines[tail_start..] {
        d.push_str(&format!("  {line}\n"));
    }
    d
}

/// Distro-aware install hint from `/etc/os-release` (`ID` + `ID_LIKE` tokens).
fn distro_hint(sys_root: &Path) -> String {
    let text = std::fs::read_to_string(sys_root.join("etc/os-release")).unwrap_or_default();
    let mut tokens: Vec<String> = Vec::new();
    for line in text.lines() {
        for key in ["ID=", "ID_LIKE="] {
            if let Some(value) = line.strip_prefix(key) {
                let value = value.trim().trim_matches('"');
                tokens.extend(value.split_whitespace().map(|t| t.to_lowercase()));
            }
        }
    }
    let has = |t: &str| tokens.iter().any(|x| x == t);
    if has("arch") {
        "sudo pacman -S vulkan-radeon  # or vulkan-intel / nvidia-utils for your GPU; \
         add vulkan-swrast for a software fallback"
            .into()
    } else if has("debian") || has("ubuntu") {
        "sudo apt install mesa-vulkan-drivers".into()
    } else if has("fedora") || has("rhel") || has("centos") {
        "sudo dnf install mesa-vulkan-drivers".into()
    } else {
        "Install your GPU's Vulkan driver package (Mesa for AMD/Intel, nvidia-utils for NVIDIA)."
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icd;

    /// A preflight over a fake root; `nvidia_smi_cmd` defaults to a command that
    /// can't match arm 2 (exits 0, no output).
    fn pf(sys_root: &Path, state: &Path) -> LinuxGpuPreflight {
        LinuxGpuPreflight {
            state_dir: state.to_path_buf(),
            probe_cmd: vec!["/bin/true".into()],
            timeout: Duration::from_secs(1),
            ladder_budget: crate::ladder::LADDER_BUDGET,
            sys_root: sys_root.to_path_buf(),
            icd_dirs: Some(vec![]),
            nvidia_smi_cmd: vec!["/bin/true".into()],
            app_version: "test".into(),
            env_snapshot: Vec::new(),
            instance_id: "test-instance".into(),
        }
    }

    fn fake_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// The ordinary evidence shape: rungs ran and failed with their output kept,
    /// so only `stderr` decides.
    fn ev(rung0_stderr: &str) -> LadderEvidence<'_> {
        LadderEvidence {
            rung0_stderr,
            rung0_no_capture: false,
            all_timed_out: false,
        }
    }

    /// A fake `nvidia-smi` in the mismatched state: nonzero exit + the phrase.
    fn mismatching_smi() -> Vec<String> {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo 'Failed to initialize NVML: Driver/library version mismatch' >&2; exit 12".into(),
        ]
    }

    #[test]
    fn missing_loader_classifies_as_no_driver_with_distro_hint() {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("etc")).unwrap();
        std::fs::write(root.path().join("etc/os-release"), "ID=arch\n").unwrap();
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &[],
            &[],
            &ev("Missing Vulkan entry points: LibraryLoadFailure(...)"),
        );
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(d.summary.contains("loader"));
        assert!(
            d.remedy.contains("pacman"),
            "arch hint expected: {}",
            d.remedy
        );
    }

    #[test]
    fn nvidia_mismatch_wins_when_smi_reports_it() {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("sys/module/nvidia")).unwrap();
        std::fs::write(root.path().join("sys/module/nvidia/version"), "570.86.16\n").unwrap();
        let state = fake_root();
        let mut p = pf(root.path(), state.path());
        p.nvidia_smi_cmd = mismatching_smi();
        let d = diagnose(
            &p,
            &[],
            &[("nvidia".into(), "570.86.16".into())],
            &ev("some stderr"),
        );
        assert_eq!(d.cause, FailureCause::DriverMismatchRebootNeeded);
        assert!(d.remedy.to_lowercase().contains("reboot"));
    }

    /// The oracle here is nvidia-smi's OUTPUT, not its exit status, so an
    /// unwritable state dir used to lose this arm outright: `File::create` failed,
    /// `run_captured` returned nothing, and a post-driver-update NVIDIA box was
    /// told to file a bug instead of to reboot. The temp-dir fallback fixes it.
    /// (Above the fallback there is still a floor — nowhere writable at all loses
    /// the arm; see `probe::run_captured`.)
    #[test]
    fn nvidia_mismatch_survives_an_unwritable_state_dir() {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("sys/module/nvidia")).unwrap();
        std::fs::write(root.path().join("sys/module/nvidia/version"), "570.86.16\n").unwrap();
        let icd_dir = fake_root();
        std::fs::write(icd_dir.path().join("nvidia_icd.json"), "{}").unwrap();
        let icds = icd::scan(
            Path::new("/nonexistent"),
            Some(&[icd_dir.path().to_path_buf()]),
        );
        let mut p = pf(root.path(), Path::new("/dev/null/nope"));
        p.nvidia_smi_cmd = mismatching_smi();
        let d = diagnose(
            &p,
            &icds,
            &[("nvidia".into(), "570.86.16".into())],
            &ev("some stderr"),
        );
        assert_eq!(
            d.cause,
            FailureCause::DriverMismatchRebootNeeded,
            "an unwritable state dir must not silence the mismatch oracle: {}",
            d.summary
        );
    }

    /// Finding 4: with nowhere writable, rung 0's stderr is empty for a reason
    /// that has nothing to do with the GPU — the detail must say which.
    #[test]
    fn a_lost_capture_is_explained_instead_of_printed_as_empty() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let lost = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &LadderEvidence {
                rung0_stderr: "",
                rung0_no_capture: true,
                all_timed_out: false,
            },
        );
        assert!(
            lost.detail.contains("could not be captured")
                && lost.detail.contains("no writable directory"),
            "the detail must say why there is no evidence: {}",
            lost.detail
        );
        assert!(
            !lost.detail.contains("(empty)"),
            "a bare (empty) hides the real fault: {}",
            lost.detail
        );
        // And a probe that ran, kept its output, and simply said nothing still
        // reads as empty — the two states must stay distinguishable.
        let silent = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev(""));
        assert!(silent.detail.contains("(empty)"), "{}", silent.detail);
    }

    #[test]
    fn nvidia_present_but_healthy_smi_falls_through() {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("sys/module/nvidia")).unwrap();
        std::fs::write(root.path().join("sys/module/nvidia/version"), "570.86.16\n").unwrap();
        // No manifests → should land on arm 3, not the mismatch arm.
        let state = fake_root();
        std::fs::create_dir_all(root.path().join("etc")).unwrap();
        std::fs::write(
            root.path().join("etc/os-release"),
            "ID=ubuntu\nID_LIKE=debian\n",
        )
        .unwrap();
        let d = diagnose(&pf(root.path(), state.path()), &[], &[], &ev("stderr"));
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(
            d.remedy.contains("apt"),
            "debian-family hint expected: {}",
            d.remedy
        );
    }

    #[test]
    fn no_render_node_classifies_as_device_access() {
        let root = fake_root();
        // One (hardware) manifest exists, so arm 3 passes; /dev/dri is absent.
        let icd_dir = fake_root();
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        let icds = icd::scan(
            Path::new("/nonexistent"),
            Some(&[icd_dir.path().to_path_buf()]),
        );
        let state = fake_root();
        let d = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev("stderr"));
        assert_eq!(d.cause, FailureCause::NoDeviceAccess);
    }

    #[test]
    fn unknown_fallthrough_keeps_the_evidence() {
        let root = fake_root();
        let icd_dir = fake_root();
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        let icds = icd::scan(
            Path::new("/nonexistent"),
            Some(&[icd_dir.path().to_path_buf()]),
        );
        // A render node exists → all specific arms pass → Unknown.
        std::fs::create_dir_all(root.path().join("dev/dri")).unwrap();
        std::fs::write(root.path().join("dev/dri/renderD128"), "").unwrap();
        let state = fake_root();
        let stderr = "ERROR blade_graphics::vulkan::init] enumerate_physical_devices: ERROR_INITIALIZATION_FAILED";
        let d = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev(stderr));
        assert_eq!(d.cause, FailureCause::Unknown);
        assert!(d.remedy.contains("--gpu-report"));
        assert!(d.detail.contains("radeon_icd.json"));
        assert!(d.detail.contains("ERROR_INITIALIZATION_FAILED"));
    }

    /// Fixture for the two stderr-signature arms: manifests present and a render
    /// node present, so only the stderr shape can decide the outcome.
    fn healthy_hardware_world() -> (tempfile::TempDir, tempfile::TempDir, Vec<IcdManifest>) {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("dev/dri")).unwrap();
        std::fs::write(root.path().join("dev/dri/renderD128"), "").unwrap();
        std::fs::create_dir_all(root.path().join("etc")).unwrap();
        std::fs::write(root.path().join("etc/os-release"), "ID=arch\n").unwrap();
        let icd_dir = fake_root();
        std::fs::write(icd_dir.path().join("intel_icd.json"), "{}").unwrap();
        let icds = icd::scan(
            Path::new("/nonexistent"),
            Some(&[icd_dir.path().to_path_buf()]),
        );
        (root, icd_dir, icds)
    }

    /// Observed live: `VK_LOADER_DRIVERS_DISABLE='*'` makes blade fail at instance
    /// creation, which it reports as the generic NoSupportedDeviceFound. Without
    /// this arm the user gets "unrecognized reason" for a one-variable fix.
    #[test]
    fn no_presentation_support_blames_driver_filtering() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let stderr = "[ERROR blade_graphics::hal::init] Instance extension \
                      \"VK_KHR_surface\" is not supported\n\
                      Unable to init GPU context: NoSupportedDeviceFound";
        let d = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev(stderr));
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(d.summary.contains("no usable driver"), "{}", d.summary);
        assert!(
            d.remedy.contains("VK_LOADER_DRIVERS_DISABLE") && d.remedy.contains("VK_DRIVER_FILES"),
            "the remedy must name the variables to unset: {}",
            d.remedy
        );
    }

    /// Every device enumerated but rejected for a missing feature — an old GPU or
    /// old driver, whose remedy (update, or accept software rendering) differs
    /// from the driver-filtering arm above.
    #[test]
    fn rejected_devices_blame_an_old_gpu_or_driver() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let stderr = "[WARN blade_graphics::hal::init] Rejected for device extension \
                      \"VK_KHR_dynamic_rendering\" not supported. Please update the driver!";
        let d = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev(stderr));
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(d.summary.contains("lacks Vulkan features"), "{}", d.summary);
        assert!(d.remedy.contains("software rasterizer"), "{}", d.remedy);
        // Carries the distro package hint for the update it just recommended.
        assert!(d.remedy.contains("pacman"), "{}", d.remedy);
    }

    /// The stderr arms must not swallow the generic case: an initialization
    /// failure with neither signature still reaches the fallthrough.
    #[test]
    fn stderr_arms_do_not_capture_unrelated_failures() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &ev("create_device: ERROR_INITIALIZATION_FAILED"),
        );
        assert_eq!(d.cause, FailureCause::Unknown);
    }

    #[test]
    fn unknown_distro_gets_the_generic_hint() {
        let root = fake_root();
        let state = fake_root();
        let d = diagnose(&pf(root.path(), state.path()), &[], &[], &ev("x"));
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(d.remedy.contains("Vulkan driver package"));
    }

    /// Observed live: a compositor socket that accepts but never answers hangs
    /// every rung identically (40.17s of silence with two manifests). The stderr
    /// is truncated mid-init, so before this arm existed the user was told
    /// "unrecognized reason" — for a session-level fault with an obvious remedy.
    #[test]
    fn every_rung_timing_out_blames_the_session_not_the_configuration() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &LadderEvidence {
                // What a killed probe leaves behind: a start, no verdict.
                rung0_stderr: "[INFO blade_graphics::hal::init] Adapter inspection",
                rung0_no_capture: false,
                all_timed_out: true,
            },
        );
        assert_eq!(d.cause, FailureCause::NoDeviceAccess);
        assert!(d.summary.contains("hung"), "{}", d.summary);
        assert!(
            d.remedy.contains("compositor") && d.remedy.to_lowercase().contains("reboot"),
            "the remedy must point at the session and the wedged driver: {}",
            d.remedy
        );
        // The converse — rungs that ran and failed must NOT reach this arm — is
        // `stderr_arms_do_not_capture_unrelated_failures` above, which asserts
        // Unknown for the same world with `all_timed_out: false`.
    }
}
